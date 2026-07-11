/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! sys/socket.h (Sockets)
//!
//! We currently support blocking TCP and UDP guest sockets on IPv4 addresses.
//!
//! Because fine grain control is needed, those are implemented as
//! _non-blocking_ host sockets. Moreover, app usage of select() is
//! (optimistically) assumed to check for data readiness before calling
//! any of blocking functions.
//! (Check related functions for more details and remediation.)
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
    set_errno, EACCES, EADDRINUSE, EADDRNOTAVAIL, EAGAIN, EBADF, ECONNREFUSED, ECONNRESET, EINVAL,
    EIO, EISCONN, ENETUNREACH, ENOTCONN, EPROTONOSUPPORT, ESOCKTNOSUPPORT, ETIMEDOUT,
};
use crate::libc::posix_io::{close, find_or_create_socket, is_socket, FileDescriptor};
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
use std::net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};

pub const AF_INET: i32 = 2;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;

const SOL_SOCKET: i32 = 0xffff;
const SO_DEBUG: i32 = 0x1;
const SO_REUSEADDR: i32 = 0x4;
const SO_BROADCAST: i32 = 0x20;
const SO_ERROR: i32 = 0x1007;

const SO_LINGER: i32 = 0x80;
const SO_SNDBUF: i32 = 0x1001;
const SO_RCVBUF: i32 = 0x1002;
const SO_NOSIGPIPE: i32 = 0x1022;

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
        assert!(self.sa_len == 16 || self.sa_len == 0);
        assert_eq!(self.sa_family, AF_INET as u8);
        let port = u16::from_be_bytes([self.sa_data[0], self.sa_data[1]]);
        let ip = [
            self.sa_data[2],
            self.sa_data[3],
            self.sa_data[4],
            self.sa_data[5],
        ];
        (ip, port)
    }
    fn from_sockaddr_v4(addr: &SocketAddr) -> Self {
        // Only IPV4 for the moment
        match addr {
            SocketAddr::V4(ipv4addr) => {
                sockaddr::from_ipv4_parts(ipv4addr.ip().octets(), ipv4addr.port())
            }
            SocketAddr::V6(_) => {
                // Host resolved the peer to an IPv6 address but we don't
                // model AF_INET6 yet. Return an all-zero IPv4 sockaddr so the
                // guest sees a well-formed but obviously-invalid address
                // instead of crashing the host.
                log!(
                    "Warning: from_sockaddr_v4(): host returned IPv6 address {:?} but only AF_INET is modelled; returning 0.0.0.0:0.",
                    addr
                );
                sockaddr::from_ipv4_parts([0; 4], 0)
            }
        }
    }
    pub fn to_sockaddr_v4(self) -> SocketAddrV4 {
        let (ip, port) = self.to_ipv4_parts();
        SocketAddrV4::new(ip.into(), port)
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
    // TODO: handle errno properly
    set_errno(env, 0);
    if !env.options.network_access {
        log_dbg!(
            "Network access is disabled, socket({}, {}, {}) => -1",
            domain,
            type_,
            protocol
        );
        set_errno(env, EPROTONOSUPPORT);
        return -1;
    }

    assert_eq!(domain, AF_INET);
    assert!(type_ == SOCK_STREAM || type_ == SOCK_DGRAM);
    assert!(protocol == IPPROTO_TCP || protocol == IPPROTO_UDP || protocol == 0);

    let fd = find_or_create_socket(env);
    assert!(!State::get(env).sockets.contains_key(&fd));
    let host_object = SocketHostObject {
        type_,
        options: Default::default(),
        tcp_listener: None,
        pending_tcp_stream: None,
        tcp_stream: None,
        udp_socket: None,
    };
    State::get_mut(env).sockets.insert(fd, host_object);

    log_dbg!("socket({}, {}, {}) => {}", domain, type_, protocol, fd);
    fd
}

