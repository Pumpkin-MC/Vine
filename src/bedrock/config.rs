use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// Default Bedrock Minecraft version supported via NetherNet.
pub const CURRENT_BEDROCK_MC_VERSION: &str = "1.26.63";

/// Default Bedrock Minecraft protocol version supported via NetherNet.
pub const CURRENT_BEDROCK_MC_PROTOCOL: u32 = 2225;

/// Fixed UDP port used by Bedrock NetherNet LAN discovery and LAN signaling.
pub const NETHERNET_DISCOVERY_PORT: u16 = 7551;

fn default_true() -> bool {
    true
}

fn default_bind_address() -> String {
    "0.0.0.0:19132".to_string()
}

fn default_motd() -> String {
    "§a§lVine §7» §fHigh-Performance Minecraft Proxy".to_string()
}

fn default_level_name() -> String {
    "Vine Proxy".to_string()
}

const fn default_protocol_version() -> u32 {
    CURRENT_BEDROCK_MC_PROTOCOL
}

fn default_version_name() -> String {
    CURRENT_BEDROCK_MC_VERSION.to_string()
}

const fn default_max_players() -> u32 {
    100
}

/// Configuration options for Bedrock Edition `NetherNet` support.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BedrockConfig {
    /// Whether the Bedrock `NetherNet` signaling, ICE, and discovery listeners are enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Shared TCP (HTTP signaling/status) and UDP (ICE/direct discovery) bind address.
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    /// Whether encrypted NetherNet LAN discovery on UDP port 7551 is enabled.
    #[serde(default = "default_true")]
    pub lan_discovery: bool,
    /// Whether Xbox Live / Mojang identity authentication is required (`accepts_self_signed = !online_mode`).
    #[serde(default = "default_true")]
    pub online_mode: bool,
    /// Whether hardcore mode is advertised in NetherNet discovery responses.
    #[serde(default)]
    pub hardcore: bool,
    /// Server name / MOTD advertised in NetherNet HTTP status and LAN discovery.
    #[serde(default = "default_motd")]
    pub motd: String,
    /// Level / world name advertised in NetherNet HTTP status and LAN discovery.
    #[serde(default = "default_level_name")]
    pub level_name: String,
    /// Bedrock protocol version advertised (2225 corresponds to Bedrock 1.26.63).
    #[serde(default = "default_protocol_version")]
    pub protocol_version: u32,
    /// Bedrock version string advertised (e.g. `"1.26.63"`).
    #[serde(default = "default_version_name")]
    pub version_name: String,
    /// Game mode ID advertised (0 = Survival, 1 = Creative, 2 = Adventure, 3 = Spectator).
    #[serde(default, alias = "game_type")]
    pub game_mode_id: u8,
    /// Maximum Bedrock players advertised.
    #[serde(default = "default_max_players")]
    pub max_players: u32,
    /// 64-bit NetherNet network ID (`0` generates a random ID on startup).
    #[serde(default, alias = "server_guid")]
    pub network_id: u64,
    /// Optional public IP advertised in rewritten WebRTC SDP host candidates when behind NAT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ip: Option<IpAddr>,
    /// Optional Bedrock NetherNet backend address (e.g. `"127.0.0.1:19133"`) to proxy signaling & ICE traffic to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_address: Option<String>,
}

impl Default for BedrockConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            bind_address: default_bind_address(),
            lan_discovery: default_true(),
            online_mode: default_true(),
            hardcore: false,
            motd: default_motd(),
            level_name: default_level_name(),
            protocol_version: default_protocol_version(),
            version_name: default_version_name(),
            game_mode_id: 0,
            max_players: default_max_players(),
            network_id: 0,
            external_ip: None,
            backend_address: None,
        }
    }
}
