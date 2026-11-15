use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, RwLock as SyncRwLock, Weak};

use wasmtime::component::{Access, HasSelf, Resource, ResourceTable};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    RequestOptions, WasiBody, WasiHttpCtx, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
};

use crate::command::{CommandDispatcher, CommandSender, ProxyCommandSource};
use crate::config::Config;
use crate::plugin::PluginManager;
use crate::plugin::api::{EventPriority, EventType, PluginContext};
use crate::plugin::loader::wasm::wasm_host::WasmPlugin;
use crate::plugin::loader::wasm::wasm_host::concurrent_store::StoreMode;
use crate::session::{PlayerAction, PlayerSession, SessionManager};

use super::wit;

#[derive(Clone)]
pub struct ProxyPluginContext {
    pub active_config: Arc<SyncRwLock<Arc<Config>>>,
    pub session_manager: Arc<SessionManager>,
    pub command_dispatcher: Arc<CommandDispatcher>,
    pub plugin_manager: Arc<PluginManager>,
}

impl ProxyPluginContext {
    #[must_use]
    pub fn current_config(&self) -> Arc<Config> {
        self.active_config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub struct ContextResource {
    pub context: Arc<PluginContext>,
}

pub struct PluginHttpHooks {
    pub allow_outbound: bool,
}

impl PluginHttpHooks {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            allow_outbound: false,
        }
    }
}

impl Default for PluginHttpHooks {
    fn default() -> Self {
        Self::new()
    }
}

impl WasiHttpHooks for PluginHttpHooks {
    fn send_request(
        &mut self,
        request: hyper::Request<WasiBody>,
        options: Option<RequestOptions>,
        fut: Box<dyn Future<Output = wasmtime_wasi_http::Result<()>> + Send>,
    ) -> Box<
        dyn Future<
                Output = wasmtime_wasi_http::Result<(
                    hyper::Response<WasiBody>,
                    Box<dyn Future<Output = wasmtime_wasi_http::Result<()>> + Send>,
                )>,
            > + Send,
    > {
        if !self.allow_outbound {
            return Box::new(async { Err(wasmtime_wasi_http::Error::HttpRequestDenied) });
        }

        wasmtime_wasi_http::default_hooks().send_request(request, options, fut)
    }
}

pub struct PluginHostState {
    pub wasi_ctx: WasiCtx,
    pub wasi_http_ctx: WasiHttpCtx,
    pub wasi_http_hooks: PluginHttpHooks,
    pub resource_table: ResourceTable,
    pub limits: wasmtime::StoreLimits,
    pub proxy: Option<ProxyPluginContext>,
    pub plugin_name: String,
    pub data_folder: PathBuf,
    pub permissions: Vec<String>,
    pub store_mode: StoreMode,
    pub plugin: Option<Weak<WasmPlugin>>,
}

impl Default for PluginHostState {
    fn default() -> Self {
        Self::new(String::new(), PathBuf::new(), None, StoreMode::Sequential)
    }
}

impl PluginHostState {
    #[must_use]
    pub fn new(
        plugin_name: String,
        data_folder: PathBuf,
        proxy: Option<ProxyPluginContext>,
        store_mode: StoreMode,
    ) -> Self {
        Self {
            wasi_ctx: WasiCtxBuilder::new()
                .inherit_stdout()
                .inherit_stderr()
                .build(),
            wasi_http_ctx: WasiHttpCtx::new(),
            wasi_http_hooks: PluginHttpHooks::new(),
            resource_table: ResourceTable::new(),
            limits: wasmtime::StoreLimitsBuilder::new().build(),
            proxy,
            plugin_name,
            data_folder,
            permissions: Vec::new(),
            store_mode,
            plugin: None,
        }
    }

    pub fn add_context(
        &mut self,
        context: Arc<PluginContext>,
    ) -> wasmtime::Result<Resource<wit::vine::plugin::context::Context>> {
        let res = self.resource_table.push(ContextResource { context })?;
        Ok(Resource::new_own(res.rep()))
    }

