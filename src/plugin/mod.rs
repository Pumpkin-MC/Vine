pub mod api;
pub mod loader;
pub mod permissions;

pub use api::*;
pub use loader::wasm::WasmPlugin;
use loader::wasm::WasmPluginLoader;
use loader::wasm::wasm_host::state::ProxyPluginContext;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock as SyncRwLock};
use tokio::sync::{OnceCell, RwLock};
use tracing::{error, info, warn};

use crate::command::{CommandSender, ProxyCommandSource};
use pumpkin_command::{ArgumentBuilder, CommandSource};

#[derive(Clone)]
pub struct RegisteredEventHandler {
    pub handler_id: u32,
    pub priority: EventPriority,
    pub plugin: Arc<WasmPlugin>,
}

/// Sanitizes a plugin name into a safe single directory component under `plugins/data/`.
#[must_use]
pub fn sanitize_plugin_dir_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "unnamed_plugin".to_string()
    } else {
        trimmed.to_string()
    }
}

pub type DependencySortResult<T> = (Vec<(T, PluginMetadata)>, Vec<(String, String)>);

/// Orders plugins topologically so every plugin loads after all of its `dependencies`.
///
/// Plugins with missing or cyclic dependencies are excluded and returned in the second tuple element
/// alongside a human-readable reason.
pub fn sort_plugins_by_dependencies<T>(
    candidates: Vec<(T, PluginMetadata)>,
) -> DependencySortResult<T> {
    let mut by_name: BTreeMap<String, (T, PluginMetadata)> = BTreeMap::new();
    let mut skipped: Vec<(String, String)> = Vec::new();

    for (item, meta) in candidates {
        if meta.name.trim().is_empty() {
            skipped.push((
                "<empty>".to_string(),
                "plugin metadata name cannot be empty".to_string(),
            ));
            continue;
        }
        if by_name.contains_key(&meta.name) {
            skipped.push((
                meta.name.clone(),
                format!("duplicate plugin name '{}'", meta.name),
            ));
            continue;
        }
        by_name.insert(meta.name.clone(), (item, meta));
    }

    // Iteratively prune any plugin whose dependency is missing.
    loop {
        let missing: Vec<(String, String)> = by_name
            .iter()
            .filter_map(|(name, (_, meta))| {
                meta.dependencies
                    .iter()
                    .find(|dep| !by_name.contains_key(*dep))
                    .map(|missing_dep| {
                        (
                            name.clone(),
                            format!("missing required dependency '{missing_dep}'"),
                        )
                    })
            })
            .collect();

        if missing.is_empty() {
            break;
        }
        for (name, reason) in missing {
            by_name.remove(&name);
            skipped.push((name, reason));
        }
    }

    let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, (_, meta)) in &by_name {
        let unique_deps: HashSet<&String> = meta.dependencies.iter().collect();
        in_degree.insert(name.clone(), unique_deps.len());
        for dep in unique_deps {
            dependents
                .entry(dep.clone())
                .or_default()
                .push(name.clone());
        }
    }
    for list in dependents.values_mut() {
        list.sort();
    }

    let mut ready: VecDeque<String> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(name, _)| name.clone())
        .collect();

    let mut ordered = Vec::with_capacity(by_name.len());
    while let Some(name) = ready.pop_front() {
        if let Some(entry) = by_name.remove(&name) {
            ordered.push(entry);
        }
        if let Some(children) = dependents.get(&name) {
            let mut newly_ready = Vec::new();
            for child in children {
                if let Some(deg) = in_degree.get_mut(child) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        newly_ready.push(child.clone());
                    }
                }
            }
            newly_ready.sort();
            for n in newly_ready {
                ready.push_back(n);
            }
        }
    }

    for name in by_name.into_keys() {
        skipped.push((
            name,
            "circular plugin dependency cycle detected".to_string(),
        ));
    }

    (ordered, skipped)
}

pub struct PluginManager {
    plugins_dir: PathBuf,
    loader: WasmPluginLoader,
    loaded_plugins: RwLock<Vec<Arc<WasmPlugin>>>,
    loaded_metadata: SyncRwLock<Vec<(PluginMetadata, String)>>,
    event_handlers: RwLock<HashMap<EventType, Vec<RegisteredEventHandler>>>,
    proxy_ctx: OnceCell<ProxyPluginContext>,
}

