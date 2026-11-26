pub mod prefixed_stream;
pub mod proxy_protocol;

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tracing::{debug, warn};

use crate::config::NetworkConfig;
pub use prefixed_stream::PrefixedRead;
pub use proxy_protocol::{ProxyHeaderResult, parse_proxy_protocol};

/// Read buffer per connection half. The packet decoder reads VarInt length prefixes one byte at
/// a time, so without buffering every byte costs a `read` syscall.
pub const IO_BUFFER_SIZE: usize = 16 * 1024;

/// Client-facing read half: prefix (PROXY protocol leftovers) over a buffered TCP read half.
pub type ClientReader = tokio::io::BufReader<PrefixedRead<tokio::net::tcp::OwnedReadHalf>>;
/// Client-facing write half. Callers must `flush()` after writing, as with any buffered writer.
pub type ClientWriter = tokio::io::BufWriter<tokio::net::tcp::OwnedWriteHalf>;
/// Backend-facing buffered read half.
pub type BackendReader = tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>;
/// Backend-facing buffered write half.
pub type BackendWriter = tokio::io::BufWriter<tokio::net::tcp::OwnedWriteHalf>;

/// Creates and tunes a Tokio `TcpListener` based on `NetworkConfig`
pub fn create_tcp_listener(
    bind_addr_str: &str,
    network: &NetworkConfig,
) -> io::Result<TcpListener> {
    let addr: SocketAddr = bind_addr_str
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    let socket = if addr.is_ipv6() {
        TcpSocket::new_v6()?
    } else {
        TcpSocket::new_v4()?
    };

    socket.set_reuseaddr(true)?;

    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
    {
        let _ = socket.set_reuseport(true);
    }

    if let Some(buf) = network.recv_buffer_size {
        let _ = socket.set_recv_buffer_size(buf as u32);
    }
    if let Some(buf) = network.send_buffer_size {
        let _ = socket.set_send_buffer_size(buf as u32);
    }

    socket.bind(addr)?;
    let listener = socket.listen(network.listen_backlog)?;
    Ok(listener)
}

/// Applies socket performance options (TCP_NODELAY, KeepAlive, Buffer sizes) to a client `TcpStream`
pub fn configure_client_socket(stream: &TcpStream, config: &NetworkConfig) -> io::Result<()> {
    if let Err(e) = stream.set_nodelay(config.tcp_nodelay) {
        warn!("Failed to set TCP_NODELAY on client socket: {}", e);
    }

    let sock = socket2::SockRef::from(stream);

    if config.tcp_keepalive_secs > 0 {
        let keepalive = socket2::TcpKeepalive::new()
            .with_time(Duration::from_secs(config.tcp_keepalive_secs))
            .with_interval(Duration::from_secs(10));

        if let Err(e) = sock.set_tcp_keepalive(&keepalive) {
            debug!("Failed to set TCP keepalive on client socket: {}", e);
        }
    }

    if let Some(buf) = config.recv_buffer_size {
        let _ = sock.set_recv_buffer_size(buf);
    }
    if let Some(buf) = config.send_buffer_size {
        let _ = sock.set_send_buffer_size(buf);
    }

    Ok(())
}

/// Applies socket performance options to a backend server `TcpStream`
pub fn configure_backend_socket(stream: &TcpStream, config: &NetworkConfig) -> io::Result<()> {
    if let Err(e) = stream.set_nodelay(config.tcp_nodelay) {
        warn!("Failed to set TCP_NODELAY on backend socket: {}", e);
    }

    let sock = socket2::SockRef::from(stream);

    if config.tcp_keepalive_secs > 0 {
        let keepalive = socket2::TcpKeepalive::new()
            .with_time(Duration::from_secs(config.tcp_keepalive_secs))
            .with_interval(Duration::from_secs(10));

        if let Err(e) = sock.set_tcp_keepalive(&keepalive) {
            debug!("Failed to set TCP keepalive on backend socket: {}", e);
        }
    }

    if let Some(buf) = config.recv_buffer_size {
        let _ = sock.set_recv_buffer_size(buf);
    }
    if let Some(buf) = config.send_buffer_size {
        let _ = sock.set_send_buffer_size(buf);
    }

    Ok(())
}

/// Raises the soft open file descriptor limit (`RLIMIT_NOFILE`) on Unix systems
/// so high-concurrency proxying (2+ sockets per player) does not hit `EMFILE`.
#[cfg(unix)]
pub fn adjust_file_descriptor_limit() {
    let mut rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    // SAFETY: Passing a valid mutable pointer to a stack-allocated `rlimit` struct.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut rlim) } != 0 {
        return;
    }

    let max_target = if rlim.rlim_max == libc::RLIM_INFINITY {
        1_048_576
    } else {
        rlim.rlim_max
    };

    if rlim.rlim_cur < max_target {
        let old_limit = rlim.rlim_cur;
        rlim.rlim_cur = max_target;

        // SAFETY: Calling `setrlimit` with a valid resource and valid pointer to initialized `rlimit`.
        let res = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const rlim) };
        if res == 0 {
            debug!("Increased open file descriptor limit from {old_limit} to {max_target}");
        } else {
            // Fallback: try setting to a reasonable high value (65,536) if max_target was rejected by the OS.
            let fallback = 65_536.min(max_target);
            if fallback > old_limit {
                rlim.rlim_cur = fallback;
                // SAFETY: Calling `setrlimit` with a valid resource and valid pointer to initialized `rlimit`.
                if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const rlim) } == 0 {
                    debug!("Increased open file descriptor limit from {old_limit} to {fallback}");
                }
            }
        }
    }

    let mut current_rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: Passing a valid mutable pointer to a stack-allocated `rlimit` struct.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut current_rlim) } == 0
        && current_rlim.rlim_cur < 4096
    {
        warn!(
            "Open file descriptor limit is low ({}). Each proxied player uses at least 2 sockets, so >1000 concurrent players may fail with 'Too many open files'. \
            Consider increasing the limit with `ulimit -n 65535` or setting `LimitNOFILE=65535` in systemd.",
            current_rlim.rlim_cur
        );
    }
}

#[cfg(not(unix))]
pub fn adjust_file_descriptor_limit() {}
