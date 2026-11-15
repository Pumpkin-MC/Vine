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
#[serde(default)]
pub struct PluginConfig {
    /// Whether the WebAssembly plugin system is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Directory from which `.wasm` plugin components are loaded.
    #[serde(default = "default_plugins_dir")]
    pub plugin_dir: String,
    /// Whether to verify Ed25519 WASM signatures on plugins during loading.
    #[serde(default)]
    pub verify_signatures: bool,
    /// Whether unsigned plugins are permitted when `verify_signatures` is enabled.
    #[serde(default = "default_true")]
    pub allow_unsigned: bool,
    /// Trusted 64-character hex Ed25519 public keys accepted for signed WASM plugins.
    #[serde(default)]
    pub trusted_public_keys: Vec<String>,
    /// Permissions globally blocked across all plugins (e.g. `["network.outbound"]`).
    #[serde(default)]
    pub blocked_permissions: Vec<String>,
    /// Whether host environment variables are inherited by WASI plugins by default.
    #[serde(default)]
    pub inherit_env: bool,
    /// Whether plugin WASI network sockets are restricted to loopback (`127.0.0.1` / `::1`) by default.
    #[serde(default)]
    pub loopback_only: bool,
    /// Optional global maximum linear memory limit in megabytes (MB) per plugin instance.
    #[serde(default)]
    pub max_memory_mb: Option<u64>,
    /// Per-plugin configuration overrides keyed by plugin metadata name.
    #[serde(default)]
    pub overrides: HashMap<String, PluginOverride>,
}

/// Per-plugin security and resource overrides.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginOverride {
    /// Whether this specific plugin is enabled.
    pub enabled: bool,
    /// Optional override for allowing unsigned execution when `verify_signatures = true`.
    pub allow_unsigned: Option<bool>,
    /// Optional maximum memory limit in megabytes (MB) for this specific plugin.
    pub max_memory_mb: Option<u64>,
    /// Additional permissions blocked specifically for this plugin.
    pub blocked_permissions: Vec<String>,
    /// Optional override for restricting network sockets to loopback addresses.
    pub loopback_only: Option<bool>,
    /// Custom environment variables injected into this plugin's WASI context.
    pub environment: HashMap<String, String>,
}

impl Default for PluginOverride {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_unsigned: None,
            max_memory_mb: None,
            blocked_permissions: Vec::new(),
            loopback_only: None,
            environment: HashMap::new(),
        }
    }
}

fn default_plugins_dir() -> String {
    "./plugins".to_string()
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            plugin_dir: default_plugins_dir(),
            verify_signatures: false,
            allow_unsigned: true,
            trusted_public_keys: Vec::new(),
            blocked_permissions: Vec::new(),
            inherit_env: false,
            loopback_only: false,
            max_memory_mb: None,
            overrides: HashMap::new(),
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
        }
    }
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
    BungeeCord,
    /// Velocity modern forwarding with HMAC-SHA256 secret (for PaperMC, Purpur, Pumpkin)
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
    /// Secret key required if forwarding mode is Velocity or Vine
    pub secret: Option<String>,
    /// Optional 64-character hex Ed25519 telemetry public key of this backend server
    /// for cryptographic topology verification in Pumpkin Account
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
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
    "https://telemetry.pumpkinmc.org/v1/heartbeat".to_string()
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
    /// Optional 64-character hex Ed25519 telemetry public key of an upstream Vine proxy
    /// when running a multi-tier proxy hierarchy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_public_key: Option<String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            endpoint: default_telemetry_endpoint(),
            interval_secs: 300,
            public: false,
            server_name: None,
            upstream_public_key: None,
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
                public_key: None,
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
                public_key: None,
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
                public_key: None,
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
    pub fn load_or_create<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.as_ref();
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            let default_config = Config::default();
            let toml_string = toml::to_string_pretty(&default_config)?;
            std::fs::write(path, toml_string)?;
            Ok(default_config)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_config_defaults() {
        let default_config = TelemetryConfig::default();
        assert!(default_config.enabled);
        assert_eq!(
            default_config.endpoint,
            "https://telemetry.pumpkinmc.org/v1/heartbeat"
        );
        assert_eq!(default_config.interval_secs, 300);
        assert!(!default_config.public);
        assert_eq!(default_config.server_name, None);
        assert_eq!(default_config.upstream_public_key, None);
    }

    #[test]
    fn test_telemetry_toml_deserialization() {
        let toml_str = r#"
            enabled = false
            endpoint = "http://localhost:5000/v1/heartbeat"
            interval_secs = 120
            public = true
            server_name = "Vine Proxy Hub"
            upstream_public_key = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60"
        "#;

        let config: TelemetryConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.endpoint, "http://localhost:5000/v1/heartbeat");
        assert_eq!(config.interval_secs, 120);
        assert!(config.public);
        assert_eq!(config.server_name.as_deref(), Some("Vine Proxy Hub"));
        assert_eq!(
            config.upstream_public_key.as_deref(),
            Some("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
        );
    }

    #[test]
    fn test_plugin_config_defaults_and_overrides_toml() {
        let defaults = PluginConfig::default();
        assert!(!defaults.verify_signatures);
        assert!(defaults.allow_unsigned);
        assert!(defaults.trusted_public_keys.is_empty());
        assert!(defaults.blocked_permissions.is_empty());
        assert!(!defaults.inherit_env);
        assert!(!defaults.loopback_only);
        assert_eq!(defaults.max_memory_mb, None);
        assert!(defaults.overrides.is_empty());

        let toml_str = r#"
            verify_signatures = true
            allow_unsigned = false
            trusted_public_keys = ["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
            blocked_permissions = ["http.outbound"]
            inherit_env = false
            loopback_only = true
            max_memory_mb = 64

            [overrides.hub-router]
            enabled = true
            allow_unsigned = true
            blocked_permissions = ["fs.write.data"]
            max_memory_mb = 32
            loopback_only = false
            environment = { API_URL = "http://127.0.0.1:8080" }
        "#;

        let parsed: PluginConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.verify_signatures);
        assert!(!parsed.allow_unsigned);
        assert_eq!(parsed.trusted_public_keys.len(), 1);
        assert_eq!(parsed.blocked_permissions, vec!["http.outbound"]);
        assert!(parsed.loopback_only);
        assert_eq!(parsed.max_memory_mb, Some(64));

        let hub = parsed
            .overrides
            .get("hub-router")
            .expect("override present");
        assert!(hub.enabled);
        assert_eq!(hub.allow_unsigned, Some(true));
        assert_eq!(hub.blocked_permissions, vec!["fs.write.data"]);
        assert_eq!(hub.max_memory_mb, Some(32));
        assert_eq!(hub.loopback_only, Some(false));
        assert_eq!(
            hub.environment.get("API_URL").map(String::as_str),
            Some("http://127.0.0.1:8080")
        );
    }
}