impl PluginManager {
    pub fn new(
        plugins_dir: PathBuf,
    ) -> Result<Arc<Self>, loader::wasm::wasm_host::PluginInitError> {
        let loader = WasmPluginLoader::new(&plugins_dir)?;
        Ok(Arc::new(Self {
            plugins_dir,
            loader,
            loaded_plugins: RwLock::new(Vec::new()),
            loaded_metadata: SyncRwLock::new(Vec::new()),
            event_handlers: RwLock::new(HashMap::new()),
            proxy_ctx: OnceCell::new(),
        }))
    }

    pub fn init_proxy_context(
        self: &Arc<Self>,
        active_config: Arc<SyncRwLock<Arc<crate::config::Config>>>,
        session_manager: Arc<crate::session::SessionManager>,
        command_dispatcher: Arc<crate::command::CommandDispatcher>,
    ) {
        let ctx = ProxyPluginContext {
            active_config,
            session_manager,
            command_dispatcher,
            plugin_manager: self.clone(),
        };
        let _ = self.proxy_ctx.set(ctx);
    }

    pub async fn register_event_handler(
        &self,
        event_type: EventType,
        priority: EventPriority,
        handler_id: u32,
        plugin: Arc<WasmPlugin>,
    ) {
        let mut handlers = self.event_handlers.write().await;
        let list = handlers.entry(event_type).or_default();
        list.push(RegisteredEventHandler {
            handler_id,
            priority,
            plugin,
        });
        list.sort_by_key(|h| h.priority);
    }

    pub async fn register_command(
        &self,
        command_id: u32,
        name: String,
        description: String,
        aliases: Vec<String>,
        permission: Option<String>,
        plugin: Arc<WasmPlugin>,
    ) {
        if let Some(proxy) = self.proxy_ctx.get() {
            let names = std::iter::once(name.clone())
                .chain(aliases)
                .collect::<Vec<_>>();
            for cmd_name in names {
                let plugin_cmd = plugin.clone();
                let perm_check = permission.clone();
                let cmd_builder =
                    pumpkin_command::argument_builder::command(cmd_name, description.clone())
                        .requires(move |src: &ProxyCommandSource| {
                            if let Some(ref p) = perm_check {
                                src.has_permission(p)
                            } else {
                                true
                            }
                        })
                        .executes(PluginCommandExecutor {
                            command_id,
                            plugin: plugin_cmd.clone(),
                        })
                        .then(
                            pumpkin_command::argument_builder::argument(
                                "args",
                                pumpkin_command::argument_types::core::string::StringArgumentType::GreedyPhrase,
                            )
                            .suggests(PluginCommandSuggestionProvider {
                                command_id,
                                plugin: plugin_cmd.clone(),
                            })
                            .executes(PluginCommandExecutor {
                                command_id,
                                plugin: plugin_cmd,
                            }),
                        );
                proxy.command_dispatcher.register(cmd_builder);
            }
            info!(
                "Registered command '/{}' from plugin '{}'",
                name, plugin.metadata.name
            );
        }
    }

    pub async fn load_plugins(&self) -> Result<usize, Box<dyn std::error::Error>> {
        if !self.plugins_dir.exists() {
            let _ = fs::create_dir_all(&self.plugins_dir);
            info!("Created plugins directory '{}'", self.plugins_dir.display());
            return Ok(0);
        }

        let start_time = std::time::Instant::now();
        let read_dir = match fs::read_dir(&self.plugins_dir) {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to read plugins directory: {}", e);
                return Ok(0);
            }
        };

        let proxy = self.proxy_ctx.get().cloned();
        let plugin_cfg = proxy
            .as_ref()
            .map(|p| p.current_config().plugins.clone())
            .unwrap_or_default();

