//! What a plugin may do outside its sandbox, granted per plugin in `vine.toml`:
//!
//! ```toml
//! [plugins.permissions]
//! my-plugin = ["http.outbound", "network.dns"]
//! ```
//!
//! Keys are the plugin's file name without `.wasm` (the same name as its data
//! folder). Names match Pumpkin's plugin permissions. Nothing is granted unless
//! listed.

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;

use wasmtime_wasi::sockets::SocketAddrUse;
use wasmtime_wasi::{FsPerms, WasiCtxBuilder};
use wasmtime_wasi_http::{RequestOptions, WasiBody, WasiHttpHooks};

/// Look up host names.
pub const NETWORK_DNS: &str = "network.dns";
/// Open outbound TCP connections and exchange UDP. Binding and listening stay denied.
pub const NETWORK_OUTBOUND: &str = "network.outbound";
/// Limit all networking to loopback addresses.
pub const NETWORK_LOOPBACK: &str = "network.loopback";
/// Make outbound HTTP requests through `wasi:http`.
pub const HTTP_OUTBOUND: &str = "http.outbound";
/// Read files in the plugin's data folder, mounted at `/data`.
pub const FS_READ_DATA: &str = "fs.read.data";
/// Read and write files in the plugin's data folder, mounted at `/data`.
pub const FS_WRITE_DATA: &str = "fs.write.data";

/// The permissions granted to one plugin.
#[derive(Clone, Debug, Default)]
pub struct Grants {
    granted: Vec<String>,
}

impl Grants {
    #[must_use]
    pub fn new(granted: &[String]) -> Self {
        Self {
            granted: granted.to_vec(),
        }
    }

    #[must_use]
    pub fn has(&self, permission: &str) -> bool {
        self.granted.iter().any(|granted| granted == permission)
    }

    /// Whether a socket use is allowed: outbound only, and loopback only when
    /// `network.loopback` is granted.
    #[must_use]
    pub fn allows_socket(&self, addr: SocketAddr, reason: SocketAddrUse) -> bool {
        let outbound = matches!(
            reason,
            SocketAddrUse::TcpConnect | SocketAddrUse::UdpSend | SocketAddrUse::UdpReceive
        );
        outbound
            && self.has(NETWORK_OUTBOUND)
            && (!self.has(NETWORK_LOOPBACK) || addr.ip().is_loopback())
    }

    /// Applies the network and file grants to a plugin's WASI context.
    pub fn apply(&self, builder: &mut WasiCtxBuilder, data_folder: &Path) {
        if self.has(NETWORK_DNS) {
            builder.allow_ip_name_lookup(true);
        }
        if self.has(NETWORK_OUTBOUND) {
            builder.allow_tcp(true);
            builder.allow_udp(true);
            let grants = self.clone();
            builder.socket_addr_check(move |addr, reason| {
                let allowed = grants.allows_socket(addr, reason);
                Box::pin(async move { allowed })
            });
        }
        let perms = if self.has(FS_WRITE_DATA) {
            FsPerms::ReadWrite
        } else if self.has(FS_READ_DATA) {
            FsPerms::ReadOnly
        } else {
            return;
        };
        if let Err(err) = builder.preopened_dir(data_folder, "/data", perms) {
            tracing::warn!(
                "Could not mount {} for a plugin: {err}",
                data_folder.display()
            );
        }
    }
}

/// Denies `wasi:http` requests unless `http.outbound` is granted.
pub struct HttpHooks {
    pub allow_outbound: bool,
}

impl WasiHttpHooks for HttpHooks {
    fn send_request(
        &mut self,
        request: hyper::Request<WasiBody>,
        options: Option<RequestOptions>,
        fut: Box<dyn Future<Output = wasmtime_wasi_http::Result<()>> + Send>,
    ) -> Box<
        dyn Future<
                Output = wasmtime_wasi_http::Result<(
                    hyper::Response<WasiBody>,
                    Box<dyn Future<Output = wasmtime_wasi_http::Result<()>> + Send>,
                )>,
            > + Send,
    > {
        if !self.allow_outbound {
            return Box::new(async { Err(wasmtime_wasi_http::Error::HttpRequestDenied) });
        }
        wasmtime_wasi_http::default_hooks().send_request(request, options, fut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants(granted: &[&str]) -> Grants {
        Grants::new(&granted.iter().map(|g| (*g).to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn nothing_is_allowed_without_grants() {
        let none = grants(&[]);
        assert!(!none.allows_socket("1.2.3.4:80".parse().unwrap(), SocketAddrUse::TcpConnect));
    }

    #[test]
    fn outbound_allows_connections_but_not_binding() {
        let outbound = grants(&[NETWORK_OUTBOUND]);
        assert!(outbound.allows_socket("1.2.3.4:80".parse().unwrap(), SocketAddrUse::TcpConnect));
        assert!(!outbound.allows_socket("0.0.0.0:25565".parse().unwrap(), SocketAddrUse::TcpBind));
    }

    #[test]
    fn loopback_limits_connections_to_localhost() {
        let local = grants(&[NETWORK_OUTBOUND, NETWORK_LOOPBACK]);
        assert!(local.allows_socket("127.0.0.1:8080".parse().unwrap(), SocketAddrUse::TcpConnect));
        assert!(!local.allows_socket("1.2.3.4:8080".parse().unwrap(), SocketAddrUse::TcpConnect));
    }
}