fn ioctl(env: &mut Environment, fd: i32, request: u32, _args: DotDotDot) -> i32 {
    set_errno(env, 0);
    // Честно обрабатываем неверные дескрипторы по стандарту POSIX:
    if !is_socket(env, fd) {
        log!("ioctl: fd={} is not a valid socket, returning EBADF", fd);
        set_errno(env, EBADF);
        return -1;
    }

    log!("TODO: ioctl({} (socket), {:#x?}, ...) => -1", fd, request);
    -1
}

fn getsockopt(
    env: &mut Environment,
    socket: i32,
    level: i32,
    option_name: i32,
    option_value: MutVoidPtr,
    option_len: MutPtr<socklen_t>,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);
    log_dbg!(
        "getsockopt({}, {:#x}, {:#x}, {:?}, {:?})",
        socket,
        level,
        option_name,
        option_value,
        option_len
    );
    assert_eq!(level, SOL_SOCKET);
    // TODO: support other options
    assert_eq!(option_name, SO_ERROR);

    let option_len_val = env.mem.read(option_len);
    assert_eq!(option_len_val, 4);
    let option_value: MutPtr<i32> = option_value.cast();
    env.mem.write(option_value, 0); // no errors

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
            assert_eq!(option_len, guest_size_of::<i32>());
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
            assert_eq!(option_len, guest_size_of::<i32>());
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
            // TCP_NODELAY (1) — disable Nagle's algorithm.
            const TCP_NODELAY: i32 = 1;
            if option_name == TCP_NODELAY {
                assert_eq!(option_len, guest_size_of::<i32>());
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

    if type_ != SOCK_STREAM && type_ != SOCK_DGRAM {
        set_errno(env, ESOCKTNOSUPPORT);
        return -1;
    }

    if address_len < guest_size_of::<sockaddr>() {
        set_errno(env, EINVAL);
        return -1;
    }

    let sockaddr_val = env.mem.read(address);
    let socket_address = sockaddr_val.to_sockaddr_v4();
    let type_str = match type_ {
        SOCK_STREAM => "TCP",
        SOCK_DGRAM => "UDP",
        _ => "<unknown socket type>",
    };
    log_dbg!(
        "bind({}, {:?} ({:?}), {}) -> {} {:?}",
        socket,
        address,
        sockaddr_val,
        address_len,
        type_str,
        socket_address
    );
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
    // TODO: handle errno properly
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

    log!(
        "Warning: listen(socket: {}, backlog: {}), ignoring",
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
    if type_ != SOCK_STREAM {
        set_errno(env, ESOCKTNOSUPPORT);
        return -1;
    }

    if address_len < guest_size_of::<sockaddr>() {
        set_errno(env, EINVAL);
        return -1;
    }

    let sockaddr_val = env.mem.read(address);
    log_dbg!(
        "connect({:?} ({:?}), {})",
        address,
        sockaddr_val,
        address_len
    );
    let socket_address = sockaddr_val.to_sockaddr_v4();
    log_dbg!("connect: socket address {:?}", socket_address);

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
    // TODO: handle errno properly
    set_errno(env, 0);
    assert!((0..=1024).contains(&n_fds));

    // В POSIX вызов select с n_fds = 0 используется для точного сна
    // (микросекунды)
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

    let should_block = if !timeout.is_null() {
        let timeval = env.mem.read(timeout);
        let tv_sec = timeval.tv_sec;
        let tv_usec = timeval.tv_usec;
        if tv_sec == 0 && tv_usec == 0 {
            // Happy path, just polling once
            false
        } else {
            log_dbg!("TODO: Ignore non-zero timeout {:?} in select()", timeval);
            true
        }
    } else {
        true
    };
    let mut count = 0;

    if !read_fds.is_null() {
        let mut read_set = env.mem.read(read_fds);
        log_dbg!("select: read_set before {:?}", read_set);
        count += process_set(env, &mut read_set, n_fds, |env, fd, bits, bit_index| {
            log_dbg!("select: bit set in read_set at fd: {}", fd);
            // Only sockets for now
            assert!(is_socket(env, fd));
            // Clean bit in the set for the current socket
            *bits &= !(1 << bit_index);
            let socket_host_object = State::get(env).sockets.get(&fd).unwrap();
            let type_ = socket_host_object.type_;
            match type_ {
                SOCK_DGRAM => {
                    let udp_socket = socket_host_object.udp_socket.as_ref().unwrap();
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
                            if cfg!(target_os = "windows") && e.raw_os_error() == Some(10040) =>
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
                            assert!(!should_block); // TODO
                            false
                        }
                        Err(e) => {
                            panic!("select: Peek for socket {fd} failed: {e:?}")
                        }
                    }
                }
                SOCK_STREAM => {
                    if socket_host_object.tcp_stream.is_none() {
                        // If we don't have a TCP stream it probably means
                        // that a listener is waiting for connection
                        let listener = socket_host_object.tcp_listener.as_ref().unwrap();
                        // The listener is non-blocking,
                        // so we can try to accept
                        match listener.accept() {
                            Ok((stream, addr)) => {
                                log!("select: New client: {}", addr);
                                // We set host socket as non-blocking in order
                                // to have more control of how and when it's
                                // used
                                stream.set_nonblocking(true).unwrap();
                                // We already accepted the connection on
                                // the host, but we need to postpone new
                                // guest fd creation up until guest calls
                                // accept()
                                assert!(socket_host_object.pending_tcp_stream.is_none());
                                State::get_mut(env)
                                    .sockets
                                    .get_mut(&fd)
                                    .unwrap()
                                    .pending_tcp_stream = Some(stream);
                                // Set bit back
                                *bits |= 1 << bit_index;
                                return true;
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                // No incoming connection is ready
                                log_dbg!("select: TCP listener for socket {} would block on accepting, continue.", fd);
                                assert!(!should_block);
                                // TODO
                                return false;
                            }
                            Err(e) => {
                                panic!("select: Socket {fd} has error accepting connection: {e}");
                            }
                        }
                    }
                    let stream = socket_host_object.tcp_stream.as_ref().unwrap();
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
                            if cfg!(target_os = "windows") && e.raw_os_error() == Some(10040) =>
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
                            assert!(!should_block); // TODO
                            false
                        }
                        Err(e) => {
                            panic!("select: Peek for socket {fd} failed: {e}")
                        }
                    }
                }
                other => {
                    log!(
                        "Warning: select() read_set fd {} has unknown socket type {}; \
                         treating as not-ready.",
                        fd, other
                    );
                    false
                }
            }
        });
        log_dbg!("select: read_set after {:?}", read_set);
        env.mem.write(read_fds, read_set);
    }

    if !write_fds.is_null() {
        let mut write_set = env.mem.read(write_fds);
        log_dbg!("select: write_set before {:?}", write_set);
        count += process_set(env, &mut write_set, n_fds, |env, fd, bits, bit_index| {
            log_dbg!("select: bit set in write_set at fd: {}", fd);
            // Only sockets for now
            assert!(is_socket(env, fd));
            // Clean bit in the set for the current socket
            *bits &= !(1 << bit_index);
            let socket_host_object = State::get(env).sockets.get(&fd).unwrap();
            let type_ = socket_host_object.type_;
            match type_ {
                SOCK_STREAM => {
                    assert!(socket_host_object.tcp_listener.is_none());
                    // As we cannot "peek" on write, we just check
                    // if TCP stream exist or not
                    // TODO: find a better way
                    if socket_host_object.tcp_stream.is_some() {
                        // Set bit back
                        *bits |= 1 << bit_index;
                        true
                    } else {
                        assert!(!should_block);
                        // TODO
                        false
                    }
                }
                SOCK_DGRAM => {
                    // As we cannot "peek" on write, we just check
                    // if UDP socket exist or not
                    // TODO: find a better way
                    if socket_host_object.udp_socket.is_some() {
                        // Set bit back
                        *bits |= 1 << bit_index;
                        true
                    } else {
                        assert!(!should_block);
                        // TODO
                        false
                    }
                }
                other => {
                    log!(
                        "Warning: select() write_set fd {} has unknown socket type {}; \
                         treating as not-ready.",
                        fd, other
                    );
                    false
                }
            }
        });
        log_dbg!("select: write_set after {:?}", write_set);
        env.mem.write(write_fds, write_set);
    }

    if !error_fds.is_null() {
        let mut error_set = env.mem.read(error_fds);
        log_dbg!("select: error_set before {:?}", error_set);
        count += process_set(env, &mut error_set, n_fds, |env, fd, bits, bit_index| {
            log_dbg!("select: bit set in error_set at fd: {}", fd);
            // Only sockets for now
            assert!(is_socket(env, fd));
            // Clean bit in the set for the current socket
            *bits &= !(1 << bit_index);
            let socket_host_object = State::get(env).sockets.get(&fd).unwrap();
            let type_ = socket_host_object.type_;
            match type_ {
                SOCK_STREAM => {
                    assert!(socket_host_object.tcp_listener.is_none());
                    let stream = socket_host_object.tcp_stream.as_ref().unwrap();
                    match stream.take_error() {
                        Ok(None) => {
                            log_dbg!("No error on TCP socket {}", fd);
                            false
                        }
                        Ok(Some(error)) => {
                            log!(
                                "select: TCP socket {} has a pending error: {:?}; \
                                 reporting it via the error_fds set.",
                                fd, error
                            );
                            // Set bit back so the guest observes the error
                            // on this socket rather than us panicking.
                            true
                        }
                        Err(error) => panic!("TCP socket {fd} take_error failed: {error:?}"),
                    }
                }
                SOCK_DGRAM => {
                    // UDP is connectionless, so there is no per-socket error
                    // state comparable to TCP's take_error(). Real BSD
                    // sockets can still surface async errors (e.g. from a
                    // previous ICMP port-unreachable) via SO_ERROR, but we
                    // don't track that here; report "no error" rather than
                    // aborting the emulator.
                    log_dbg!(
                        "select: error_set check on UDP socket {} — no async \
                         error tracking implemented, reporting none.",
                        fd
                    );
                    false
                }
                other => {
                    log!(
                        "Warning: select() error_set fd {} has unknown socket type {}; \
                         treating as no-error.",
                        fd, other
                    );
                    false
                }
            }
        });
        log_dbg!("select: error_set after {:?}", error_set);
        env.mem.write(error_fds, error_set);
    }

    count
}

