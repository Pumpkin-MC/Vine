//! Timer and background task scheduling API for Vine plugins.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::Context;
use crate::wit::vine::plugin::scheduler;

/// Thread-safe callback invoked when a scheduled timer fires.
pub type TaskHandler = Arc<dyn Fn() + Send + Sync>;

pub(crate) struct TaskRegistry {
    handlers: BTreeMap<u32, TaskHandler>,
    next_id: u32,
}

pub(crate) static TASK_HANDLERS: Mutex<TaskRegistry> = Mutex::new(TaskRegistry {
    handlers: BTreeMap::new(),
    next_id: 0,
});

impl TaskRegistry {
    pub fn register(&mut self, handler: TaskHandler) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.handlers.insert(id, handler);
        id
    }

    pub fn get(&self, id: u32) -> Option<TaskHandler> {
        self.handlers.get(&id).cloned()
    }
}

/// Schedules a one-shot callback to run after `delay_ms` milliseconds.
pub fn schedule_delayed_ms<F>(delay_ms: u64, handler: F) -> u32
where
    F: Fn() + Send + Sync + 'static,
{
    let handler_id = TASK_HANDLERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .register(Arc::new(handler));
    scheduler::schedule_delayed(handler_id, delay_ms)
}

/// Schedules a repeating callback to run after `delay_ms` milliseconds and every `period_ms`
/// milliseconds thereafter.
pub fn schedule_repeating_ms<F>(delay_ms: u64, period_ms: u64, handler: F) -> u32
where
    F: Fn() + Send + Sync + 'static,
{
    let handler_id = TASK_HANDLERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .register(Arc::new(handler));
    scheduler::schedule_repeating(handler_id, delay_ms, period_ms)
}

/// Cancels a scheduled task by its task ID. Returns `true` if an active task was cancelled.
pub fn cancel_task(task_id: u32) -> bool {
    scheduler::cancel_task(task_id)
}

/// Extension trait providing ergonomic [`Duration`]-based scheduling on [`Context`].
pub trait SchedulerExt {
    /// Schedules a one-shot task to run after `delay`.
    fn schedule_delayed<F>(&self, delay: Duration, handler: F) -> u32
    where
        F: Fn() + Send + Sync + 'static;

    /// Schedules a repeating task to run after `delay` and every `period` thereafter.
    fn schedule_repeating<F>(&self, delay: Duration, period: Duration, handler: F) -> u32
    where
        F: Fn() + Send + Sync + 'static;
}

impl SchedulerExt for Context {
    fn schedule_delayed<F>(&self, delay: Duration, handler: F) -> u32
    where
        F: Fn() + Send + Sync + 'static,
    {
        let ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
        schedule_delayed_ms(ms, handler)
    }

    fn schedule_repeating<F>(&self, delay: Duration, period: Duration, handler: F) -> u32
    where
        F: Fn() + Send + Sync + 'static,
    {
        let delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
        let period_ms = u64::try_from(period.as_millis()).unwrap_or(u64::MAX).max(1);
        schedule_repeating_ms(delay_ms, period_ms, handler)
    }
}
