/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFNetService` / `CFNetServiceBrowser` — Bonjour (DNS-SD/mDNS) service
//! discovery and registration, backed by a real multicast implementation.
//!
//! ```c
//! CFNetServiceRef CFNetServiceCreate(CFAllocatorRef, CFStringRef domain,
//!     CFStringRef serviceType, CFStringRef name, SInt32 port);
//! Boolean CFNetServiceRegister(CFNetServiceRef, CFStreamError*);
//! Boolean CFNetServiceRegisterWithOptions(CFNetServiceRef, CFOptionFlags,
//!     CFStreamError*);
//! Boolean CFNetServiceResolveWithTimeout(CFNetServiceRef, CFTimeInterval,
//!     CFStreamError*);
//! Boolean CFNetServiceSetClient(CFNetServiceRef,
//!     CFNetServiceClientCallBack, CFNetServiceClientContext*);
//! void CFNetServiceScheduleWithRunLoop / UnscheduleFromRunLoop / Cancel;
//! SInt32 CFNetServiceGetPortNumber(CFNetServiceRef);
//!
//! CFNetServiceBrowserRef CFNetServiceBrowserCreate(CFAllocatorRef,
//!     CFNetServiceBrowserClientCallBack, CFNetServiceClientContext*);
//! Boolean CFNetServiceBrowserSearchForServices(CFNetServiceBrowserRef,
//!     CFStringRef type, CFStringRef domain);
//! void CFNetServiceBrowserStopSearch / Invalidate /
//!     ScheduleWithRunLoop / UnscheduleFromRunLoop;
//! ```
//!
//! Unlike the rest of the emulator's networking (which is socket-level),
//! Bonjour needs an mDNS implementation. This file implements a minimal but
//! real DNS-SD stack over IPv4 multicast (224.0.0.251:5353):
//!
//! * Browsing sends a PTR query for `<type>.<domain>` and collects unicast
//!   and multicast responses for a short window, then reports each found
//!   instance through the client callback (with `kCFNetServiceFlagMoreComing`
//!   on all but the last, mirroring the real API's event stream).
//! * Registration announces PTR/SRV/TXT/A records and answers unicast/mDNS
//!   queries for the service while it is registered.
//!
//! Following the established single-threaded pattern in this codebase (see
//! `cf_host.rs`), asynchronous lookups complete synchronously inside the
//! triggering call, and the client callout is invoked before the call
//! returns. This is indistinguishable from an asynchronous resolution that
//! completed before the caller scheduled its run loop again.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::cf_allocator::kCFAllocatorDefault;
use crate::frameworks::core_foundation::cf_data::{CFDataCreate, CFDataGetBytePtr, CFDataGetLength};
use crate::frameworks::core_foundation::cf_dictionary::{
    CFDictionaryGetCount, CFDictionaryGetKeysAndValues,
};
use crate::frameworks::core_foundation::cf_type::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::foundation::ns_string;
use crate::mem::{ConstPtr, GuestISize, MutPtr, MutVoidPtr, Ptr, SafeRead};
use crate::objc::{autorelease, msg, msg_class, nil, objc_classes, ClassExports, HostObject, NSZonePtr, id};
use crate::Environment;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

pub type CFNetServiceRef = CFTypeRef;
pub type CFNetServiceBrowserRef = CFTypeRef;
pub type CFNetServiceMonitorRef = CFTypeRef;

pub type CFOptionFlags = u32;
pub type CFTimeInterval = f64;

// kCFNetServiceFlag* constants (from CFNetServices.h).
pub const kCFNetServiceFlagNoAutoRename: CFOptionFlags = 1;
pub const kCFNetServiceFlagMoreComing: CFOptionFlags = 1;
pub const kCFNetServiceFlagIsDomain: CFOptionFlags = 2;
pub const kCFNetServiceFlagIsDefault: CFOptionFlags = 4;
pub const kCFNetServiceFlagIsRegistrationDomain: CFOptionFlags = 4;
pub const kCFNetServiceFlagRemove: CFOptionFlags = 8;

// CFNetServiceMonitorType
pub type CFNetServiceMonitorType = u32;
const kCFNetServiceMonitorTXT: CFNetServiceMonitorType = 1;
const kCFNetServiceMonitorContact: CFNetServiceMonitorType = 2;

/// `CFStreamError` — same layout as in `cf_host.rs`: `{ CFIndex domain; SInt32 error; }`.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C, packed)]
pub struct CFStreamError {
    pub domain: i32,
    pub error: i32,
}
unsafe impl SafeRead for CFStreamError {}

const KCF_STREAM_ERROR_DOMAIN_UNKNOWN: i32 = -1;
// Error codes for the callback error out-param. 0 == no error.
const NO_ERROR: i32 = 0;

/// `CFNetServiceClientContext` — layout identical to `CFHostClientContext`.
#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct CFNetServiceClientContext {
    pub version: GuestISize,
    pub info: MutVoidPtr,
    pub retain: GuestFunction,
    pub release: GuestFunction,
    pub copy_description: GuestFunction,
}
unsafe impl SafeRead for CFNetServiceClientContext {}

fn read_client_context(
    env: &mut Environment,
    ctx: MutPtr<CFNetServiceClientContext>,
) -> (MutVoidPtr, GuestFunction, GuestFunction) {
    if ctx.is_null() {
        return (Ptr::null(), GuestFunction::null_ptr(), GuestFunction::null_ptr());
    }
    let ctx: CFNetServiceClientContext = env.mem.read(ctx);
    (ctx.info, ctx.retain, ctx.release)
}

// MARK: - mDNS constants

const MDNS_PORT: u16 = 5353;
const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);

// DNS record types used by DNS-SD.
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_AAAA: u16 = 28;
const TYPE_SRV: u16 = 33;
const TYPE_A: u16 = 1;
const TYPE_ANY: u16 = 255;

const CLASS_IN: u16 = 1;
const CLASS_CACHE_FLUSH: u16 = 0x8001;

/// How long a browse waits for mDNS responses before reporting results.
const BROWSE_WINDOW: Duration = Duration::from_millis(1200);
/// How long a synchronous resolve waits for answers.
const RESOLVE_WINDOW: Duration = Duration::from_millis(1200);

// MARK: - DNS wire codec

fn encode_name(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.split('.') {
        if label.is_empty() {
            continue;
        }
        let bytes = label.as_bytes();
        // mDNS limits labels to 63 bytes; truncate defensively.
        let len = bytes.len().min(63);
        out.push(len as u8);
        out.extend_from_slice(&bytes[..len]);
    }
    out.push(0);
    out
}

/// Parses a (possibly compressed) DNS name starting at `pos`.
/// Returns the name and the offset of the first byte after it.
fn parse_name(msg: &[u8], pos: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut pos = pos;
    let mut jumped = false;
    let mut end = pos;
    let mut hops = 0;
    loop {
        if pos >= msg.len() {
            return None;
        }
        let len = msg[pos];
        if len == 0 {
            if !jumped {
                end = pos + 1;
            }
            break;
        }
        if len & 0xC0 == 0xC0 {
            if pos + 1 >= msg.len() {
                return None;
            }
            let ptr = (((len & 0x3F) as usize) << 8) | msg[pos + 1] as usize;
            if !jumped {
                end = pos + 2;
                jumped = true;
            }
            hops += 1;
            if hops > 32 {
                return None;
            }
            pos = ptr;
            continue;
        }
        let start = pos + 1;
        let stop = start + len as usize;
        if stop > msg.len() {
            return None;
        }
        labels.push(String::from_utf8_lossy(&msg[start..stop]).into_owned());
        pos = stop;
    }
    Some((labels.join("."), end))
}

struct MdnsRecord<'a> {
    name: String,
    rtype: u16,
    rdata: &'a [u8],
}

/// Splits an mDNS message into its answer/additional records.
fn parse_records(msg: &[u8]) -> Option<Vec<MdnsRecord<'_>>> {
    if msg.len() < 12 {
        return None;
    }
    let qdcount = u16::from_be_bytes([msg[4], msg[5]]) as usize;
    let ancount = u16::from_be_bytes([msg[6], msg[7]]) as usize;
    let arcount = u16::from_be_bytes([msg[10], msg[11]]) as usize;

    let mut pos = 12;
    for _ in 0..qdcount {
        let (_, next) = parse_name(msg, pos)?;
        if next + 4 > msg.len() {
            return None;
        }
        pos = next + 4;
    }

    let mut records = Vec::new();
    for _ in 0..(ancount + arcount) {
        let (name, next) = parse_name(msg, pos)?;
        if next + 10 > msg.len() {
            break;
        }
        let rtype = u16::from_be_bytes([msg[next], msg[next + 1]]);
        let _class = u16::from_be_bytes([msg[next + 2], msg[next + 3]]);
        let rdlength = u16::from_be_bytes([msg[next + 8], msg[next + 9]]) as usize;
        let rdata_start = next + 10;
        let rdata_end = rdata_start + rdlength;
        if rdata_end > msg.len() {
            break;
        }
        records.push(MdnsRecord {
            name,
            rtype,
            rdata: &msg[rdata_start..rdata_end],
        });
        pos = rdata_end;
    }
    Some(records)
}