    pub fn get_context(
        &self,
        resource: &Resource<wit::vine::plugin::context::Context>,
    ) -> wasmtime::Result<&ContextResource> {
        Ok(self
            .resource_table
            .get(&Resource::new_borrow(resource.rep()))?)
    }
}

impl WasiView for PluginHostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.resource_table,
        }
    }
}

impl WasiHttpView for PluginHostState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.wasi_http_ctx,
            table: &mut self.resource_table,
            hooks: &mut self.wasi_http_hooks,
        }
    }
}

fn session_to_player_info(session: PlayerSession) -> wit::vine::plugin::types::PlayerInfo {
    wit::vine::plugin::types::PlayerInfo {
        uuid: session.uuid.to_string(),
        username: session.username,
        current_server: session.current_server,
        client_ip: session.client_ip,
        protocol_version: session.protocol_version,
        client_brand: session.client_brand,
        locale: session.locale,
        view_distance: session.view_distance,
    }
}

impl wit::vine::plugin::types::Host for PluginHostState {}
impl wit::vine::plugin::event::Host for PluginHostState {}
impl wit::vine::plugin::context::Host for PluginHostState {}
impl wit::vine::plugin::ipc::Host for PluginHostState {}

impl wit::vine::plugin::logging::Host for PluginHostState {
    async fn log(
        &mut self,
        level: wit::vine::plugin::logging::Level,
        message: String,
    ) -> wasmtime::Result<()> {
        match level {
            wit::vine::plugin::logging::Level::Trace => {
                tracing::trace!("[Plugin: {}] {}", self.plugin_name, message);
            }
            wit::vine::plugin::logging::Level::Debug => {
                tracing::debug!("[Plugin: {}] {}", self.plugin_name, message);
            }
            wit::vine::plugin::logging::Level::Info => {
                tracing::info!("[Plugin: {}] {}", self.plugin_name, message);
            }
            wit::vine::plugin::logging::Level::Warn => {
                tracing::warn!("[Plugin: {}] {}", self.plugin_name, message);
            }
            wit::vine::plugin::logging::Level::Error => {
                tracing::error!("[Plugin: {}] {}", self.plugin_name, message);
            }
        }
        Ok(())
    }
}

