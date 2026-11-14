use serde::{Deserialize, Serialize};

/// Bedrock Edition server-list replies.
///
/// Vine can't carry Bedrock players yet: this only answers RakNet unconnected
/// pings, so the proxy shows up in a Bedrock client's server list. Clients from
/// 26.60 on connect over NetherNet and don't send these pings, and older clients
/// that see the entry can't join through it. Off by default so the proxy
/// doesn't hold UDP 19132, which a Bedrock bridge such as Geyser usually needs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BedrockConfig {
    /// Whether to answer Bedrock (RakNet) server-list pings. Off by default.
    pub enabled: bool,
    /// UDP address and port for the Bedrock listener (default Minecraft Bedrock port is 19132)
    pub bind_address: String,
    /// Primary MOTD line shown in Bedrock server lists
    pub motd: String,
    /// Secondary MOTD line / subtitle shown in Bedrock server lists
    pub sub_motd: String,
    /// Protocol version advertised (2169 corresponds to Bedrock 1.26.45)
    pub protocol_version: u32,
    /// Bedrock version string advertised (e.g. "1.26.45")
    pub version_name: String,
    /// Level / world name displayed in Bedrock client ping
    pub level_name: String,
    /// Game mode displayed in Bedrock client ping ("Survival", "Creative", etc.)
    pub game_mode: String,
    /// Game mode ID (0 = Survival, 1 = Creative, 2 = Adventure, 3 = Spectator)
    pub game_mode_id: u32,
    /// Maximum Bedrock players displayed in ping (0 to inherit global server max_players)
    pub max_players: u32,
    /// Server GUID for RakNet (0 automatically generates a random GUID on startup)
    pub server_guid: u64,
}

impl Default for BedrockConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: "0.0.0.0:19132".to_string(),
            motd: "§a§lVine §7» §fHigh-Performance Minecraft Proxy".to_string(),
            sub_motd: "Bedrock Support • 1.26.x".to_string(),
            protocol_version: 2169,
            version_name: "1.26.45".to_string(),
            level_name: "Vine Proxy".to_string(),
            game_mode: "Survival".to_string(),
            game_mode_id: 0,
            max_players: 100,
            server_guid: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BedrockConfig;

    #[test]
    fn listener_is_off_by_default() {
        assert!(!BedrockConfig::default().enabled);
        let parsed: BedrockConfig = toml::from_str("").unwrap();
        assert!(!parsed.enabled);
    }
}