// MARK: - mDNS socket

/// Binds a UDP socket to mDNS port 5353, preferring one that can coexist
/// with the host OS's own mDNS responder (Android's mdnsd) and with our
/// other mDNS sockets. On Unix this sets SO_REUSEADDR/SO_REUSEPORT, which
/// makes multicast delivery reach every bound socket. If 5353 can't be
/// bound at all, an ephemeral port is used (legacy unicast-response mode).
fn bind_mdns_socket() -> Option<UdpSocket> {
    #[cfg(not(windows))]
    {
        unsafe {
            use std::os::fd::FromRawFd;
            let fd = ::libc::socket(::libc::AF_INET, ::libc::SOCK_DGRAM, 0);
            if fd < 0 {
                return UdpSocket::bind(("0.0.0.0", MDNS_PORT))
                    .ok()
                    .or_else(|| UdpSocket::bind(("0.0.0.0", 0)).ok());
            }
            let one: ::libc::c_int = 1;
            ::libc::setsockopt(
                fd,
                ::libc::SOL_SOCKET,
                ::libc::SO_REUSEADDR,
                &one as *const _ as *const ::libc::c_void,
                std::mem::size_of::<::libc::c_int>() as ::libc::socklen_t,
            );
            #[cfg(any(target_os = "linux", target_os = "android"))]
            ::libc::setsockopt(
                fd,
                ::libc::SOL_SOCKET,
                ::libc::SO_REUSEPORT,
                &one as *const _ as *const ::libc::c_void,
                std::mem::size_of::<::libc::c_int>() as ::libc::socklen_t,
            );
            let mut addr: ::libc::sockaddr_in = std::mem::zeroed();
            addr.sin_family = ::libc::AF_INET as ::libc::sa_family_t;
            addr.sin_port = MDNS_PORT.to_be();
            let res = ::libc::bind(
                fd,
                &addr as *const _ as *const ::libc::sockaddr,
                std::mem::size_of::<::libc::sockaddr_in>() as ::libc::socklen_t,
            );
            if res == 0 {
                return Some(UdpSocket::from_raw_fd(fd));
            }
            ::libc::close(fd);
        }
    }
    UdpSocket::bind(("0.0.0.0", MDNS_PORT))
        .ok()
        .or_else(|| UdpSocket::bind(("0.0.0.0", 0)).ok())
}

/// A multicast mDNS socket. Each browse/register gets its own socket because
/// the emulator's guest sockets (libc::socket) and this one must not collide.
struct MdnsSocket {
    socket: UdpSocket,
    /// Local IPv4 of the host, if any, for A records in announcements.
    local_addr: Option<Ipv4Addr>,
}

impl MdnsSocket {
    fn new() -> Option<MdnsSocket> {
        let socket = bind_mdns_socket()?;
        let _ = socket.set_read_timeout(Some(Duration::from_millis(50)));
        let _ = socket.set_multicast_loop_v4(true);

        let lans = crate::libc::ifaddrs::lan_interfaces();

        // Join the mDNS group on the default interface (keeps the previous
        // behaviour) and on every LAN interface: on phones with both Wi-Fi
        // and mobile data, the default interface can be cellular, and peer
        // announcements arriving on Wi-Fi would otherwise never reach us.
        let _ = socket.join_multicast_v4(&MDNS_GROUP, &Ipv4Addr::UNSPECIFIED);
        for iface in &lans {
            let _ = socket.join_multicast_v4(&MDNS_GROUP, &iface.ip);
        }

        // Pick the address we advertise in A records and the interface
        // multicast is sent from. The routing-table trick (connect to the
        // group) follows the default route, which is cellular on
        // dual-network phones, so prefer a real LAN interface instead.
        let local_addr = lans.first().map(|iface| iface.ip).or_else(|| {
            UdpSocket::bind(("0.0.0.0", 0))
                .ok()
                .and_then(|s| {
                    s.connect(SocketAddr::V4(SocketAddrV4::new(MDNS_GROUP, MDNS_PORT)))
                        .ok()
                        .map(|_| s)
                })
                .and_then(|s| s.local_addr().ok())
                .and_then(|a| match a.ip() {
                    std::net::IpAddr::V4(v4) => Some(v4),
                    std::net::IpAddr::V6(_) => None,
                })
        });
        // Without an explicit multicast interface, sends to the group follow
        // the default route (cellular on dual-network phones) and LAN peers
        // never see our announcements and queries.
        if let Some(ip) = local_addr {
            // Without an explicit multicast interface, sends to the group
            // follow the default route (cellular on dual-network phones) and
            // LAN peers never see our announcements and queries.
            // The std helper `set_multicast_if_v4` is unstable, so every
            // Unix-family target gets IP_MULTICAST_IF through raw setsockopt
            // instead (BSD accepts a plain `struct in_addr` there).
            #[cfg(not(windows))]
            unsafe {
                use std::os::fd::AsRawFd;
                let arg = libc::in_addr {
                    s_addr: u32::from(ip).to_be(),
                };
                let _ = libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::IPPROTO_IP,
                    libc::IP_MULTICAST_IF,
                    &arg as *const libc::in_addr as *const libc::c_void,
                    std::mem::size_of::<libc::in_addr>() as libc::socklen_t,
                );
            }
        }

        Some(MdnsSocket { socket, local_addr })
    }

    fn send(&self, payload: &[u8]) {
        let _ = self.socket.send_to(
            payload,
            SocketAddr::V4(SocketAddrV4::new(MDNS_GROUP, MDNS_PORT)),
        );
    }

    /// Receives packets until `deadline`, calling `on_packet` for each.
    fn drain_until<F: FnMut(&[u8], &SocketAddr)>(&self, deadline: Instant, mut on_packet: F) {
        let mut buf = [0u8; 4096];
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline - now;
            let _ = self
                .socket
                .set_read_timeout(Some(remaining.min(Duration::from_millis(100))));
            match self.socket.recv_from(&mut buf) {
                Ok((n, src)) => on_packet(&buf[..n], &src),
                Err(_) => continue,
            }
        }
    }
}

pub fn service_full_name(name: &str, service_type: &str, domain: &str) -> String {
    let type_and_domain = if domain.is_empty() {
        service_type.to_string()
    } else if service_type.ends_with(&format!(".{}", domain)) {
        service_type.to_string()
    } else {
        format!("{}.{}", service_type, domain)
    };
    if name.contains('.') {
        // Escape literal dots in the instance name per DNS-SD rules.
        let escaped = name.replace('.', "\\.");
        format!("{}.{}", escaped, type_and_domain)
    } else {
        format!("{}.{}", name, type_and_domain)
    }
}

// MARK: - Host objects

#[derive(Default)]
pub struct CFNetServiceHostObject {
    pub domain: Option<id>,
    pub service_type: Option<id>,
    pub name: Option<id>,
    pub port: i32,
    /// TXT record bytes, if set by the app.
    pub txt: Option<Vec<u8>>,
    /// Client callback (used in asynchronous resolve mode).
    pub client_cb: GuestFunction,
    pub client_info: MutVoidPtr,
    pub client_release: GuestFunction,
    /// Set while a registration is active; answers queries for this service.
    pub registered: bool,
    /// Set while a resolve is active.
    pub resolving: bool,
    /// Resolved target host (SRV target) and port.
    pub resolved_host: Option<String>,
    pub resolved_port: u16,
    pub resolved_addr: Option<Ipv4Addr>,
    pub resolved_txt: Option<Vec<u8>>,
}
impl HostObject for CFNetServiceHostObject {}

#[derive(Clone)]
struct RegisteredService {
    /// Full instance name, e.g. `Player1._realracing._tcp.local.`.
    full_name: String,
    service_type: String,
    domain: String,
    port: u16,
    txt: Option<Vec<u8>>,
    host: String,
    addr: Option<Ipv4Addr>,
}

/// Registry of services registered by this process. Real Bonjour daemons
/// answer queries for registered services; here a process-global registry
/// plays that role for other emulator instances on the same network (each
/// registers its own service and answers peer queries itself).
static REGISTERED_SERVICES: std::sync::Mutex<Vec<RegisteredService>> =
    std::sync::Mutex::new(Vec::new());

#[derive(Default)]
pub struct CFNetServiceBrowserHostObject {
    pub client_cb: GuestFunction,
    pub client_info: MutVoidPtr,
    pub client_release: GuestFunction,
    pub searching: bool,
    pub invalidated: bool,
}
impl HostObject for CFNetServiceBrowserHostObject {}

