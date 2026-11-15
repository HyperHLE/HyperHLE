/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! sys/socket.h (Sockets)
//!
//! We currently support blocking TCP and UDP guest sockets on IPv4 and IPv6 addresses.
//!
//! Host sockets are non-blocking so the emulator can poll them safely. Blocking
//! guest calls poll and yield the guest thread; `O_NONBLOCK` and `MSG_DONTWAIT`
//! still return `EAGAIN` immediately.
//!
//! Other note: Rust std::net APIs are "too high level" sometimes,
//! thus some workarounds need to be implemented.
//! (e.g. [TcpListener] does both bind() and listen() on a call
//! to [TcpListener::bind])
//!
//! Useful resources:
//! - [Beej's Guide to Network Programming](https://beej.us/guide/bgnet/html/index-wide.html)

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::errno::{
    get_errno, set_errno, EACCES, EADDRINUSE, EADDRNOTAVAIL, EAFNOSUPPORT, EAGAIN, EBADF,
    ECONNABORTED, ECONNREFUSED, ECONNRESET, EFAULT, EINVAL, EIO, EISCONN, ENETUNREACH,
    ENOPROTOOPT, ENOTCONN, ENOTSUP, ENOTTY, EPROTONOSUPPORT, ESOCKTNOSUPPORT, ETIMEDOUT,
};
use crate::libc::posix_io::{
    close, find_or_create_socket, is_nonblocking, is_socket, set_nonblocking, FileDescriptor,
    POLLERR, POLLHUP, POLLIN, POLLOUT, POLLPRI, POLLRDNORM, POLLWRNORM,
};
use crate::libc::time::timeval;
use crate::mem::{
    guest_size_of, ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, Ptr, SafeRead,
};
use crate::Environment;

use crate::abi::DotDotDot;
use crate::libc::netdb::{socklen_t, IPPROTO_TCP, IPPROTO_UDP};
use std::collections::{HashMap, HashSet};
use std::io;
use std::io::{Read, Write};
use std::net::{
    Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpListener, TcpStream, UdpSocket,
};

pub const AF_INET: i32 = 2;
pub const AF_INET6: i32 = 30;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;

const SOL_SOCKET: i32 = 0xffff;
const SO_DEBUG: i32 = 0x1;
const SO_REUSEADDR: i32 = 0x4;
const SO_BROADCAST: i32 = 0x20;
const SO_ERROR: i32 = 0x1007;

const SO_KEEPALIVE: i32 = 0x8;
const SO_TYPE: i32 = 0x1003;
const SO_NREAD: i32 = 0x1020;
const TCP_NODELAY: i32 = 1;
const SO_LINGER: i32 = 0x80;
const SO_SNDBUF: i32 = 0x1001;
const SO_RCVBUF: i32 = 0x1002;
const SO_NOSIGPIPE: i32 = 0x1022;
const MSG_DONTWAIT: i32 = 0x80;

#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct linger {
    pub l_onoff: i32,
    pub l_linger: i32,
}
unsafe impl SafeRead for linger {}

#[allow(non_camel_case_types)]
pub type sa_family_t = u8;

#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
#[allow(non_camel_case_types)]
pub struct sockaddr {
    sa_len: u8,
    sa_family: sa_family_t,
    sa_data: [u8; 14],
}
unsafe impl SafeRead for sockaddr {}
impl sockaddr {
    /// Makes an IPv4 sockaddr from 4 bytes for ip and a port.
    ///
    /// Port is expected to be native endian and
    /// will be converted to big endian internally.
    pub fn from_ipv4_parts(octets: [u8; 4], port: u16) -> Self {
        let mut addr = sockaddr {
            sa_len: 16,
            sa_family: AF_INET as u8,
            sa_data: [0; 14],
        };
        addr.sa_data[0..2].copy_from_slice(&port.to_be_bytes());
        addr.sa_data[2..6].copy_from_slice(&octets);
        addr
    }
    /// Returns 4 bytes for ip and a port.
    ///
    /// Port is returned in the native endian format.
    fn to_ipv4_parts(self) -> ([u8; 4], u16) {
        if !(self.sa_len == 16 || self.sa_len == 0) {
            log!(
                "Warning: sockaddr with sa_len {} (expected 16 or 0); treating as IPv4 address",
                self.sa_len
            );
        }
        if self.sa_family != AF_INET as u8 {
            log!(
                "Warning: sockaddr with unsupported sa_family {}; treating as IPv4 wildcard address",
                self.sa_family
            );
            return ([0, 0, 0, 0], 0);
        }
        let port = u16::from_be_bytes([self.sa_data[0], self.sa_data[1]]);
        let ip = [
            self.sa_data[2],
            self.sa_data[3],
            self.sa_data[4],
            self.sa_data[5],
        ];
        (ip, port)
    }
    pub fn to_sockaddr_v4(self) -> SocketAddrV4 {
        let (ip, port) = self.to_ipv4_parts();
        SocketAddrV4::new(ip.into(), port)
    }
}

