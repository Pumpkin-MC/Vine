//! Host proxy query, player control, and inter-plugin IPC functions.

pub use crate::wit::vine::plugin::types::{PlayerInfo, ServerInfo};

use crate::wit::vine::plugin::{ipc as ipc_wit, proxy as proxy_wit};

/// Returns the running Vine proxy version string.
#[must_use]
pub fn proxy_version() -> String {
    proxy_wit::get_proxy_version()
}

/// Returns the total number of connected players on the proxy.
#[must_use]
pub fn player_count() -> u32 {
    proxy_wit::get_player_count()
}

/// Returns information about all currently connected players.
#[must_use]
pub fn players() -> Vec<PlayerInfo> {
    proxy_wit::get_players()
}

/// Looks up a connected player by username (case-insensitive).
#[must_use]
pub fn player_by_name(username: &str) -> Option<PlayerInfo> {
    proxy_wit::get_player_by_name(username)
}

/// Looks up a connected player by UUID string.
#[must_use]
pub fn player_by_uuid(uuid: &str) -> Option<PlayerInfo> {
    proxy_wit::get_player_by_uuid(uuid)
}

/// Returns all configured backend servers and their current online player counts.
#[must_use]
pub fn servers() -> Vec<ServerInfo> {
    proxy_wit::get_servers()
}

/// Broadcasts a system chat message to all connected players and the proxy console.
pub fn broadcast(message: &str) {
    proxy_wit::broadcast(message);
}

/// Sends a system chat message to a specific player by UUID.
pub fn send_message(player_uuid: &str, message: &str) -> bool {
    proxy_wit::send_message(player_uuid, message)
}

/// Disconnects/kicks a player by UUID with `reason`.
pub fn disconnect_player(player_uuid: &str, reason: &str) -> bool {
    proxy_wit::disconnect_player(player_uuid, reason)
}

/// Connects/switches a player by UUID to a configured backend server.
pub fn connect_player(player_uuid: &str, server_name: &str) -> bool {
    proxy_wit::connect_player(player_uuid, server_name)
}

/// Transfers a player (1.20.5+ `Transfer` packet) to an external `host:port`.
pub fn transfer_player(player_uuid: &str, host: &str, port: u16) -> bool {
    proxy_wit::transfer_player(player_uuid, host, port)
}

/// Sends a custom payload / plugin message (`channel`, `data`) to a player by UUID.
pub fn send_plugin_message(player_uuid: &str, channel: &str, data: &[u8]) -> bool {
    proxy_wit::send_plugin_message(player_uuid, channel, data)
}

/// Executes a proxy command as the console sender.
pub fn execute_command(command: &str) -> bool {
    proxy_wit::execute_command(command)
}

/// Sends a binary IPC message to another loaded plugin by name and awaits its reply.
pub fn send_ipc_message(target_plugin: &str, message: &[u8]) -> Result<Vec<u8>, String> {
    ipc_wit::send_ipc_message(target_plugin, message)
}