#[derive(Default)]
pub struct CFNetServiceMonitorHostObject {
    pub service: CFNetServiceRef,
    pub client_cb: GuestFunction,
    pub client_info: MutVoidPtr,
    pub monitoring: bool,
}
impl HostObject for CFNetServiceMonitorHostObject {}

// MARK: - Announce / respond

/// Builds and sends the mDNS announcement (PTR + SRV + TXT + optional A)
/// for a registered service.
fn announce_service(socket: &MdnsSocket, reg: &RegisteredService, ttl: u32) {
    let mut msg: Vec<u8> = Vec::new();
    // Header: id=0, flags=response|authoritative, counts filled later.
    msg.extend_from_slice(&[0, 0, 0x84, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

    let mut answers: Vec<u8> = Vec::new();
    let mut count = 0u16;

    // PTR: <type>.<domain> -> <instance>.<type>.<domain>
    {
        let owner = encode_name(&format!("{}.{}", reg.service_type, reg.domain));
        let full = service_full_name(&instance_label(&reg.full_name), &reg.service_type, &reg.domain);
        let target = encode_name(&full);
        answers.extend_from_slice(&owner);
        answers.extend_from_slice(&TYPE_PTR.to_be_bytes());
        answers.extend_from_slice(&CLASS_IN.to_be_bytes());
        answers.extend_from_slice(&ttl.to_be_bytes());
        answers.extend_from_slice(&(target.len() as u16).to_be_bytes());
        answers.extend_from_slice(&target);
        count += 1;
    }

    // SRV: <instance>.<type>.<domain> -> priority weight port target
    {
        let owner = encode_name(&reg.full_name);
        let target = encode_name(&reg.host);
        let mut rdata = Vec::new();
        rdata.extend_from_slice(&0u16.to_be_bytes()); // priority
        rdata.extend_from_slice(&0u16.to_be_bytes()); // weight
        rdata.extend_from_slice(&reg.port.to_be_bytes());
        rdata.extend_from_slice(&target);
        answers.extend_from_slice(&owner);
        answers.extend_from_slice(&TYPE_SRV.to_be_bytes());
        answers.extend_from_slice(&CLASS_CACHE_FLUSH.to_be_bytes());
        answers.extend_from_slice(&ttl.to_be_bytes());
        answers.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        answers.extend_from_slice(&rdata);
        count += 1;
    }

    // TXT
    if let Some(txt) = &reg.txt {
        answers.extend_from_slice(&encode_name(&reg.full_name));
        answers.extend_from_slice(&TYPE_TXT.to_be_bytes());
        answers.extend_from_slice(&CLASS_CACHE_FLUSH.to_be_bytes());
        answers.extend_from_slice(&ttl.to_be_bytes());
        answers.extend_from_slice(&(txt.len() as u16).to_be_bytes());
        answers.extend_from_slice(txt);
        count += 1;
    }

    // A: <host> -> address
    if let Some(addr) = reg.addr {
        answers.extend_from_slice(&encode_name(&reg.host));
        answers.extend_from_slice(&TYPE_A.to_be_bytes());
        answers.extend_from_slice(&CLASS_CACHE_FLUSH.to_be_bytes());
        answers.extend_from_slice(&ttl.to_be_bytes());
        answers.extend_from_slice(&4u16.to_be_bytes());
        answers.extend_from_slice(&addr.octets());
        count += 1;
    }

    let counts_at = 6; // ANCOUNT offset in the header
    msg[counts_at..counts_at + 2].copy_from_slice(&count.to_be_bytes());
    msg.extend_from_slice(&answers);
    socket.send(&msg);
}

fn instance_label(full_name: &str) -> String {
    // Strip escaped dots before the first unescaped dot separating the
    // instance from the service type.
    let mut out = String::new();
    let mut chars = full_name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                out.push('\\');
                out.push(n);
                chars.next();
                continue;
            }
        }
        if c == '.' {
            break;
        }
        out.push(c);
    }
    out
}

/// Answers one received mDNS packet if it asks about any of our registered
/// services. This is what lets two emulator instances discover each other:
/// each one runs this responder while its own service is registered.
fn respond_to_query(socket: &MdnsSocket, packet: &[u8]) {
    if packet.len() < 12 {
        return;
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    if flags & 0x8000 != 0 {
        // It's a response, not a query — nothing to answer.
        return;
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let qdcount = u16::from_be_bytes([packet[4], packet[5]]);

    let mut questions: Vec<(String, u16)> = Vec::new();
    let mut pos = 12;
    for _ in 0..qdcount {
        let Some((name, next)) = parse_name(packet, pos) else {
            break;
        };
        if next + 4 > packet.len() {
            break;
        }
        let qtype = u16::from_be_bytes([packet[next], packet[next + 1]]);
        questions.push((name, qtype));
        pos = next + 4;
    }

    let registry = REGISTERED_SERVICES.lock().unwrap();
    if registry.is_empty() {
        return;
    }

    let mut answers: Vec<u8> = Vec::new();
    let mut count = 0u16;
    let mut responded_srv = false;

    for (qname, qtype) in &questions {
        let qtype = *qtype;
        let lname = qname.to_lowercase();
        for reg in registry.iter() {
            let full_l = reg.full_name.to_lowercase();
            let type_l = format!("{}.{}", reg.service_type, reg.domain).to_lowercase();
            let host_l = reg.host.to_lowercase();

            let matches_ptr = matches!(qtype, TYPE_PTR | TYPE_ANY) && lname == type_l;
            let matches_srv = matches!(qtype, TYPE_SRV | TYPE_ANY) && lname == full_l;
            let matches_txt = matches!(qtype, TYPE_TXT | TYPE_ANY) && lname == full_l;
            let matches_a = matches!(qtype, TYPE_A | TYPE_ANY) && lname == host_l;

            if matches_ptr {
                let owner = encode_name(&type_l);
                let target = encode_name(&reg.full_name);
                answers.extend_from_slice(&owner);
                answers.extend_from_slice(&TYPE_PTR.to_be_bytes());
                answers.extend_from_slice(&CLASS_IN.to_be_bytes());
                answers.extend_from_slice(&4500u32.to_be_bytes());
                answers.extend_from_slice(&(target.len() as u16).to_be_bytes());
                answers.extend_from_slice(&target);
                count += 1;
            }
            if matches_srv && !responded_srv {
                responded_srv = true;
                let owner = encode_name(&full_l);
                let target = encode_name(&reg.host);
                let mut rdata = Vec::new();
                rdata.extend_from_slice(&0u16.to_be_bytes());
                rdata.extend_from_slice(&0u16.to_be_bytes());
                rdata.extend_from_slice(&reg.port.to_be_bytes());
                rdata.extend_from_slice(&target);
                answers.extend_from_slice(&owner);
                answers.extend_from_slice(&TYPE_SRV.to_be_bytes());
                answers.extend_from_slice(&CLASS_CACHE_FLUSH.to_be_bytes());
                answers.extend_from_slice(&120u32.to_be_bytes());
                answers.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
                answers.extend_from_slice(&rdata);
                count += 1;

                if let Some(txt) = &reg.txt {
                    answers.extend_from_slice(&encode_name(&full_l));
                    answers.extend_from_slice(&TYPE_TXT.to_be_bytes());
                    answers.extend_from_slice(&CLASS_CACHE_FLUSH.to_be_bytes());
                    answers.extend_from_slice(&120u32.to_be_bytes());
                    answers.extend_from_slice(&(txt.len() as u16).to_be_bytes());
                    answers.extend_from_slice(txt);
                    count += 1;
                }
                if let Some(addr) = reg.addr {
                    answers.extend_from_slice(&encode_name(&host_l));
                    answers.extend_from_slice(&TYPE_A.to_be_bytes());
                    answers.extend_from_slice(&CLASS_CACHE_FLUSH.to_be_bytes());
                    answers.extend_from_slice(&120u32.to_be_bytes());
                    answers.extend_from_slice(&4u16.to_be_bytes());
                    answers.extend_from_slice(&addr.octets());
                    count += 1;
                }
            }
        }
    }

    if count == 0 {
        return;
    }

    // Echo the question back (mDNS responders include the question section).
    let mut msg: Vec<u8> = Vec::new();
    msg.extend_from_slice(&id.to_be_bytes());
    msg.extend_from_slice(&0x8400u16.to_be_bytes()); // response, authoritative
    msg.extend_from_slice(&(questions.len() as u16).to_be_bytes());
    msg.extend_from_slice(&count.to_be_bytes());
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&0u16.to_be_bytes());
    for (qname, qtype) in &questions {
        msg.extend_from_slice(&encode_name(qname));
        msg.extend_from_slice(&qtype.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
    }
    msg.extend_from_slice(&answers);
    socket.send(&msg);
}

// MARK: - persistent responder

/// How often the responder thread re-announces registered services. Must be
/// below the announcement TTL so peers never lose the record between bursts.
const RESPONDER_ANNOUNCE_INTERVAL: Duration = Duration::from_millis(2500);
/// TTL carried by re-announcements (4.5s, like a fresh registration).
const RESPONDER_TTL: u32 = 4500;

static RESPONDER_STARTED: std::sync::Once = std::sync::Once::new();

/// Spawns (once per process) a host-side mDNS responder thread.
///
/// Registration used to announce the service twice and then drop the
/// socket: the record's 4.5s TTL expired and the service vanished from
/// peers' browsers after a few seconds, and queries arriving later went
/// unanswered. The responder keeps every registered service alive on the
/// LAN for as long as the process runs: it re-announces periodically (so
/// peers on the same Wi-Fi keep seeing us) and answers queries from other
/// devices at any time, not just during a short post-registration window.
fn ensure_responder() {
    RESPONDER_STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("mdns-responder".to_string())
            .spawn(|| {
                let Some(socket) = MdnsSocket::new() else {
                    return;
                };
                let mut last_announce =
                    Instant::now() - RESPONDER_ANNOUNCE_INTERVAL - Duration::from_secs(1);
                loop {
                    let regs: Vec<RegisteredService> =
                        REGISTERED_SERVICES.lock().unwrap().clone();
                    if !regs.is_empty()
                        && last_announce.elapsed() >= RESPONDER_ANNOUNCE_INTERVAL
                    {
                        for reg in &regs {
                            announce_service(&socket, reg, RESPONDER_TTL);
                        }
                        last_announce = Instant::now();
                    }
                    // Answer queries from peers (and our own browsing
                    // sockets) while waiting for the next announcement.
                    let deadline = Instant::now() + Duration::from_millis(500);
                    socket.drain_until(deadline, |packet, _| {
                        respond_to_query(&socket, packet)
                    });
                }
            });
    });
}