/// Write a guest sockaddr to `address`, truncating the copy to the buffer
/// size the caller provided via `address_len` (BSD semantics), so a short
/// guest buffer is never overrun. The full size is stored through
/// `address_len` when it is non-null.
fn write_sockaddr_bounded(
    env: &mut Environment,
    address: MutPtr<sockaddr>,
    address_len: MutPtr<socklen_t>,
    value: SocketAddr,
    domain: i32,
) {
    if address.is_null() {
        return;
    }
    let bytes = socket_addr_to_sockaddr_bytes(socket_addr_for_domain(value, domain));
    let full_len = bytes.len() as u32;
    let provided = if address_len.is_null() {
        full_len
    } else {
        env.mem.read(address_len)
    };
    if !address.is_null() {
        let copy_len = provided.min(full_len);
        if copy_len > 0 {
            env.mem
                .bytes_at_mut(address.cast::<u8>(), copy_len)
                .copy_from_slice(&bytes[..copy_len as usize]);
        }
    }
    if !address_len.is_null() {
        env.mem.write(address_len, full_len);
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
#[allow(non_camel_case_types)]
pub struct fd_set {
    // 32 4-byte ints should be enough for 1024 file descriptors
    fds_bits: [i32; 32],
}
unsafe impl SafeRead for fd_set {}

struct SocketHostObject {
    /// Type of the socket, [SOCK_STREAM] for TCP or [SOCK_DGRAM] for UDP
    type_: i32,
    /// Set of options
    options: HashSet<i32>,
    /// TCP socket which is yet to be connected
    tcp_listener: Option<TcpListener>,
    /// TCP socket which was connected on host, but not (yet) on the guest side
    pending_tcp_stream: Option<TcpStream>,
    /// Already connected TCP socket
    tcp_stream: Option<TcpStream>,
    /// UDP socket
    udp_socket: Option<UdpSocket>,
    /// Domain of the socket
    domain: i32,
    /// Address recorded by `bind()` while network access is disabled. Such a
    /// socket never gets a host socket: it can be bound and listened on, but
    /// no connection ever arrives (see [socket]).
    offline_address: Option<SocketAddr>,
}

#[derive(Default)]
pub struct State {
    sockets: HashMap<i32, SocketHostObject>,
}
impl State {
    fn get(env: &Environment) -> &Self {
        &env.libc_state.socket
    }
    fn get_mut(env: &mut Environment) -> &mut Self {
        &mut env.libc_state.socket
    }
}

fn socket(env: &mut Environment, domain: i32, type_: i32, protocol: i32) -> FileDescriptor {
    // errno is set on every failure path below.
    set_errno(env, 0);
    // Creating a socket works even when network access is disabled, as it
    // does on a real device without connectivity: the socket only becomes
    // a host socket in bind(), connect() or sendto(), and those refuse to
    // touch the host network in that case. Failing here instead broke apps
    // whose audio engine opens a listening socket while initializing (e.g.
    // FMOD's profiler in Where's My Water), which then gave up on sound.

    if domain != AF_INET && domain != AF_INET6 {
        set_errno(env, EAFNOSUPPORT);
        return -1;
    }

    if type_ != SOCK_STREAM && type_ != SOCK_DGRAM {
        set_errno(env, ESOCKTNOSUPPORT);
        return -1;
    }

    if protocol != IPPROTO_TCP && protocol != IPPROTO_UDP && protocol != 0 {
        set_errno(env, EPROTONOSUPPORT);
        return -1;
    }

    let fd = find_or_create_socket(env);
    if State::get(env).sockets.contains_key(&fd) {
        // Should be unreachable: find_or_create_socket() returns an fd with
        // no open file object, so it cannot have a socket either. Log and
        // overwrite a stale entry rather than panicking.
        log!(
            "Warning: socket(): fd {} already had a stale socket entry",
            fd
        );
    }
    let host_object = SocketHostObject {
        type_,
        options: Default::default(),
        tcp_listener: None,
        pending_tcp_stream: None,
        tcp_stream: None,
        udp_socket: None,
        domain,
        offline_address: None,
    };
    State::get_mut(env).sockets.insert(fd, host_object);

    log_dbg!("socket({}, {}, {}) => {}", domain, type_, protocol, fd);
    fd
}

fn ioctl(env: &mut Environment, fd: i32, request: u32, args: DotDotDot) -> i32 {
    set_errno(env, 0);
    // Handle invalid descriptors per POSIX.
    if !is_socket(env, fd) {
        log!("ioctl: fd={} is not a valid socket, returning EBADF", fd);
        set_errno(env, EBADF);
        return -1;
    }

    // Darwin request codes for the two operations real iOS apps actually
    // issue on sockets. Everything else fails with ENOTTY rather than
    // crashing or silently lying about success.
    const FIONBIO: u32 = 0x8004667E;
    const FIONREAD: u32 = 0x4004667F;

    match request {
        FIONREAD => {
            let mut varargs = args.start();
            let out: MutPtr<i32> = varargs.next(env);
            if !out.is_null() {
                let available = socket_bytes_available(env, fd);
                env.mem.write(out, available);
                log_dbg!("ioctl({}, FIONREAD) => {} bytes", fd, available);
            } else {
                log_dbg!("ioctl({}, FIONREAD) with NULL arg, ignored", fd);
            }
            0
        }
        FIONBIO => {
            // Argument is a pointer to int: non-zero enables non-blocking
            // mode. Host sockets are always non-blocking here, so both
            // directions are accepted, but the flag is recorded on the
            // descriptor: `read(2)`/`write(2)` have to emulate blocking
            // themselves, and they must not wait when the guest asked for
            // non-blocking I/O.
            let mut varargs = args.start();
            let flag_ptr: MutPtr<i32> = varargs.next(env);
            let flag = if flag_ptr.is_null() {
                0
            } else {
                env.mem.read(flag_ptr)
            };
            set_nonblocking(env, fd, flag != 0);
            log_dbg!(
                "ioctl({}, FIONBIO, {}) => 0 (host sockets are non-blocking)",
                fd,
                flag
            );
            0
        }
        request => {
            log!(
                "ioctl({} (socket), {:#x}, ...) is not supported, returning ENOTTY",
                fd,
                request
            );
            set_errno(env, ENOTTY);
            -1
        }
    }
}

/// Number of bytes immediately available for reading on a socket fd, as
/// reported by FIONREAD and SO_NREAD. Peeked (non-destructive); returns 0
/// when the count cannot be determined.
fn socket_bytes_available(env: &mut Environment, fd: i32) -> i32 {
    const PEEK_CAP: usize = 65536; // one maximum-size UDP datagram
    let Some(sock) = State::get(env).sockets.get(&fd) else {
        return 0;
    };
    if let Some(udp) = sock.udp_socket.as_ref() {
        let mut buf = vec![0u8; PEEK_CAP];
        return (udp.peek(&mut buf).unwrap_or(0).min(i32::MAX as usize)) as i32;
    }
    if let Some(stream) = sock.tcp_stream.as_ref() {
        let mut buf = vec![0u8; PEEK_CAP];
        return match stream.peek(&mut buf) {
            Ok(n) => (n.min(i32::MAX as usize)) as i32,
            // A reset connection reads as "0 bytes available", not an error.
            Err(ref e) if e.kind() == io::ErrorKind::ConnectionReset => 0,
            Err(_) => 0,
        };
    }
    0
}

fn getsockopt(
    env: &mut Environment,
    socket: i32,
    level: i32,
    option_name: i32,
    option_value: MutVoidPtr,
    option_len: MutPtr<socklen_t>,
) -> i32 {
    // errno is set on every failure path below.
    set_errno(env, 0);
    log_dbg!(
        "getsockopt({}, {:#x}, {:#x}, {:?}, {:?})",
        socket,
        level,
        option_name,
        option_value,
        option_len
    );
    let Some(sock) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };

    if option_len.is_null() || option_value.is_null() {
        set_errno(env, EINVAL);
        return -1;
    }
    let option_len_val = env.mem.read(option_len);

    // Write the integer answer for the common 4-byte options.
    let int_answer: Option<i32> = match (level, option_name) {
        (SOL_SOCKET, SO_ERROR) => Some(0), // no pending error is ever tracked
        (SOL_SOCKET, SO_TYPE) => Some(sock.type_),
        (SOL_SOCKET, SO_SNDBUF) | (SOL_SOCKET, SO_RCVBUF) => {
            // Plausible default; host buffer sizes are not exposed by
            // Rust's std::net.
            Some(0x10000)
        }
        (SOL_SOCKET, SO_DEBUG)
        | (SOL_SOCKET, SO_REUSEADDR)
        | (SOL_SOCKET, SO_BROADCAST)
        | (SOL_SOCKET, SO_KEEPALIVE)
        | (SOL_SOCKET, SO_NOSIGPIPE) => Some(sock.options.contains(&option_name) as i32),
        (SOL_SOCKET, SO_NREAD) => Some(socket_bytes_available(env, socket)),
        (IPPROTO_TCP, TCP_NODELAY) => Some(sock.options.contains(&TCP_NODELAY) as i32),
        (SOL_SOCKET, SO_LINGER) => None, // struct option, handled below
        // Unknown IPPROTO_TCP options (e.g. TCP_INFO) get a zeroed buffer:
        // sane defaults beat failing the app. Unknown SOL_SOCKET options are
        // reported with ENOPROTOOPT like a real socket would.
        (IPPROTO_TCP, _) => None,
        (level, option_name) => {
            log!(
                "getsockopt: unhandled level={:#x} option={:#x} on socket {}",
                level,
                option_name,
                socket
            );
            set_errno(env, ENOPROTOOPT);
            return -1;
        }
    };

    if let Some(value) = int_answer {
        if option_len_val < guest_size_of::<socklen_t>() {
            set_errno(env, EINVAL);
            return -1;
        }
        let option_value: MutPtr<i32> = option_value.cast();
        env.mem.write(option_value, value);
        env.mem.write(option_len, guest_size_of::<i32>());
    } else if level == SOL_SOCKET && option_name == SO_LINGER {
        if option_len_val < guest_size_of::<linger>() {
            set_errno(env, EINVAL);
            return -1;
        }
        // Report lingering disabled; a zeroed struct is a valid answer.
        let option_value: MutPtr<linger> = option_value.cast();
        env.mem.write(
            option_value,
            linger {
                l_onoff: 0,
                l_linger: 0,
            },
        );
        env.mem.write(option_len, guest_size_of::<linger>());
    } else {
        // Zero-fill struct-valued options (e.g. TCP_INFO) so the guest sees
        // well-formed data instead of an error.
        let bytes = env.mem.bytes_at_mut(option_value.cast(), option_len_val);
        bytes.fill(0);
    }
    0 // Success
}

fn setsockopt(
    env: &mut Environment,
    socket: i32,
    level: i32,
    option_name: i32,
    option_value: ConstVoidPtr,
    option_len: socklen_t,
) -> i32 {
    set_errno(env, 0);
    log_dbg!(
        "setsockopt({}, {:#x}, {:#x}, {:?}, {})",
        socket,
        level,
        option_name,
        option_value,
        option_len
    );
    let Some(sock) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };
    let type_ = sock.type_;

    match (level, option_name) {
        (SOL_SOCKET, SO_DEBUG) => {
            // Silently ignore SO_DEBUG — requires elevated privileges on most
            // platforms; apps set this speculatively and don't check the
            // result.
            log_dbg!("setsockopt: ignoring SO_DEBUG on socket {}", socket);
            0
        }
        (SOL_SOCKET, SO_REUSEADDR) | (SOL_SOCKET, SO_BROADCAST) | (SOL_SOCKET, SO_NOSIGPIPE) => {
            // Guest-controlled option_len: report EINVAL instead of
            // asserting when an app passes a wrong size.
            if option_len < guest_size_of::<i32>() {
                set_errno(env, EINVAL);
                return -1;
            }
            let val: i32 = env.mem.read(option_value.cast());
            if val != 0 {
                State::get_mut(env)
                    .sockets
                    .get_mut(&socket)
                    .unwrap()
                    .options
                    .insert(option_name);
            } else {
                State::get_mut(env)
                    .sockets
                    .get_mut(&socket)
                    .unwrap()
                    .options
                    .remove(&option_name);
            }
            // Apply SO_BROADCAST immediately if the UDP socket already exists.
            if option_name == SO_BROADCAST {
                if let Some(udp) = State::get(env)
                    .sockets
                    .get(&socket)
                    .unwrap()
                    .udp_socket
                    .as_ref()
                {
                    if let Err(e) = udp.set_broadcast(val != 0) {
                        log!("setsockopt: set_broadcast failed: {}", e);
                        set_errno(env, EIO);
                        return -1;
                    }
                }
            }
            // SO_NOSIGPIPE просто сохраняется в options.
            // В Rust попытка записи в закрытый сокет и так возвращает
            // ErrorKind::BrokenPipe вместо убийства процесса.
            0
        }
        (SOL_SOCKET, SO_LINGER) => {
            // Некоторые приложения (например, Minecraft PE) передают 4 байта
            // (размер обычного int)
            // вместо положенных 8 байт (struct linger). Обрабатываем оба
            // варианта легально:
            let (l_onoff, l_linger) = if option_len == guest_size_of::<linger>() {
                let linger_val: linger = env.mem.read(option_value.cast());
                (linger_val.l_onoff, linger_val.l_linger)
            } else if option_len == guest_size_of::<i32>() {
                let val: i32 = env.mem.read(option_value.cast());
                (val, 0)
            } else {
                log!(
                    "setsockopt: SO_LINGER with invalid option_len {}, returning EINVAL",
                    option_len
                );
                set_errno(env, EINVAL);
                return -1;
            };

            let duration = if l_onoff != 0 {
                Some(std::time::Duration::from_secs(l_linger.max(0) as u64))
            } else {
                None
            };

            if type_ == SOCK_STREAM {
                if let Some(_stream) = State::get(env)
                    .sockets
                    .get(&socket)
                    .unwrap()
                    .tcp_stream
                    .as_ref()
                {
                    // Имитируем успешную установку SO_LINGER. Реальный вызов
                    // stream.set_linger
                    // заменен на логирование, так как фича `tcp_linger`
                    // нестабильна в std::net
                    log!("setsockopt: SO_LINGER (duration: {:?}) requested, ignoring due to unstable tcp_linger feature", duration);
                }
            }
            0
        }
        (SOL_SOCKET, SO_SNDBUF) | (SOL_SOCKET, SO_RCVBUF) => {
            if option_len < guest_size_of::<i32>() {
                set_errno(env, EINVAL);
                return -1;
            }
            let buf_size: i32 = env.mem.read(option_value.cast());

            // Rust std::net не экспортирует управление размером буфера
            // (set_recv_buffer_size).
            // Но современные ОС сами отлично балансируют TCP-окно
            // (auto-tuning), что работает
            // намного лучше фиксированных лимитов из старых iOS-приложений.
            // Честно валидируем чтение памяти гостя и подтверждаем успех.
            log_dbg!(
                "setsockopt: evaluated buffer size {:#x} to {} bytes",
                option_name,
                buf_size
            );
            0
        }
        (level, option_name) if level == IPPROTO_TCP => {
            // TCP_NODELAY — disable Nagle's algorithm.
            if option_name == TCP_NODELAY {
                if option_len < guest_size_of::<i32>() {
                    set_errno(env, EINVAL);
                    return -1;
                }
                let val: i32 = env.mem.read(option_value.cast());
                if type_ == SOCK_STREAM {
                    if let Some(stream) = State::get(env)
                        .sockets
                        .get(&socket)
                        .unwrap()
                        .tcp_stream
                        .as_ref()
                    {
                        if let Err(e) = stream.set_nodelay(val != 0) {
                            log!("setsockopt TCP_NODELAY failed: {}", e);
                            set_errno(env, EIO);
                            return -1;
                        }
                    }
                    // If stream doesn't exist yet, store it for later.
                    if val != 0 {
                        State::get_mut(env)
                            .sockets
                            .get_mut(&socket)
                            .unwrap()
                            .options
                            .insert(TCP_NODELAY);
                    }
                }
                0
            } else {
                log!(
                    "setsockopt: unhandled IPPROTO_TCP option {:#x}, ignoring",
                    option_name
                );
                0
            }
        }
        (level, option_name) => {
            log!(
                "setsockopt: unhandled level={:#x} option={:#x} on socket {}, ignoring",
                level,
                option_name,
                socket
            );
            0 // Return success rather than crashing the app
        }
    }
}

