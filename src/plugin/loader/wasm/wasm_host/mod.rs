pub mod concurrent_store;
pub mod inspect;
pub mod scheduler;
pub mod signature;
pub mod state;
pub mod wit;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Cache, CacheConfig, Engine, Store};
use wasmtime_wasi::{FsPerms, WasiCtxBuilder};

use crate::config::PluginConfig;
use crate::plugin::api::{Plugin, PluginContext, PluginFuture, PluginMetadata};
use crate::plugin::permissions::{self, socket_policy_for_permissions};
use concurrent_store::{LegacyStore, LegacySyncReentry, StoreMode, TokioSpawner};
use inspect::scan_component_names;
use scheduler::PluginTaskScheduler;
use signature::{sha256_hex, strip_signature_sections, verify_wasm_signature};
use state::{PluginHostState, ProxyPluginContext};

#[derive(Error, Debug)]
pub enum PluginInitError {
    #[error("Failed to read plugin file '{0}': {1}")]
    FileReadFailed(PathBuf, std::io::Error),
    #[error("Plugin '{0}' does not export a recognized vine:plugin/metadata interface")]
    UnknownApiVersion(PathBuf),
    #[error("Plugin '{0}' targets unsupported API version '{1}' (expected 0.1.x)")]
    UnsupportedApiVersion(PathBuf, String),
    #[error("Plugin '{0}' failed signature check: {1}")]
    SignatureVerificationFailed(PathBuf, String),
    #[error("Failed to compile WebAssembly component: {0}")]
    ComponentCompilationFailed(wasmtime::Error),
    #[error("Failed to serialize compiled component cache: {0}")]
    ComponentCacheSerializeFailed(wasmtime::Error),
    #[error("Failed to setup linker: {0}")]
    LinkerSetupFailed(wasmtime::Error),
    #[error("Failed to instantiate plugin: {0}")]
    InstantiationFailed(wasmtime::Error),
    #[error("Failed to initialize plugin: {0}")]
    InitPluginFailed(wasmtime::Error),
    #[error("Failed to retrieve metadata: {0}")]
    GetMetadataFailed(wasmtime::Error),
    #[error("Engine creation failed: {0}")]
    EngineCreationFailed(wasmtime::Error),
}

pub fn setup_linker(engine: &Engine, mode: StoreMode) -> wasmtime::Result<Linker<PluginHostState>> {
    let mut linker = Linker::<PluginHostState>::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
    if mode == StoreMode::Concurrent {
        wasmtime_wasi::p3::add_to_linker(&mut linker)?;
        wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    }
    wit::Plugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut PluginHostState| state)?;
    Ok(linker)
}

pub struct WasmPlugin {
    pub instance: wit::Plugin,
    pub store: LegacyStore,
    pub metadata: PluginMetadata,
    pub binding_language: Option<String>,
    pub task_scheduler: Arc<PluginTaskScheduler>,
}

impl Drop for WasmPlugin {
    fn drop(&mut self) {
        self.store.discard();
    }
}

