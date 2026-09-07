use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::mpsc;
use uuid::Uuid;

use pumpkin_util::text::TextComponent;

/// Actions that can be dispatched to an active player's connection
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerAction {
    /// Request connecting the player to a target backend server in-proxy
    Connect { server_name: String },
    /// Request transfer of the player to a target host and port
    Transfer { host: String, port: u16 },
    /// Send a system chat message to the player
    Message(TextComponent),
}

/// Represents an active connected player session on the proxy
#[derive(Clone)]
pub struct PlayerSession {
    pub username: String,
    pub uuid: Uuid,
    pub current_server: String,
    pub action_tx: mpsc::UnboundedSender<PlayerAction>,
}

/// Thread-safe registry of connected player sessions
pub struct SessionManager {
    sessions: RwLock<HashMap<String, PlayerSession>>,
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
        }
    }

    /// Registers a new active player session
    pub fn register(&self, session: PlayerSession) {
        if let Ok(mut lock) = self.sessions.write() {
            lock.insert(session.username.to_ascii_lowercase(), session);
        }
    }

    /// Unregisters an active session by username
    pub fn unregister(&self, username: &str) -> Option<PlayerSession> {
        if let Ok(mut lock) = self.sessions.write() {
            lock.remove(&username.to_ascii_lowercase())
        } else {
            None
        }
    }

    /// Updates the current server for an active session
    pub fn update_server(&self, username: &str, new_server: &str) {
        if let Ok(mut lock) = self.sessions.write()
            && let Some(session) = lock.get_mut(&username.to_ascii_lowercase())
        {
            session.current_server = new_server.to_string();
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
