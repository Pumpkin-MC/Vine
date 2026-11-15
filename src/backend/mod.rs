pub mod bridge;
pub mod forwarding;

pub use bridge::{BackendBridge, BridgeError};
pub use forwarding::{ForwardingError, ForwardingHelper};

use crate::config::{BackendConfig, RoutingConfig};
use std::collections::HashMap;
use std::sync::RwLock;

struct RouterState {
    routing: RoutingConfig,
    servers: HashMap<String, BackendConfig>,
}

pub struct Router {
    state: RwLock<RouterState>,
}

impl Router {
    pub fn new(routing: RoutingConfig, servers: HashMap<String, BackendConfig>) -> Self {
        Self {
            state: RwLock::new(RouterState { routing, servers }),
        }
    }

    /// Updates routing rules and backend definitions at runtime (used by `/vine reload`).
    pub fn reload(&self, routing: RoutingConfig, servers: HashMap<String, BackendConfig>) {
        if let Ok(mut guard) = self.state.write() {
            guard.routing = routing;
            guard.servers = servers;
        }
    }

    /// Resolves which backend server to connect to based on virtual host or default
    pub fn resolve_server(&self, virtual_host: &str) -> Option<(String, BackendConfig)> {
        let guard = self.state.read().ok()?;
        if let Some(target_name) = guard.routing.forced_hosts.get(virtual_host)
            && let Some(cfg) = guard.servers.get(target_name)
        {
            return Some((target_name.clone(), cfg.clone()));
        }

        if let Some(cfg) = guard.servers.get(&guard.routing.default_server) {
            return Some((guard.routing.default_server.clone(), cfg.clone()));
        }

        if let Some(fallback_name) = &guard.routing.fallback_server
            && let Some(cfg) = guard.servers.get(fallback_name)
        {
            return Some((fallback_name.clone(), cfg.clone()));
        }

        guard
            .servers
            .iter()
            .next()
            .map(|(name, cfg)| (name.clone(), cfg.clone()))
    }
}
