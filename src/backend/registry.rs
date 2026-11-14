//! The backend servers Vine can route players to.
//!
//! Seeded from `[servers]` in the config and changeable at runtime (by plugins,
//! through `register-server` and `unregister-server`), so a network can add and
//! remove servers without restarting the proxy. Every clone shares the same
//! servers, so the router, status pings, commands and plugins all see a change
//! at once. Serializes as the same map `[servers]` always was.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::BackendConfig;

#[derive(Clone, Default)]
pub struct ServerRegistry {
    servers: Arc<RwLock<HashMap<String, BackendConfig>>>,
}

impl ServerRegistry {
    #[must_use]
    pub fn new(servers: HashMap<String, BackendConfig>) -> Self {
        Self {
            servers: Arc::new(RwLock::new(servers)),
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, BackendConfig>> {
        self.servers.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, BackendConfig>> {
        self.servers.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// The server's config, if it's registered.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<BackendConfig> {
        self.read().get(name).cloned()
    }

    #[must_use]
    pub fn contains_key(&self, name: &str) -> bool {
        self.read().contains_key(name)
    }

    /// Registered server names, sorted.
    pub fn keys(&self) -> impl Iterator<Item = String> {
        let mut names: Vec<String> = self.read().keys().cloned().collect();
        names.sort();
        names.into_iter()
    }

    /// Every registered server, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (String, BackendConfig)> {
        let mut servers: Vec<(String, BackendConfig)> = self
            .read()
            .iter()
            .map(|(name, config)| (name.clone(), config.clone()))
            .collect();
        servers.sort_by(|a, b| a.0.cmp(&b.0));
        servers.into_iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.read().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    /// Adds or replaces a server. Returns the config it replaced, if any.
    pub fn insert(&self, name: String, config: BackendConfig) -> Option<BackendConfig> {
        self.write().insert(name, config)
    }

    /// Removes a server. Players already on it stay connected; only new
    /// connections and switches can no longer pick it.
    pub fn remove(&self, name: &str) -> Option<BackendConfig> {
        self.write().remove(name)
    }
}

impl From<HashMap<String, BackendConfig>> for ServerRegistry {
    fn from(servers: HashMap<String, BackendConfig>) -> Self {
        Self::new(servers)
    }
}

impl fmt::Debug for ServerRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl Serialize for ServerRegistry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.read().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ServerRegistry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        HashMap::deserialize(deserializer).map(Self::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ForwardingMode;

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
    fn clones_share_changes() {
        let registry = ServerRegistry::default();
        let router_view = registry.clone();

        registry.insert("game-1".into(), backend("10.0.0.2:25565"));
        assert_eq!(
            router_view.get("game-1").map(|c| c.address),
            Some("10.0.0.2:25565".to_string())
        );

        registry.remove("game-1");
        assert!(!router_view.contains_key("game-1"));
    }

    #[test]
    fn serializes_as_the_servers_map() {
        let registry = ServerRegistry::default();
        registry.insert("lobby".into(), backend("127.0.0.1:25566"));

        let toml = toml::to_string(&registry).unwrap();
        let back: ServerRegistry = toml::from_str(&toml).unwrap();
        assert_eq!(back.keys().collect::<Vec<_>>(), vec!["lobby".to_string()]);
        assert_eq!(back.get("lobby").unwrap().address, "127.0.0.1:25566");
    }
}
