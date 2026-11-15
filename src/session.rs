use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::mpsc;
use uuid::Uuid;

use pumpkin_util::text::TextComponent;

use crate::telemetry::TelemetrySessionTracker;

/// Actions that can be dispatched to an active player's connection
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerAction {
    /// Request connecting the player to a target backend server in-proxy
    Connect { server_name: String },
    /// Request transfer of the player to a target host and port
    Transfer { host: String, port: u16 },
    /// Send a system chat message to the player
    Message(TextComponent),
    /// Disconnect/kick the player with a reason
    Disconnect(String),
    /// Send a custom plugin message / payload to the player
    PluginMessage { channel: String, data: Vec<u8> },
}

/// Represents an active connected player session on the proxy
#[derive(Clone)]
pub struct PlayerSession {
    pub username: String,
    pub uuid: Uuid,
    pub current_server: String,
    pub action_tx: mpsc::UnboundedSender<PlayerAction>,
    pub client_ip: String,
    pub protocol_version: u32,
    pub client_brand: Option<String>,
    pub locale: Option<String>,
    pub view_distance: Option<u8>,
}

/// Thread-safe registry of connected player sessions
pub struct SessionManager {
    sessions: RwLock<HashMap<String, PlayerSession>>,
    telemetry_sessions: TelemetrySessionTracker,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            telemetry_sessions: TelemetrySessionTracker::default(),
        }
    }

    /// Returns a reference to the telemetry session tracker.
    #[must_use]
    pub const fn telemetry_sessions(&self) -> &TelemetrySessionTracker {
        &self.telemetry_sessions
    }

    /// Registers a new active player session
    pub fn register(&self, session: PlayerSession) {
        self.telemetry_sessions.record_join(session.uuid);
        if let Ok(mut lock) = self.sessions.write() {
            lock.insert(session.username.to_ascii_lowercase(), session);
        }
    }

    /// Unregisters an active session by username
    pub fn unregister(&self, username: &str) -> Option<PlayerSession> {
        let removed = if let Ok(mut lock) = self.sessions.write() {
            lock.remove(&username.to_ascii_lowercase())
        } else {
            None
        };
        if let Some(ref session) = removed {
            self.telemetry_sessions.record_leave(session);
        }
        removed
    }

    /// Updates the current server for an active session
    pub fn update_server(&self, username: &str, new_server: &str) {
        if let Ok(mut lock) = self.sessions.write()
            && let Some(session) = lock.get_mut(&username.to_ascii_lowercase())
        {
            session.current_server = new_server.to_string();
        }
    }

    /// Updates the client brand (`minecraft:brand`) for an active session
    pub fn update_client_brand(&self, username: &str, brand: String) {
        if let Ok(mut lock) = self.sessions.write()
            && let Some(session) = lock.get_mut(&username.to_ascii_lowercase())
        {
            session.client_brand = Some(brand);
        }
    }

    /// Updates client locale and view distance for an active session
    pub fn update_client_settings(&self, username: &str, locale: String, view_distance: u8) {
        if let Ok(mut lock) = self.sessions.write()
            && let Some(session) = lock.get_mut(&username.to_ascii_lowercase())
        {
            session.locale = Some(locale);
            session.view_distance = Some(view_distance);
        }
    }

    /// Looks up an active session by username (case-insensitive)
    pub fn get_session(&self, username: &str) -> Option<PlayerSession> {
        if let Ok(lock) = self.sessions.read() {
            lock.get(&username.to_ascii_lowercase()).cloned()
        } else {
            None
        }
    }

    /// Returns a list of all active sessions
    pub fn list_sessions(&self) -> Vec<PlayerSession> {
        if let Ok(lock) = self.sessions.read() {
            lock.values().cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Returns the number of currently registered sessions
    pub fn count(&self) -> usize {
        if let Ok(lock) = self.sessions.read() {
            lock.len()
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_registration_and_lookup() {
        let manager = SessionManager::new();
        assert_eq!(manager.count(), 0);

        let (tx, _rx) = mpsc::unbounded_channel();
        let session = PlayerSession {
            username: "Steve".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "lobby".to_string(),
            action_tx: tx,
            client_ip: "127.0.0.1:12345".to_string(),
            protocol_version: 765,
            client_brand: None,
            locale: None,
            view_distance: None,
        };

        manager.register(session);
        assert_eq!(manager.count(), 1);

        let found = manager.get_session("steve");
        assert!(found.is_some());
        assert_eq!(found.unwrap().username, "Steve");

        let removed = manager.unregister("STEVE");
        assert!(removed.is_some());
        assert_eq!(manager.count(), 0);
        assert!(manager.get_session("Steve").is_none());
    }

    #[test]
    fn test_session_update_server() {
        let manager = SessionManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = PlayerSession {
            username: "Alex".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "lobby".to_string(),
            action_tx: tx,
            client_ip: "127.0.0.1:12346".to_string(),
            protocol_version: 765,
            client_brand: None,
            locale: None,
            view_distance: None,
        };
        manager.register(session);
        assert_eq!(manager.get_session("Alex").unwrap().current_server, "lobby");

        manager.update_server("alex", "survival");
        assert_eq!(
            manager.get_session("Alex").unwrap().current_server,
            "survival"
        );
    }
}
