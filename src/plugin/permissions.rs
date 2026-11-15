use std::net::SocketAddr;
use wasmtime_wasi::sockets::SocketAddrUse;

/// Allows the plugin to perform DNS resolution (`wasi:sockets/ip-name-lookup`).
pub const NETWORK_DNS: &str = "network.dns";

/// Allows the plugin to use TCP sockets (connect and bind).
pub const NETWORK_TCP: &str = "network.tcp";

/// Allows the plugin to use UDP sockets (connect, send/receive, and bind).
pub const NETWORK_UDP: &str = "network.udp";

/// Allows the plugin to initiate outbound TCP connections.
pub const NETWORK_TCP_CONNECT: &str = "network.tcp.connect";

/// Allows the plugin to bind TCP listeners.
pub const NETWORK_TCP_BIND: &str = "network.tcp.bind";

/// Allows the plugin to connect UDP sockets and send/receive datagrams.
pub const NETWORK_UDP_CONNECT: &str = "network.udp.connect";

/// Allows the plugin to bind UDP sockets to local ports.
pub const NETWORK_UDP_BIND: &str = "network.udp.bind";

/// Allows the plugin to send datagrams on non-connected UDP sockets.
pub const NETWORK_UDP_OUTGOING_DATAGRAM: &str = "network.udp.outgoingdatagram";

/// Restricts all networking permissions to loopback addresses only.
pub const NETWORK_LOOPBACK: &str = "network.loopback";

/// Allows the plugin to make outbound TCP/UDP connections.
pub const NETWORK_OUTBOUND: &str = "network.outbound";

/// Allows the plugin to make outbound HTTP connections (`wasi:http`).
pub const HTTP_OUTBOUND: &str = "http.outbound";

/// Allows the plugin to read files within its private data folder (`data`).
pub const FS_READ_DATA: &str = "fs.read.data";

/// Allows the plugin to read and write files within its private data folder (`data`).
pub const FS_WRITE_DATA: &str = "fs.write.data";

/// Allows the plugin to read all host environment variables.
pub const SYS_ENV: &str = "sys.env";

/// Prefix for allowing specific host environment variables (`"sys.env.<NAME>"`).
pub const SYS_ENV_PREFIX: &str = "sys.env.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SocketPolicy {
    pub tcp_connect: bool,
    pub tcp_bind: bool,
    pub udp_send: bool,
    pub udp_receive: bool,
    pub udp_bind: bool,
    pub loopback_only: bool,
}

impl SocketPolicy {
    #[must_use]
    pub const fn allows(self, addr: SocketAddr, reason: SocketAddrUse) -> bool {
        let implicit_outbound_bind = addr.ip().is_unspecified()
            && addr.port() == 0
            && match reason {
                SocketAddrUse::TcpBind => self.tcp_connect,
                SocketAddrUse::UdpBind => self.udp_send,
                _ => false,
            };

        let operation_allowed = match reason {
            SocketAddrUse::TcpConnect => self.tcp_connect,
            SocketAddrUse::TcpBind => self.tcp_bind || implicit_outbound_bind,
            SocketAddrUse::TcpListen | SocketAddrUse::TcpAccept => self.tcp_bind,
            SocketAddrUse::UdpBind => self.udp_bind || implicit_outbound_bind,
            SocketAddrUse::UdpSend => self.udp_send,
            SocketAddrUse::UdpReceive => self.udp_receive,
        };

        operation_allowed
            && (!self.loopback_only || addr.ip().is_loopback() || implicit_outbound_bind)
    }
}

#[must_use]
pub fn socket_policy_for_permissions(
    has_permission: impl Fn(&str) -> bool,
    loopback_only: bool,
) -> SocketPolicy {
    let network_outbound = has_permission(NETWORK_OUTBOUND);
    let tcp_allowed = has_permission(NETWORK_TCP);
    let udp_allowed = has_permission(NETWORK_UDP);
    let tcp_connect = tcp_allowed || network_outbound || has_permission(NETWORK_TCP_CONNECT);
    let tcp_bind = tcp_allowed || has_permission(NETWORK_TCP_BIND);
    let udp_connected = udp_allowed || network_outbound || has_permission(NETWORK_UDP_CONNECT);
    let udp_bind = udp_allowed || has_permission(NETWORK_UDP_BIND);
    let udp_send = udp_connected || has_permission(NETWORK_UDP_OUTGOING_DATAGRAM);
    let udp_receive = udp_connected || udp_bind;

    SocketPolicy {
        tcp_connect,
        tcp_bind,
        udp_send,
        udp_receive,
        udp_bind,
        loopback_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn empty_policy_denies_all_socket_uses() {
        let policy = SocketPolicy::default();
        let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 25565);
        for use_kind in [
            SocketAddrUse::TcpConnect,
            SocketAddrUse::TcpBind,
            SocketAddrUse::TcpListen,
            SocketAddrUse::TcpAccept,
            SocketAddrUse::UdpBind,
            SocketAddrUse::UdpSend,
            SocketAddrUse::UdpReceive,
        ] {
            assert!(!policy.allows(loopback, use_kind));
        }
    }

    #[test]
    fn loopback_restriction_blocks_public_outbound() {
        let policy = socket_policy_for_permissions(|p| p == NETWORK_OUTBOUND, true);
        let wildcard = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        let local = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let remote = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)), 8080);

        assert!(policy.allows(wildcard, SocketAddrUse::TcpBind));
        assert!(policy.allows(local, SocketAddrUse::TcpConnect));
        assert!(!policy.allows(remote, SocketAddrUse::TcpConnect));
    }
}
