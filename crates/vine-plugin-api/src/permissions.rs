//! Capability permission constants for Vine WebAssembly plugins.
//!
//! Include these permission identifiers in [`crate::PluginMetadata::permissions`]
//! to request sandboxed WASI host capabilities.

/// Allows the plugin to perform DNS resolution (`wasi:sockets/ip-name-lookup`).
pub const NETWORK_DNS: &str = "network.dns";

/// Allows the plugin to use TCP sockets (connect and bind).
pub const NETWORK_TCP: &str = "network.tcp";

/// Allows the plugin to use UDP sockets (connect, send/receive, and bind).
pub const NETWORK_UDP: &str = "network.udp";

/// Allows the plugin to initiate outbound TCP connections.
pub const NETWORK_TCP_CONNECT: &str = "network.tcp.connect";

/// Allows the plugin to bind local TCP listeners.
pub const NETWORK_TCP_BIND: &str = "network.tcp.bind";

/// Allows the plugin to connect UDP sockets and send/receive datagrams.
pub const NETWORK_UDP_CONNECT: &str = "network.udp.connect";

/// Allows the plugin to bind UDP sockets to local ports.
pub const NETWORK_UDP_BIND: &str = "network.udp.bind";

/// Allows the plugin to send datagrams on non-connected UDP sockets.
pub const NETWORK_UDP_OUTGOING_DATAGRAM: &str = "network.udp.outgoingdatagram";

/// Restricts all networking permissions to loopback addresses (`127.0.0.1` / `::1`) only.
pub const NETWORK_LOOPBACK: &str = "network.loopback";

/// Allows the plugin to make outbound TCP and UDP connections.
pub const NETWORK_OUTBOUND: &str = "network.outbound";

/// Allows the plugin to perform outbound HTTP/HTTPS requests via `wasi:http`.
pub const HTTP_OUTBOUND: &str = "http.outbound";

/// Allows the plugin to read files within its private data folder (`data`).
pub const FS_READ_DATA: &str = "fs.read.data";

/// Allows the plugin to read and write files within its private data folder (`data`).
pub const FS_WRITE_DATA: &str = "fs.write.data";

/// Allows the plugin to read all host environment variables.
pub const SYS_ENV: &str = "sys.env";

/// Prefix for allowing a single host environment variable (e.g. `"sys.env.API_KEY"`).
pub const SYS_ENV_PREFIX: &str = "sys.env.";