fn bind(
    env: &mut Environment,
    socket: i32,
    address: ConstPtr<sockaddr>,
    address_len: socklen_t,
) -> i32 {
    set_errno(env, 0);
    let Some(sock) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };
    let type_ = sock.type_;
    let domain = sock.domain;

    if type_ != SOCK_STREAM && type_ != SOCK_DGRAM {
        set_errno(env, ESOCKTNOSUPPORT);
        return -1;
    }

    let socket_address = match read_socket_addr(env, address, address_len)
        .and_then(|address| host_socket_addr_for_domain(address, domain))
    {
        Ok(address) => address,
        Err(error) => {
            set_errno(env, error);
            return -1;
        }
    };
    let type_str = match type_ {
        SOCK_STREAM => "TCP",
        SOCK_DGRAM => "UDP",
        _ => "<unknown socket type>",
    };
    log_dbg!(
        "bind({}, {:?}, {}) -> {} {:?}",
        socket,
        address,
        address_len,
        type_str,
        socket_address
    );
    if !env.options.network_access {
        let sock = State::get_mut(env).sockets.get_mut(&socket).unwrap();
        if sock.offline_address.is_some() {
            // already bound
            set_errno(env, EINVAL);
            return -1;
        }
        log_dbg!(
            "bind({}): network access is disabled, not binding on host",
            socket
        );
        sock.offline_address = Some(socket_address);
        return 0;
    }
    match type_ {
        SOCK_STREAM => {
            if State::get(env)
                .sockets
                .get(&socket)
                .unwrap()
                .tcp_listener
                .is_some()
            {
                set_errno(env, EINVAL);
                // already bound
                return -1;
            }
            match TcpListener::bind(socket_address) {
                Ok(host_socket) => {
                    if let Err(e) = host_socket.set_nonblocking(true) {
                        log!("bind: TCP set_nonblocking failed: {}", e);
                        set_errno(env, EIO);
                        return -1;
                    }
                    // Apply SO_REUSEADDR if set (best-effort; std doesn't
                    // expose it directly)
                    State::get_mut(env)
                        .sockets
                        .get_mut(&socket)
                        .unwrap()
                        .tcp_listener = Some(host_socket);
                }
                Err(e) => {
                    log!(
                        "bind: TcpListener::bind({:?}) failed: {}",
                        socket_address,
                        e
                    );
                    let errno = match e.kind() {
                        io::ErrorKind::AddrInUse => EADDRINUSE,
                        io::ErrorKind::AddrNotAvailable => EADDRNOTAVAIL,
                        io::ErrorKind::PermissionDenied => EACCES,
                        _ => EIO,
                    };
                    set_errno(env, errno);
                    return -1;
                }
            }
        }
        SOCK_DGRAM => {
            if State::get(env)
                .sockets
                .get(&socket)
                .unwrap()
                .udp_socket
                .is_some()
            {
                set_errno(env, EINVAL);
                // already bound
                return -1;
            }
            // Collect options before the mutable borrow below
            let options: Vec<i32> = State::get(env)
                .sockets
                .get(&socket)
                .unwrap()
                .options
                .iter()
                .copied()
                .collect();
            match UdpSocket::bind(socket_address) {
                Ok(host_socket) => {
                    if let Err(e) = host_socket.set_nonblocking(true) {
                        log!("bind: UDP set_nonblocking failed: {}", e);
                        set_errno(env, EIO);
                        return -1;
                    }
                    for option in options {
                        if option == SO_BROADCAST {
                            if let Err(e) = host_socket.set_broadcast(true) {
                                log!("bind: set_broadcast failed: {}", e);
                                set_errno(env, EIO);
                                return -1;
                            }
                        }
                    }
                    State::get_mut(env)
                        .sockets
                        .get_mut(&socket)
                        .unwrap()
                        .udp_socket = Some(host_socket);
                }
                Err(e) => {
                    log!("bind: UdpSocket::bind({:?}) failed: {}", socket_address, e);
                    let errno = match e.kind() {
                        io::ErrorKind::AddrInUse => EADDRINUSE,
                        io::ErrorKind::AddrNotAvailable => EADDRNOTAVAIL,
                        io::ErrorKind::PermissionDenied => EACCES,
                        _ => EIO,
                    };
                    set_errno(env, errno);
                    return -1;
                }
            }
        }
        other => {
            // We checked type_ is SOCK_STREAM or SOCK_DGRAM above, but be
            // defensive against future refactors.
            log!(
                "Warning: bind(): unexpected socket type {} on fd {}; returning ESOCKTNOSUPPORT.",
                other,
                socket
            );
            set_errno(env, ESOCKTNOSUPPORT);
            return -1;
        }
    }

    0 // Success
}

fn listen(env: &mut Environment, socket: i32, backlog: i32) -> i32 {
    // errno is set on every failure path below. The host socket made by
    // bind() already listens, so there is nothing else to do.
    set_errno(env, 0);
    let type_ = match State::get(env).sockets.get(&socket) {
        Some(s) => s.type_,
        None => {
            log!("listen: unknown socket fd={}, returning EBADF", socket);
            set_errno(env, EBADF);
            return -1;
        }
    };
    if type_ != SOCK_STREAM {
        set_errno(env, ESOCKTNOSUPPORT);
        return -1;
    }

    log_dbg!(
        "listen(socket: {}, backlog: {}): already listening on host",
        socket,
        backlog
    );
    0 // Success
}

fn connect(
    env: &mut Environment,
    socket: i32,
    address: ConstPtr<sockaddr>,
    address_len: socklen_t,
) -> i32 {
    set_errno(env, 0);
    let Some(sock) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };
    let type_ = sock.type_;
    let domain = sock.domain;
    if type_ != SOCK_STREAM {
        set_errno(env, ESOCKTNOSUPPORT);
        return -1;
    }

    let socket_address = match read_socket_addr(env, address, address_len)
        .and_then(|address| host_socket_addr_for_domain(address, domain))
    {
        Ok(address) => address,
        Err(error) => {
            set_errno(env, error);
            return -1;
        }
    };
    log_dbg!(
        "connect({:?}, {}) -> {:?}",
        address,
        address_len,
        socket_address
    );
    if !env.options.network_access {
        log!(
            "connect({}, {:?}): network access is disabled, returning ENETUNREACH",
            socket,
            socket_address
        );
        set_errno(env, ENETUNREACH);
        return -1;
    }

    if State::get(env)
        .sockets
        .get(&socket)
        .unwrap()
        .tcp_stream
        .is_some()
    {
        set_errno(env, EISCONN);
        return -1;
    }

    match TcpStream::connect(socket_address) {
        Ok(host_stream) => {
            if let Err(e) = host_stream.set_nonblocking(true) {
                log!("connect: set_nonblocking failed: {}", e);
                set_errno(env, EIO);
                return -1;
            }
            State::get_mut(env)
                .sockets
                .get_mut(&socket)
                .unwrap()
                .tcp_stream = Some(host_stream);
            0 // Success
        }
        Err(e) => {
            log!(
                "connect: TcpStream::connect({:?}) failed: {}",
                socket_address,
                e
            );
            let errno = match e.kind() {
                std::io::ErrorKind::ConnectionRefused => ECONNREFUSED,
                std::io::ErrorKind::TimedOut => ETIMEDOUT,
                std::io::ErrorKind::AddrNotAvailable => EADDRNOTAVAIL,
                std::io::ErrorKind::AddrInUse => EADDRINUSE,
                std::io::ErrorKind::NetworkUnreachable => ENETUNREACH,
                std::io::ErrorKind::PermissionDenied => EACCES,
                _ => EIO,
            };
            set_errno(env, errno);
            -1
        }
    }
}