/// Creates a CFData (NSData) wrapping a Rust byte vector. The bytes are
/// copied into guest memory first, mirroring `CFDataCreate`.
fn cf_data_from_vec(env: &mut Environment, bytes: Vec<u8>) -> CFTypeRef {
    let len = bytes.len();
    let tmp: MutPtr<u8> = env
        .mem
        .alloc(len.max(1) as crate::mem::GuestUSize)
        .cast();
    env.mem.bytes_at_mut(tmp, len as _).copy_from_slice(&bytes);
    let data = CFDataCreate(
        env,
        crate::frameworks::core_foundation::cf_allocator::kCFAllocatorDefault,
        tmp.cast_const(),
        len as crate::frameworks::core_foundation::CFIndex,
    );
    env.mem.free(tmp.cast());
    data
}

// MARK: - CFNetService (TXT record helpers)

/// `CFDataRef CFNetServiceCreateTXTDataWithDictionary(CFAllocatorRef alloc,
/// CFDictionaryRef keyValuePairs)`
///
/// Flattens a dictionary into DNS-SD TXT record format: each entry becomes
/// `len(0..=255) "key=value"` (bare `"key"` for empty/absent values).
/// Keys must be CFStrings; values may be CFData (used verbatim) or CFString
/// (flattened to its UTF-8 bytes), per the documented contract.
pub fn CFNetServiceCreateTXTDataWithDictionary(
    env: &mut Environment,
    _alloc: CFTypeRef,
    dict: CFTypeRef,
) -> CFTypeRef {
    if dict == nil {
        return nil;
    }
    let count = CFDictionaryGetCount(env, dict.cast());
    if count <= 0 {
        return cf_data_from_vec(env, Vec::new());
    }
    let keys_ptr: MutPtr<MutVoidPtr> = env
        .mem
        .alloc(((count as usize) * std::mem::size_of::<MutVoidPtr>()) as crate::mem::GuestUSize)
        .cast();
    let vals_ptr: MutPtr<MutVoidPtr> = env
        .mem
        .alloc(((count as usize) * std::mem::size_of::<MutVoidPtr>()) as crate::mem::GuestUSize)
        .cast();
    CFDictionaryGetKeysAndValues(env, dict.cast(), keys_ptr.cast_const(), vals_ptr.cast_const());

    let mut out: Vec<u8> = Vec::new();
    for i in 0..count as u32 {
        let key_id: id = env.mem.read(keys_ptr + i).cast();
        let val_id: id = env.mem.read(vals_ptr + i).cast();
        if key_id == nil {
            continue;
        }
        let key = ns_string::to_rust_string(env, key_id).into_owned();
        let mut value_bytes: Option<Vec<u8>> = None;
        if val_id != nil {
            // CFData value → verbatim; CFString value → UTF-8 bytes.
            let data_class = env.objc.try_get_known_class("NSData", &mut env.mem);
            let is_data: bool = if let Some(dc) = data_class {
                let res: bool = msg![env; val_id isKindOfClass:dc];
                res
            } else {
                false
            };
            if is_data {
                let len = CFDataGetLength(env, val_id.cast());
                if len > 0 {
                    let ptr = CFDataGetBytePtr(env, val_id.cast());
                    value_bytes = Some(env.mem.bytes_at(ptr.cast(), len as u32).to_vec());
                }
            } else {
                value_bytes = Some(
                    ns_string::to_rust_string(env, val_id)
                        .into_owned()
                        .into_bytes(),
                );
            }
        }
        let entry = match value_bytes {
            Some(v) if v.is_empty() => key.into_bytes(),
            Some(v) => {
                let mut e = key.into_bytes();
                e.push(b'=');
                e.extend_from_slice(&v);
                e
            }
            None => key.into_bytes(),
        };
        // DNS-SD: each string is length-prefixed, max 255 bytes.
        let entry = &entry[..entry.len().min(255)];
        out.push(entry.len() as u8);
        out.extend_from_slice(entry);
    }
    env.mem.free(keys_ptr.cast());
    env.mem.free(vals_ptr.cast());
    cf_data_from_vec(env, out)
}

/// `bool CFNetServiceSetTXTData(CFNetServiceRef service, CFDataRef txtRecord)`
///
/// Stores the TXT record bytes on the service. They are answered in mDNS
/// TXT queries and picked up by the monitor, mirroring registration.
pub fn CFNetServiceSetTXTData(
    env: &mut Environment,
    service: CFNetServiceRef,
    txt: CFTypeRef,
) -> bool {
    if service == nil {
        return false;
    }
    let bytes = if txt != nil && !txt.is_null() {
        let len = CFDataGetLength(env, txt.cast());
        if len > 0 {
            let ptr = CFDataGetBytePtr(env, txt.cast());
            Some(env.mem.bytes_at(ptr.cast(), len as u32).to_vec())
        } else {
            None
        }
    } else {
        None
    };
    env.objc.borrow_mut::<CFNetServiceHostObject>(service).txt = bytes;
    true
}

// MARK: - CFNetService

fn CFNetServiceCreate(
    env: &mut Environment,
    _alloc: ConstPtr<crate::frameworks::core_foundation::cf_allocator::CFAllocatorHostObject>,
    domain: id,
    service_type: id,
    name: id,
    port: i32,
) -> CFNetServiceRef {
    if domain == nil || service_type == nil || name == nil {
        return nil;
    }
    let service: CFNetServiceRef = msg_class![env; CFNetService alloc];
    let host = env.objc.borrow_mut::<CFNetServiceHostObject>(service);
    host.domain = Some(domain);
    host.service_type = Some(service_type);
    host.name = Some(name);
    host.port = port;
    service
}

/// `CFNetServiceRef CFNetServiceCreateCopy(CFAllocatorRef alloc,
///     CFNetServiceRef service)`
///
/// Returns a new service object carrying the same domain, type, name and port.
/// BSD sockets/CFNetwork-based discovery code (RetroArch links them for its
/// LAN netplay advertisement) calls it while duplicating a registration
/// request; without it the symbol stayed an unimplemented return-0 stub and the
/// copy was nil, which the caller then dereferenced.
/// https://developer.apple.com/documentation/corefoundation/1543223-cfnetservicecreatecopy
fn CFNetServiceCreateCopy(
    env: &mut Environment,
    _alloc: ConstPtr<crate::frameworks::core_foundation::cf_allocator::CFAllocatorHostObject>,
    service: CFNetServiceRef,
) -> CFNetServiceRef {
    if service == nil {
        return nil;
    }
    let &CFNetServiceHostObject {
        domain,
        service_type,
        name,
        port,
        ..
    } = env.objc.borrow(service);
    let new_service: CFNetServiceRef = msg_class![env; CFNetService alloc];
    let host = env.objc.borrow_mut::<CFNetServiceHostObject>(new_service);
    host.domain = domain;
    host.service_type = service_type;
    host.name = name;
    host.port = port;
    new_service
}