impl wit::vine::plugin::command::Host for PluginHostState {
    async fn reply(
        &mut self,
        sender: wit::vine::plugin::types::CommandSender,
        message: String,
    ) -> wasmtime::Result<()> {
        if let Some(proxy) = &self.proxy {
            match sender.sender_type {
                wit::vine::plugin::types::CommandSenderType::Console => {
                    tracing::info!("[Console] {}", message);
                }
                wit::vine::plugin::types::CommandSenderType::Player => {
                    if let Some(uuid_str) = sender.player_uuid
                        && let Ok(target_uuid) = uuid::Uuid::parse_str(&uuid_str)
                        && let Some(session) = proxy
                            .session_manager
                            .list_sessions()
                            .into_iter()
                            .find(|s| s.uuid == target_uuid)
                    {
                        let _ = session.action_tx.send(PlayerAction::Message(
                            pumpkin_util::text::TextComponent::text(message),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

impl wit::vine::plugin::proxy::Host for PluginHostState {
    async fn get_proxy_version(&mut self) -> wasmtime::Result<String> {
        Ok(env!("CARGO_PKG_VERSION").to_string())
    }

    async fn get_player_count(&mut self) -> wasmtime::Result<u32> {
        Ok(self
            .proxy
            .as_ref()
            .map_or(0, |p| p.session_manager.count() as u32))
    }

    async fn get_players(&mut self) -> wasmtime::Result<Vec<wit::vine::plugin::types::PlayerInfo>> {
        if let Some(proxy) = &self.proxy {
            let list = proxy
                .session_manager
                .list_sessions()
                .into_iter()
                .map(session_to_player_info)
                .collect();
            Ok(list)
        } else {
            Ok(Vec::new())
        }
    }

    async fn get_player_by_name(
        &mut self,
        username: String,
    ) -> wasmtime::Result<Option<wit::vine::plugin::types::PlayerInfo>> {
        if let Some(proxy) = &self.proxy {
            Ok(proxy
                .session_manager
                .get_session(&username)
                .map(session_to_player_info))
        } else {
            Ok(None)
        }
    }

    async fn get_player_by_uuid(
        &mut self,
        uuid: String,
    ) -> wasmtime::Result<Option<wit::vine::plugin::types::PlayerInfo>> {
        if let Some(proxy) = &self.proxy
            && let Ok(target_uuid) = uuid::Uuid::parse_str(&uuid)
        {
            let session = proxy
                .session_manager
                .list_sessions()
                .into_iter()
                .find(|s| s.uuid == target_uuid);
            return Ok(session.map(session_to_player_info));
        }
        Ok(None)
    }

    async fn get_servers(&mut self) -> wasmtime::Result<Vec<wit::vine::plugin::types::ServerInfo>> {
        if let Some(proxy) = &self.proxy {
            let cfg = proxy.current_config();
            let sessions = proxy.session_manager.list_sessions();
            let mut list = Vec::with_capacity(cfg.servers.len());
            for (name, srv) in &cfg.servers {
                let player_count = sessions
                    .iter()
                    .filter(|s| s.current_server == *name)
                    .count() as u32;
                list.push(wit::vine::plugin::types::ServerInfo {
                    name: name.clone(),
                    address: srv.address.clone(),
                    player_count,
                });
            }
            list.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(list)
        } else {
            Ok(Vec::new())
        }
    }

    async fn broadcast(&mut self, message: String) -> wasmtime::Result<()> {
        if let Some(proxy) = &self.proxy {
            for session in proxy.session_manager.list_sessions() {
                let _ = session.action_tx.send(PlayerAction::Message(
                    pumpkin_util::text::TextComponent::text(message.clone()),
                ));
            }
            tracing::info!("[Broadcast] {}", message);
        }
        Ok(())
    }

    async fn send_message(
        &mut self,
        player_uuid: String,
        message: String,
    ) -> wasmtime::Result<bool> {
        if let Some(proxy) = &self.proxy
            && let Ok(target_uuid) = uuid::Uuid::parse_str(&player_uuid)
            && let Some(session) = proxy
                .session_manager
                .list_sessions()
                .into_iter()
                .find(|s| s.uuid == target_uuid)
        {
            let _ = session.action_tx.send(PlayerAction::Message(
                pumpkin_util::text::TextComponent::text(message),
            ));
            return Ok(true);
        }
        Ok(false)
    }

    async fn disconnect_player(
        &mut self,
        player_uuid: String,
        reason: String,
    ) -> wasmtime::Result<bool> {
        if let Some(proxy) = &self.proxy
            && let Ok(target_uuid) = uuid::Uuid::parse_str(&player_uuid)
            && let Some(session) = proxy
                .session_manager
                .list_sessions()
                .into_iter()
                .find(|s| s.uuid == target_uuid)
        {
            let _ = session.action_tx.send(PlayerAction::Disconnect(reason));
            return Ok(true);
        }
        Ok(false)
    }

    async fn connect_player(
        &mut self,
        player_uuid: String,
        server_name: String,
    ) -> wasmtime::Result<bool> {
        if let Some(proxy) = &self.proxy
            && proxy.current_config().servers.contains_key(&server_name)
            && let Ok(target_uuid) = uuid::Uuid::parse_str(&player_uuid)
            && let Some(session) = proxy
                .session_manager
                .list_sessions()
                .into_iter()
                .find(|s| s.uuid == target_uuid)
        {
            let _ = session
                .action_tx
                .send(PlayerAction::Connect { server_name });
            return Ok(true);
        }
        Ok(false)
    }

    async fn transfer_player(
        &mut self,
        player_uuid: String,
        host: String,
        port: u16,
    ) -> wasmtime::Result<bool> {
        if host.trim().is_empty() || port == 0 {
            return Ok(false);
        }
        if let Some(proxy) = &self.proxy
            && let Ok(target_uuid) = uuid::Uuid::parse_str(&player_uuid)
            && let Some(session) = proxy
                .session_manager
                .list_sessions()
                .into_iter()
                .find(|s| s.uuid == target_uuid)
        {
            let _ = session
                .action_tx
                .send(PlayerAction::Transfer { host, port });
            return Ok(true);
        }
        Ok(false)
    }

    async fn send_plugin_message(
        &mut self,
        player_uuid: String,
        channel: String,
        data: Vec<u8>,
    ) -> wasmtime::Result<bool> {
        if let Some(proxy) = &self.proxy
            && let Ok(target_uuid) = uuid::Uuid::parse_str(&player_uuid)
            && let Some(session) = proxy
                .session_manager
                .list_sessions()
                .into_iter()
                .find(|s| s.uuid == target_uuid)
        {
            let _ = session
                .action_tx
                .send(PlayerAction::PluginMessage { channel, data });
            return Ok(true);
        }
        Ok(false)
    }
}

impl wit::vine::plugin::proxy::HostWithStore<PluginHostState> for HasSelf<PluginHostState> {
    async fn execute_command(
        mut host: Access<'_, PluginHostState, Self>,
        command: String,
    ) -> wasmtime::Result<bool> {
        let (plugin, proxy_opt, store_mode) = {
            let state = host.get();
            let plugin = state
                .plugin
                .as_ref()
                .and_then(Weak::upgrade)
                .ok_or_else(|| wasmtime::Error::msg("Plugin reference is no longer valid"))?;
            (plugin, state.proxy.clone(), state.store_mode)
        };

        let Some(proxy) = proxy_opt else {
            return Ok(false);
        };

        let run = move || {
            let source = ProxyCommandSource {
                sender: CommandSender::Console,
                config: proxy.current_config(),
                session_manager: proxy.session_manager.clone(),
            };
            proxy
                .command_dispatcher
                .execute_input(&command, &source)
                .is_ok()
        };

        match store_mode {
            StoreMode::Sequential => plugin.store.pump_blocking(&mut host, run).await,
            StoreMode::Concurrent => plugin.store.spawn_blocking_in_active_context(run).await,
        }
    }
}

impl wit::vine::plugin::scheduler::Host for PluginHostState {
    async fn schedule_delayed(&mut self, handler_id: u32, delay_ms: u64) -> wasmtime::Result<u32> {
        let plugin = self
            .plugin
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| wasmtime::Error::msg("Plugin reference is no longer valid"))?;
        let scheduler = Arc::clone(&plugin.task_scheduler);
        Ok(scheduler.schedule_delayed(plugin, handler_id, delay_ms))
    }

    async fn schedule_repeating(
        &mut self,
        handler_id: u32,
        delay_ms: u64,
        period_ms: u64,
    ) -> wasmtime::Result<u32> {
        let plugin = self
            .plugin
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| wasmtime::Error::msg("Plugin reference is no longer valid"))?;
        let scheduler = Arc::clone(&plugin.task_scheduler);
        Ok(scheduler.schedule_repeating(plugin, handler_id, delay_ms, period_ms))
    }

    async fn cancel_task(&mut self, task_id: u32) -> wasmtime::Result<bool> {
        let plugin = self
            .plugin
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| wasmtime::Error::msg("Plugin reference is no longer valid"))?;
        Ok(plugin.task_scheduler.cancel_task(task_id))
    }
}

impl wit::vine::plugin::ipc::HostWithStore<PluginHostState> for HasSelf<PluginHostState> {
    async fn send_ipc_message(
        mut host: Access<'_, PluginHostState, Self>,
        target_plugin: String,
        message: Vec<u8>,
    ) -> wasmtime::Result<Result<Vec<u8>, String>> {
        let (plugin, sender_name, proxy_opt, store_mode) = {
            let state = host.get();
            let plugin = state
                .plugin
                .as_ref()
                .and_then(Weak::upgrade)
                .ok_or_else(|| wasmtime::Error::msg("Plugin reference is no longer valid"))?;
            (
                plugin,
                state.plugin_name.clone(),
                state.proxy.clone(),
                state.store_mode,
            )
        };

        let Some(proxy) = proxy_opt else {
            return Ok(Err("Proxy context is not initialized".to_string()));
        };

        let fut = proxy
            .plugin_manager
            .send_ipc_message(&sender_name, &target_plugin, &message);
        match store_mode {
            StoreMode::Sequential => plugin.store.pump_reentry(&mut host, fut).await,
            StoreMode::Concurrent => plugin.store.in_active_context(fut).await,
        }
    }
}

impl wit::vine::plugin::context::HostContext for PluginHostState {
    async fn register_event(
        &mut self,
        context: Resource<wit::vine::plugin::context::Context>,
        handler_id: u32,
        event_type: wit::vine::plugin::event::EventType,
        priority: wit::vine::plugin::types::EventPriority,
    ) -> wasmtime::Result<()> {
        let plugin = self
            .plugin
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| wasmtime::Error::msg("Plugin reference is no longer valid"))?;

        let api_event_type = match event_type {
            wit::vine::plugin::event::EventType::ProxyPing => EventType::ProxyPing,
            wit::vine::plugin::event::EventType::PlayerPreLogin => EventType::PlayerPreLogin,
            wit::vine::plugin::event::EventType::PlayerLogin => EventType::PlayerLogin,
            wit::vine::plugin::event::EventType::PlayerJoin => EventType::PlayerJoin,
            wit::vine::plugin::event::EventType::ServerConnect => EventType::ServerConnect,
            wit::vine::plugin::event::EventType::ServerConnected => EventType::ServerConnected,
            wit::vine::plugin::event::EventType::PlayerDisconnect => EventType::PlayerDisconnect,
            wit::vine::plugin::event::EventType::PlayerChat => EventType::PlayerChat,
            wit::vine::plugin::event::EventType::PlayerCommand => EventType::PlayerCommand,
            wit::vine::plugin::event::EventType::PluginMessage => EventType::PluginMessage,
        };

        let api_priority = match priority {
            wit::vine::plugin::types::EventPriority::Lowest => EventPriority::Lowest,
            wit::vine::plugin::types::EventPriority::Low => EventPriority::Low,
            wit::vine::plugin::types::EventPriority::Normal => EventPriority::Normal,
            wit::vine::plugin::types::EventPriority::High => EventPriority::High,
            wit::vine::plugin::types::EventPriority::Highest => EventPriority::Highest,
            wit::vine::plugin::types::EventPriority::Monitor => EventPriority::Monitor,
        };

        if let Some(proxy) = &self.proxy {
            proxy
                .plugin_manager
                .register_event_handler(api_event_type, api_priority, handler_id, plugin)
                .await;
        }

        let ctx_res = self.get_context(&context)?;
        let mut events = ctx_res.context.registered_events.lock().await;
        events.push((handler_id, api_event_type, api_priority));

        Ok(())
    }

    async fn register_command(
        &mut self,
        context: Resource<wit::vine::plugin::context::Context>,
        command_id: u32,
        name: String,
        description: String,
        aliases: Vec<String>,
        permission: Option<String>,
    ) -> wasmtime::Result<()> {
        let plugin = self
            .plugin
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| wasmtime::Error::msg("Plugin reference is no longer valid"))?;

        if let Some(proxy) = &self.proxy {
            proxy
                .plugin_manager
                .register_command(command_id, name, description, aliases, permission, plugin)
                .await;
        }

        let ctx_res = self.get_context(&context)?;
        let mut commands = ctx_res.context.registered_commands.lock().await;
        commands.push(command_id);

        Ok(())
    }

    async fn get_data_folder(
        &mut self,
        _context: Resource<wit::vine::plugin::context::Context>,
    ) -> wasmtime::Result<String> {
        Ok("data".to_string())
    }

    async fn drop(
        &mut self,
        rep: Resource<wit::vine::plugin::context::Context>,
    ) -> wasmtime::Result<()> {
        let _ = self
            .resource_table
            .delete::<ContextResource>(Resource::new_own(rep.rep()));
        Ok(())
    }
}