fn select(
    env: &mut Environment,
    n_fds: i32,
    read_fds: MutPtr<fd_set>,
    write_fds: MutPtr<fd_set>,
    error_fds: MutPtr<fd_set>,
    timeout: MutPtr<timeval>,
) -> i32 {
    // fd_set can only describe descriptors 0..=1024; a negative or
    // out-of-range n_fds is invalid guest input. Report EINVAL instead of
    // panicking on the assert.
    set_errno(env, 0);
    if !(0..=1024).contains(&n_fds) {
        log!("select: invalid n_fds {}, returning EINVAL", n_fds);
        set_errno(env, EINVAL);
        return -1;
    }

    // POSIX: select with n_fds = 0 is a precise (microsecond) sleep.
    if n_fds == 0 {
        if !timeout.is_null() {
            let timeval = env.mem.read(timeout);
            if timeval.tv_sec > 0 || timeval.tv_usec > 0 {
                let total_sleep =
                    std::time::Duration::from_secs(timeval.tv_sec.try_into().unwrap_or(0))
                        + std::time::Duration::from_micros(timeval.tv_usec.try_into().unwrap_or(0));
                env.sleep(total_sleep);
            }
        }
        return 0;
        // Ни один дескриптор не готов
    }

    // Read the timeout once: a null pointer means block indefinitely, a
    // zero timeout means a single poll.
    let timeout_duration: Option<std::time::Duration> = if !timeout.is_null() {
        let timeval = env.mem.read(timeout);
        if timeval.tv_sec < 0 || timeval.tv_usec < 0 {
            set_errno(env, EINVAL);
            return -1;
        }
        Some(
            std::time::Duration::from_secs(timeval.tv_sec.try_into().unwrap_or(0))
                + std::time::Duration::from_micros(timeval.tv_usec.try_into().unwrap_or(0)),
        )
    } else {
        None
    };
    // Poll repeatedly, sleeping between polls, until something is ready or
    // the timeout expires. env.sleep() yields the guest thread so other
    // threads keep running while we wait.
    let deadline = timeout_duration.map(|d| std::time::Instant::now() + d);
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

    // Snapshot the fd sets once; every poll starts from a fresh copy because
    // processing clears bits of not-ready descriptors.
    let read_orig = (!read_fds.is_null()).then(|| env.mem.read(read_fds));
    let write_orig = (!write_fds.is_null()).then(|| env.mem.read(write_fds));
    let error_orig = (!error_fds.is_null()).then(|| env.mem.read(error_fds));

    let mut count;
    let mut invalid_fd = false;
    loop {
        count = 0;
        if let Some(orig) = read_orig {
            let mut read_set = orig;
            log_dbg!("select: read_set before {:?}", read_set);
            count += process_set(env, &mut read_set, n_fds, |env, fd, bits, bit_index| {
                log_dbg!("select: bit set in read_set at fd: {}", fd);
                if !is_socket(env, fd) {
                    log!("select: read_set contains non-socket fd {}; ignoring", fd);
                    *bits &= !(1 << bit_index);
                    invalid_fd = true;
                    return false;
                }
                // Clean bit in the set for the current socket
                *bits &= !(1 << bit_index);
                let socket_host_object = match State::get(env).sockets.get(&fd) {
                    Some(s) => s,
                    None => return false,
                };
                let type_ = socket_host_object.type_;
                match type_ {
                    SOCK_DGRAM => {
                        let Some(udp_socket) = socket_host_object.udp_socket.as_ref() else {
                            // No host socket yet (never bound nor sent to):
                            // nothing can be readable; treat as not-ready.
                            return false;
                        };
                        // Peek just one byte to check if we have some data
                        let mut buf = [0; 1];
                        match udp_socket.peek(&mut buf) {
                            Ok(received) => {
                                log_dbg!("select: Socket {} peeked {} bytes", fd, received);
                                // Set bit back
                                *bits |= 1 << bit_index;
                                true
                            }
                            // On Windows, if we receive more bytes than we peek,
                            // it will error, but it means that there is some data!
                            Err(ref e)
                                if cfg!(target_os = "windows")
                                    && e.raw_os_error() == Some(10040) =>
                            {
                                // 10040 code is WSAEMSGSIZE
                                log_dbg!(
                                    "[Windows case] select: received {} bytes (at least)",
                                    buf.len()
                                );
                                // Set bit back
                                *bits |= 1 << bit_index;
                                true
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                log_dbg!("select: Socket {} would block on peeking, continue.", fd);
                                // Not ready; the caller loop will re-poll or
                                // expire the timeout as appropriate.
                                false
                            }
                            Err(e) => {
                                log!(
                                    "select: Peek for socket {fd} failed: {e:?}; treating as not-ready."
                                );
                                false
                            }
                        }
                    }
                    SOCK_STREAM => {
                        if socket_host_object.tcp_stream.is_none() {
                            // If we don't have a TCP stream it probably means
                            // that a listener is waiting for connection
                            let Some(listener) = socket_host_object.tcp_listener.as_ref() else {
                                // Neither stream nor listener yet: not ready.
                                return false;
                            };
                            // The listener is non-blocking,
                            // so we can try to accept
                            match listener.accept() {
                                Ok((stream, addr)) => {
                                    log!("select: New client: {}", addr);
                                    // We set host socket as non-blocking in
                                    // order to have more control of how and
                                    // when it's used
                                    if let Err(e) = stream.set_nonblocking(true) {
                                        // If we cannot make the stream
                                        // non-blocking, drop it and report
                                        // not-ready rather than risking a
                                        // blocking host call later.
                                        log!("select: set_nonblocking failed: {}", e);
                                        return false;
                                    }
                                    // We already accepted the connection on
                                    // the host, but we need to postpone new
                                    // guest fd creation up until guest calls
                                    // accept()
                                    if socket_host_object.pending_tcp_stream.is_some() {
                                        log!(
                                            "select: socket {} already has a pending stream; dropping new one",
                                            fd
                                        );
                                    } else {
                                        State::get_mut(env)
                                            .sockets
                                            .get_mut(&fd)
                                            .unwrap()
                                            .pending_tcp_stream = Some(stream);
                                    }
                                    // Set bit back
                                    *bits |= 1 << bit_index;
                                    return true;
                                }
                                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                    // No incoming connection is ready
                                    log_dbg!(
                                        "select: TCP listener for socket {} would block on accepting, continue.",
                                        fd
                                    );
                                    // Not ready; the caller loop will re-poll
                                    // or expire the timeout as appropriate.
                                    return false;
                                }
                                Err(e) => {
                                    log!(
                                        "select: Socket {fd} has error accepting connection: {e}; treating as not-ready."
                                    );
                                    return false;
                                }
                            }
                        }
                        let Some(stream) = socket_host_object.tcp_stream.as_ref() else {
                            // Neither listening on nor connected to the host
                            // network (e.g. bound while network access is
                            // disabled), so it never becomes readable.
                            return false;
                        };
                        // Peek just one byte to check if we have some data
                        let mut buf = [0; 1];
                        match stream.peek(&mut buf) {
                            Ok(received) => {
                                log_dbg!("select: received {} bytes (at least)", received);
                                // Set bit back
                                *bits |= 1 << bit_index;
                                true
                            }
                            // On Windows, if we receive more bytes than we peek,
                            // it will error, but it means that there is some data!
                            Err(ref e)
                                if cfg!(target_os = "windows")
                                    && e.raw_os_error() == Some(10040) =>
                            {
                                // 10040 code is WSAEMSGSIZE
                                log_dbg!(
                                    "[Windows case] select: received {} bytes (at least)",
                                    buf.len()
                                );
                                // Set bit back
                                *bits |= 1 << bit_index;
                                true
                            }
                            // As tested on macOS, this marks socket as readable
                            Err(ref e) if e.kind() == io::ErrorKind::ConnectionReset => {
                                log!("select: Peek for socket {}: ConnectionReset", fd);
                                // Set bit back
                                *bits |= 1 << bit_index;
                                true
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                log_dbg!(
                                    "select: TCP stream for socket {} would block on peeking, continue.",
                                    fd
                                );
                                // Not ready; the caller loop will re-poll or
                                // expire the timeout as appropriate.
                                false
                            }
                            Err(e) => {
                                log!(
                                    "select: Peek for socket {fd} failed: {e}; treating as not-ready."
                                );
                                false
                            }
                        }
                    }
                    other => {
                        log!(
                            "Warning: select() read_set fd {} has unknown socket type {}; \
                             treating as not-ready.",
                            fd,
                            other
                        );
                        false
                    }
                }
            });
            log_dbg!("select: read_set after {:?}", read_set);
            env.mem.write(read_fds, read_set);
        }

        if let Some(orig) = write_orig {
            let mut write_set = orig;
            log_dbg!("select: write_set before {:?}", write_set);
            count += process_set(env, &mut write_set, n_fds, |env, fd, bits, bit_index| {
                log_dbg!("select: bit set in write_set at fd: {}", fd);
                if !is_socket(env, fd) {
                    log!("select: write_set contains non-socket fd {}; ignoring", fd);
                    *bits &= !(1 << bit_index);
                    invalid_fd = true;
                    return false;
                }
                // Clean bit in the current socket set
                *bits &= !(1 << bit_index);
                let socket_host_object = match State::get(env).sockets.get(&fd) {
                    Some(s) => s,
                    None => return false,
                };
                let type_ = socket_host_object.type_;
                match type_ {
                    SOCK_STREAM => {
                        // A connected stream is writable; a listening socket
                        // is too (it can accept). Without either there is
                        // nothing to write to yet.
                        if socket_host_object.tcp_stream.is_some()
                            || socket_host_object.tcp_listener.is_some()
                        {
                            // Set bit back
                            *bits |= 1 << bit_index;
                            true
                        } else {
                            false
                        }
                    }
                    SOCK_DGRAM => {
                        // UDP sockets are almost always writable once they
                        // exist; without a host socket there is nothing to
                        // write with yet.
                        if socket_host_object.udp_socket.is_some() {
                            // Set bit back
                            *bits |= 1 << bit_index;
                            true
                        } else {
                            false
                        }
                    }
                    other => {
                        log!(
                            "Warning: select() write_set fd {} has unknown socket type {}; \
                             treating as not-ready.",
                            fd,
                            other
                        );
                        false
                    }
                }
            });
            log_dbg!("select: write_set after {:?}", write_set);
            env.mem.write(write_fds, write_set);
        }

        if let Some(orig) = error_orig {
            let mut error_set = orig;
            log_dbg!("select: error_set before {:?}", error_set);
            count += process_set(env, &mut error_set, n_fds, |env, fd, bits, bit_index| {
                log_dbg!("select: bit set in error_set at fd: {}", fd);
                if !is_socket(env, fd) {
                    log!("select: error_set contains non-socket fd {}; ignoring", fd);
                    *bits &= !(1 << bit_index);
                    invalid_fd = true;
                    return false;
                }
                // Clean bit in the current socket set
                *bits &= !(1 << bit_index);
                let socket_host_object = match State::get(env).sockets.get(&fd) {
                    Some(s) => s,
                    None => return false,
                };
                let type_ = socket_host_object.type_;
                match type_ {
                    SOCK_STREAM => {
                        let Some(stream) = socket_host_object.tcp_stream.as_ref() else {
                            // No connected stream: no error state to report.
                            return false;
                        };
                        match stream.take_error() {
                            Ok(None) => {
                                log_dbg!("No error on TCP socket {}", fd);
                                false
                            }
                            Ok(Some(error)) => {
                                log!(
                                    "select: TCP socket {} has a pending error: {:?}; \
                                     reporting it via the error_fds set.",
                                    fd,
                                    error
                                );
                                // Set bit back so the guest observes the error
                                // on this socket rather than us panicking.
                                true
                            }
                            Err(error) => {
                                log!(
                                    "select: TCP socket {fd} take_error failed: {error:?}; treating as no-error."
                                );
                                false
                            }
                        }
                    }
                    SOCK_DGRAM => {
                        // UDP is connectionless, so there is no per-socket
                        // error state comparable to TCP's take_error(). Real
                        // BSD sockets can still surface async errors (e.g.
                        // from a previous ICMP port-unreachable) via
                        // SO_ERROR, but we don't track that here; report
                        // "no error" rather than aborting the emulator.
                        false
                    }
                    other => {
                        log!(
                            "Warning: select() error_set fd {} has unknown socket type {}; \
                             treating as no-error.",
                            fd,
                            other
                        );
                        false
                    }
                }
            });
            log_dbg!("select: error_set after {:?}", error_set);
            env.mem.write(error_fds, error_set);
        }

        if count > 0 || invalid_fd {
            break;
        }
        match deadline {
            Some(deadline) => {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                env.sleep(POLL_INTERVAL.min(deadline - now));
            }
            None => env.sleep(POLL_INTERVAL),
        }
    }
    if invalid_fd {
        set_errno(env, EBADF);
        return -1;
    }
    count
}

fn process_set<F: FnMut(&mut Environment, FileDescriptor, &mut i32, i32) -> bool>(
    env: &mut Environment,
    set: &mut fd_set,
    n_fds: i32,
    mut process_bit: F,
) -> i32 {
    let mut fds_bits = set.fds_bits;
    let mut count = 0;
    'outer: for (i, bits) in fds_bits.iter_mut().enumerate() {
        for bit_index in 0..32i32 {
            let fd: FileDescriptor = (i as i32) * 32 + bit_index;
            if fd >= n_fds {
                break 'outer;
            }
            if (*bits & (1 << bit_index)) != 0 && process_bit(env, fd, bits, bit_index) {
                count += 1;
            }
        }
    }
    set.fds_bits = fds_bits;
    count
}