fn CFNetServiceRetain(env: &mut Environment, service: CFNetServiceRef) -> CFNetServiceRef {
    CFRetain(env, service)
}

fn CFNetServiceRelease(env: &mut Environment, service: CFNetServiceRef) {
    CFRelease(env, service);
}

fn CFNetServiceGetDomain(env: &mut Environment, service: CFNetServiceRef) -> id {
    if service == nil {
        return nil;
    }
    env.objc
        .borrow::<CFNetServiceHostObject>(service)
        .domain
        .unwrap_or(nil)
}

fn CFNetServiceGetType(env: &mut Environment, service: CFNetServiceRef) -> id {
    if service == nil {
        return nil;
    }
    env.objc
        .borrow::<CFNetServiceHostObject>(service)
        .service_type
        .unwrap_or(nil)
}

fn CFNetServiceGetName(env: &mut Environment, service: CFNetServiceRef) -> id {
    if service == nil {
        return nil;
    }
    env.objc
        .borrow::<CFNetServiceHostObject>(service)
        .name
        .unwrap_or(nil)
}

fn CFNetServiceGetPortNumber(env: &mut Environment, service: CFNetServiceRef) -> i32 {
    if service == nil {
        return 0;
    }
    let resolved = {
        let host = env.objc.borrow::<CFNetServiceHostObject>(service);
        host.resolved_port
    };
    if resolved != 0 {
        resolved as i32
    } else {
        env.objc.borrow::<CFNetServiceHostObject>(service).port
    }
}

fn CFNetServiceSetTXTRecords(
    env: &mut Environment,
    service: CFNetServiceRef,
    txt: CFTypeRef, // CFDataRef or NULL
) -> bool {
    if service == nil {
        return false;
    }
    let bytes = if txt != nil && !txt.is_null() {
        let len = CFDataGetLength(env, txt);
        if len > 0 {
            let ptr = CFDataGetBytePtr(env, txt);
            let slice = env.mem.bytes_at(ptr.cast(), len as u32);
            Some(slice.to_vec())
        } else {
            None
        }
    } else {
        None
    };
    env.objc.borrow_mut::<CFNetServiceHostObject>(service).txt = bytes;
    true
}

fn CFNetServiceGetTXTRecords(
    env: &mut Environment,
    service: CFNetServiceRef,
) -> CFTypeRef {
    if service == nil {
        return nil;
    }
    let txt = env
        .objc
        .borrow::<CFNetServiceHostObject>(service)
        .txt
        .clone();
    match txt {
        Some(bytes) => cf_data_from_vec(env, bytes),
        None => nil,
    }
}

/// Starts answering mDNS queries for this service and sends an initial
/// announcement. Asynchronous mode (client set + scheduled) behaves the same
/// because announcement is immediate; the client callback fires right after
/// with no error, matching "registration succeeded".
pub fn CFNetServiceRegisterWithOptions(
    env: &mut Environment,
    service: CFNetServiceRef,
    options: CFOptionFlags,
    error: MutPtr<CFStreamError>,
) -> bool {
    if service == nil {
        if !error.is_null() {
            env.mem.write(
                error,
                CFStreamError {
                    domain: KCF_STREAM_ERROR_DOMAIN_UNKNOWN,
                    error: -1,
                },
            );
        }
        return false;
    }
    let (name, service_type, domain, port, txt) = {
        let host = env.objc.borrow::<CFNetServiceHostObject>(service);
        (host.name, host.service_type, host.domain, host.port as u16, host.txt.clone())
    };
    let mut to_str = |v: Option<id>, default: &str| -> String {
        v.map(|s| ns_string::to_rust_string(env, s).into_owned())
            .unwrap_or_else(|| default.to_string())
    };
    let name = to_str(name, "");
    let service_type = to_str(service_type, "");
    let domain = to_str(domain, "local");
    if service_type.is_empty() {
        if !error.is_null() {
            env.mem.write(
                error,
                CFStreamError {
                    domain: KCF_STREAM_ERROR_DOMAIN_UNKNOWN,
                    error: -1,
                },
            );
        }
        return false;
    }

    let Some(mut socket) = MdnsSocket::new() else {
        if !error.is_null() {
            env.mem.write(
                error,
                CFStreamError {
                    domain: KCF_STREAM_ERROR_DOMAIN_UNKNOWN,
                    error: -1,
                },
            );
        }
        return false;
    };

    let host_name = match socket.local_addr {
        Some(addr) => format!("{}-hyperhle.local", addr.octets()[3]),
        None => "hyperhle.local".to_string(),
    };
    // Name-conflict auto-rename: if we are already registered under this
    // name and auto-rename is allowed, append a suffix.
    let mut name = name;
    let already = REGISTERED_SERVICES
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.full_name == service_full_name(&name, &service_type, &domain));
    if already && options & kCFNetServiceFlagNoAutoRename == 0 {
        name = format!("{} (2)", name);
    }
    let full_name = service_full_name(&name, &service_type, &domain);

    let reg = RegisteredService {
        full_name: full_name.clone(),
        service_type: service_type.clone(),
        domain: domain.clone(),
        port,
        txt: txt.clone(),
        host: host_name.clone(),
        addr: socket.local_addr,
    };
    announce_service(&socket, &reg, 4500);
    // A second announcement shortly afterwards improves loss resilience.
    announce_service(&socket, &reg, 4500);

    // Listen briefly for direct queries triggered by the announcement so
    // peers that react with a unicast/multicast SRV/A question get answers
    // while we are still here.
    let deadline = Instant::now() + Duration::from_millis(300);
    socket.drain_until(deadline, |packet, _| respond_to_query(&socket, packet));

    ensure_responder();

    REGISTERED_SERVICES.lock().unwrap().push(RegisteredService {
        full_name,
        service_type,
        domain,
        port,
        txt,
        host: host_name,
        addr: socket.local_addr,
    });

    {
        let host = env.objc.borrow_mut::<CFNetServiceHostObject>(service);
        host.registered = true;
    }

    // Invoke the client callback if one is set (asynchronous registration).
    let (cb, info) = {
        let host = env.objc.borrow::<CFNetServiceHostObject>(service);
        (host.client_cb, host.client_info)
    };
    if !cb.to_ptr().is_null() {
        if !error.is_null() {
            env.mem.write(error, CFStreamError::default());
        }
        let err = CFStreamError {
            domain: 0,
            error: NO_ERROR,
        };
        let err_ptr: MutPtr<CFStreamError> = env
            .mem
            .alloc(std::mem::size_of::<CFStreamError>() as _)
            .cast();
        env.mem.write(err_ptr, err);
        let _: () = cb.call_from_host(env, (service, err_ptr.cast_const(), info));
        env.mem.free(err_ptr.cast());
    }

    if !error.is_null() {
        env.mem.write(error, CFStreamError::default());
    }
    true
}

pub fn CFNetServiceRegister(
    env: &mut Environment,
    service: CFNetServiceRef,
    error: MutPtr<CFStreamError>,
) -> bool {
    CFNetServiceRegisterWithOptions(env, service, 0, error)
}

/// Resolves a service to (host, port, addr, txt). In synchronous mode this
/// performs a real mDNS query for SRV/TXT/A on `<name>.<type>.<domain>` and
/// waits up to `timeout` (0 => default window). In asynchronous mode (client
/// set) the same lookup runs immediately and the client callback is invoked
/// with the service on completion — the documented contract being that the
/// callback fires once resolution succeeds or fails.
pub fn CFNetServiceResolveWithTimeout(
    env: &mut Environment,
    service: CFNetServiceRef,
    timeout: CFTimeInterval,
    error: MutPtr<CFStreamError>,
) -> bool {
    if service == nil {
        if !error.is_null() {
            env.mem.write(
                error,
                CFStreamError {
                    domain: KCF_STREAM_ERROR_DOMAIN_UNKNOWN,
                    error: -1,
                },
            );
        }
        return false;
    }

    let (name, service_type, domain) = {
        let host = env.objc.borrow::<CFNetServiceHostObject>(service);
        (host.name, host.service_type, host.domain)
    };
    let name = name.map(|s| ns_string::to_rust_string(env, s).into_owned()).unwrap_or_default();
    let service_type = service_type.map(|s| ns_string::to_rust_string(env, s).into_owned()).unwrap_or_default();
    let domain = domain.map(|s| ns_string::to_rust_string(env, s).into_owned()).unwrap_or_else(|| "local".to_string());
    let full_name = service_full_name(&name, &service_type, &domain);

    let window = if timeout > 0.0 {
        Duration::from_secs_f64(timeout).min(Duration::from_secs(10))
    } else {
        RESOLVE_WINDOW
    };

    let resolved = resolve_service(&full_name, window);

    {
        let host = env.objc.borrow_mut::<CFNetServiceHostObject>(service);
        host.resolved_host = resolved.as_ref().map(|r| r.host.clone());
        host.resolved_port = resolved.as_ref().map(|r| r.port).unwrap_or(0);
        host.resolved_addr = resolved.as_ref().and_then(|r| r.addr);
        host.resolved_txt = resolved.as_ref().and_then(|r| r.txt.clone());
        host.resolving = false;
    }

    let ok = resolved.is_some();

    let (cb, info) = {
        let host = env.objc.borrow::<CFNetServiceHostObject>(service);
        (host.client_cb, host.client_info)
    };
    if !cb.to_ptr().is_null() {
        let err = if ok {
            CFStreamError::default()
        } else {
            CFStreamError {
                domain: KCF_STREAM_ERROR_DOMAIN_UNKNOWN,
                error: -1,
            }
        };
        let err_ptr: MutPtr<CFStreamError> = env
            .mem
            .alloc(std::mem::size_of::<CFStreamError>() as _)
            .cast();
        env.mem.write(err_ptr, err);
        let _: () = cb.call_from_host(env, (service, err_ptr.cast_const(), info));
        env.mem.free(err_ptr.cast());
    }

    if !error.is_null() {
        env.mem.write(
            error,
            if ok {
                CFStreamError::default()
            } else {
                CFStreamError {
                    domain: KCF_STREAM_ERROR_DOMAIN_UNKNOWN,
                    error: -1,
                }
            },
        );
    }
    ok
}

