use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};

fn default_true() -> bool {
    true
}

fn default_compression_level() -> i32 {
    6
}

fn default_shutdown_timeout() -> u64 {
    10
}

fn default_tcp_keepalive() -> u64 {
    60
}

fn default_handshake_timeout() -> u64 {
    10
}

fn default_status_timeout() -> u64 {
    5
}

fn default_login_timeout() -> u64 {
    15
}

fn default_backend_connect_timeout() -> u64 {
    5
}

fn default_backend_login_timeout() -> u64 {
    15
}

fn default_backend_retries() -> u32 {
    2
}

fn default_listen_backlog() -> u32 {
    1024
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    pub motd: MotdConfig,
    pub security: SecurityConfig,
    pub routing: RoutingConfig,
    pub servers: HashMap<String, BackendConfig>,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub bedrock: crate::bedrock::BedrockConfig,
    #[serde(default)]
    pub permissions: PermissionsConfig,
    #[serde(default)]
    pub plugins: PluginConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_plugins_dir")]
    pub plugin_dir: String,
}

fn default_plugins_dir() -> String {
    "./plugins".to_string()
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            plugin_dir: default_plugins_dir(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Address and port for the proxy to listen on
    pub bind_address: String,
    /// Whether to verify player accounts with Mojang session servers
    pub online_mode: bool,
    /// Send player IP to Mojang during session authentication to prevent proxy hijacking
    pub prevent_proxy_connections: bool,
    /// Packet compression threshold (-1 to disable, e.g. 256 for standard compression)
    pub compression_threshold: i32,
    /// zlib / flate2 compression level (1 = fastest, 6 = default balance, 9 = best compression)
    #[serde(default = "default_compression_level")]
    pub compression_level: i32,
    /// Graceful shutdown timeout in seconds to wait for active connections to close
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout_secs: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Enable TCP_NODELAY (disable Nagle's algorithm) on client and backend sockets.
    /// Crucial for eliminating latency spikes in Minecraft packet transmission.
    #[serde(default = "default_true")]
    pub tcp_nodelay: bool,

    /// TCP keepalive interval in seconds on client and backend sockets (0 to disable).
    #[serde(default = "default_tcp_keepalive")]
    pub tcp_keepalive_secs: u64,

    /// Timeout in seconds to wait for initial client Handshake packet
    #[serde(default = "default_handshake_timeout")]
    pub client_handshake_timeout_secs: u64,

    /// Timeout in seconds to wait for client Status/Ping packets
    #[serde(default = "default_status_timeout")]
    pub client_status_timeout_secs: u64,

    /// Timeout in seconds to wait for client LoginStart and Encryption packets
    #[serde(default = "default_login_timeout")]
    pub client_login_timeout_secs: u64,

    /// Timeout in seconds when connecting to a backend server
    #[serde(default = "default_backend_connect_timeout")]
    pub backend_connect_timeout_secs: u64,

    /// Timeout in seconds for backend socket reads during the login handshake phase
    #[serde(default = "default_backend_login_timeout")]
    pub backend_login_timeout_secs: u64,

    /// Number of connection retry attempts when connecting to a backend server before failing
    #[serde(default = "default_backend_retries")]
    pub backend_connect_retry_attempts: u32,

    /// Optional TCP receive buffer size (SO_RCVBUF) in bytes
    #[serde(default)]
    pub recv_buffer_size: Option<usize>,

    /// Optional TCP send buffer size (SO_SNDBUF) in bytes
    #[serde(default)]
    pub send_buffer_size: Option<usize>,

    /// TCP listen backlog queue size
    #[serde(default = "default_listen_backlog")]
    pub listen_backlog: u32,

    /// Support HAProxy PROXY Protocol (v1 and v2) from upstream reverse proxies (e.g. Cloudflare Spectrum, HAProxy)
    #[serde(default)]
    pub proxy_protocol: bool,

    /// Log incoming server list status pings in the console
    #[serde(default)]
    pub log_pings: bool,

    /// Enable BungeeCord / Velocity plugin message channel handling (bungeecord:main)
    #[serde(default = "default_true")]
    pub bungee_plugin_message_channel: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            tcp_nodelay: default_true(),
            tcp_keepalive_secs: default_tcp_keepalive(),
            client_handshake_timeout_secs: default_handshake_timeout(),
            client_status_timeout_secs: default_status_timeout(),
            client_login_timeout_secs: default_login_timeout(),
            backend_connect_timeout_secs: default_backend_connect_timeout(),
            backend_login_timeout_secs: default_backend_login_timeout(),
            backend_connect_retry_attempts: default_backend_retries(),
            recv_buffer_size: None,
            send_buffer_size: None,
            listen_backlog: default_listen_backlog(),
            proxy_protocol: false,
            log_pings: false,
            bungee_plugin_message_channel: true,
        }
    }
}