fn accept(
    env: &mut Environment,
    socket: i32,
    address: MutPtr<sockaddr>,
    address_len: MutPtr<socklen_t>,
) -> FileDescriptor {
    // errno is set on every failure path below.
    set_errno(env, 0);
    let Some(socket_host_object) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };
    let type_ = socket_host_object.type_;
    let domain = socket_host_object.domain;
    if type_ != SOCK_STREAM {
        // accept(2) is only defined for stream sockets; some apps probe
        // other types, so fail with POSIX semantics instead of aborting.
        log!(
            "accept: socket {} has unsupported type {}; returning EOPNOTSUPP",
            socket,
            type_
        );
        set_errno(env, ENOTSUP);
        return -1;
    }

    if let Some(stream) = State::get_mut(env)
        .sockets
        .get_mut(&socket)
        .unwrap()
        .pending_tcp_stream
        .take()
    {
        // peer_addr() can fail if the peer reset the connection between
        // select() and accept(); fall back to a wildcard address instead
        // of crashing.
        let addr = stream
            .peer_addr()
            .unwrap_or_else(|_| unspecified_socket_addr(domain));
        // We have already accepted TCP socket, we now need to
        // let guest know as well!
        let new_fd = find_or_create_socket(env);
        if State::get(env).sockets.contains_key(&new_fd) {
            log!("Warning: accept(): fd {} had a stale socket entry", new_fd);
        }
        let host_object = SocketHostObject {
            type_: SOCK_STREAM,
            options: Default::default(),
            tcp_listener: None,
            pending_tcp_stream: None,
            tcp_stream: Some(stream),
            udp_socket: None,
            domain,
            offline_address: None,
        };
        State::get_mut(env).sockets.insert(new_fd, host_object);
        if !address.is_null() {
            write_sockaddr_bounded(env, address, address_len, addr, domain);
        }
        return new_fd;
    }

    // re-borrow
    let socket_host_object = State::get(env).sockets.get(&socket).unwrap();
    let Some(listener) = socket_host_object.tcp_listener.as_ref() else {
        if socket_host_object.offline_address.is_some() {
            // Listening without network access: no client can connect.
            set_errno(env, EAGAIN);
        } else {
            log!(
                "accept: socket {} is not listening, returning EINVAL",
                socket
            );
            set_errno(env, EINVAL);
        }
        return -1;
    };
    match listener.accept() {
        Ok((stream, addr)) => {
            log!("accept: New client: {}", addr);
            // FIX: was unimplemented!() — a direct (non-select-driven) accept()
            // that got a connection immediately used to panic the whole
            // emulator. Mirror the select() path above: register the new
            // stream as its own guest socket and report the peer address,
            // exactly like a real accept(2) does.
            stream.set_nonblocking(true).unwrap();
            let new_fd = find_or_create_socket(env);
            if State::get(env).sockets.contains_key(&new_fd) {
                log!("Warning: accept(): fd {} had a stale socket entry", new_fd);
            }
            let host_object = SocketHostObject {
                type_: SOCK_STREAM,
                options: Default::default(),
                tcp_listener: None,
                pending_tcp_stream: None,
                tcp_stream: Some(stream),
                udp_socket: None,
                domain,
                offline_address: None,
            };
            State::get_mut(env).sockets.insert(new_fd, host_object);
            if !address.is_null() {
                write_sockaddr_bounded(env, address, address_len, addr, domain);
            }
            new_fd
        }
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
            // No incoming connection is ready.
            // FIX: was unimplemented!() — a blocking accept() with no
            // pending connection used to crash the emulator instead of
            // reporting EAGAIN/EWOULDBLOCK like a real non-blocking accept(2)
            // would. Guest apps are expected to poll via select()/accept()
            // in a loop, so surfacing EAGAIN here lets that pattern work
            // instead of aborting the whole process.
            log_dbg!(
                "accept: TCP listener for socket {} would block on accepting; \
                 returning EAGAIN for thread {}",
                socket,
                env.current_thread
            );
            set_errno(env, EAGAIN);
            -1
        }
        Err(e) => {
            // accept(2) reports transient connection aborts via
            // ECONNABORTED; apps retry, so do not crash the emulator.
            log!("accept: Socket {socket} error accepting connection: {e}");
            set_errno(env, ECONNABORTED);
            -1
        }
    }
}

fn recv(
    env: &mut Environment,
    socket: i32,
    buffer: MutVoidPtr,
    length: GuestUSize,
    flags: i32,
) -> i32 {
    recvfrom(env, socket, buffer, length, flags, Ptr::null(), Ptr::null())
}

fn blocking_recvfrom_should_wait(nonblocking: bool, flags: i32, has_host_socket: bool) -> bool {
    !nonblocking && flags & MSG_DONTWAIT == 0 && has_host_socket
}

fn try_recvfrom(
    env: &mut Environment,
    socket: i32,
    type_: i32,
    buffer: MutVoidPtr,
    length: GuestUSize,
    peek: bool,
) -> Result<(usize, Option<SocketAddr>), i32> {
    let Some(socket_host_object) = env.libc_state.socket.sockets.get(&socket) else {
        return Err(EBADF);
    };
    if socket_host_object.type_ != type_ {
        return Err(EBADF);
    }

    match type_ {
        SOCK_DGRAM => {
            let Some(udp_socket) = socket_host_object.udp_socket.as_ref() else {
                return Err(EAGAIN);
            };
            let buf = env.mem.bytes_at_mut(buffer.cast(), length);
            let result = if peek {
                udp_socket.peek_from(buf)
            } else {
                udp_socket.recv_from(buf)
            };
            match result {
                Ok((read, address)) => Ok((read, Some(address))),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => Err(EAGAIN),
                Err(e) => {
                    log!("recvfrom: UDP socket {socket} IO error: {e}");
                    Err(EIO)
                }
            }
        }
        SOCK_STREAM => {
            let Some(mut tcp_stream) = socket_host_object.tcp_stream.as_ref() else {
                return Err(ENOTCONN);
            };
            let buf = env.mem.bytes_at_mut(buffer.cast(), length);
            let read = match if peek {
                tcp_stream.peek(buf)
            } else {
                tcp_stream.read(buf)
            } {
                Ok(read) => read,
                Err(ref e) if e.kind() == io::ErrorKind::ConnectionReset => {
                    log!("recvfrom: TCP socket {socket}: ConnectionReset");
                    return Err(ECONNRESET);
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => return Err(EAGAIN),
                Err(e) => {
                    log!("recvfrom: TCP socket {socket} IO error: {e}");
                    return Err(EIO);
                }
            };
            Ok((read, tcp_stream.peer_addr().ok()))
        }
        _ => Err(ESOCKTNOSUPPORT),
    }
}

fn recvfrom(
    env: &mut Environment,
    socket: i32,
    buffer: MutVoidPtr,
    length: GuestUSize,
    flags: i32,
    address: MutPtr<sockaddr>,
    address_len: MutPtr<socklen_t>,
) -> i32 {
    // errno is set on every failure path below.
    set_errno(env, 0);

    log_dbg!(
        "recvfrom({}, {:?}, {}, {}, {:?}, {:?})",
        socket,
        buffer,
        length,
        flags,
        address,
        address_len
    );
    if !State::get(env).sockets.contains_key(&socket) {
        set_errno(env, EBADF);
        log!(
            "Warning: recvfrom({}, ...) failed for unknown socket, returning -1",
            socket
        );
        return -1;
    }

    let type_ = match State::get(env).sockets.get(&socket) {
        Some(s) => s.type_,
        None => {
            log!("socket op: unknown fd={}, returning EBADF", socket);
            set_errno(env, EBADF);
            return -1;
        }
    };
    let domain = State::get(env).sockets.get(&socket).unwrap().domain;
    if type_ != SOCK_STREAM && type_ != SOCK_DGRAM {
        set_errno(env, ESOCKTNOSUPPORT);
        return -1;
    }

    // MSG_PEEK and MSG_DONTWAIT are handled below; MSG_WAITALL is currently a
    // no-op and MSG_NOSIGNAL is unnecessary because we never raise SIGPIPE.
    const MSG_PEEK: i32 = 0x1;
    const MSG_WAITALL: i32 = 0x40;
    const MSG_NOSIGNAL: i32 = 0x800;
    let peek = flags & MSG_PEEK != 0;
    let nonblocking = is_nonblocking(env, socket);
    let ignored = flags & !(MSG_PEEK | MSG_DONTWAIT | MSG_WAITALL | MSG_NOSIGNAL);
    if ignored != 0 {
        log!(
            "Warning: recvfrom({}, ...) ignoring unsupported flags {:#x}",
            socket,
            ignored
        );
    }

    // RakNet's RecvFromLoop uses a blocking UDP socket and immediately retries
    // after a negative recvfrom result. Returning EAGAIN while idle makes that
    // guest thread spin instead of waiting for a packet.
    loop {
        match try_recvfrom(env, socket, type_, buffer, length, peek) {
            Ok((num_bytes_read, addr)) => {
                if let Some(addr) = addr {
                    if !address.is_null() {
                        write_sockaddr_bounded(env, address, address_len, addr, domain);
                    }
                }
                log_dbg!(
                    "recvfrom: Socket {} received {} bytes from addr {:?}",
                    socket,
                    num_bytes_read,
                    addr
                );
                // Cap at i32::MAX so a huge (guest-bug) count can never fail the
                // conversion to the ssize_t-compatible return type.
                return num_bytes_read.min(i32::MAX as usize) as i32;
            }
            Err(EAGAIN) => {
                let has_host_socket =
                    State::get(env)
                        .sockets
                        .get(&socket)
                        .is_some_and(|socket| match type_ {
                            SOCK_DGRAM => socket.udp_socket.is_some(),
                            SOCK_STREAM => socket.tcp_stream.is_some(),
                            _ => false,
                        });
                if blocking_recvfrom_should_wait(nonblocking, flags, has_host_socket) {
                    env.sleep(BLOCKING_IO_POLL_INTERVAL);
                } else {
                    set_errno(env, EAGAIN);
                    return -1;
                }
            }
            Err(errno) => {
                set_errno(env, errno);
                return -1;
            }
        }
    }
}

fn send(
    env: &mut Environment,
    socket: i32,
    buffer: MutVoidPtr,
    length: GuestUSize,
    flags: i32,
) -> i32 {
    set_errno(env, 0);
    let Some(sock) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };
    let type_ = sock.type_;
    // MSG_NOSIGNAL is a no-op here (we never raise SIGPIPE) and MSG_DONTWAIT
    // matches our always-non-blocking host sockets. MSG_OOB is genuinely
    // unsupported; ignore it with a warning rather than crashing.
    const MSG_OOB: i32 = 0x4;
    if flags & MSG_OOB != 0 {
        log!(
            "Warning: send({}, ...) ignoring unsupported MSG_OOB flag",
            socket
        );
    }

    match type_ {
        SOCK_STREAM => {
            let Some(mut stream) = State::get(env)
                .sockets
                .get(&socket)
                .unwrap()
                .tcp_stream
                .as_ref()
            else {
                set_errno(env, ENOTCONN);
                return -1;
            };
            let buf = env.mem.bytes_at(buffer.cast(), length);
            match stream.write(buf) {
                Ok(n) => {
                    log_dbg!("send: wrote {} bytes to TCP socket {}", n, socket);
                    n.min(i32::MAX as usize) as i32
                }
                Err(ref e)
                    if e.kind() == io::ErrorKind::BrokenPipe
                        || e.kind() == io::ErrorKind::ConnectionReset =>
                {
                    log!("send: TCP socket {} connection lost: {}", socket, e);
                    set_errno(env, ECONNRESET);
                    -1
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    log_dbg!("send: TCP socket {} would block", socket);
                    set_errno(env, EAGAIN);
                    -1
                }
                Err(e) => {
                    log!("send: TCP socket {} IO error: {}", socket, e);
                    set_errno(env, EIO);
                    -1
                }
            }
        }
        SOCK_DGRAM => {
            // send() on a connected UDP socket — use send() not send_to()
            let Some(udp) = State::get(env)
                .sockets
                .get(&socket)
                .unwrap()
                .udp_socket
                .as_ref()
            else {
                set_errno(env, EBADF);
                return -1;
            };
            let buf = env.mem.bytes_at(buffer.cast(), length);
            match udp.send(buf) {
                Ok(n) => {
                    log_dbg!("send: sent {} bytes on UDP socket {}", n, socket);
                    n.min(i32::MAX as usize) as i32
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    set_errno(env, EAGAIN);
                    -1
                }
                Err(e) => {
                    log!("send: UDP socket {} IO error: {}", socket, e);
                    set_errno(env, EIO);
                    -1
                }
            }
        }
        _ => {
            set_errno(env, ESOCKTNOSUPPORT);
            -1
        }
    }
}