        let mut paths: Vec<PathBuf> = read_dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && self.loader.can_load(p))
            .collect();
        paths.sort();

        let mut initialized: Vec<(Arc<WasmPlugin>, PluginMetadata)> = Vec::new();
        for path in paths {
            info!("Loading plugin from '{}'...", path.display());
            match self.loader.load(&path, proxy.clone()).await {
                Ok((plugin, metadata)) => {
                    if let Some(ov) = plugin_cfg.overrides.get(&metadata.name)
                        && !ov.enabled
                    {
                        info!(
                            "Skipping plugin '{}' (disabled by plugins.overrides.{})",
                            metadata.name, metadata.name
                        );
                        continue;
                    }
                    initialized.push((plugin, metadata));
                }
                Err(e) => {
                    error!("Failed to load plugin '{}': {}", path.display(), e);
                }
            }
        }

        let (ordered, skipped) = sort_plugins_by_dependencies(initialized);
        for (name, reason) in skipped {
            error!("Skipping plugin '{name}': {reason}");
        }

        let mut count = 0;
        let mut loaded_names: HashSet<String> = HashSet::new();
        for (plugin, metadata) in ordered {
            if let Some(failed_dep) = metadata
                .dependencies
                .iter()
                .find(|dep| !loaded_names.contains(*dep))
            {
                error!(
                    "Skipping plugin '{}' because dependency '{}' failed to initialize",
                    metadata.name, failed_dep
                );
                continue;
            }

            let data_folder = self
                .plugins_dir
                .join("data")
                .join(sanitize_plugin_dir_name(&metadata.name));
            let context = Arc::new(PluginContext::new(metadata.clone(), data_folder));

            match plugin.on_load(context).await {
                Ok(()) => {
                    let language = plugin
                        .binding_language
                        .clone()
                        .unwrap_or_else(|| "wasm".to_string());
                    info!(
                        "Loaded plugin '{}' v{} [{}] by {}",
                        metadata.name,
                        metadata.version,
                        language,
                        if metadata.authors.is_empty() {
                            "unknown".to_string()
                        } else {
                            metadata.authors.join(", ")
                        }
                    );
                    loaded_names.insert(metadata.name.clone());
                    if let Ok(mut meta_guard) = self.loaded_metadata.write() {
                        meta_guard.push((metadata, language));
                    }
                    self.loaded_plugins.write().await.push(plugin);
                    count += 1;
                }
                Err(e) => {
                    error!("Plugin '{}' failed in on_load: {}", metadata.name, e);
                }
            }
        }

        info!("Loaded {} plugin(s) in {:?}", count, start_time.elapsed());
        Ok(count)
    }

    pub async fn unload_plugins(&self) {
        let mut plugins = {
            let mut lock = self.loaded_plugins.write().await;
            std::mem::take(&mut *lock)
        };
        if let Ok(mut meta_guard) = self.loaded_metadata.write() {
            meta_guard.clear();
        }
        self.event_handlers.write().await.clear();

        if let Some(proxy) = self.proxy_ctx.get() {
            proxy.command_dispatcher.reset_to_builtins();
        }

        // Unload in reverse topological order so dependents shut down before dependencies.
        plugins.reverse();
        for plugin in plugins {
            let data_folder = self
                .plugins_dir
                .join("data")
                .join(sanitize_plugin_dir_name(&plugin.metadata.name));
            let context = Arc::new(PluginContext::new(plugin.metadata.clone(), data_folder));
            if let Err(e) = plugin.on_unload(context).await {
                error!("Error unloading plugin '{}': {}", plugin.metadata.name, e);
            } else {
                info!("Unloaded plugin '{}'", plugin.metadata.name);
            }
        }
    }

    /// Unloads all currently active plugins and reloads `.wasm` components from disk.
    pub async fn reload_plugins(&self) -> Result<usize, Box<dyn std::error::Error>> {
        self.unload_plugins().await;
        self.load_plugins().await
    }

    /// Routes a binary IPC message from `sender` to `target` plugin.
    pub async fn send_ipc_message(
        &self,
        sender: &str,
        target: &str,
        message: &[u8],
    ) -> Result<Vec<u8>, String> {
        let target_plugin = {
            let plugins = self.loaded_plugins.read().await;
            plugins.iter().find(|p| p.metadata.name == target).cloned()
        };
        let Some(plugin) = target_plugin else {
            return Err(format!("target plugin '{target}' is not loaded"));
        };

        plugin
            .handle_ipc_message(sender.to_string(), message.to_vec())
            .await
            .map_err(|e| format!("IPC call into '{target}' trapped: {e}"))?
    }

    /// Synchronously returns metadata for all currently loaded plugins.
    #[must_use]
    pub fn list_plugins(&self) -> Vec<PluginMetadata> {
        self.loaded_metadata
            .read()
            .map(|guard| guard.iter().map(|(m, _)| m.clone()).collect())
            .unwrap_or_default()
    }

    /// Synchronously returns `(PluginMetadata, language)` pairs for all loaded plugins.
    #[must_use]
    pub fn list_plugins_with_language(&self) -> Vec<(PluginMetadata, String)> {
        self.loaded_metadata
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Returns telemetry metadata (including detected guest binding language) for loaded plugins.
    pub async fn active_plugins_telemetry(&self) -> Vec<crate::telemetry::PluginInfo> {
        let plugins = self.loaded_plugins.read().await;
        plugins
            .iter()
            .map(|p| crate::telemetry::PluginInfo {
                name: p.metadata.name.clone(),
                version: p.metadata.version.clone(),
                language: Some(
                    p.binding_language
                        .clone()
                        .unwrap_or_else(|| "wasm".to_string()),
                ),
            })
            .collect()
    }

    // --- Event dispatchers ---

    pub async fn fire_proxy_ping(&self, event: &mut ProxyPingEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::ProxyPing).cloned().unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt = loader::wasm::wasm_host::wit::vine::plugin::event::Event::ProxyPing(
                loader::wasm::wasm_host::wit::vine::plugin::event::ProxyPingEvent {
                    client_ip: event.client_ip.clone(),
                    protocol_version: event.protocol_version,
                    virtual_host: event.virtual_host.clone(),
                    motd: event.motd.clone(),
                    max_players: event.max_players,
                    online_players: event.online_players,
                    version_name: event.version_name.clone(),
                },
            );
            match handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await
            {
                Ok(loader::wasm::wasm_host::wit::vine::plugin::event::Event::ProxyPing(res)) => {
                    event.motd = res.motd;
                    event.max_players = res.max_players;
                    event.online_players = res.online_players;
                    event.version_name = res.version_name;
                }
                Ok(_) => {}
                Err(e) => {
                    error!(
                        "Error in plugin '{}' handling ProxyPing: {}",
                        handler.plugin.metadata.name, e
                    );
                }
            }
        }
    }

    pub async fn fire_player_pre_login(&self, event: &mut PlayerPreLoginEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::PlayerPreLogin)
                .cloned()
                .unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt = loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerPreLogin(
                loader::wasm::wasm_host::wit::vine::plugin::event::PlayerPreLoginEvent {
                    client_ip: event.client_ip.clone(),
                    username: event.username.clone(),
                    protocol_version: event.protocol_version,
                    virtual_host: event.virtual_host.clone(),
                    cancelled: event.cancelled,
                    cancel_reason: event.cancel_reason.clone(),
                },
            );
            match handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await
            {
                Ok(loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerPreLogin(
                    res,
                )) => {
                    event.cancelled = res.cancelled;
                    event.cancel_reason = res.cancel_reason;
                    if event.cancelled {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    error!(
                        "Error in plugin '{}' handling PlayerPreLogin: {}",
                        handler.plugin.metadata.name, e
                    );
                }
            }
        }
    }

    pub async fn fire_player_login(&self, event: &mut PlayerLoginEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::PlayerLogin)
                .cloned()
                .unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt = loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerLogin(
                loader::wasm::wasm_host::wit::vine::plugin::event::PlayerLoginEvent {
                    uuid: event.uuid.clone(),
                    username: event.username.clone(),
                    client_ip: event.client_ip.clone(),
                    cancelled: event.cancelled,
                    cancel_reason: event.cancel_reason.clone(),
                },
            );
            match handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await
            {
                Ok(loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerLogin(res)) => {
                    event.cancelled = res.cancelled;
                    event.cancel_reason = res.cancel_reason;
                    if event.cancelled {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    error!(
                        "Error in plugin '{}' handling PlayerLogin: {}",
                        handler.plugin.metadata.name, e
                    );
                }
            }
        }
    }

    pub async fn fire_player_join(&self, event: &PlayerJoinEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::PlayerJoin).cloned().unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt = loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerJoin(
                loader::wasm::wasm_host::wit::vine::plugin::event::PlayerJoinEvent {
                    uuid: event.uuid.clone(),
                    username: event.username.clone(),
                    client_ip: event.client_ip.clone(),
                    initial_server: event.initial_server.clone(),
                },
            );
            let _ = handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await;
        }
    }

    pub async fn fire_server_connect(&self, event: &mut ServerConnectEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::ServerConnect)
                .cloned()
                .unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt = loader::wasm::wasm_host::wit::vine::plugin::event::Event::ServerConnect(
                loader::wasm::wasm_host::wit::vine::plugin::event::ServerConnectEvent {
                    uuid: event.uuid.clone(),
                    username: event.username.clone(),
                    current_server: event.current_server.clone(),
                    target_server: event.target_server.clone(),
                    cancelled: event.cancelled,
                    cancel_reason: event.cancel_reason.clone(),
                },
            );
            match handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await
            {
                Ok(loader::wasm::wasm_host::wit::vine::plugin::event::Event::ServerConnect(
                    res,
                )) => {
                    event.target_server = res.target_server;
                    event.cancelled = res.cancelled;
                    event.cancel_reason = res.cancel_reason;
                    if event.cancelled {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    error!(
                        "Error in plugin '{}' handling ServerConnect: {}",
                        handler.plugin.metadata.name, e
                    );
                }
            }
        }
    }

    pub async fn fire_server_connected(&self, event: &ServerConnectedEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::ServerConnected)
                .cloned()
                .unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt = loader::wasm::wasm_host::wit::vine::plugin::event::Event::ServerConnected(
                loader::wasm::wasm_host::wit::vine::plugin::event::ServerConnectedEvent {
                    uuid: event.uuid.clone(),
                    username: event.username.clone(),
                    server_name: event.server_name.clone(),
                },
            );
            let _ = handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await;
        }
    }

    pub async fn fire_player_disconnect(&self, event: &PlayerDisconnectEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::PlayerDisconnect)
                .cloned()
                .unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt =
                loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerDisconnect(
                    loader::wasm::wasm_host::wit::vine::plugin::event::PlayerDisconnectEvent {
                        uuid: event.uuid.clone(),
                        username: event.username.clone(),
                        last_server: event.last_server.clone(),
                    },
                );
            let _ = handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await;
        }
    }

    pub async fn fire_player_chat(&self, event: &mut PlayerChatEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::PlayerChat).cloned().unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt = loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerChat(
                loader::wasm::wasm_host::wit::vine::plugin::event::PlayerChatEvent {
                    uuid: event.uuid.clone(),
                    username: event.username.clone(),
                    message: event.message.clone(),
                    cancelled: event.cancelled,
                },
            );
            match handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await
            {
                Ok(loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerChat(res)) => {
                    event.message = res.message;
                    event.cancelled = res.cancelled;
                    if event.cancelled {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    error!(
                        "Error in plugin '{}' handling PlayerChat: {}",
                        handler.plugin.metadata.name, e
                    );
                }
            }
        }
    }

    pub async fn fire_player_command(&self, event: &mut PlayerCommandEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::PlayerCommand)
                .cloned()
                .unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt = loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerCommand(
                loader::wasm::wasm_host::wit::vine::plugin::event::PlayerCommandEvent {
                    uuid: event.uuid.clone(),
                    username: event.username.clone(),
                    command: event.command.clone(),
                    cancelled: event.cancelled,
                },
            );
            match handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await
            {
                Ok(loader::wasm::wasm_host::wit::vine::plugin::event::Event::PlayerCommand(
                    res,
                )) => {
                    event.command = res.command;
                    event.cancelled = res.cancelled;
                    if event.cancelled {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    error!(
                        "Error in plugin '{}' handling PlayerCommand: {}",
                        handler.plugin.metadata.name, e
                    );
                }
            }
        }
    }

    pub async fn fire_plugin_message(&self, event: &mut PluginMessageEvent) {
        let handlers = {
            let map = self.event_handlers.read().await;
            map.get(&EventType::PluginMessage)
                .cloned()
                .unwrap_or_default()
        };
        for handler in handlers {
            let wit_evt = loader::wasm::wasm_host::wit::vine::plugin::event::Event::PluginMessage(
                loader::wasm::wasm_host::wit::vine::plugin::event::PluginMessageEvent {
                    channel: event.channel.clone(),
                    data: event.data.clone(),
                    player_uuid: event.player_uuid.clone(),
                    server_name: event.server_name.clone(),
                    cancelled: event.cancelled,
                },
            );
            match handler
                .plugin
                .handle_event(handler.handler_id, wit_evt)
                .await
            {
                Ok(loader::wasm::wasm_host::wit::vine::plugin::event::Event::PluginMessage(
                    res,
                )) => {
                    event.data = res.data;
                    event.cancelled = res.cancelled;
                    if event.cancelled {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    error!(
                        "Error in plugin '{}' handling PluginMessage: {}",
                        handler.plugin.metadata.name, e
                    );
                }
            }
        }
    }
}

