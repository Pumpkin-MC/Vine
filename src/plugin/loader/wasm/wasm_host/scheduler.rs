//! The `scheduler` interface: delayed and repeating plugin tasks.
//!
//! Each task is a Tokio task that calls the plugin's `handle-task` export when
//! due. It holds only a weak reference to the plugin, so an unloaded plugin's
//! tasks stop on their own; dropping the plugin's store aborts any still
//! waiting.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Weak;
use std::time::Duration;

use tokio::task::AbortHandle;

use super::WasmPlugin;
use super::state::PluginHostState;
use super::wit;

/// The tasks one plugin has scheduled. Aborts them all when dropped.
#[derive(Default)]
pub struct Tasks {
    next_id: u32,
    running: HashMap<u32, AbortHandle>,
}

impl Tasks {
    /// Starts a task that waits `delay`, then calls `run` once, or every
    /// `period` if set. Each run is awaited before the next period starts, so
    /// a slow run delays the next instead of overlapping it. The task stops
    /// when `run` returns false.
    pub fn schedule<F, Fut>(&mut self, delay: Duration, period: Option<Duration>, run: F) -> u32
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = bool> + Send,
    {
        self.running.retain(|_, handle| !handle.is_finished());
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            loop {
                if !run().await {
                    return;
                }
                match period {
                    Some(period) => tokio::time::sleep(period).await,
                    None => return,
                }
            }
        });
        self.running.insert(id, handle.abort_handle());
        id
    }

    pub fn cancel(&mut self, id: u32) {
        if let Some(handle) = self.running.remove(&id) {
            handle.abort();
        }
    }
}

impl Drop for Tasks {
    fn drop(&mut self) {
        for handle in self.running.values() {
            handle.abort();
        }
    }
}

/// Calls the plugin's `handle-task`. Returns false once the plugin is gone.
async fn run_task(plugin: Weak<WasmPlugin>, handler_id: u32) -> bool {
    let Some(plugin) = plugin.upgrade() else {
        return false;
    };
    if let Err(e) = plugin.handle_task(handler_id).await {
        tracing::error!(
            "[Plugin: {}] task {} failed: {}",
            plugin.metadata.name,
            handler_id,
            e
        );
    }
    true
}

impl PluginHostState {
    fn schedule(
        &mut self,
        handler_id: u32,
        delay_ms: u64,
        period_ms: Option<u64>,
    ) -> wasmtime::Result<u32> {
        let plugin = self
            .plugin
            .clone()
            .ok_or_else(|| wasmtime::Error::msg("Plugin reference is not set yet"))?;
        Ok(self.tasks.schedule(
            Duration::from_millis(delay_ms),
            period_ms.map(|ms| Duration::from_millis(ms.max(1))),
            move || run_task(plugin.clone(), handler_id),
        ))
    }
}

impl wit::vine::plugin::scheduler::Host for PluginHostState {
    async fn schedule_delayed_task(
        &mut self,
        handler_id: u32,
        delay_ms: u64,
    ) -> wasmtime::Result<u32> {
        self.schedule(handler_id, delay_ms, None)
    }

    async fn schedule_repeating_task(
        &mut self,
        handler_id: u32,
        delay_ms: u64,
        period_ms: u64,
    ) -> wasmtime::Result<u32> {
        self.schedule(handler_id, delay_ms, Some(period_ms))
    }

    async fn cancel_task(&mut self, task_id: u32) -> wasmtime::Result<()> {
        self.tasks.cancel(task_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn counter() -> (
        Arc<AtomicU32>,
        impl Fn() -> std::future::Ready<bool> + Send + 'static,
    ) {
        let count = Arc::new(AtomicU32::new(0));
        let runs = count.clone();
        (count, move || {
            runs.fetch_add(1, Ordering::SeqCst);
            std::future::ready(true)
        })
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_task_runs_once() {
        let mut tasks = Tasks::default();
        let (count, run) = counter();
        tasks.schedule(Duration::from_millis(100), None, run);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn repeating_task_runs_until_cancelled() {
        let mut tasks = Tasks::default();
        let (count, run) = counter();
        let id = tasks.schedule(Duration::ZERO, Some(Duration::from_millis(100)), run);
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(count.load(Ordering::SeqCst), 4);
        tasks.cancel(id);
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert_eq!(count.load(Ordering::SeqCst), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn tasks_stop_when_dropped_or_the_plugin_is_gone() {
        let mut tasks = Tasks::default();
        let (count, run) = counter();
        tasks.schedule(Duration::ZERO, Some(Duration::from_millis(100)), run);
        tokio::time::sleep(Duration::from_millis(150)).await;
        drop(tasks);
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert_eq!(count.load(Ordering::SeqCst), 2);

        let mut tasks = Tasks::default();
        let gone = Arc::new(AtomicU32::new(0));
        let runs = gone.clone();
        tasks.schedule(
            Duration::ZERO,
            Some(Duration::from_millis(100)),
            move || {
                runs.fetch_add(1, Ordering::SeqCst);
                std::future::ready(false)
            },
        );
        tokio::time::sleep(Duration::from_millis(1000)).await;
        assert_eq!(gone.load(Ordering::SeqCst), 1);
    }
}