fn sendto(
    env: &mut Environment,
    socket: i32,
    buffer: MutVoidPtr,
    length: GuestUSize,
    flags: i32,
    dest_address: MutPtr<sockaddr>,
    dest_address_len: socklen_t,
) -> i32 {
    // errno is set on every failure path below.
    set_errno(env, 0);
    let type_ = match State::get(env).sockets.get(&socket) {
        Some(s) => s.type_,
        None => {
            log!("sendto: unknown socket fd={}, returning EBADF", socket);
            set_errno(env, EBADF);
            return -1;
        }
    };
    let domain = State::get(env).sockets.get(&socket).unwrap().domain;
    if type_ != SOCK_DGRAM {
        log!(
            "sendto: socket fd={} is not SOCK_DGRAM, returning ESOCKTNOSUPPORT",
            socket
        );
        set_errno(env, ESOCKTNOSUPPORT);
        return -1;
    }

    if flags != 0 {
        log!("sendto: flags={} ignored", flags);
    }

    let socket_address = match read_socket_addr(env, dest_address.cast_const(), dest_address_len)
        .and_then(|address| host_socket_addr_for_domain(address, domain))
    {
        Ok(address) => address,
        Err(error) => {
            set_errno(env, error);
            return -1;
        }
    };
    log_dbg!(
        "sendto({}, {:?}, {}, {}, {:?}, {:?}, {})",
        socket,
        buffer,
        length,
        flags,
        dest_address,
        socket_address,
        dest_address_len
    );
    if !env.options.network_access {
        log!(
            "sendto({}, {:?}): network access is disabled, returning ENETUNREACH",
            socket,
            socket_address
        );
        set_errno(env, ENETUNREACH);
        return -1;
    }
    let num_bytes_written = match type_ {
        SOCK_DGRAM => {
            if State::get(env)
                .sockets
                .get(&socket)
                .unwrap()
                .udp_socket
                .is_none()
            {
                // Lazy host socket creation: binding to the wildcard with
                // an ephemeral port matches what a fresh BSD socket does on
                // the first sendto(2). This must work for any destination,
                // not just broadcast ones.
                let host_socket = match UdpSocket::bind(if socket_address.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                }) {
                    Ok(s) => s,
                    Err(e) => {
                        log!("sendto: lazy UdpSocket::bind failed: {}", e);
                        set_errno(env, EADDRNOTAVAIL);
                        return -1;
                    }
                };
                // We set host socket as non-blocking in order to have
                // more control of how and when it's used
                if let Err(e) = host_socket.set_nonblocking(true) {
                    log!("sendto: set_nonblocking failed: {}", e);
                    set_errno(env, EIO);
                    return -1;
                }
                for &option in &State::get(env).sockets.get(&socket).unwrap().options {
                    if option == SO_BROADCAST {
                        if let Err(e) = host_socket.set_broadcast(true) {
                            log!("sendto: set_broadcast failed: {}", e);
                            set_errno(env, EIO);
                            return -1;
                        }
                    }
                }
                State::get_mut(env)
                    .sockets
                    .get_mut(&socket)
                    .unwrap()
                    .udp_socket = Some(host_socket);
            }
            // A UDP socket with no host socket yet (never bound nor sent
            // to) has nothing to receive; EAGAIN fits the non-blocking
            // model instead of crashing on the unwrap.
            let udp_socket = match env
                .libc_state
                .socket
                .sockets
                .get(&socket)
                .and_then(|s| s.udp_socket.as_ref())
            {
                Some(udp) => udp,
                None => {
                    set_errno(env, EAGAIN);
                    return -1;
                }
            };
            if matches!(socket_address, SocketAddr::V4(address) if address.ip().is_broadcast()) {
                match udp_socket.local_addr() {
                    Ok(local) if !local.ip().is_unspecified() => {
                        log!(
                            "Warning: sendto: broadcast from bound socket {}                              may not reach all hosts",
                            socket
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log!("Warning: sendto: local_addr failed: {}", e);
                    }
                }
            }
            let buf = env.mem.bytes_at(buffer.cast(), length);
            match udp_socket.send_to(buf, socket_address) {
                Ok(written) => written,
                // FIX: was unimplemented!() — return EAGAIN so the app's
                // non-blocking network loop can retry without crashing.
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    log_dbg!(
                        "sendto: UDP socket {} would block on sending, \
                        returning EAGAIN for thread {}",
                        socket,
                        env.current_thread
                    );
                    set_errno(env, EAGAIN);
                    return -1;
                }
                Err(e) => {
                    log!("sendto: Socket {socket} IO error: {e}");
                    let errno = match e.kind() {
                        io::ErrorKind::NetworkUnreachable => ENETUNREACH,
                        _ => EIO,
                    };
                    set_errno(env, errno);
                    return -1;
                }
            }
        }
        _ => unreachable!(),
    };
    log_dbg!(
        "sendto: written {} bytes to UDP socket {} (address {:?})",
        num_bytes_written,
        socket,
        socket_address
    );
    num_bytes_written.min(i32::MAX as usize) as i32
}

const SHUT_RD: i32 = 0;
const SHUT_WR: i32 = 1;
const SHUT_RDWR: i32 = 2;
fn shutdown(env: &mut Environment, socket: i32, how: i32) -> i32 {
    log_dbg!("shutdown({}, {})", socket, how);
    let Some(sock) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };
    let host_shutdown = match how {
        SHUT_RD => Some(std::net::Shutdown::Read),
        SHUT_WR => Some(std::net::Shutdown::Write),
        SHUT_RDWR => None, // handled by close() below
        _ => {
            set_errno(env, EINVAL);
            return -1;
        }
    };
    if let Some(direction) = host_shutdown {
        // A partial shutdown leaves the fd valid, so only touch an
        // underlying connected stream; listeners and UDP sockets are
        // effectively unaffected by SHUT_RD/SHUT_WR.
        if let Some(stream) = sock.tcp_stream.as_ref() {
            if let Err(e) = stream.shutdown(direction) {
                log!("shutdown: socket {} stream shutdown failed: {}", socket, e);
                set_errno(env, EIO);
                return -1;
            }
        }
        return 0;
    }
    close(env, socket)
}

fn getsockname(
    env: &mut Environment,
    socket: i32,
    address: MutPtr<sockaddr>,
    address_len: MutPtr<socklen_t>,
) -> i32 {
    set_errno(env, 0);
    let Some(sock) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };
    let domain = sock.domain;
    let local_addr: SocketAddr = match sock.type_ {
        SOCK_STREAM => {
            if let Some(stream) = &sock.tcp_stream {
                match stream.local_addr() {
                    Ok(addr) => addr,
                    Err(e) => {
                        log!("getsockname: local_addr failed: {}", e);
                        set_errno(env, EINVAL);
                        return -1;
                    }
                }
            } else if let Some(listener) = &sock.tcp_listener {
                match listener.local_addr() {
                    Ok(addr) => addr,
                    Err(e) => {
                        log!("getsockname: listener local_addr failed: {}", e);
                        set_errno(env, EINVAL);
                        return -1;
                    }
                }
            } else if let Some(addr) = sock.offline_address {
                addr
            } else {
                set_errno(env, EINVAL);
                return -1;
            }
        }
        SOCK_DGRAM => {
            if let Some(udp) = &sock.udp_socket {
                match udp.local_addr() {
                    Ok(addr) => addr,
                    Err(e) => {
                        log!("getsockname: udp local_addr failed: {}", e);
                        set_errno(env, EINVAL);
                        return -1;
                    }
                }
            } else if let Some(addr) = sock.offline_address {
                addr
            } else {
                set_errno(env, EINVAL);
                return -1;
            }
        }
        _ => {
            set_errno(env, EBADF);
            return -1;
        }
    };

    if !address.is_null() {
        write_sockaddr_bounded(env, address, address_len, local_addr, domain);
    }
    0
}

fn getpeername(
    env: &mut Environment,
    socket: i32,
    address: MutPtr<sockaddr>,
    address_len: MutPtr<socklen_t>,
) -> i32 {
    set_errno(env, 0);
    let Some(sock) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };
    let domain = sock.domain;
    let peer_addr: SocketAddr = match &sock.tcp_stream {
        Some(stream) => match stream.peer_addr() {
            Ok(addr) => addr,
            Err(e) => {
                log!("getpeername: peer_addr failed: {}", e);
                set_errno(env, EINVAL);
                return -1;
            }
        },
        None => {
            set_errno(env, ENOTCONN);
            return -1;
        }
    };

    if !address.is_null() {
        write_sockaddr_bounded(env, address, address_len, peer_addr, domain);
    }
    0
}