fn build_wit_command_sender(
    source: &ProxyCommandSource,
) -> loader::wasm::wasm_host::wit::vine::plugin::types::CommandSender {
    match &source.sender {
        CommandSender::Console => {
            loader::wasm::wasm_host::wit::vine::plugin::types::CommandSender {
                sender_type:
                    loader::wasm::wasm_host::wit::vine::plugin::types::CommandSenderType::Console,
                name: "CONSOLE".to_string(),
                player_uuid: None,
            }
        }
        CommandSender::Player { username, .. } => {
            let uuid = source
                .session_manager
                .get_session(username)
                .map(|s| s.uuid.to_string());
            loader::wasm::wasm_host::wit::vine::plugin::types::CommandSender {
                sender_type:
                    loader::wasm::wasm_host::wit::vine::plugin::types::CommandSenderType::Player,
                name: username.clone(),
                player_uuid: uuid,
            }
        }
    }
}

struct PluginCommandExecutor {
    command_id: u32,
    plugin: Arc<WasmPlugin>,
}

impl pumpkin_command::node::CommandExecutor<ProxyCommandSource> for PluginCommandExecutor {
    fn execute(
        &self,
        context: &pumpkin_command::context::command_context::CommandContext<ProxyCommandSource>,
    ) -> pumpkin_command::node::CommandExecutorResult {
        let args: Vec<String> = if let Ok(phrase) =
            pumpkin_command::argument_types::core::string::StringArgumentType::get(context, "args")
        {
            phrase.split_whitespace().map(str::to_string).collect()
        } else {
            Vec::new()
        };

        let sender = build_wit_command_sender(&context.source);
        let plugin = self.plugin.clone();
        let command_id = self.command_id;
        let handle = tokio::runtime::Handle::current();
        let result = tokio::task::block_in_place(|| {
            handle.block_on(async move { plugin.handle_command(command_id, sender, args).await })
        });

        match result {
            Ok(Ok(code)) => Ok(code),
            Ok(Err(err_msg)) => {
                context
                    .source
                    .send_message(crate::console::legacy_component(format!(
                        "§cError: {err_msg}"
                    )));
                Ok(0)
            }
            Err(e) => {
                context
                    .source
                    .send_message(crate::console::legacy_component(format!(
                        "§cPlugin command error: {e}"
                    )));
                Ok(0)
            }
        }
    }
}