fn default_ping_passthrough() -> String {
    "DISABLED".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MotdConfig {
    /// Line 1 of the server list ping description
    pub line1: String,
    /// Line 2 of the server list ping description
    pub line2: String,
    /// Maximum players shown in the server list ping
    pub max_players: u32,
    /// Custom version display string (e.g. "Vine 1.7.x - 26.x")
    pub version_name: String,
    /// Protocol version reported to clients (776 is 26.2)
    pub protocol_version: i32,
    /// Optional sample player list shown on hover in server list
    pub sample_players: Vec<String>,
    /// Optional path to a 64x64 PNG file for the server list icon (favicons)
    #[serde(default)]
    pub icon_path: Option<String>,
    /// Whether to show the player sample list on hover in the multiplayer menu
    #[serde(default = "default_true")]
    pub show_sample_players: bool,
    /// Ping passthrough mode matching Velocity: "DISABLED", "ALL", "DESCRIPTION", "MODS"
    #[serde(default = "default_ping_passthrough")]
    pub ping_passthrough: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Enable connection rate limiting per IP
    pub rate_limit_enabled: bool,
    /// Maximum new TCP connections per second per IP
    pub max_connections_per_sec: u32,
    /// Burst allowance for new connections
    pub connection_burst: u32,
    /// Maximum packets per second from a single client before being kicked for flooding
    pub max_packets_per_sec: u32,
    /// Maximum raw packet size allowed in bytes (default 2MB)
    pub max_packet_size: usize,
    /// Enforce strict Minecraft username validation (^[a-zA-Z0-9_]{2,16}$)
    pub validate_usernames: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Default backend server name to connect to
    pub default_server: String,
    /// Fallback server if default server is offline
    pub fallback_server: Option<String>,
    /// Priority order of servers to try when connecting or falling back (Velocity `try` list)
    #[serde(default)]
    pub try_servers: Vec<String>,
    /// Virtual host routing: maps domain/hostname to backend server name
    pub forced_hosts: HashMap<String, String>,
    /// Automatically try fallback server if connection to target server fails
    #[serde(default = "default_true")]
    pub try_fallback_on_connect_failure: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ForwardingMode {
    /// Plain connection without forwarding info (for Vanilla servers in offline-mode)
    None,
    /// BungeeCord IP forwarding (for Bukkit, Spigot, PaperMC with bungeecord: true)
    #[serde(alias = "legacy")]
    BungeeCord,
    /// Velocity modern forwarding with HMAC-SHA256 secret (for PaperMC, Purpur, Pumpkin)
    #[serde(alias = "modern")]
    Velocity,
    /// Vine modern forwarding with Ed25519 asymmetric signatures, timestamp, and challenge nonce (for Pumpkin)
    Vine,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackendConfig {
    /// Address of backend server (e.g. "127.0.0.1:25566")
    pub address: String,
    /// Forwarding mode to use when connecting to this backend
    pub forwarding: ForwardingMode,
    /// Secret key required if forwarding mode is Velocity
    pub secret: Option<String>,
    /// Optional custom 64x64 PNG status icon path for this backend / virtual host
    #[serde(default)]
    pub icon_path: Option<String>,
    /// Optional custom MOTD Line 1 for this backend / virtual host
    #[serde(default)]
    pub motd_line1: Option<String>,
    /// Optional custom MOTD Line 2 for this backend / virtual host
    #[serde(default)]
    pub motd_line2: Option<String>,
}

fn default_telemetry_interval() -> u64 {
    300
}

fn default_telemetry_endpoint() -> String {
    "https://market.pumpkinmc.org/api/v1/rest/telemetry/heartbeat".to_string()
}

/// Telemetry configuration options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether anonymous telemetry is enabled (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Custom telemetry backend ingestion endpoint
    #[serde(default = "default_telemetry_endpoint")]
    pub endpoint: String,
    /// Heartbeat interval in seconds (default: 300 seconds / 5 minutes). Minimum 60s.
    #[serde(default = "default_telemetry_interval")]
    pub interval_secs: u64,
    /// Whether to opt-in to displaying this proxy in the public community directory.
    #[serde(default)]
    pub public: bool,
    /// Public server/proxy name displayed on the analytics dashboard if public is true.
    #[serde(default)]
    pub server_name: Option<String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            endpoint: default_telemetry_endpoint(),
            interval_secs: 300,
            public: false,
            server_name: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut servers = HashMap::new();
        servers.insert(
            "lobby".to_string(),
            BackendConfig {
                address: "127.0.0.1:25566".to_string(),
                forwarding: ForwardingMode::Velocity,
                secret: Some("vine-velocity-secret-change-me".to_string()),
                icon_path: None,
                motd_line1: None,
                motd_line2: None,
            },
        );
        servers.insert(
            "survival".to_string(),
            BackendConfig {
                address: "127.0.0.1:25567".to_string(),
                forwarding: ForwardingMode::BungeeCord,
                secret: None,
                icon_path: None,
                motd_line1: Some("§b§lVine Survival §7» §fWelcome back!".to_string()),
                motd_line2: Some("§7Custom world • Economy • Land Claims".to_string()),
            },
        );
        servers.insert(
            "vanilla".to_string(),
            BackendConfig {
                address: "127.0.0.1:25568".to_string(),
                forwarding: ForwardingMode::None,
                secret: None,
                icon_path: None,
                motd_line1: None,
                motd_line2: None,
            },
        );

        let mut forced_hosts = HashMap::new();
        forced_hosts.insert("survival.example.com".to_string(), "survival".to_string());
        forced_hosts.insert("vanilla.example.com".to_string(), "vanilla".to_string());

        Self {
            server: ServerConfig {
                bind_address: "0.0.0.0:25565".to_string(),
                online_mode: true,
                prevent_proxy_connections: true,
                compression_threshold: 256,
                compression_level: 6,
                shutdown_timeout_secs: 10,
            },
            network: NetworkConfig::default(),
            motd: MotdConfig {
                line1: "§a§lVine §7» §fHigh-Performance Minecraft Proxy".to_string(),
                line2: "§7Multi-version support §8• §e1.7.x - 26.x".to_string(),
                max_players: 100,
                version_name: "Vine 1.7.x - 26.x".to_string(),
                protocol_version: 776,
                sample_players: vec![
                    "§aVine Multi-Version Proxy".to_string(),
                    "§7Secure • Fast • Reliable".to_string(),
                ],
                icon_path: None,
                show_sample_players: true,
                ping_passthrough: default_ping_passthrough(),
            },
            security: SecurityConfig {
                rate_limit_enabled: true,
                max_connections_per_sec: 5,
                connection_burst: 10,
                max_packets_per_sec: 500,
                max_packet_size: 2 * 1024 * 1024,
                validate_usernames: true,
            },
            routing: RoutingConfig {
                default_server: "lobby".to_string(),
                fallback_server: Some("survival".to_string()),
                try_servers: vec!["lobby".to_string(), "survival".to_string()],
                forced_hosts,
                try_fallback_on_connect_failure: true,
            },
            servers,
            telemetry: TelemetryConfig::default(),
            bedrock: crate::bedrock::BedrockConfig::default(),
            permissions: PermissionsConfig::default(),
            plugins: PluginConfig::default(),
        }
    }
}

pub use crate::permissions::{GroupConfig, PermissionsConfig, UserPermissionConfig};

impl Config {
    /// Loads a configuration from a TOML string, supporting either native Vine syntax or standard Velocity 1:1 syntax.
    pub fn load_from_str(content: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let val: toml::Value = toml::from_str(content)?;
        if let toml::Value::Table(ref table) = val
            && (table.contains_key("bind")
                || table.contains_key("player-info-forwarding-mode")
                || table.contains_key("config-version")
                || table.contains_key("show-max-players"))
        {
            let vel: VelocityConfig = toml::from_str(content)?;
            return Ok(vel.into_config());
        }

        let config: Config = toml::from_str(content)?;
        Ok(config)
    }

    pub fn load_or_create<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.as_ref();
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            Self::load_from_str(&content)
        } else {
            let default_config = Config::default();
            let toml_string = toml::to_string_pretty(&default_config)?;
            std::fs::write(path, toml_string)?;
            Ok(default_config)
        }
    }
}