impl WasmPlugin {
    pub async fn on_load(&self, context: Arc<PluginContext>) -> Result<(), String> {
        let _ = fs::create_dir_all(&context.data_folder);

        let plugin_cfg: PluginConfig = self
            .store
            .call(|state| {
                state
                    .proxy
                    .as_ref()
                    .map(|p| p.current_config().plugins.clone())
                    .unwrap_or_default()
            })
            .await
            .map_err(|e| e.to_string())?;

        let plugin_override = plugin_cfg.overrides.get(&self.metadata.name);

        let is_blocked = |perm: &str| {
            plugin_cfg.blocked_permissions.iter().any(|b| b == perm)
                || plugin_override.is_some_and(|o| o.blocked_permissions.iter().any(|b| b == perm))
        };

        let filtered_permissions: Vec<String> = self
            .metadata
            .permissions
            .iter()
            .filter(|p| !is_blocked(p))
            .cloned()
            .collect();

        let has_permission = |p: &str| filtered_permissions.iter().any(|perm| perm == p);

        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdout();
        builder.inherit_stderr();

        if has_permission(permissions::NETWORK_DNS) {
            builder.allow_ip_name_lookup(true);
        }

        let loopback_only = plugin_override
            .and_then(|o| o.loopback_only)
            .unwrap_or(plugin_cfg.loopback_only)
            || has_permission(permissions::NETWORK_LOOPBACK);

        let socket_policy = socket_policy_for_permissions(has_permission, loopback_only);
        builder.allow_tcp(socket_policy.tcp_connect || socket_policy.tcp_bind);
        builder.allow_udp(
            socket_policy.udp_send || socket_policy.udp_receive || socket_policy.udp_bind,
        );
        builder.socket_addr_check(move |addr, reason| {
            Box::pin(async move { socket_policy.allows(addr, reason) })
        });

        let allow_http_outbound = has_permission(permissions::HTTP_OUTBOUND);

        if plugin_cfg.inherit_env || has_permission(permissions::SYS_ENV) {
            builder.inherit_env();
        } else {
            for (key, value) in std::env::vars() {
                let perm = format!("{}{}", permissions::SYS_ENV_PREFIX, key);
                if has_permission(&perm) {
                    builder.env(key, value);
                }
            }
        }

        if let Some(ov) = plugin_override {
            for (key, value) in &ov.environment {
                builder.env(key, value);
            }
        }

        // Preopen the plugin's sandboxed `data` folder. By default, grant read/write access to its
        // own isolated `plugins/data/<name>` folder unless explicitly blocked via `fs.write.data` /
        // `fs.read.data` in `blocked_permissions` (or if permissions are explicitly listed without fs).
        let requests_explicit_fs = self
            .metadata
            .permissions
            .iter()
            .any(|p| p == permissions::FS_READ_DATA || p == permissions::FS_WRITE_DATA);
        let may_write_data = if requests_explicit_fs {
            has_permission(permissions::FS_WRITE_DATA)
        } else {
            !is_blocked(permissions::FS_WRITE_DATA) && !is_blocked(permissions::FS_READ_DATA)
        };
        let may_read_data = may_write_data
            || if requests_explicit_fs {
                has_permission(permissions::FS_READ_DATA)
            } else {
                !is_blocked(permissions::FS_READ_DATA)
            };

        if may_read_data {
            let perms = if may_write_data {
                FsPerms::ReadWrite
            } else {
                FsPerms::ReadOnly
            };
            builder
                .preopened_dir(&context.data_folder, "data", perms)
                .map_err(|e| format!("failed to preopen plugin data directory: {e}"))?;
        }

        let max_memory_mb = plugin_override
            .and_then(|o| o.max_memory_mb)
            .or(plugin_cfg.max_memory_mb);

        let wasi_ctx = builder.build();
        let data_folder = context.data_folder.clone();
        let func = self.instance.func_on_load();
        let context_clone = context.clone();

        self.store
            .call_guest(move |mut guest| {
                Box::pin(async move {
                    guest.with(|mut store| {
                        if let Some(mb) = max_memory_mb {
                            let limit_bytes = usize::try_from(mb)
                                .unwrap_or(usize::MAX)
                                .saturating_mul(1024 * 1024);
                            store.data_mut().limits = wasmtime::StoreLimitsBuilder::new()
                                .memory_size(limit_bytes)
                                .build();
                        }
                        store.data_mut().wasi_ctx = wasi_ctx;
                        store.data_mut().wasi_http_hooks.allow_outbound = allow_http_outbound;
                        store.data_mut().permissions = filtered_permissions;
                        store.data_mut().data_folder = data_folder;
                    });
                    let (context_res, context_rep) = guest.with(|mut store| {
                        let res = store.data_mut().add_context(context_clone)?;
                        let rep = res.rep();
                        Ok::<_, wasmtime::Error>((res, rep))
                    })?;
                    let result = guest.call(func, (context_res,)).await.map(|(r,)| r);
                    guest.with(|mut store| {
                        let _ = store
                            .data_mut()
                            .resource_table
                            .delete::<state::ContextResource>(
                                wasmtime::component::Resource::new_own(context_rep),
                            );
                    });
                    result
                })
            })
            .await
            .map_err(|e| e.to_string())?
    }