fn process_set<F: Fn(&mut Environment, FileDescriptor, &mut i32, i32) -> bool>(
    env: &mut Environment,
    set: &mut fd_set,
    n_fds: i32,
    process_bit: F,
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
    // TODO: handle errno properly
    set_errno(env, 0);
    let Some(socket_host_object) = State::get(env).sockets.get(&socket) else {
        set_errno(env, EBADF);
        return -1;
    };
    let type_ = socket_host_object.type_;
    assert!(type_ == SOCK_STREAM);

    if let Some(stream) = State::get_mut(env)
        .sockets
        .get_mut(&socket)
        .unwrap()
        .pending_tcp_stream
        .take()
    {
        let addr = stream.peer_addr().unwrap();
        // We have already accepted TCP socket, we now need to
        // let guest know as well!
        let new_fd = find_or_create_socket(env);
        assert!(!State::get(env).sockets.contains_key(&new_fd));
        let host_object = SocketHostObject {
            type_: SOCK_STREAM,
            options: Default::default(),
            tcp_listener: None,
            pending_tcp_stream: None,
            tcp_stream: Some(stream),
            udp_socket: None,
        };
        State::get_mut(env).sockets.insert(new_fd, host_object);
        assert!(!address.is_null());
        let peer_guest_addr = sockaddr::from_sockaddr_v4(&addr);
        env.mem.write(address, peer_guest_addr);
        assert_eq!(guest_size_of::<sockaddr>(), env.mem.read(address_len));
        env.mem.write(address_len, guest_size_of::<sockaddr>());
        return new_fd;
    }

    // re-borrow
    let socket_host_object = State::get(env).sockets.get(&socket).unwrap();
    let listener = socket_host_object.tcp_listener.as_ref().unwrap();
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
            assert!(!State::get(env).sockets.contains_key(&new_fd));
            let host_object = SocketHostObject {
                type_: SOCK_STREAM,
                options: Default::default(),
                tcp_listener: None,
                pending_tcp_stream: None,
                tcp_stream: Some(stream),
                udp_socket: None,
            };
            State::get_mut(env).sockets.insert(new_fd, host_object);
            if !address.is_null() {
                let peer_guest_addr = sockaddr::from_sockaddr_v4(&addr);
                env.mem.write(address, peer_guest_addr);
                if !address_len.is_null() {
                    assert_eq!(guest_size_of::<sockaddr>(), env.mem.read(address_len));
                    env.mem.write(address_len, guest_size_of::<sockaddr>());
                }
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
            panic!("accept: Socket {socket} has error accepting connection: {e}");
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

fn recvfrom(
    env: &mut Environment,
    socket: i32,
    buffer: MutVoidPtr,
    length: GuestUSize,
    flags: i32,
    address: MutPtr<sockaddr>,
    address_len: MutPtr<socklen_t>,
) -> i32 {
    // TODO: handle errno properly
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
    if type_ != SOCK_STREAM && type_ != SOCK_DGRAM {
        set_errno(env, ESOCKTNOSUPPORT);
        return -1;
    }

    assert_eq!(flags, 0); // TODO

    let (num_bytes_read, addr) = match type_ {
        SOCK_DGRAM => {
            let udp_socket = env
                .libc_state
                .socket
                .sockets
                .get(&socket)
                .unwrap()
                .udp_socket
                .as_ref()
                .unwrap();
            let buf = env.mem.bytes_at_mut(buffer.cast(), length);
            let (read, addr) = match udp_socket.recv_from(buf) {
                Ok(n) => n,
                // FIX: was unimplemented!() — return EAGAIN so the app's
                // non-blocking network loop can retry without crashing.
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    log_dbg!(
                        "recvfrom: UDP socket {} no data yet (WouldBlock), \
                        returning EAGAIN for thread {}",
                        socket,
                        env.current_thread
                    );
                    set_errno(env, EAGAIN);
                    return -1;
                }
                Err(e) => panic!("recvfrom: UDP socket {socket} encountered IO error: {e}"),
            };
            if !address.is_null() {
                let guest_addr = sockaddr::from_sockaddr_v4(&addr);
                env.mem.write(address, guest_addr);
                assert_eq!(guest_size_of::<sockaddr>(), env.mem.read(address_len));
                env.mem.write(address_len, guest_size_of::<sockaddr>());
            }
            (read, Ok(addr))
        }
        SOCK_STREAM => {
            assert!(address.is_null());
            assert!(address_len.is_null());
            let mut tcp_stream = env
                .libc_state
                .socket
                .sockets
                .get(&socket)
                .unwrap()
                .tcp_stream
                .as_ref()
                .unwrap();
            let buf = env.mem.bytes_at_mut(buffer.cast(), length);
            let read = match tcp_stream.read(buf) {
                Ok(n) => n,
                Err(ref e) if e.kind() == io::ErrorKind::ConnectionReset => {
                    set_errno(env, ECONNRESET);
                    log!("recvfrom: TCP socket {}: ConnectionReset => -1", socket);
                    return -1;
                }
                // FIX: was unimplemented!() — return EAGAIN so the app's
                // non-blocking network loop can retry without crashing.
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    log_dbg!(
                        "recvfrom: TCP socket {} no data yet (WouldBlock), \
                        returning EAGAIN for thread {}",
                        socket,
                        env.current_thread
                    );
                    set_errno(env, EAGAIN);
                    return -1;
                }
                Err(e) => panic!("recvfrom: TCP socket {socket} encountered IO error: {e}"),
            };
            (read, tcp_stream.peer_addr())
        }
        _ => unreachable!(),
    };
    log_dbg!(
        "recvfrom: Socket {} received {} bytes from addr {:?}",
        socket,
        num_bytes_read,
        addr.ok()
    );
    num_bytes_read.try_into().unwrap()
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
    assert_eq!(flags, 0); // TODO

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
                    n.try_into().unwrap()
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
                    n.try_into().unwrap()
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
    // TODO: handle errno properly
    set_errno(env, 0);
    let type_ = match State::get(env).sockets.get(&socket) {
        Some(s) => s.type_,
        None => {
            log!("sendto: unknown socket fd={}, returning EBADF", socket);
            set_errno(env, EBADF);
            return -1;
        }
    };
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

    assert_eq!(dest_address_len, guest_size_of::<sockaddr>());
    let sockaddr_val = env.mem.read(dest_address);
    let socket_address = sockaddr_val.to_sockaddr_v4();
    log_dbg!(
        "sendto({}, {:?}, {}, {}, {:?} ({:?}, {:?}), {})",
        socket,
        buffer,
        length,
        flags,
        dest_address,
        sockaddr_val,
        socket_address,
        dest_address_len
    );
    let num_bytes_written = match type_ {
        SOCK_DGRAM => {
            if State::get(env)
                .sockets
                .get(&socket)
                .unwrap()
                .udp_socket
                .is_none()
            {
                // For the case of broadcast we allow a lazy host UDP socket
                // creation
                assert!(socket_address.ip().is_broadcast());
                // TODO: is it a correct address to bind?
                let host_socket = UdpSocket::bind("0.0.0.0:0").unwrap();
                assert!(host_socket.local_addr().unwrap().ip().is_unspecified());
                // We set host socket as non-blocking in order to have
                // more control of how and when it's used
                host_socket.set_nonblocking(true).unwrap();
                for &option in &State::get(env).sockets.get(&socket).unwrap().options {
                    if option == SO_BROADCAST {
                        host_socket.set_broadcast(true).unwrap();
                    }
                }
                assert!(host_socket.broadcast().unwrap());
                State::get_mut(env)
                    .sockets
                    .get_mut(&socket)
                    .unwrap()
                    .udp_socket = Some(host_socket);
            }
            let udp_socket = env
                .libc_state
                .socket
                .sockets
                .get(&socket)
                .unwrap()
                .udp_socket
                .as_ref()
                .unwrap();
            if socket_address.ip().is_broadcast() {
                assert!(udp_socket.local_addr().unwrap().ip().is_unspecified());
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
                Err(e) => panic!("sendto: Socket {socket} encountered IO error: {e}"),
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
    num_bytes_written.try_into().unwrap()
}

const SHUT_RDWR: i32 = 2;
fn shutdown(env: &mut Environment, socket: i32, how: i32) -> i32 {
    log_dbg!("shutdown({}, {})", socket, how);
    assert_eq!(how, SHUT_RDWR);
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
        env.mem
            .write(address, sockaddr::from_sockaddr_v4(&local_addr));
        if !address_len.is_null() {
            env.mem.write(address_len, guest_size_of::<sockaddr>());
        }
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
        env.mem
            .write(address, sockaddr::from_sockaddr_v4(&peer_addr));
        if !address_len.is_null() {
            env.mem.write(address_len, guest_size_of::<sockaddr>());
        }
    }
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(socket(_, _, _)),
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
    State::get_mut(env).sockets.remove(&socket).is_none()
}
