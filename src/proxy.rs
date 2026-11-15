use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::auth::{Authenticator, KeyStore};
use crate::backend::Router;
use crate::client::ClientHandler;
use crate::command::{CommandDispatcher, CommandSender, ProxyCommandSource};
use crate::config::Config;
use crate::network;
use crate::security::ConnectionRateLimiter;
use crate::session::SessionManager;
use crate::status::{StatusHandler, StatusIconManager};
use crate::telemetry;

pub struct ProxyServer {
    active_config: Arc<RwLock<Arc<Config>>>,
    keystore: Arc<KeyStore>,
    authenticator: Arc<Authenticator>,
    rate_limiter: Arc<ConnectionRateLimiter>,
    status_handler: Arc<StatusHandler>,
    router: Arc<Router>,
    online_players: Arc<AtomicUsize>,
    session_manager: Arc<SessionManager>,
    command_dispatcher: Arc<CommandDispatcher>,
    plugin_manager: Arc<crate::plugin::PluginManager>,
}

fn build_icon_manager(config: &Config) -> Arc<StatusIconManager> {
    let server_icons: HashMap<String, String> = config
        .servers
        .iter()
        .filter_map(|(name, s)| s.icon_path.as_ref().map(|p| (name.clone(), p.clone())))
        .collect();

    Arc::new(StatusIconManager::new(
        config.motd.icon_path.clone(),
        server_icons,
        config.routing.forced_hosts.clone(),
        config.routing.default_server.clone(),
    ))
}

