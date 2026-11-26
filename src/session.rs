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

    /// Registers a new active player session.
    ///
    /// Returns the previous session registered under the same (case-insensitive) username, if
    /// any, so the caller can disconnect the older connection instead of leaving it unmanaged.
    pub fn register(&self, session: PlayerSession) -> Option<PlayerSession> {
        let uuid = session.uuid;
        let key = session.username.to_ascii_lowercase();
        let replaced = if let Ok(mut lock) = self.sessions.write() {
            lock.insert(key, session)
        } else {
            None
        };
        if let Some(ref old) = replaced {
            self.telemetry_sessions.record_leave(old);
        }
        self.telemetry_sessions.record_join(uuid);
        replaced
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

    /// Unregisters the session for `username` only if it still belongs to the connection that
    /// owns `action_tx`.
    ///
    /// A connection must use this (not [`Self::unregister`]) on teardown: if the same username
    /// logged in again in the meantime, the newer connection's session must stay registered.
    pub fn unregister_connection(
        &self,
        username: &str,
        action_tx: &mpsc::UnboundedSender<PlayerAction>,
    ) -> Option<PlayerSession> {
        let key = username.to_ascii_lowercase();
        let removed = if let Ok(mut lock) = self.sessions.write() {
            match lock.get(&key) {
                Some(session) if session.action_tx.same_channel(action_tx) => lock.remove(&key),
                _ => None,
            }
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

    /// Sends a disconnect action to every active session and returns how many were notified.
    pub fn disconnect_all(&self, reason: &str) -> usize {
        let sessions = self.list_sessions();
        let mut notified = 0;
        for session in &sessions {
            if session
                .action_tx
                .send(PlayerAction::Disconnect(reason.to_string()))
                .is_ok()
            {
                notified += 1;
            }
        }
        notified
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

    #[test]
    fn test_session_disconnect_all() {
        let manager = SessionManager::new();
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        manager.register(PlayerSession {
            username: "Steve".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "lobby".to_string(),
            action_tx: tx1,
            client_ip: "127.0.0.1:1000".to_string(),
            protocol_version: 776,
            client_brand: None,
            locale: None,
            view_distance: None,
        });
        manager.register(PlayerSession {
            username: "Alex".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "survival".to_string(),
            action_tx: tx2,
            client_ip: "127.0.0.1:1001".to_string(),
            protocol_version: 776,
            client_brand: None,
            locale: None,
            view_distance: None,
        });

        assert_eq!(manager.disconnect_all("Proxy is shutting down."), 2);
        assert_eq!(
            rx1.try_recv(),
            Ok(PlayerAction::Disconnect(
                "Proxy is shutting down.".to_string()
            ))
        );
        assert_eq!(
            rx2.try_recv(),
            Ok(PlayerAction::Disconnect(
                "Proxy is shutting down.".to_string()
            ))
        );
    }

    fn test_session(name: &str, tx: mpsc::UnboundedSender<PlayerAction>) -> PlayerSession {
        PlayerSession {
            username: name.to_string(),
            uuid: Uuid::new_v4(),
            current_server: "lobby".to_string(),
            action_tx: tx,
            client_ip: "127.0.0.1:1".to_string(),
            protocol_version: 765,
            client_brand: None,
            locale: None,
            view_distance: None,
        }
    }

    #[test]
    fn test_register_returns_replaced_session() {
        let manager = SessionManager::new();
        let (old_tx, _old_rx) = mpsc::unbounded_channel();
        let (new_tx, _new_rx) = mpsc::unbounded_channel();

        assert!(
            manager
                .register(test_session("Steve", old_tx.clone()))
                .is_none()
        );
        let replaced = manager
            .register(test_session("steve", new_tx))
            .expect("second login with the same name replaces the first");
        assert!(replaced.action_tx.same_channel(&old_tx));
        assert_eq!(manager.count(), 1);
    }

    #[test]
    fn test_unregister_connection_keeps_newer_session() {
        let manager = SessionManager::new();
        let (old_tx, _old_rx) = mpsc::unbounded_channel();
        let (new_tx, _new_rx) = mpsc::unbounded_channel();

        manager.register(test_session("Steve", old_tx.clone()));
        manager.register(test_session("Steve", new_tx.clone()));

        // The old connection tears down after the new one registered: it must not remove it.
        assert!(manager.unregister_connection("Steve", &old_tx).is_none());
        assert_eq!(manager.count(), 1);
        assert!(
            manager
                .get_session("Steve")
                .unwrap()
                .action_tx
                .same_channel(&new_tx)
        );

        assert!(manager.unregister_connection("steve", &new_tx).is_some());
        assert_eq!(manager.count(), 0);
    }
}