    pub async fn on_unload(self: &Arc<Self>, context: Arc<PluginContext>) -> Result<(), String> {
        self.task_scheduler.disable_plugin(self);
        let func = self.instance.func_on_unload();
        let context_clone = context.clone();
        self.store
            .shutdown(move |mut guest| {
                Box::pin(async move {
                    let (context_res, context_rep) = guest.with(|mut store| {
                        let res = store.data_mut().add_context(context_clone)?;
                        let rep = res.rep();
                        Ok::<_, wasmtime::Error>((res, rep))
                    })?;
                    let result = guest.call(func, (context_res,)).await.map(|(r,)| r);
                    guest.with(|mut store| {
                        let _ = store
                            .data_mut()
                            .resource_table
                            .delete::<state::ContextResource>(
                                wasmtime::component::Resource::new_own(context_rep),
                            );
                    });
                    result
                })
            })
            .await
            .map_err(|e| e.to_string())?
    }

    pub async fn handle_event(
        &self,
        handler_id: u32,
        event: wit::vine::plugin::event::Event,
    ) -> Result<wit::vine::plugin::event::Event, wasmtime::Error> {
        let func = self.instance.func_handle_event();
        self.store
            .call_guest(move |mut guest| {
                Box::pin(async move {
                    let (res,) = guest.call(func, (handler_id, event)).await?;
                    Ok(res)
                })
            })
            .await
    }

    pub async fn handle_command(
        &self,
        command_id: u32,
        sender: wit::vine::plugin::types::CommandSender,
        args: Vec<String>,
    ) -> Result<Result<i32, String>, wasmtime::Error> {
        let func = self.instance.func_handle_command();
        self.store
            .call_guest(move |mut guest| {
                Box::pin(async move {
                    let (res,) = guest.call(func, (command_id, sender, args)).await?;
                    Ok(res)
                })
            })
            .await
    }

    pub async fn handle_command_suggestion(
        &self,
        command_id: u32,
        sender: wit::vine::plugin::types::CommandSender,
        args: Vec<String>,
    ) -> Result<Vec<String>, wasmtime::Error> {
        let func = self.instance.func_handle_command_suggestion();
        self.store
            .call_guest(move |mut guest| {
                Box::pin(async move {
                    let (res,) = guest.call(func, (command_id, sender, args)).await?;
                    Ok(res)
                })
            })
            .await
    }

    pub async fn handle_task(&self, handler_id: u32) -> Result<(), wasmtime::Error> {
        let func = self.instance.func_handle_task();
        self.store
            .call_guest(move |mut guest| {
                Box::pin(async move {
                    let () = guest.call(func, (handler_id,)).await?;
                    Ok(())
                })
            })
            .await
    }

    pub async fn handle_ipc_message(
        &self,
        sender_plugin: String,
        message: Vec<u8>,
    ) -> Result<Result<Vec<u8>, String>, wasmtime::Error> {
        let func = self.instance.func_handle_ipc_message();
        self.store
            .call_guest(move |mut guest| {
                Box::pin(async move {
                    let (res,) = guest.call(func, (sender_plugin, message)).await?;
                    Ok(res)
                })
            })
            .await
    }
}

impl Plugin for Arc<WasmPlugin> {
    fn on_load(&self, context: Arc<PluginContext>) -> PluginFuture<'_, Result<(), String>> {
        Box::pin(async move { WasmPlugin::on_load(self, context).await })
    }

    fn on_unload(&self, context: Arc<PluginContext>) -> PluginFuture<'_, Result<(), String>> {
        Box::pin(async move { WasmPlugin::on_unload(self, context).await })
    }
}

pub struct PluginRuntime {
    engine_p2: Engine,
    engine_p3: Engine,
    cache_dir: PathBuf,
    linker_p2: Linker<PluginHostState>,
    linker_p3: Linker<PluginHostState>,
    reentry: LegacySyncReentry,
    spawner: Arc<TokioSpawner>,
    task_scheduler: Arc<PluginTaskScheduler>,
}

fn new_engine(cache_dir: &Path, mode: StoreMode) -> Result<Engine, PluginInitError> {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    if mode == StoreMode::Concurrent {
        config.wasm_component_model_async(true);
    }
    config.concurrency_support(mode == StoreMode::Concurrent);
    config.gc_support(true);
    config.wasm_gc(true);
    config.wasm_exceptions(true);
    config.wasm_function_references(true);

    if let Ok(abs_cache) = std::path::absolute(cache_dir) {
        let mut cache_cfg = CacheConfig::new();
        cache_cfg.with_directory(abs_cache);
        if let Ok(cache) = Cache::new(cache_cfg) {
            config.cache(Some(cache));
        }
    }

    Engine::new(&config).map_err(PluginInitError::EngineCreationFailed)
}