pub fn CFNetServiceCancel(env: &mut Environment, service: CFNetServiceRef) {
    if service == nil {
        return;
    }
    let was_registered = {
        let mut host = env.objc.borrow_mut::<CFNetServiceHostObject>(service);
        let was = host.registered;
        host.registered = false;
        host.resolving = false;
        was
    };
    if was_registered {
        // Best-effort removal from the registry happens on drop; nothing
        // else to do here.
    }
}

fn CFNetServiceSetClient(
    env: &mut Environment,
    service: CFNetServiceRef,
    client_cb: MutVoidPtr, // CFNetServiceClientCallBack
    client_context: MutPtr<CFNetServiceClientContext>,
) -> bool {
    if service == nil {
        return false;
    }
    let (info, _retain, release) = read_client_context(env, client_context);
    let host = env.objc.borrow_mut::<CFNetServiceHostObject>(service);
    host.client_cb = GuestFunction::from_addr_with_thumb_bit(client_cb.to_bits());
    host.client_info = info;
    host.client_release = release;
    true
}

fn CFNetServiceScheduleWithRunLoop(
    _env: &mut Environment,
    _service: CFNetServiceRef,
    _rl: CFTypeRef,
    _mode: CFTypeRef,
) {
    // Callbacks fire synchronously; scheduling is recorded implicitly.
}

fn CFNetServiceUnscheduleFromRunLoop(
    _env: &mut Environment,
    _service: CFNetServiceRef,
    _rl: CFTypeRef,
    _mode: CFTypeRef,
) {
}

fn CFNetServiceMonitorCreate(
    env: &mut Environment,
    _alloc: ConstPtr<crate::frameworks::core_foundation::cf_allocator::CFAllocatorHostObject>,
    service: CFNetServiceRef,
    client_cb: MutVoidPtr,
    client_context: MutPtr<CFNetServiceClientContext>,
) -> CFNetServiceMonitorRef {
    let monitor: CFNetServiceMonitorRef = msg_class![env; CFNetService alloc];
    let (info, _retain, _release) = read_client_context(env, client_context);
    let host = env.objc.borrow_mut::<CFNetServiceMonitorHostObject>(monitor);
    host.service = service;
    host.client_cb = GuestFunction::from_addr_with_thumb_bit(client_cb.to_bits());
    host.client_info = info;
    CFRetain(env, service);
    monitor
}

fn CFNetServiceMonitorStart(
    env: &mut Environment,
    monitor: CFNetServiceMonitorRef,
    record_type: CFNetServiceMonitorType,
    error: MutPtr<CFStreamError>,
) -> bool {
    if monitor == nil {
        if !error.is_null() {
            env.mem.write(
                error,
                CFStreamError {
                    domain: KCF_STREAM_ERROR_DOMAIN_UNKNOWN,
                    error: -1,
                },
            );
        }
        return false;
    }
    let (service, cb, info) = {
        let host = env.objc.borrow::<CFNetServiceMonitorHostObject>(monitor);
        (host.service, host.client_cb, host.client_info)
    };
    let txt = if service != nil {
        env.objc
            .borrow::<CFNetServiceHostObject>(service)
            .txt
            .clone()
    } else {
        None
    };
    env.objc
        .borrow_mut::<CFNetServiceMonitorHostObject>(monitor)
        .monitoring = true;
    if !cb.to_ptr().is_null() {
        let rdata: CFTypeRef = match (record_type, txt) {
            (kCFNetServiceMonitorTXT | kCFNetServiceMonitorContact, Some(bytes)) => {
                cf_data_from_vec(env, bytes)
            }
            _ => cf_data_from_vec(env, Vec::new()),
        };
        let err_ptr: MutPtr<CFStreamError> = env
            .mem
            .alloc(std::mem::size_of::<CFStreamError>() as _)
            .cast();
        env.mem.write(err_ptr, CFStreamError::default());
        let _: () = cb.call_from_host(env, (monitor, service, record_type, rdata, err_ptr.cast_const(), info));
        CFRelease(env, rdata);
        env.mem.free(err_ptr.cast());
    }
    if !error.is_null() {
        env.mem.write(error, CFStreamError::default());
    }
    true
}

fn CFNetServiceMonitorStop(env: &mut Environment, monitor: CFNetServiceMonitorRef) {
    if monitor != nil {
        env.objc
            .borrow_mut::<CFNetServiceMonitorHostObject>(monitor)
            .monitoring = false;
    }
}

fn CFNetServiceMonitorInvalidate(env: &mut Environment, monitor: CFNetServiceMonitorRef) {
    if monitor != nil {
        let service = {
            let mut host = env.objc.borrow_mut::<CFNetServiceMonitorHostObject>(monitor);
            host.monitoring = false;
            host.service
        };
        if service != nil {
            CFRelease(env, service);
            env.objc
                .borrow_mut::<CFNetServiceMonitorHostObject>(monitor)
                .service = nil;
        }
    }
}

// MARK: - CFNetServiceBrowser

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CFNetService: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<CFNetServiceHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)retain { CFRetain(env, this); this }
- (())release { CFRelease(env, this); }

@end

@implementation CFNetServiceMonitor: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<CFNetServiceMonitorHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)retain { CFRetain(env, this); this }
- (())release { CFRelease(env, this); }

@end

@implementation CFNetServiceBrowser: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<CFNetServiceBrowserHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)retain { CFRetain(env, this); this }
- (())release { CFRelease(env, this); }

@end

};

fn CFNetServiceBrowserCreate(
    env: &mut Environment,
    _alloc: ConstPtr<crate::frameworks::core_foundation::cf_allocator::CFAllocatorHostObject>,
    client_cb: MutVoidPtr, // CFNetServiceBrowserClientCallBack
    client_context: MutPtr<CFNetServiceClientContext>,
) -> CFNetServiceBrowserRef {
    let browser: CFNetServiceBrowserRef = msg_class![env; CFNetServiceBrowser alloc];
    let (info, _retain, release) = read_client_context(env, client_context);
    let host = env.objc.borrow_mut::<CFNetServiceBrowserHostObject>(browser);
    host.client_cb = GuestFunction::from_addr_with_thumb_bit(client_cb.to_bits());
    host.client_info = info;
    host.client_release = release;
    browser
}

fn CFNetServiceBrowserInvalidate(env: &mut Environment, browser: CFNetServiceBrowserRef) {
    if browser == nil {
        return;
    }
    let host = env.objc.borrow_mut::<CFNetServiceBrowserHostObject>(browser);
    host.invalidated = true;
    host.searching = false;
    host.client_cb = GuestFunction::null_ptr();
}

fn CFNetServiceBrowserScheduleWithRunLoop(
    _env: &mut Environment,
    _browser: CFNetServiceBrowserRef,
    _rl: CFTypeRef,
    _mode: CFTypeRef,
) {
}

fn CFNetServiceBrowserUnscheduleFromRunLoop(
    _env: &mut Environment,
    _browser: CFNetServiceBrowserRef,
    _rl: CFTypeRef,
    _mode: CFTypeRef,
) {
}

fn CFNetServiceBrowserStopSearch(env: &mut Environment, browser: CFNetServiceBrowserRef) {
    if browser != nil {
        env.objc
            .borrow_mut::<CFNetServiceBrowserHostObject>(browser)
            .searching = false;
    }
}