impl ProxyServer {
    pub fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_inner(config, None)
    }

    pub fn with_config_path(
        config: Config,
        config_path: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_inner(config, Some(config_path))
    }

    fn new_inner(
        config: Config,
        config_path: Option<PathBuf>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let config = Arc::new(config);
        let active_config = Arc::new(RwLock::new(config.clone()));
        let keystore = Arc::new(KeyStore::new()?);
        let authenticator = Arc::new(Authenticator::new());

        let rate_limiter = Arc::new(ConnectionRateLimiter::new(
            config.security.rate_limit_enabled,
            config.security.max_connections_per_sec,
            config.security.connection_burst,
        ));

        let online_players = Arc::new(AtomicUsize::new(0));
        let session_manager = Arc::new(SessionManager::new());
        let command_dispatcher = Arc::new(CommandDispatcher::new());

        let icon_manager = build_icon_manager(&config);

        let status_handler = Arc::new(StatusHandler::new(
            config.motd.clone(),
            online_players.clone(),
            icon_manager,
            config.servers.clone(),
            config.routing.forced_hosts.clone(),
        ));

        let router = Arc::new(Router::new(config.routing.clone(), config.servers.clone()));

        let plugins_dir = PathBuf::from(&config.plugins.plugin_dir);
        let plugin_manager = crate::plugin::PluginManager::new(plugins_dir)?;
        plugin_manager.init_proxy_context(
            active_config.clone(),
            session_manager.clone(),
            command_dispatcher.clone(),
        );

        command_dispatcher
            .runtime()
            .set_plugin_manager(&plugin_manager);

        let reload_path = config_path.unwrap_or_else(|| PathBuf::from("vine.toml"));
        let reload_active_cfg = active_config.clone();
        let reload_router = router.clone();
        let reload_status = status_handler.clone();
        let reload_rate_limiter = rate_limiter.clone();
        command_dispatcher
            .runtime()
            .set_reload_handler(Arc::new(move || {
                let content = std::fs::read_to_string(&reload_path)
                    .map_err(|e| format!("could not read '{}': {e}", reload_path.display()))?;
                let parsed: Config = toml::from_str(&content)
                    .map_err(|e| format!("invalid TOML in '{}': {e}", reload_path.display()))?;
                let new_cfg = Arc::new(parsed);

                let new_icons = build_icon_manager(&new_cfg);
                reload_router.reload(new_cfg.routing.clone(), new_cfg.servers.clone());
                reload_status.reload(
                    new_cfg.motd.clone(),
                    new_icons,
                    new_cfg.servers.clone(),
                    new_cfg.routing.forced_hosts.clone(),
                );
                reload_rate_limiter.reload(
                    new_cfg.security.rate_limit_enabled,
                    new_cfg.security.max_connections_per_sec,
                    new_cfg.security.connection_burst,
                );
                if let Ok(mut guard) = reload_active_cfg.write() {
                    *guard = new_cfg.clone();
                }
                info!(
                    "Reloaded configuration from '{}' ({} backend server(s))",
                    reload_path.display(),
                    new_cfg.servers.len()
                );
                Ok(new_cfg)
            }));

        Ok(Self {
            active_config,
            keystore,
            authenticator,
            rate_limiter,
            status_handler,
            router,
            online_players,
            session_manager,
            command_dispatcher,
            plugin_manager,
        })
    }

    fn current_config(&self) -> Arc<Config> {
        self.active_config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Run the proxy listener loop
    pub async fn run(
        &self,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (internal_shutdown_tx, _) = broadcast::channel(1);
        self.run_with_shutdown(internal_shutdown_tx, shutdown_rx)
            .await
    }

    /// Run the proxy listener loop with an attached shutdown sender so `/end` and `/stop`
    /// can trigger graceful shutdown.
    pub async fn run_with_shutdown(
        &self,
        shutdown_tx: broadcast::Sender<()>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.command_dispatcher
            .runtime()
            .set_shutdown_sender(shutdown_tx.clone());
        let mut command_shutdown_rx = shutdown_tx.subscribe();

        let startup_config = self.current_config();
        let bind_addr = &startup_config.server.bind_address;
        let listener = network::create_tcp_listener(bind_addr, &startup_config.network)?;

        info!("=====================================================");
        info!("  Vine Minecraft Proxy is listening on {}", bind_addr);
        info!("  Online mode: {}", startup_config.server.online_mode);
        info!("  Multi-version support: 1.7.2 through 1.21.x / 26.x");
        info!("  Networking options:");
        info!("    - TCP_NODELAY: {}", startup_config.network.tcp_nodelay);
        info!(
            "    - TCP KeepAlive: {}s",
            startup_config.network.tcp_keepalive_secs
        );
        info!(
            "    - PROXY Protocol v1/v2: {}",
            startup_config.network.proxy_protocol
        );
        info!(
            "    - Handshake timeout: {}s",
            startup_config.network.client_handshake_timeout_secs
        );
        info!(
            "    - Login timeout: {}s",
            startup_config.network.client_login_timeout_secs
        );
        info!(
            "    - Backend connect timeout: {}s",
            startup_config.network.backend_connect_timeout_secs
        );
        info!(
            "    - Compression level: {} (threshold: {} bytes)",
            startup_config.server.compression_level, startup_config.server.compression_threshold
        );
        info!(
            "  Configured backend servers: {}",
            startup_config.servers.len()
        );
        for (name, server) in &startup_config.servers {
            let icon_info = server.icon_path.as_deref().unwrap_or("none");
            info!(
                "    - [{}] {} (Forwarding: {:?}, Icon: {})",
                name, server.address, server.forwarding, icon_info
            );
        }
        info!(
            "  Default route: '{}'",
            startup_config.routing.default_server
        );
        if startup_config.telemetry.enabled {
            info!(
                "  Telemetry: enabled (endpoint: {}, interval: {}s)",
                startup_config.telemetry.endpoint, startup_config.telemetry.interval_secs
            );
        } else {
            info!("  Telemetry: disabled");
        }
        if startup_config.bedrock.enabled {
            info!(
                "  Bedrock Edition (NetherNet): enabled (TCP/UDP {}, LAN discovery: {}, v{} / proto {})",
                startup_config.bedrock.bind_address,
                startup_config.bedrock.lan_discovery,
                startup_config.bedrock.version_name,
                startup_config.bedrock.protocol_version
            );
        } else {
            info!("  Bedrock Edition (NetherNet): disabled");
        }
        info!("=====================================================");

        if startup_config.bedrock.enabled {
            let bedrock_cfg = startup_config.bedrock.clone();
            let online_players = self.online_players.clone();
            let bedrock_shutdown = shutdown_rx.resubscribe();

            tokio::spawn(async move {
                if let Err(e) = crate::bedrock::run_bedrock_listener(
                    bedrock_cfg,
                    online_players,
                    bedrock_shutdown,
                )
                .await
                {
                    error!("Bedrock NetherNet listener terminated with error: {}", e);
                }
            });
        }

        let console_dispatcher = self.command_dispatcher.clone();
        let console_active_config = self.active_config.clone();
        let console_session_manager = self.session_manager.clone();
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel();

        std::thread::Builder::new()
            .name("console-stdin".to_string())
            .spawn(move || {
                let stdin = std::io::stdin();
                let mut line = String::new();
                while let Ok(n) = stdin.read_line(&mut line) {
                    if n == 0 {
                        break;
                    }
                    if stdin_tx.send(line.trim().to_string()).is_err() {
                        break;
                    }
                    line.clear();
                }
            })
            .ok();

        let mut stdin_shutdown = shutdown_rx.resubscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stdin_shutdown.recv() => break,
                    cmd_opt = stdin_rx.recv() => {
                        match cmd_opt {
                            Some(trimmed) => {
                                if !trimmed.is_empty() {
                                    let cfg = console_active_config
                                        .read()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .clone();
                                    let source = ProxyCommandSource {
                                        sender: CommandSender::Console,
                                        config: cfg,
                                        session_manager: console_session_manager.clone(),
                                    };
                                    console_dispatcher.handle_command(&source, &trimmed);
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        if startup_config.plugins.enabled {
            info!(
                "Loading plugins from '{}'...",
                startup_config.plugins.plugin_dir
            );
            if let Err(e) = self.plugin_manager.load_plugins().await {
                error!("Failed to load plugins: {}", e);
            }
        }

        let telemetry_handle = telemetry::start_telemetry(
            self.active_config.clone(),
            self.online_players.clone(),
            self.session_manager.clone(),
            self.plugin_manager.clone(),
            shutdown_rx.resubscribe(),
        );

        loop {
            tokio::select! {
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, peer_addr)) => {
                            let config = self.current_config();
                            let keystore = self.keystore.clone();
                            let authenticator = self.authenticator.clone();
                            let status_handler = self.status_handler.clone();
                            let router = self.router.clone();
                            let online_players = self.online_players.clone();
                            let rate_limiter = self.rate_limiter.clone();
                            let session_manager = self.session_manager.clone();
                            let command_dispatcher = self.command_dispatcher.clone();
                            let plugin_manager = self.plugin_manager.clone();

                            tokio::spawn(async move {
                                if let Err(e) = network::configure_client_socket(&stream, &config.network) {
                                    debug!("[{}] Failed to configure client socket: {}", peer_addr, e);
                                }

                                let mut stream = stream;
                                let mut effective_addr = peer_addr;
                                let mut leftover_bytes = Vec::new();

                                if config.network.proxy_protocol {
                                    match network::parse_proxy_protocol(&mut stream, peer_addr).await {
                                        Ok(res) => {
                                             effective_addr = res.client_addr;
                                             leftover_bytes = res.leftover_bytes;
                                             debug!("[{}] PROXY protocol resolved client IP: {}", peer_addr, effective_addr);
                                        }
                                        Err(err) => {
                                            warn!("[{}] Dropped connection: PROXY protocol error: {}", peer_addr, err);
                                            return;
                                        }
                                    }
                                }

                                if !rate_limiter.check_connection(&effective_addr.ip()) {
                                    warn!("[{}] Dropped connection: Rate limit exceeded", effective_addr);
                                    return;
                                }

                                let handler = ClientHandler::new(
                                    effective_addr,
                                    config,
                                    keystore,
                                    authenticator,
                                    status_handler,
                                    router,
                                    online_players,
                                    session_manager,
                                    command_dispatcher,
                                    plugin_manager,
                                );

                                if let Err(err) = handler.handle(stream, leftover_bytes).await {
                                    debug!("[{}] Connection closed: {}", effective_addr, err);
                                }
                            });
                        }
                        Err(e) => {
                            error!("TCP listener accept error: {}", e);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Shutting down Vine proxy listener...");
                    break;
                }
                _ = command_shutdown_rx.recv() => {
                    let _ = shutdown_tx.send(());
                    info!("Shutting down Vine proxy listener...");
                    break;
                }
            }
        }

        info!("Vine proxy has stopped accepting connections.");

        if startup_config.plugins.enabled {
            info!("Unloading plugins...");
            self.plugin_manager.unload_plugins().await;
        }

        if let Some(handle) = telemetry_handle {
            let timeout_secs = startup_config.server.shutdown_timeout_secs.max(1);
            let _ = tokio::time::timeout(Duration::from_secs(timeout_secs), handle).await;
        }

        Ok(())
    }
}