impl PluginRuntime {
    pub fn new(plugins_dir: &Path) -> Result<Self, PluginInitError> {
        let cache_dir = plugins_dir.join("cache");
        let _ = fs::create_dir_all(&cache_dir);

        let engine_p2 = new_engine(&cache_dir, StoreMode::Sequential)?;
        let engine_p3 = new_engine(&cache_dir, StoreMode::Concurrent)?;
        let linker_p2 = setup_linker(&engine_p2, StoreMode::Sequential)
            .map_err(PluginInitError::LinkerSetupFailed)?;
        let linker_p3 = setup_linker(&engine_p3, StoreMode::Concurrent)
            .map_err(PluginInitError::LinkerSetupFailed)?;
        let reentry = LegacySyncReentry::new();
        let handle = tokio::runtime::Handle::current();
        let spawner = Arc::new(TokioSpawner::new(handle.clone()));
        let task_scheduler = Arc::new(PluginTaskScheduler::new(handle));

        Ok(Self {
            engine_p2,
            engine_p3,
            cache_dir,
            linker_p2,
            linker_p3,
            reentry,
            spawner,
            task_scheduler,
        })
    }

    fn load_or_compile_component(
        &self,
        engine: &Engine,
        mode: StoreMode,
        wasm_bytes: &[u8],
    ) -> Result<Component, PluginInitError> {
        let _ = fs::create_dir_all(&self.cache_dir);
        let hash = sha256_hex(wasm_bytes);
        let mode_tag = match mode {
            StoreMode::Sequential => "p2",
            StoreMode::Concurrent => "p3",
        };
        let cache_path = self.cache_dir.join(format!(
            "{hash}-{mode_tag}-{}.cwasm",
            env!("CARGO_PKG_VERSION")
        ));

        if cache_path.exists() {
            // SAFETY: Cache artifact is keyed by SHA-256 of the clean WASM bytes, store mode, and
            // Vine version, and Wasmtime additionally validates engine/target compatibility headers.
            match unsafe { Component::deserialize_file(engine, &cache_path) } {
                Ok(component) => return Ok(component),
                Err(_) => {
                    let _ = fs::remove_file(&cache_path);
                }
            }
        }

        let component = Component::new(engine, wasm_bytes)
            .map_err(PluginInitError::ComponentCompilationFailed)?;
        if let Ok(serialized) = component
            .serialize()
            .map_err(PluginInitError::ComponentCacheSerializeFailed)
        {
            let _ = fs::write(&cache_path, serialized);
        }
        Ok(component)
    }