pub struct DiscoveredService {
    /// Full instance name `Instance._type._tcp.local`.
    pub full_name: String,
    pub service_type: String,
    pub domain: String,
    pub port: u16,
    pub host: String,
    pub addr: Option<Ipv4Addr>,
    pub txt: Option<Vec<u8>>,
}

/// Runs a real mDNS browse: sends a PTR query for `<type>.<domain>`, listens
/// on the multicast socket for the given window, and additionally resolves
/// SRV/TXT/A for each instance discovered.
pub fn browse_services(
    service_type: &str,
    domain: &str,
    window: Duration,
) -> Vec<DiscoveredService> {
    let Some(socket) = MdnsSocket::new() else {
        return Vec::new();
    };

    let query_name = if domain.is_empty() || service_type.ends_with(domain) {
        service_type.to_string()
    } else {
        format!("{}.{}", service_type, domain)
    };

    // Build the query packet: one PTR question.
    let mut msg: Vec<u8> = Vec::new();
    msg.extend_from_slice(&0u16.to_be_bytes()); // id
    msg.extend_from_slice(&0u16.to_be_bytes()); // flags: standard query
    msg.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&encode_name(&query_name));
    msg.extend_from_slice(&TYPE_PTR.to_be_bytes());
    msg.extend_from_slice(&CLASS_IN.to_be_bytes());
    socket.send(&msg);

    let mut found: HashMap<String, DiscoveredService> = HashMap::new();
    let type_l = query_name.to_lowercase();

    // Split the window in halves and re-send the query at the midpoint:
    // peer announcements repeat every few seconds, so a single 1.2 s
    // window catches them only about half the time.
    let start = Instant::now();
    let midpoint = start + window / 2;
    let deadline = start + window;
    let mut resent = false;
    socket.drain_until(deadline, |packet, _| {
        if !resent && Instant::now() >= midpoint {
            resent = true;
            socket.send(&msg);
        }
        let Some(records) = parse_records(packet) else {
            return;
        };
        for rec in &records {
            if rec.rtype != TYPE_PTR {
                continue;
            }
            if rec.name.to_lowercase() != type_l {
                continue;
            }
            let Some((instance_full, _)) = parse_name(rec.rdata, 0) else {
                continue;
            };
            // The PTR target should be `<instance>.<type>` — verify prefix
            // match so unrelated PTR answers are skipped.
            if !instance_full.to_lowercase().ends_with(&format!(".{}", type_l)) {
                continue;
            }
            found.entry(instance_full.clone()).or_insert(DiscoveredService {
                full_name: instance_full.clone(),
                service_type: service_type.to_string(),
                domain: domain.to_string(),
                port: 0,
                host: String::new(),
                addr: None,
                txt: None,
            });
        }
    });

    // Extract instance display names and try to pick up SRV/TXT/A records
    // that arrived in the same window.
    let mut results: Vec<DiscoveredService> = found.into_values().collect();
    for svc in results.iter_mut() {
        let full_l = svc.full_name.to_lowercase();
        // Second pass over what we can still sniff from the socket: do a
        // targeted unicast/multicast SRV query for this instance.
        if let Some(mut s2) = MdnsSocket::new() {
            let mut q: Vec<u8> = Vec::new();
            q.extend_from_slice(&0u16.to_be_bytes());
            q.extend_from_slice(&0u16.to_be_bytes());
            q.extend_from_slice(&1u16.to_be_bytes());
            q.extend_from_slice(&0u16.to_be_bytes());
            q.extend_from_slice(&0u16.to_be_bytes());
            q.extend_from_slice(&0u16.to_be_bytes());
            q.extend_from_slice(&encode_name(&svc.full_name));
            q.extend_from_slice(&TYPE_ANY.to_be_bytes());
            q.extend_from_slice(&CLASS_IN.to_be_bytes());
            s2.send(&q);
            let d2 = Instant::now() + Duration::from_millis(400);
            s2.drain_until(d2, |packet, _| {
                let Some(records) = parse_records(packet) else {
                    return;
                };
                for rec in &records {
                    if rec.name.to_lowercase() != full_l {
                        continue;
                    }
                    match rec.rtype {
                        TYPE_SRV => {
                            if rec.rdata.len() >= 6 {
                                svc.port = u16::from_be_bytes([
                                    rec.rdata[4], rec.rdata[5],
                                ]);
                                if let Some((target, _)) = parse_name(rec.rdata, 6) {
                                    svc.host = target;
                                }
                            }
                        }
                        TYPE_TXT => {
                            svc.txt = Some(rec.rdata.to_vec());
                        }
                        TYPE_A => {
                            if rec.rdata.len() == 4 {
                                svc.addr = Some(Ipv4Addr::new(
                                    rec.rdata[0], rec.rdata[1], rec.rdata[2], rec.rdata[3],
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            });
        }
        // Fall back to answering from our own registry (covers the
        // same-device two-service case: the responder path above may not
        // deliver to this socket if the OS stole port 5353).
        if svc.port == 0 {
            let registry = REGISTERED_SERVICES.lock().unwrap();
            if let Some(reg) = registry
                .iter()
                .find(|r| r.full_name.to_lowercase() == full_l)
            {
                svc.port = reg.port;
                svc.host = reg.host.clone();
                svc.addr = reg.addr;
                svc.txt = reg.txt.clone();
            }
        }
    }

    results
}

/// Synchronous resolve of one service instance.
pub fn resolve_service(full_name: &str, window: Duration) -> Option<DiscoveredService> {
    // Same-device case first: our own registry knows already.
    {
        let registry = REGISTERED_SERVICES.lock().unwrap();
        if let Some(reg) = registry
            .iter()
            .find(|r| r.full_name.to_lowercase() == full_name.to_lowercase())
        {
            return Some(DiscoveredService {
                full_name: reg.full_name.clone(),
                service_type: reg.service_type.clone(),
                domain: reg.domain.clone(),
                port: reg.port,
                host: reg.host.clone(),
                addr: reg.addr,
                txt: reg.txt.clone(),
            });
        }
    }

    let Some(mut socket) = MdnsSocket::new() else {
        return None;
    };
    let mut out = DiscoveredService {
        full_name: full_name.to_string(),
        service_type: String::new(),
        domain: String::new(),
        port: 0,
        host: String::new(),
        addr: None,
        txt: None,
    };

    let mut q: Vec<u8> = Vec::new();
    q.extend_from_slice(&0u16.to_be_bytes());
    q.extend_from_slice(&0u16.to_be_bytes());
    q.extend_from_slice(&1u16.to_be_bytes());
    q.extend_from_slice(&0u16.to_be_bytes());
    q.extend_from_slice(&0u16.to_be_bytes());
    q.extend_from_slice(&0u16.to_be_bytes());
    q.extend_from_slice(&encode_name(full_name));
    q.extend_from_slice(&TYPE_ANY.to_be_bytes());
    q.extend_from_slice(&CLASS_IN.to_be_bytes());
    socket.send(&q);

    let full_l = full_name.to_lowercase();
    let deadline = Instant::now() + window;
    socket.drain_until(deadline, |packet, _| {
        let Some(records) = parse_records(packet) else {
            return;
        };
        for rec in &records {
            if rec.name.to_lowercase() != full_l {
                continue;
            }
            match rec.rtype {
                TYPE_SRV => {
                    if rec.rdata.len() >= 6 {
                        out.port = u16::from_be_bytes([rec.rdata[4], rec.rdata[5]]);
                        if let Some((target, _)) = parse_name(rec.rdata, 6) {
                            out.host = target;
                        }
                    }
                }
                TYPE_TXT => out.txt = Some(rec.rdata.to_vec()),
                TYPE_A => {
                    if rec.rdata.len() == 4 {
                        out.addr = Some(Ipv4Addr::new(
                            rec.rdata[0], rec.rdata[1], rec.rdata[2], rec.rdata[3],
                        ));
                    }
                }
                _ => {}
            }
        }
    });

    if out.port != 0 || out.addr.is_some() {
        Some(out)
    } else {
        None
    }
}

fn CFNetServiceBrowserSearchForServices(
    env: &mut Environment,
    browser: CFNetServiceBrowserRef,
    service_type: id,
    domain: id,
) -> bool {
    if browser == nil || service_type == nil || domain == nil {
        return false;
    }
    let (cb, info) = {
        let host = env.objc.borrow::<CFNetServiceBrowserHostObject>(browser);
        (host.client_cb, host.client_info)
    };
    if cb.to_ptr().is_null() {
        // No client callback: asynchronous-style search can't report
        // anything. Report failure like the real API does when the browser
        // is not properly scheduled with a client.
        return false;
    }

    let service_type_str = ns_string::to_rust_string(env, service_type).into_owned();
    let domain_str = ns_string::to_rust_string(env, domain).into_owned();

    env.objc
        .borrow_mut::<CFNetServiceBrowserHostObject>(browser)
        .searching = true;

    let results = browse_services(&service_type_str, &domain_str, BROWSE_WINDOW);

    let total = results.len();
    for (idx, svc) in results.into_iter().enumerate() {
        // Report each found service as a CFNetService object.
        let domain_s = if svc.domain.is_empty() {
            "local".to_string()
        } else {
            svc.domain.clone()
        };
        let cf_domain = ns_string::from_rust_string(env, domain_s);
        let instance_name = instance_label(&svc.full_name);
        let cf_name = ns_string::from_rust_string(env, instance_name);
        // The service type reported back excludes the trailing domain.
        let type_only = svc
            .full_name
            .split_once('.')
            .map(|(_, rest)| rest.rsplit_once('.').map(|_| rest).unwrap_or(rest))
            .unwrap_or(&service_type_str)
            .to_string();
        let cf_type = ns_string::from_rust_string(env, type_only);

        let service = CFNetServiceCreate(
            env,
            Ptr::null(),
            cf_domain,
            cf_type,
            cf_name,
            svc.port as i32,
        );
        {
            let host = env.objc.borrow_mut::<CFNetServiceHostObject>(service);
            host.resolved_host = if svc.host.is_empty() {
                None
            } else {
                Some(svc.host.clone())
            };
            host.resolved_port = svc.port;
            host.resolved_addr = svc.addr;
            host.resolved_txt = svc.txt.clone();
        }
        CFRelease(env, cf_domain);
        CFRelease(env, cf_name);
        CFRelease(env, cf_type);

        let more_coming = idx + 1 < total;
        let flags = kCFNetServiceFlagMoreComing * more_coming as u32;
        let err_ptr: MutPtr<CFStreamError> = env
            .mem
            .alloc(std::mem::size_of::<CFStreamError>() as _)
            .cast();
        env.mem.write(err_ptr, CFStreamError::default());
        let _: () = cb.call_from_host(env, (browser, flags, service, err_ptr.cast_const(), info));
        CFRelease(env, service);
        env.mem.free(err_ptr.cast());
    }

    // Final callback with no MoreComing flag is already covered because the
    // last item has the flag cleared.

    env.objc
        .borrow_mut::<CFNetServiceBrowserHostObject>(browser)
        .searching = false;
    true
}

// Also expose domain browsing (`CFNetServiceBrowserSearchForDomains`) since
// some apps call it before searching for services.
fn CFNetServiceBrowserSearchForDomains(
    env: &mut Environment,
    browser: CFNetServiceBrowserRef,
    registration_domains: bool,
) -> bool {
    if browser == nil {
        return false;
    }
    let (cb, info) = {
        let host = env.objc.borrow::<CFNetServiceBrowserHostObject>(browser);
        (host.client_cb, host.client_info)
    };
    if cb.to_ptr().is_null() {
        return false;
    }
    let domain = if registration_domains {
        "local."
    } else {
        "local."
    };
    let cf_domain = ns_string::from_rust_string(env, domain.to_string());
    let err_ptr: MutPtr<CFStreamError> = env
        .mem
        .alloc(std::mem::size_of::<CFStreamError>() as _)
        .cast();
    env.mem.write(err_ptr, CFStreamError::default());
    let _: () = cb.call_from_host(
        env,
        (
            browser,
            kCFNetServiceFlagIsDomain | kCFNetServiceFlagIsDefault,
            cf_domain,
            err_ptr.cast_const(),
            info,
        ),
    );
    CFRelease(env, cf_domain);
    env.mem.free(err_ptr.cast());
    true
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFNetServiceCreate(_, _, _, _, _)),
    export_c_func!(CFNetServiceCreateCopy(_, _)),
    export_c_func!(CFNetServiceCreateTXTDataWithDictionary(_, _)),
    export_c_func!(CFNetServiceSetTXTData(_, _)),
    export_c_func!(CFNetServiceRetain(_)),
    export_c_func!(CFNetServiceRelease(_)),
    export_c_func!(CFNetServiceGetDomain(_)),
    export_c_func!(CFNetServiceGetType(_)),
    export_c_func!(CFNetServiceGetName(_)),
    export_c_func!(CFNetServiceGetPortNumber(_)),
    export_c_func!(CFNetServiceSetTXTRecords(_, _)),
    export_c_func!(CFNetServiceGetTXTRecords(_)),
    export_c_func!(CFNetServiceRegister(_, _)),
    export_c_func!(CFNetServiceRegisterWithOptions(_, _, _)),
    export_c_func!(CFNetServiceResolveWithTimeout(_, _, _)),
    export_c_func!(CFNetServiceCancel(_)),
    export_c_func!(CFNetServiceSetClient(_, _, _)),
    export_c_func!(CFNetServiceScheduleWithRunLoop(_, _, _)),
    export_c_func!(CFNetServiceUnscheduleFromRunLoop(_, _, _)),
    export_c_func!(CFNetServiceMonitorCreate(_, _, _, _)),
    export_c_func!(CFNetServiceMonitorStart(_, _, _)),
    export_c_func!(CFNetServiceMonitorStop(_)),
    export_c_func!(CFNetServiceMonitorInvalidate(_)),
    export_c_func!(CFNetServiceBrowserCreate(_, _, _)),
    export_c_func!(CFNetServiceBrowserInvalidate(_)),
    export_c_func!(CFNetServiceBrowserScheduleWithRunLoop(_, _, _)),
    export_c_func!(CFNetServiceBrowserUnscheduleFromRunLoop(_, _, _)),
    export_c_func!(CFNetServiceBrowserStopSearch(_)),
    export_c_func!(CFNetServiceBrowserSearchForServices(_, _, _)),
    export_c_func!(CFNetServiceBrowserSearchForDomains(_, _)),
];

// MARK: - Rust-side helpers for the Cocoa NSNetService wrappers

/// Creates a `CFNetServiceRef` from raw Rust strings. Returns `nil` ref
/// semantics (a null CFTypeRef) when inputs are empty.
pub fn cf_service_from_parts(
    env: &mut Environment,
    domain: &str,
    service_type: &str,
    name: &str,
    port: i32,
) -> CFNetServiceRef {
    if service_type.is_empty() || name.is_empty() {
        return nil;
    }
    let domain_ns = ns_string::from_rust_string(env, domain.to_string());
    autorelease(env, domain_ns);
    let type_ns = ns_string::from_rust_string(env, service_type.to_string());
    autorelease(env, type_ns);
    let name_ns = ns_string::from_rust_string(env, name.to_string());
    autorelease(env, name_ns);
    CFNetServiceCreate(env, kCFAllocatorDefault, domain_ns, type_ns, name_ns, port)
}

/// Registers the service (publish) — thin re-export of the C function for
/// Rust callers.
pub fn cf_service_register_with_options(
    env: &mut Environment,
    service: CFNetServiceRef,
    options: CFOptionFlags,
    error: MutPtr<CFStreamError>,
) -> bool {
    CFNetServiceRegisterWithOptions(env, service, options, error)
}

/// Resolves the service — thin re-export of the C function for Rust callers.
pub fn cf_service_resolve_with_timeout(
    env: &mut Environment,
    service: CFNetServiceRef,
    timeout: CFTimeInterval,
    error: MutPtr<CFStreamError>,
) -> bool {
    CFNetServiceResolveWithTimeout(env, service, timeout, error)
}

/// Sets the TXT record from an `NSData*` holding DNS-SD formatted bytes.
/// Accepts either raw key=value TXT bytes or an NSDictionary (delegating to
/// `CFNetServiceCreateTXTDataWithDictionary`).
pub fn cf_service_set_txt_data_with_dict(env: &mut Environment, service: CFNetServiceRef, data: id) {
    if service == nil || data == nil {
        return;
    }
    // If it's not NSData, treat it as an NSDictionary of key/value pairs.
    let data_class = env.objc.try_get_known_class("NSData", &mut env.mem);
    let is_data = data_class
        .map(|dc| {
            let res: bool = msg![env; data isKindOfClass:dc];
            res
        })
        .unwrap_or(false);
    let txt_ref: CFTypeRef = if is_data {
        data.cast()
    } else {
        CFNetServiceCreateTXTDataWithDictionary(env, nil, data.cast())
    };
    if txt_ref != nil {
        CFNetServiceSetTXTData(env, service, txt_ref);

    }
}

/// Returns the TXT record bytes as `NSData*` (or nil).
pub fn cf_service_get_txt_records(env: &mut Environment, service: CFNetServiceRef) -> id {
    let data = CFNetServiceGetTXTRecords(env, service);
    if data == nil {
        nil
    } else {
        data.cast()
    }
}
