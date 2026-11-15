//! WIT-backed logging and `tracing` integration for Vine plugins.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::wit;

/// Log severity level forwarded to the Vine host logger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Very fine-grained diagnostic information.
    Trace,
    /// Diagnostic information useful during development.
    Debug,
    /// General informational messages.
    Info,
    /// Potentially harmful situations that do not stop execution.
    Warn,
    /// Errors that require attention.
    Error,
}

impl LogLevel {
    const fn to_wit(self) -> wit::vine::plugin::logging::Level {
        match self {
            Self::Trace => wit::vine::plugin::logging::Level::Trace,
            Self::Debug => wit::vine::plugin::logging::Level::Debug,
            Self::Info => wit::vine::plugin::logging::Level::Info,
            Self::Warn => wit::vine::plugin::logging::Level::Warn,
            Self::Error => wit::vine::plugin::logging::Level::Error,
        }
    }
}

/// Sends a log message directly to the Vine proxy host at the given severity level.
pub fn log(level: LogLevel, message: &str) {
    wit::vine::plugin::logging::log(level.to_wit(), message);
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl MessageVisitor {
    fn finish(self) -> String {
        if self.fields.is_empty() {
            self.message
        } else if self.message.is_empty() {
            self.fields
        } else {
            format!("{} ({})", self.message, self.fields)
        }
    }
}

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            if !self.fields.is_empty() {
                self.fields.push_str(", ");
            }
            let _ = write!(self.fields, "{}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            if !self.fields.is_empty() {
                self.fields.push_str(", ");
            }
            let _ = write!(self.fields, "{}={value:?}", field.name());
        }
    }
}

/// A [`tracing::Subscriber`] that forwards `tracing` macros inside a WASM plugin
/// directly to the Vine host logger over WIT.
pub(crate) struct WitSubscriber {
    next_id: AtomicU64,
}

impl WitSubscriber {
    pub const fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
        }
    }
}

impl tracing::Subscriber for WitSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        tracing::span::Id::from_u64(id)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let level = match *event.metadata().level() {
            tracing::Level::TRACE => LogLevel::Trace,
            tracing::Level::DEBUG => LogLevel::Debug,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::ERROR => LogLevel::Error,
        };
        log(level, &visitor.finish());
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct CaptureSubscriber(Arc<Mutex<Vec<String>>>);

    impl tracing::Subscriber for CaptureSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut visitor = MessageVisitor::default();
            event.record(&mut visitor);
            self.0.lock().unwrap().push(visitor.finish());
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    #[test]
    fn visitor_formats_message_and_fields() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = CaptureSubscriber(Arc::clone(&captured));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(count = 42, "hello");
        });
        assert_eq!(captured.lock().unwrap().as_slice(), &["hello (count=42)"]);
    }
}