    pub async fn init_plugin(
        &self,
        path: &Path,
        proxy: Option<ProxyPluginContext>,
    ) -> Result<(Arc<WasmPlugin>, PluginMetadata), PluginInitError> {
        let raw_bytes =
            fs::read(path).map_err(|e| PluginInitError::FileReadFailed(path.to_path_buf(), e))?;

        let plugin_cfg = proxy
            .as_ref()
            .map(|p| p.current_config().plugins.clone())
            .unwrap_or_default();

        let signature_status = verify_wasm_signature(&raw_bytes, &plugin_cfg.trusted_public_keys);
        if plugin_cfg.verify_signatures {
            if signature_status.is_signed && !signature_status.is_valid {
                return Err(PluginInitError::SignatureVerificationFailed(
                    path.to_path_buf(),
                    signature_status
                        .error
                        .unwrap_or_else(|| "invalid signature".to_string()),
                ));
            }
            let any_override_allows_unsigned = plugin_cfg
                .overrides
                .values()
                .any(|o| o.allow_unsigned == Some(true));
            if !signature_status.is_signed
                && !plugin_cfg.allow_unsigned
                && !any_override_allows_unsigned
            {
                return Err(PluginInitError::SignatureVerificationFailed(
                    path.to_path_buf(),
                    "unsigned plugins are disallowed by plugins.allow_unsigned = false".to_string(),
                ));
            }
        }

        let wasm_bytes = strip_signature_sections(&raw_bytes);
        let names = scan_component_names(&wasm_bytes)?;
        let Some(ref api_ver) = names.metadata_version else {
            return Err(PluginInitError::UnknownApiVersion(path.to_path_buf()));
        };
        if !api_ver.starts_with("0.1.") {
            return Err(PluginInitError::UnsupportedApiVersion(
                path.to_path_buf(),
                api_ver.clone(),
            ));
        }

        let store_mode = if names.wasi_p3_import.is_some() {
            StoreMode::Concurrent
        } else {
            StoreMode::Sequential
        };
        let (engine, linker) = match store_mode {
            StoreMode::Sequential => (&self.engine_p2, &self.linker_p2),
            StoreMode::Concurrent => (&self.engine_p3, &self.linker_p3),
        };

        let component = self.load_or_compile_component(engine, store_mode, &wasm_bytes)?;

        let instance_pre = linker
            .instantiate_pre(&component)
            .map_err(PluginInitError::InstantiationFailed)?;

        let plugin_pre =
            wit::PluginPre::new(instance_pre).map_err(PluginInitError::InstantiationFailed)?;

        let fallback_name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let mut store = Store::new(
            engine,
            PluginHostState::new(fallback_name, PathBuf::new(), proxy, store_mode),
        );
        store.limiter(|state| &mut state.limits);

        let plugin = self
            .reentry
            .scope_bootstrap(plugin_pre.instantiate_async(&mut store))
            .await
            .map_err(PluginInitError::InstantiationFailed)?;

        let legacy_store = concurrent_store::start_legacy_store(
            store,
            self.reentry.clone(),
            Arc::clone(&self.spawner) as Arc<dyn pumpkin_plugin_runtime::RuntimeSpawner>,
            store_mode,
        )
        .await
        .map_err(PluginInitError::InstantiationFailed)?;

        let func_init = plugin.func_init_plugin();
        if let Err(err) = legacy_store
            .call_guest(move |mut guest| {
                Box::pin(async move {
                    let () = guest.call(func_init, ()).await?;
                    Ok(())
                })
            })
            .await
        {
            legacy_store.discard();
            return Err(PluginInitError::InitPluginFailed(err));
        }

        let func_meta = plugin.vine_plugin_metadata().func_get_metadata();
        let raw_meta = match legacy_store
            .call_guest(move |mut guest| {
                Box::pin(async move {
                    let (meta,) = guest.call(func_meta, ()).await?;
                    Ok(meta)
                })
            })
            .await
        {
            Ok(meta) => meta,
            Err(err) => {
                legacy_store.discard();
                return Err(PluginInitError::GetMetadataFailed(err));
            }
        };

        if plugin_cfg.verify_signatures && !signature_status.is_signed {
            let allow_unsigned = plugin_cfg
                .overrides
                .get(&raw_meta.name)
                .and_then(|o| o.allow_unsigned)
                .unwrap_or(plugin_cfg.allow_unsigned);
            if !allow_unsigned {
                legacy_store.discard();
                return Err(PluginInitError::SignatureVerificationFailed(
                    path.to_path_buf(),
                    format!(
                        "plugin '{}' is unsigned and disallowed by override",
                        raw_meta.name
                    ),
                ));
            }
            tracing::warn!(
                "Plugin '{}' ({}) is unsigned.",
                raw_meta.name,
                path.display()
            );
        }

        let metadata = PluginMetadata {
            name: raw_meta.name.clone(),
            version: raw_meta.version,
            authors: raw_meta.authors,
            description: raw_meta.description,
            dependencies: raw_meta.dependencies,
            permissions: raw_meta.permissions.clone(),
        };

        let plugin_name = raw_meta.name;
        let plugin_perms = raw_meta.permissions;
        if let Err(err) = legacy_store
            .call(move |state| {
                state.plugin_name = plugin_name;
                state.permissions = plugin_perms;
            })
            .await
        {
            legacy_store.discard();
            return Err(PluginInitError::InstantiationFailed(err));
        }

        let wasm_plugin = Arc::new(WasmPlugin {
            instance: plugin,
            store: legacy_store,
            metadata: metadata.clone(),
            binding_language: names.binding_language,
            task_scheduler: Arc::clone(&self.task_scheduler),
        });

        let weak = Arc::downgrade(&wasm_plugin);
        wasm_plugin
            .store
            .call(move |state| {
                state.plugin = Some(weak);
            })
            .await
            .map_err(PluginInitError::InstantiationFailed)?;

        Ok((wasm_plugin, metadata))
    }
}