/// Readiness одного сокета для poll(2): логика зеркальна select(), включая
/// неблокирующий accept-пробник для слушателя (готовое соединение
/// откладывается в `pending_tcp_stream` до гостевого `accept()`).
/// Возвращает `revents`: нормальные биты отфильтрованы по `events`,
/// POLLERR/POLLHUP — безусловно (POSIX).
pub(crate) fn socket_poll_events(
    env: &mut Environment,
    fd: FileDescriptor,
    events: i16,
) -> i16 {
    let want_read = events & (POLLIN | POLLPRI) != 0;
    let want_write = events & POLLOUT != 0;

    // Фаза 0: копии флагов — чтобы не держать заимствование state,
    // пока в фазах 1–2 делаются новые обращения к env.
    let (type_, has_stream, has_listener, has_udp) = {
        let Some(sock) = State::get(env).sockets.get(&fd) else {
            // Записи в сокет-таблице нет — как в select(): не готов.
            return 0;
        };
        (
            sock.type_,
            sock.tcp_stream.is_some(),
            sock.tcp_listener.is_some(),
            sock.udp_socket.is_some(),
        )
    };

    let mut ready = 0i16;
    let mut hangup = false;

    // Фаза 1: опрос ошибок TCP (POLLERR безусловный).
    if type_ == SOCK_STREAM && has_stream {
        if let Some(sock) = State::get(env).sockets.get(&fd) {
            if let Some(stream) = sock.tcp_stream.as_ref() {
                match stream.take_error() {
                    Ok(Some(_)) => ready |= POLLERR,
                    Ok(None) => {}
                    Err(e) => log_dbg!("poll: take_error for socket {} failed: {}", fd, e),
                }
            }
        }
    }

    // Фаза 2: чтение.
    if want_read {
        if type_ == SOCK_STREAM && has_stream {
            if let Some(sock) = State::get(env).sockets.get(&fd) {
                if let Some(stream) = sock.tcp_stream.as_ref() {
                    let mut buf = [0u8; 1];
                    match stream.peek(&mut buf) {
                        // EOF: remote закрыл соединение → читаемо + HUP.
                        Ok(0) => {
                            ready |= POLLIN | POLLRDNORM;
                            hangup = true;
                        }
                        Ok(_) => ready |= POLLIN | POLLRDNORM,
                        // On Windows, if we receive more bytes than we peek,
                        // it will error, but it means that there is some data.
                        Err(ref e)
                            if cfg!(target_os = "windows")
                                && e.raw_os_error() == Some(10040) =>
                        {
                            ready |= POLLIN | POLLRDNORM;
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(e) => log_dbg!("poll: peek for socket {} failed: {}", fd, e),
                    }
                }
            }
        } else if type_ == SOCK_STREAM && has_listener {
            // Пробник accept() — только по POLLIN (побочный эффект: новое
            // соединение сразу принимается на хосте, как в select()).
            let accepted = if let Some(sock) = State::get(env).sockets.get(&fd) {
                match sock.tcp_listener.as_ref() {
                    Some(listener) => match listener.accept() {
                        Ok((stream, addr)) => {
                            log!("poll: New client: {}", addr);
                            if let Err(e) = stream.set_nonblocking(true) {
                                log!("poll: set_nonblocking failed: {}", e);
                                None
                            } else {
                                Some(stream)
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
                        Err(e) => {
                            log!("poll: accept for socket {} failed: {}", fd, e);
                            None
                        }
                    },
                    None => None,
                }
            } else {
                None
            };
            if let Some(stream) = accepted {
                if let Some(sock) = State::get_mut(env).sockets.get_mut(&fd) {
                    if sock.pending_tcp_stream.is_some() {
                        log!(
                            "poll: socket {} already has a pending stream; dropping new one",
                            fd
                        );
                    } else {
                        sock.pending_tcp_stream = Some(stream);
                    }
                }
                ready |= POLLIN | POLLRDNORM;
            }
        } else if type_ == SOCK_DGRAM && has_udp {
            if let Some(sock) = State::get(env).sockets.get(&fd) {
                if let Some(udp) = sock.udp_socket.as_ref() {
                    let mut buf = [0u8; 1];
                    match udp.peek(&mut buf) {
                        Ok(_) => ready |= POLLIN | POLLRDNORM,
                        Err(ref e)
                            if cfg!(target_os = "windows")
                                && e.raw_os_error() == Some(10040) =>
                        {
                            ready |= POLLIN | POLLRDNORM;
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(e) => log_dbg!("poll: peek for UDP socket {} failed: {}", fd, e),
                    }
                }
            }
        }
    }

    // Фаза 3: запись (по флагам, без заимствований).
    if want_write {
        if type_ == SOCK_STREAM && (has_stream || has_listener) {
            ready |= POLLOUT | POLLWRNORM;
        } else if type_ == SOCK_DGRAM && has_udp {
            ready |= POLLOUT | POLLWRNORM;
        }
    }

    if hangup {
        ready |= POLLHUP;
    }
    ready
}

/// `int ___darwin_check_fd_set_overflow(int fd, const void *fdset,
///      int fdsetsize)`
///
/// Newer SDKs implement `FD_ISSET` as a call to this helper instead of reading
/// the bitmask inline, so builds from around iOS 10 on (RetroArch's armv7 slice
/// included) bind it at load time and call it from every `select()` loop.
/// Without a host implementation it stayed an `unhandled external relocation`:
/// `FD_ISSET` then had no safe place to read and the readiness loop stalled.
///
/// It answers whether `fd`'s bit is set in `fdset`, exactly like the macro it
/// replaced. `fdsetsize` is `sizeof(fd_set)`; a descriptor outside that range
/// is reported as not set, which is what Darwin does instead of reading past
/// the caller's buffer.
/// https://opensource.apple.com/source/Libc/Libc-1158.1.2/include/sys/_types/_fd_def.h.auto.html
fn __darwin_check_fd_set_overflow(
    env: &mut Environment,
    fd: i32,
    fdset: ConstPtr<u8>,
    fdsetsize: i32,
) -> i32 {
    if fd < 0 || fdset.is_null() {
        return 0;
    }
    let byte_index = (fd / 8) as u32;
    if fdsetsize >= 0 && byte_index >= fdsetsize as u32 {
        return 0;
    }
    let bytes = env.mem.bytes_at(fdset, byte_index + 1);
    ((bytes[byte_index as usize] >> (fd % 8)) & 1) as i32
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(socket(_, _, _)),
    export_c_func!(__darwin_check_fd_set_overflow(_, _, _)),
    export_c_func!(ioctl(_, _, _)),
    export_c_func!(getsockopt(_, _, _, _, _)),
    export_c_func!(setsockopt(_, _, _, _, _)),
    export_c_func!(bind(_, _, _)),
    export_c_func!(listen(_, _)),
    export_c_func!(connect(_, _, _)),
    export_c_func!(select(_, _, _, _, _)),
    export_c_func!(accept(_, _, _)),
    export_c_func!(recv(_, _, _, _)),
    export_c_func!(recvfrom(_, _, _, _, _, _)),
    export_c_func!(send(_, _, _, _)),
    export_c_func!(sendto(_, _, _, _, _, _)), // ИСПРАВЛЕНИЕ: здесь 6 подчеркиваний вместо 7
    export_c_func!(shutdown(_, _)),
    export_c_func!(getsockname(_, _, _)),
    export_c_func!(getpeername(_, _, _)),
];

/// A helper to close a socket, not a part of API
pub fn close_socket(env: &mut Environment, socket: i32) -> bool {
    // True when the socket existed and was removed (the old is_none()
    // check had the sense inverted).
    State::get_mut(env).sockets.remove(&socket).is_some()
}

/// Duplicate a host socket handle for [duplicate_socket]. A refusal from the
/// host is logged instead of failing the whole duplication.
fn try_clone_host_socket<T>(what: &str, result: io::Result<T>) -> Option<T> {
    match result {
        Ok(socket) => Some(socket),
        Err(e) => {
            log!(
                "Warning: duplicate_socket: could not clone {}: {}",
                what,
                e
            );
            None
        }
    }
}

/// Duplicate a socket descriptor: registers `new_fd` as another descriptor for
/// the same host socket, which is what `dup(2)` / `fcntl(F_DUPFD)` produce.
///
/// Returns `false` when `old_fd` is not an open socket.
pub fn duplicate_socket(env: &mut Environment, old_fd: i32, new_fd: i32) -> bool {
    let Some(socket) = State::get(env).sockets.get(&old_fd) else {
        log!(
            "Warning: duplicate_socket({}, {}): source is not an open socket",
            old_fd,
            new_fd
        );
        return false;
    };
    // try_clone() duplicates the host handle, so both descriptors refer to the
    // same connection and each can be closed independently.
    let duplicated = SocketHostObject {
        type_: socket.type_,
        options: socket.options.clone(),
        tcp_listener: socket
            .tcp_listener
            .as_ref()
            .map(|listener| try_clone_host_socket("tcp_listener", listener.try_clone()))
            .flatten(),
        pending_tcp_stream: socket
            .pending_tcp_stream
            .as_ref()
            .map(|stream| try_clone_host_socket("pending_tcp_stream", stream.try_clone()))
            .flatten(),
        tcp_stream: socket
            .tcp_stream
            .as_ref()
            .map(|stream| try_clone_host_socket("tcp_stream", stream.try_clone()))
            .flatten(),
        udp_socket: socket
            .udp_socket
            .as_ref()
            .map(|udp| try_clone_host_socket("udp_socket", udp.try_clone()))
            .flatten(),
        domain: socket.domain,
        offline_address: socket.offline_address,
    };
    State::get_mut(env).sockets.insert(new_fd, duplicated);
    log_dbg!("duplicate_socket({}, {})", old_fd, new_fd);
    true
}

/// How long a blocking `read(2)`/`write(2)` on a socket keeps polling before it
/// gives up. Host sockets are always non-blocking, so a blocking guest call is
/// emulated by polling (the same approach `select()` uses); the bound exists so
/// that a peer which never sends (or never drains) cannot wedge the emulator.
const BLOCKING_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// How long the guest thread sleeps between polls while waiting for a socket.
/// [Environment::sleep] yields the thread, so the window and other threads keep
/// running while we wait.
const BLOCKING_IO_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

/// `read(2)` on a socket descriptor.
///
/// Socket descriptors use a placeholder [crate::fs::GuestFile::Socket] that
/// cannot do I/O; the real host socket lives in this module's state. Generic
/// I/O entry points therefore dispatch to here (see
/// [crate::libc::posix_io::read]) instead of failing with "read not supported
/// on socket via GuestFile", which broke every app that reads a socket with
/// `read()`, `fread()` or `NSFileHandle` (e.g. the networking in Minion Rush).
///
/// A blocking descriptor (the default) waits for data by polling with
/// [Environment::sleep]; a descriptor with `O_NONBLOCK` gets the `EAGAIN` that
/// [recv] produces, like a real non-blocking socket. Errno is left set by
/// [recv] on failure.
pub fn read_socket(
    env: &mut Environment,
    socket: i32,
    buffer: MutVoidPtr,
    length: GuestUSize,
) -> i32 {
    // POSIX: a zero-length read has no effect and reports success.
    if length == 0 {
        return 0;
    }
    if buffer.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }

    let nonblocking = is_nonblocking(env, socket);
    let deadline = std::time::Instant::now() + BLOCKING_IO_TIMEOUT;

    loop {
        let res = recv(env, socket, buffer, length, MSG_DONTWAIT);
        if res >= 0 {
            // 0 means the peer closed the connection, i.e. EOF.
            log_dbg!("read on socket {} => {} bytes", socket, res);
            return res;
        }
        // errno is already set by recv(). Retrying only makes sense for a
        // blocking descriptor that is merely not ready yet.
        if nonblocking || get_errno(env) != EAGAIN {
            return -1;
        }
        if std::time::Instant::now() >= deadline {
            log!(
                "Warning: read on socket {} timed out after {:?}, returning -1 (EAGAIN)",
                socket,
                BLOCKING_IO_TIMEOUT
            );
            return -1;
        }
        env.sleep(BLOCKING_IO_POLL_INTERVAL);
    }
}

/// `write(2)` on a socket descriptor. See [read_socket] for why this exists
/// (it used to fail with "write not supported on socket via GuestFile").
///
/// A blocking descriptor keeps sending until every byte has been handed to the
/// host socket; if the send buffer fills up part-way through, the short count
/// is returned and the guest retries the rest (which is legal for `write(2)`
/// and is what a real socket does when the write is interrupted). A descriptor
/// with `O_NONBLOCK` performs a single [send] and may therefore write fewer
/// bytes than requested. Errno is left set by [send] on failure.
pub fn write_socket(
    env: &mut Environment,
    socket: i32,
    buffer: ConstVoidPtr,
    length: GuestUSize,
) -> i32 {
    // POSIX: a zero-length write on a socket sends an empty datagram / nothing
    // at all and reports success.
    if length == 0 {
        return 0;
    }
    if buffer.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }

    // Byte-wise base pointer, so partial writes can advance it.
    let base: ConstPtr<u8> = buffer.cast();
    let nonblocking = is_nonblocking(env, socket);
    let deadline = std::time::Instant::now() + BLOCKING_IO_TIMEOUT;
    let mut written: GuestUSize = 0;

    loop {
        let chunk: MutVoidPtr = (base + written).cast_void().cast_mut();
        let res = send(env, socket, chunk, length - written, 0);
        if res > 0 {
            written += res as GuestUSize;
            if written >= length || nonblocking {
                log_dbg!("write on socket {} => {} bytes", socket, written);
                return written as i32;
            }
            continue;
        }
        if res == 0 {
            // Should be unreachable: the guest asked for at least one byte and
            // the host reported success with nothing sent. Report an I/O error
            // (send() leaves errno at 0) instead of spinning forever.
            log!(
                "Warning: write on socket {} sent 0 bytes, returning -1 (EIO)",
                socket
            );
            set_errno(env, EIO);
            return -1;
        }
        // errno is already set by send(); anything other than "not ready yet"
        // is a real error for the guest.
        if nonblocking || get_errno(env) != EAGAIN {
            return -1;
        }
        if written > 0 {
            // Some data went out but the send buffer is now full: report the
            // short count and let the guest retry the rest, rather than
            // blocking the emulator on a peer that stopped reading.
            log_dbg!(
                "write on socket {} wrote {} of {} bytes before it would block",
                socket,
                written,
                length
            );
            return written as i32;
        }
        if std::time::Instant::now() >= deadline {
            log!(
                "Warning: write on socket {} timed out after {:?}, returning -1 (EAGAIN)",
                socket,
                BLOCKING_IO_TIMEOUT
            );
            return -1;
        }
        env.sleep(BLOCKING_IO_POLL_INTERVAL);
    }
}

pub fn socket_addr_to_sockaddr_bytes(address: SocketAddr) -> Vec<u8> {
    match address {
        SocketAddr::V4(address) => {
            let mut bytes = vec![0; 16];
            bytes[0] = 16;
            bytes[1] = AF_INET as u8;
            bytes[2..4].copy_from_slice(&address.port().to_be_bytes());
            bytes[4..8].copy_from_slice(&address.ip().octets());
            bytes
        }
        SocketAddr::V6(address) => {
            let mut bytes = vec![0; 28];
            bytes[0] = 28;
            bytes[1] = AF_INET6 as u8;
            bytes[2..4].copy_from_slice(&address.port().to_be_bytes());
            bytes[4..8].copy_from_slice(&address.flowinfo().to_be_bytes());
            bytes[8..24].copy_from_slice(&address.ip().octets());
            bytes[24..28].copy_from_slice(&address.scope_id().to_ne_bytes());
            bytes
        }
    }
}

pub fn socket_addr_from_sockaddr_bytes(bytes: &[u8]) -> Result<SocketAddr, i32> {
    if bytes.len() < 2 {
        return Err(EINVAL);
    }
    match bytes[1] as i32 {
        AF_INET => {
            if bytes.len() < 16 || (bytes[0] != 0 && bytes[0] < 16) {
                return Err(EINVAL);
            }
            let port = u16::from_be_bytes([bytes[2], bytes[3]]);
            let ip = Ipv4Addr::new(bytes[4], bytes[5], bytes[6], bytes[7]);
            Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        AF_INET6 => {
            if bytes.len() < 28 || (bytes[0] != 0 && bytes[0] < 28) {
                return Err(EINVAL);
            }
            let port = u16::from_be_bytes([bytes[2], bytes[3]]);
            let flowinfo = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
            let ip = Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[8..24]).unwrap());
            let scope_id = u32::from_ne_bytes(bytes[24..28].try_into().unwrap());
            Ok(SocketAddr::V6(SocketAddrV6::new(
                ip, port, flowinfo, scope_id,
            )))
        }
        _ => Err(EAFNOSUPPORT),
    }
}

pub fn read_socket_addr(
    env: &mut Environment,
    address: ConstPtr<sockaddr>,
    address_len: socklen_t,
) -> Result<SocketAddr, i32> {
    if address.is_null() || address_len < 2 {
        return Err(EINVAL);
    }
    let address_bytes = address.cast::<u8>();
    let family = env.mem.read(address_bytes + 1) as i32;
    let required_len = match family {
        AF_INET => 16,
        AF_INET6 => 28,
        _ => return Err(EAFNOSUPPORT),
    };
    if address_len < required_len {
        return Err(EINVAL);
    }
    let bytes = env.mem.bytes_at(address_bytes, required_len).to_vec();
    socket_addr_from_sockaddr_bytes(&bytes)
}

fn socket_addr_for_domain(address: SocketAddr, domain: i32) -> SocketAddr {
    match (domain, address) {
        (AF_INET6, SocketAddr::V4(address)) => SocketAddr::V6(SocketAddrV6::new(
            address.ip().to_ipv6_mapped(),
            address.port(),
            0,
            0,
        )),
        (AF_INET, SocketAddr::V6(address)) => match address.ip().to_ipv4_mapped() {
            Some(ip) => SocketAddr::V4(SocketAddrV4::new(ip, address.port())),
            None => SocketAddr::V6(address),
        },
        (_, address) => address,
    }
}

fn host_socket_addr_for_domain(address: SocketAddr, domain: i32) -> Result<SocketAddr, i32> {
    match (domain, address) {
        (AF_INET, SocketAddr::V4(address)) | (AF_INET6, SocketAddr::V4(address)) => {
            Ok(SocketAddr::V4(address))
        }
        (AF_INET, SocketAddr::V6(address)) | (AF_INET6, SocketAddr::V6(address)) => {
            match address.ip().to_ipv4_mapped() {
                Some(ip) => Ok(SocketAddr::V4(SocketAddrV4::new(ip, address.port()))),
                None if domain == AF_INET6 => Ok(SocketAddr::V6(address)),
                None => Err(EAFNOSUPPORT),
            }
        }
        _ => Err(EAFNOSUPPORT),
    }
}

fn unspecified_socket_addr(domain: i32) -> SocketAddr {
    if domain == AF_INET6 {
        SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
    } else {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
    }
}

#[cfg(test)]
mod socket_tests {
    use super::*;

    #[test]
    fn recvfrom_waiting_respects_nonblocking_flags_and_socket_state() {
        assert!(blocking_recvfrom_should_wait(false, 0, true));
        assert!(!blocking_recvfrom_should_wait(true, 0, true));
        assert!(!blocking_recvfrom_should_wait(false, MSG_DONTWAIT, true));
        assert!(!blocking_recvfrom_should_wait(false, 0, false));
    }

    #[test]
    fn ipv4_sockaddr_round_trips() {
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 443));
        let bytes = socket_addr_to_sockaddr_bytes(address);
        assert_eq!(bytes.len(), 16);
        assert_eq!(bytes[0], 16);
        assert_eq!(bytes[1], AF_INET as u8);
        assert_eq!(socket_addr_from_sockaddr_bytes(&bytes), Ok(address));
    }

    #[test]
    fn ipv6_sockaddr_round_trips() {
        let address = SocketAddr::V6(SocketAddrV6::new(
            "2001:db8::1234".parse().unwrap(),
            443,
            0x1234,
            7,
        ));
        let bytes = socket_addr_to_sockaddr_bytes(address);
        assert_eq!(bytes.len(), 28);
        assert_eq!(bytes[0], 28);
        assert_eq!(bytes[1], AF_INET6 as u8);
        assert_eq!(socket_addr_from_sockaddr_bytes(&bytes), Ok(address));
    }

    #[test]
    fn ipv4_mapped_ipv6_addresses_use_the_host_ipv4_stack() {
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 443));
        let guest_address = socket_addr_for_domain(address, AF_INET6);
        assert!(
            matches!(guest_address, SocketAddr::V6(value) if value.ip().to_ipv4_mapped() == Some(Ipv4Addr::new(192, 0, 2, 1)))
        );
        assert_eq!(
            host_socket_addr_for_domain(guest_address, AF_INET6),
            Ok(address)
        );
        let native_ipv6 =
            SocketAddr::V6(SocketAddrV6::new("2001:db8::1".parse().unwrap(), 443, 0, 0));
        assert_eq!(
            host_socket_addr_for_domain(native_ipv6, AF_INET),
            Err(EAFNOSUPPORT)
        );
    }
}
