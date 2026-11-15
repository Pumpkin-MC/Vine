use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::task::AbortHandle;

use super::WasmPlugin;

struct ActiveTask {
    plugin: Weak<WasmPlugin>,
    abort_handle: AbortHandle,
}

/// Async tokio-backed task scheduler for WASM plugins.
///
/// Tracks all scheduled one-shot and repeating timers per plugin so tasks can be cancelled
/// individually or aborted automatically when a plugin unloads.
pub struct PluginTaskScheduler {
    tasks: Mutex<HashMap<u32, ActiveTask>>,
    disabled_plugins: Mutex<Vec<Weak<WasmPlugin>>>,
    next_id: AtomicU32,
    runtime: tokio::runtime::Handle,
}

impl PluginTaskScheduler {
    #[must_use]
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            disabled_plugins: Mutex::new(Vec::new()),
            next_id: AtomicU32::new(1),
            runtime,
        }
    }

    fn is_disabled(disabled: &mut Vec<Weak<WasmPlugin>>, plugin: &Arc<WasmPlugin>) -> bool {
        disabled.retain(|w| w.strong_count() > 0);
        let target = Arc::downgrade(plugin);
        disabled.iter().any(|w| Weak::ptr_eq(w, &target))
    }

    pub fn schedule_delayed(
        self: &Arc<Self>,
        plugin: Arc<WasmPlugin>,
        handler_id: u32,
        delay_ms: u64,
    ) -> u32 {
        let task_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut disabled = self
                .disabled_plugins
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if Self::is_disabled(&mut disabled, &plugin) {
                return task_id;
            }
        }

        let weak_plugin = Arc::downgrade(&plugin);
        let scheduler_ref = Arc::clone(self);
        let join = self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            scheduler_ref
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&task_id);
            if let Err(err) = plugin.handle_task(handler_id).await {
                tracing::error!(
                    plugin = %plugin.metadata.name,
                    task_id,
                    handler_id,
                    %err,
                    "WASM scheduled one-shot task failed"
                );
            }
        });

        self.tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                task_id,
                ActiveTask {
                    plugin: weak_plugin,
                    abort_handle: join.abort_handle(),
                },
            );
        task_id
    }

    pub fn schedule_repeating(
        self: &Arc<Self>,
        plugin: Arc<WasmPlugin>,
        handler_id: u32,
        delay_ms: u64,
        period_ms: u64,
    ) -> u32 {
        let task_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut disabled = self
                .disabled_plugins
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if Self::is_disabled(&mut disabled, &plugin) {
                return task_id;
            }
        }

        let period = Duration::from_millis(period_ms.max(1));
        let weak_plugin = Arc::downgrade(&plugin);
        let join = self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Err(err) = plugin.handle_task(handler_id).await {
                    tracing::error!(
                        plugin = %plugin.metadata.name,
                        task_id,
                        handler_id,
                        %err,
                        "WASM scheduled repeating task failed"
                    );
                }
            }
        });

        self.tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                task_id,
                ActiveTask {
                    plugin: weak_plugin,
                    abort_handle: join.abort_handle(),
                },
            );
        task_id
    }

    pub fn cancel_task(&self, task_id: u32) -> bool {
        if let Some(entry) = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&task_id)
        {
            entry.abort_handle.abort();
            true
        } else {
            false
        }
    }

    pub fn disable_plugin(&self, plugin: &Arc<WasmPlugin>) {
        let target = Arc::downgrade(plugin);
        {
            let mut disabled = self
                .disabled_plugins
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !Self::is_disabled(&mut disabled, plugin) {
                disabled.push(target.clone());
            }
        }

        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tasks.retain(|_, entry| {
            if Weak::ptr_eq(&entry.plugin, &target) || entry.plugin.strong_count() == 0 {
                entry.abort_handle.abort();
                false
            } else {
                true
            }
        });
    }
}
