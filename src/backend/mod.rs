pub mod bridge;
pub mod forwarding;
pub mod registry;

pub use bridge::{BackendBridge, BridgeError};
pub use forwarding::{ForwardingError, ForwardingHelper};
pub use registry::ServerRegistry;

use crate::config::{BackendConfig, RoutingConfig};

pub struct Router {
    routing: RoutingConfig,
    servers: ServerRegistry,
}

impl Router {
    pub fn new(routing: RoutingConfig, servers: ServerRegistry) -> Self {
        Self { routing, servers }
    }

    /// Resolves which backend server to connect to based on virtual host or default
    pub fn resolve_server(&self, virtual_host: &str) -> Option<(String, BackendConfig)> {
        if let Some(target_name) = self.routing.forced_hosts.get(virtual_host)
            && let Some(cfg) = self.servers.get(target_name)
        {
            return Some((target_name.clone(), cfg));
        }

        if let Some(cfg) = self.servers.get(&self.routing.default_server) {
            return Some((self.routing.default_server.clone(), cfg));
        }

        if let Some(fallback_name) = &self.routing.fallback_server
            && let Some(cfg) = self.servers.get(fallback_name)
        {
            return Some((fallback_name.clone(), cfg));
        }

        self.servers.iter().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ForwardingMode};

    fn backend(address: &str) -> BackendConfig {
        BackendConfig {
            address: address.to_string(),
            forwarding: ForwardingMode::None,
            secret: None,
            icon_path: None,
            motd_line1: None,
            motd_line2: None,
        }
    }

    #[test]
    fn router_sees_servers_registered_after_startup() {
        let config = Config::default();
        let router = Router::new(config.routing.clone(), config.servers.clone());

        // A plugin registers a server through its own clone of the config.
        let plugin_view = config.clone();
        plugin_view
            .servers
            .insert("lobby".into(), backend("10.0.0.5:25565"));
        let mut routing = config.routing.clone();
        routing.default_server = "lobby".into();
        let router_with_default = Router::new(routing, config.servers.clone());

        assert_eq!(
            router_with_default
                .resolve_server("play.example.com")
                .map(|(name, cfg)| (name, cfg.address)),
            Some(("lobby".to_string(), "10.0.0.5:25565".to_string()))
        );
        assert!(router.servers.contains_key("lobby"));

        plugin_view.servers.remove("lobby");
        assert!(!router.servers.contains_key("lobby"));
    }

    #[test]
    fn draining_is_shared_by_every_config_clone() {
        let config = Config::default();
        let client_view = config.clone();
        assert!(!client_view.runtime.is_draining());
        config.runtime.set_draining(true);
        assert!(client_view.runtime.is_draining());
        config.runtime.set_draining(false);
        assert!(!client_view.runtime.is_draining());
    }
}