/// 1:1 Velocity configuration layout (`velocity.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct VelocityConfig {
    #[serde(default)]
    pub config_version: Option<String>,
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub motd: Option<String>,
    #[serde(default)]
    pub show_max_players: Option<u32>,
    #[serde(default = "default_true")]
    pub online_mode: bool,
    #[serde(default)]
    pub prevent_client_proxy_connections: bool,
    #[serde(default = "default_velocity_forwarding")]
    pub player_info_forwarding_mode: String,
    #[serde(default)]
    pub forwarding_secret: Option<String>,
    #[serde(default)]
    pub announce_forge: bool,
    #[serde(default = "default_true")]
    pub kick_to_fallback: bool,
    #[serde(default = "default_ping_passthrough")]
    pub ping_passthrough: String,
    #[serde(default = "default_true")]
    pub sample_player_list: bool,

    #[serde(default)]
    pub servers: Option<toml::Table>,
    #[serde(default)]
    pub forced_hosts: Option<HashMap<String, toml::Value>>,
    #[serde(default)]
    pub advanced: Option<VelocityAdvancedConfig>,
}

fn default_velocity_forwarding() -> String {
    "modern".to_string()
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct VelocityAdvancedConfig {
    #[serde(default)]
    pub compression_threshold: Option<i32>,
    #[serde(default)]
    pub compression_level: Option<i32>,
    #[serde(default)]
    pub login_ratelimit: Option<u64>,
    #[serde(default)]
    pub connection_timeout: Option<u64>,
    #[serde(default)]
    pub read_timeout: Option<u64>,
    #[serde(default)]
    pub haproxy_protocol: Option<bool>,
    #[serde(default)]
    pub tcp_fast_open: Option<bool>,
    #[serde(default = "default_true")]
    pub bungee_plugin_message_channel: bool,
    #[serde(default)]
    pub show_ping_requests: Option<bool>,
    #[serde(default = "default_true")]
    pub failover_on_unexpected_server_disconnect: bool,
    #[serde(default = "default_true")]
    pub announce_proxy_commands: bool,
}

impl VelocityConfig {
    pub fn into_config(self) -> Config {
        let mut base = Config::default();

        if let Some(bind) = self.bind {
            base.server.bind_address = bind;
        }
        base.server.online_mode = self.online_mode;
        base.server.prevent_proxy_connections = self.prevent_client_proxy_connections;

        if let Some(motd) = self.motd {
            base.motd.line1 = motd;
            base.motd.line2.clear();
        }
        if let Some(max_p) = self.show_max_players {
            base.motd.max_players = max_p;
        }
        base.motd.show_sample_players = self.sample_player_list;
        base.motd.ping_passthrough = self.ping_passthrough;

        let global_forwarding = match self.player_info_forwarding_mode.to_ascii_lowercase().as_str() {
            "legacy" | "bungeecord" => ForwardingMode::BungeeCord,
            "vine" => ForwardingMode::Vine,
            "none" => ForwardingMode::None,
            _ => ForwardingMode::Velocity,
        };

        let global_secret = self.forwarding_secret.clone();

        if let Some(advanced) = self.advanced {
            if let Some(ct) = advanced.compression_threshold {
                base.server.compression_threshold = ct;
            }
            if let Some(cl) = advanced.compression_level
                && cl >= 0
            {
                base.server.compression_level = cl;
            }
            if let Some(ct_ms) = advanced.connection_timeout {
                base.network.backend_connect_timeout_secs = (ct_ms / 1000).max(1);
            }
            if let Some(rt_ms) = advanced.read_timeout {
                base.network.backend_login_timeout_secs = (rt_ms / 1000).max(1);
            }
            if let Some(hp) = advanced.haproxy_protocol {
                base.network.proxy_protocol = hp;
            }
            base.network.bungee_plugin_message_channel = advanced.bungee_plugin_message_channel;
            base.routing.try_fallback_on_connect_failure =
                advanced.failover_on_unexpected_server_disconnect;
        }

        base.routing.try_fallback_on_connect_failure = self.kick_to_fallback;

        if let Some(servers_table) = self.servers {
            let mut parsed_servers = HashMap::new();
            let mut try_list = Vec::new();

            for (key, val) in servers_table {
                if key.eq_ignore_ascii_case("try") {
                    if let toml::Value::Array(arr) = val {
                        for item in arr {
                            if let toml::Value::String(s) = item {
                                try_list.push(s);
                            }
                        }
                    }
                } else if let toml::Value::String(addr) = val {
                    parsed_servers.insert(
                        key,
                        BackendConfig {
                            address: addr,
                            forwarding: global_forwarding.clone(),
                            secret: global_secret.clone(),
                            icon_path: None,
                            motd_line1: None,
                            motd_line2: None,
                        },
                    );
                } else if let Ok(cfg) = val.try_into::<BackendConfig>() {
                    parsed_servers.insert(key, cfg);
                }
            }

            if !parsed_servers.is_empty() {
                base.servers = parsed_servers;
            }

            if !try_list.is_empty() {
                base.routing.default_server = try_list[0].clone();
                base.routing.fallback_server = try_list.get(1).cloned();
                base.routing.try_servers = try_list;
            }
        }

        if let Some(forced) = self.forced_hosts {
            let mut map = HashMap::new();
            for (host, val) in forced {
                if let toml::Value::Array(arr) = val {
                    if let Some(toml::Value::String(target)) = arr.first() {
                        map.insert(host, target.clone());
                    }
                } else if let toml::Value::String(target) = val {
                    map.insert(host, target);
                }
            }
            if !map.is_empty() {
                base.routing.forced_hosts = map;
            }
        }

        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_velocity_toml_1_to_1_deserialization() {
        let velocity_toml = r#"
            config-version = "2.7"
            bind = "0.0.0.0:25577"
            motd = "<#09add3>A Velocity Server"
            show-max-players = 500
            online-mode = true
            prevent-client-proxy-connections = false
            player-info-forwarding-mode = "modern"
            forwarding-secret = "test-forwarding-secret"
            announce-forge = false
            kick-to-fallback = true
            ping-passthrough = "ALL"
            sample-player-list = true

            [servers]
            lobby = "127.0.0.1:30066"
            factions = "127.0.0.1:30067"
            try = [
                "lobby",
                "factions"
            ]

            [forced-hosts]
            "lobby.example.com" = [
                "lobby"
            ]

            [advanced]
            compression-threshold = 256
            compression-level = -1
            connection-timeout = 5000
            read-timeout = 30000
            bungee-plugin-message-channel = true
            failover-on-unexpected-server-disconnect = true
        "#;

        let config = Config::load_from_str(velocity_toml).expect("Failed to parse velocity.toml");
        assert_eq!(config.server.bind_address, "0.0.0.0:25577");
        assert_eq!(config.motd.line1, "<#09add3>A Velocity Server");
        assert_eq!(config.motd.max_players, 500);
        assert!(config.server.online_mode);
        assert_eq!(config.routing.default_server, "lobby");
        assert_eq!(config.routing.fallback_server.as_deref(), Some("factions"));
        assert_eq!(config.routing.try_servers, vec!["lobby", "factions"]);
        assert!(config.servers.contains_key("lobby"));
        assert_eq!(config.servers["lobby"].address, "127.0.0.1:30066");
        assert_eq!(config.servers["lobby"].forwarding, ForwardingMode::Velocity);
        assert_eq!(config.routing.forced_hosts.get("lobby.example.com").map(String::as_str), Some("lobby"));
        assert!(config.network.bungee_plugin_message_channel);
    }

    #[test]
    fn test_telemetry_config_defaults() {
        let default_config = TelemetryConfig::default();
        assert!(default_config.enabled);
        assert_eq!(
            default_config.endpoint,
            "https://market.pumpkinmc.org/api/v1/rest/telemetry/heartbeat"
        );
        assert_eq!(default_config.interval_secs, 300);
        assert!(!default_config.public);
        assert_eq!(default_config.server_name, None);
    }

    #[test]
    fn test_telemetry_toml_deserialization() {
        let toml_str = r#"
            enabled = false
            endpoint = "http://localhost:5000/api/v1/rest/telemetry/heartbeat"
            interval_secs = 120
            public = true
            server_name = "Vine Proxy Hub"
        "#;

        let config: TelemetryConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.enabled);
        assert_eq!(
            config.endpoint,
            "http://localhost:5000/api/v1/rest/telemetry/heartbeat"
        );
        assert_eq!(config.interval_secs, 120);
        assert!(config.public);
        assert_eq!(config.server_name.as_deref(), Some("Vine Proxy Hub"));
    }
}