struct PluginCommandSuggestionProvider {
    command_id: u32,
    plugin: Arc<WasmPlugin>,
}

impl pumpkin_command::suggestion::provider::SuggestionProvider<ProxyCommandSource>
    for PluginCommandSuggestionProvider
{
    fn suggest(
        &self,
        context: &pumpkin_command::context::command_context::CommandContext<ProxyCommandSource>,
        builder: pumpkin_command::suggestion::suggestions::SuggestionsBuilder,
    ) -> pumpkin_command::suggestion::provider::SuggestionProviderResult {
        let args: Vec<String> = if let Ok(phrase) =
            pumpkin_command::argument_types::core::string::StringArgumentType::get(context, "args")
        {
            phrase.split_whitespace().map(str::to_string).collect()
        } else {
            Vec::new()
        };

        let sender = build_wit_command_sender(&context.source);
        let plugin = self.plugin.clone();
        let command_id = self.command_id;
        let handle = tokio::runtime::Handle::current();
        let suggestions_res = tokio::task::block_in_place(|| {
            handle.block_on(async move {
                plugin
                    .handle_command_suggestion(command_id, sender, args)
                    .await
            })
        });

        let mut builder = builder;
        if let Ok(suggestions) = suggestions_res {
            for s in suggestions {
                builder = builder.suggest(s);
            }
        }
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandDispatcher;
    use crate::config::Config;
    use crate::session::SessionManager;

    fn make_meta(name: &str, deps: &[&str]) -> PluginMetadata {
        PluginMetadata {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            authors: vec!["Tester".to_string()],
            description: None,
            dependencies: deps.iter().map(|d| (*d).to_string()).collect(),
            permissions: Vec::new(),
        }
    }

    #[test]
    fn test_sanitize_plugin_dir_name_blocks_traversal() {
        assert_eq!(sanitize_plugin_dir_name("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitize_plugin_dir_name("my-cool_plugin"), "my-cool_plugin");
        assert_eq!(sanitize_plugin_dir_name("..."), "unnamed_plugin");
    }

    #[test]
    fn test_dependency_topological_sort_and_error_filtering() {
        let candidates = vec![
            (1, make_meta("chat-filter", &["auth-core", "perms"])),
            (2, make_meta("perms", &["auth-core"])),
            (3, make_meta("auth-core", &[])),
            (4, make_meta("broken", &["non-existent"])),
            (5, make_meta("cycle-a", &["cycle-b"])),
            (6, make_meta("cycle-b", &["cycle-a"])),
        ];

        let (ordered, skipped) = sort_plugins_by_dependencies(candidates);
        let ordered_names: Vec<&str> = ordered.iter().map(|(_, m)| m.name.as_str()).collect();
        assert_eq!(ordered_names, vec!["auth-core", "perms", "chat-filter"]);

        let skipped_names: Vec<&str> = skipped.iter().map(|(n, _)| n.as_str()).collect();
        assert!(skipped_names.contains(&"broken"));
        assert!(skipped_names.contains(&"cycle-a"));
        assert!(skipped_names.contains(&"cycle-b"));
    }

    #[tokio::test]
    async fn test_plugin_manager_empty_dir() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let active_config = Arc::new(SyncRwLock::new(Arc::new(Config::default())));
        let dispatcher = Arc::new(CommandDispatcher::new());
        let session_manager = Arc::new(SessionManager::new());
        let manager =
            PluginManager::new(temp_dir.path().to_path_buf()).expect("failed to create manager");
        manager.init_proxy_context(active_config, session_manager, dispatcher);

        let count = manager.load_plugins().await.expect("load failed");
        assert_eq!(count, 0);

        manager.unload_plugins().await;
    }

    #[tokio::test]
    async fn test_plugin_manager_event_dispatch() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let active_config = Arc::new(SyncRwLock::new(Arc::new(Config::default())));
        let dispatcher = Arc::new(CommandDispatcher::new());
        let session_manager = Arc::new(SessionManager::new());
        let manager =
            PluginManager::new(temp_dir.path().to_path_buf()).expect("failed to create manager");
        manager.init_proxy_context(active_config, session_manager, dispatcher);

        let mut ping = ProxyPingEvent {
            client_ip: "127.0.0.1".to_string(),
            protocol_version: 765,
            virtual_host: "localhost".to_string(),
            motd: "A Minecraft Proxy".to_string(),
            max_players: 100,
            online_players: 0,
            version_name: "Vine 1.21".to_string(),
        };
        manager.fire_proxy_ping(&mut ping).await;
        assert_eq!(ping.motd, "A Minecraft Proxy");
        assert_eq!(ping.max_players, 100);

        let mut pre_login = PlayerPreLoginEvent {
            client_ip: "127.0.0.1".to_string(),
            username: "Player1".to_string(),
            protocol_version: 765,
            virtual_host: "localhost".to_string(),
            cancelled: false,
            cancel_reason: None,
        };
        manager.fire_player_pre_login(&mut pre_login).await;
        assert!(!pre_login.cancelled);

        let mut login = PlayerLoginEvent {
            uuid: "00000000-0000-0000-0000-000000000001".to_string(),
            username: "Player1".to_string(),
            client_ip: "127.0.0.1".to_string(),
            cancelled: false,
            cancel_reason: None,
        };
        manager.fire_player_login(&mut login).await;
        assert!(!login.cancelled);

        let mut connect = ServerConnectEvent {
            uuid: "00000000-0000-0000-0000-000000000001".to_string(),
            username: "Player1".to_string(),
            current_server: None,
            target_server: "lobby".to_string(),
            cancelled: false,
            cancel_reason: None,
        };
        manager.fire_server_connect(&mut connect).await;
        assert_eq!(connect.target_server, "lobby");
        assert!(!connect.cancelled);

        let connected = ServerConnectedEvent {
            uuid: "00000000-0000-0000-0000-000000000001".to_string(),
            username: "Player1".to_string(),
            server_name: "lobby".to_string(),
        };
        manager.fire_server_connected(&connected).await;

        let join = PlayerJoinEvent {
            uuid: "00000000-0000-0000-0000-000000000001".to_string(),
            username: "Player1".to_string(),
            client_ip: "127.0.0.1".to_string(),
            initial_server: "lobby".to_string(),
        };
        manager.fire_player_join(&join).await;

        let mut chat = PlayerChatEvent {
            uuid: "00000000-0000-0000-0000-000000000001".to_string(),
            username: "Player1".to_string(),
            message: "Hello proxy!".to_string(),
            cancelled: false,
        };
        manager.fire_player_chat(&mut chat).await;
        assert_eq!(chat.message, "Hello proxy!");
        assert!(!chat.cancelled);

        let mut cmd = PlayerCommandEvent {
            uuid: "00000000-0000-0000-0000-000000000001".to_string(),
            username: "Player1".to_string(),
            command: "ping".to_string(),
            cancelled: false,
        };
        manager.fire_player_command(&mut cmd).await;
        assert!(!cmd.cancelled);

        let mut msg = PluginMessageEvent {
            channel: "vine:test".to_string(),
            data: vec![1, 2, 3],
            player_uuid: Some("00000000-0000-0000-0000-000000000001".to_string()),
            server_name: Some("lobby".to_string()),
            cancelled: false,
        };
        manager.fire_plugin_message(&mut msg).await;
        assert!(!msg.cancelled);

        let disc = PlayerDisconnectEvent {
            uuid: "00000000-0000-0000-0000-000000000001".to_string(),
            username: "Player1".to_string(),
            last_server: Some("lobby".to_string()),
        };
        manager.fire_player_disconnect(&disc).await;
    }

    #[test]
    fn test_event_priority_ordering() {
        assert!(EventPriority::Lowest < EventPriority::Low);
        assert!(EventPriority::Low < EventPriority::Normal);
        assert!(EventPriority::Normal < EventPriority::High);
        assert!(EventPriority::High < EventPriority::Highest);
        assert!(EventPriority::Highest < EventPriority::Monitor);
    }

    #[test]
    fn test_plugin_metadata_serde() {
        let meta = PluginMetadata {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            authors: vec!["Author1".to_string()],
            description: Some("A test plugin".to_string()),
            dependencies: vec!["other-plugin".to_string()],
            permissions: vec![permissions::FS_READ_DATA.to_string()],
        };

        let json = serde_json::to_string(&meta).expect("serialization failed");
        let decoded: PluginMetadata = serde_json::from_str(&json).expect("deserialization failed");
        assert_eq!(decoded.name, "test-plugin");
        assert_eq!(decoded.version, "1.0.0");
        assert_eq!(decoded.authors, vec!["Author1".to_string()]);
        assert_eq!(decoded.dependencies, vec!["other-plugin".to_string()]);
        assert_eq!(decoded.permissions, vec![permissions::FS_READ_DATA]);
    }
}
