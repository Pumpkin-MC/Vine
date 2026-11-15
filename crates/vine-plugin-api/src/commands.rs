//! Proxy command registration and tab-completion helpers for Vine plugins.

use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

pub use crate::wit::vine::plugin::types::{CommandSender, CommandSenderType};

use crate::Context;
use crate::wit::vine::plugin::command as command_wit;

pub(crate) static NEXT_COMMAND_ID: AtomicU32 = AtomicU32::new(0);
pub(crate) static COMMAND_HANDLERS: Mutex<BTreeMap<u32, Arc<dyn CommandHandler>>> =
    Mutex::new(BTreeMap::new());
pub(crate) static COMMAND_SUGGESTION_HANDLERS: Mutex<
    BTreeMap<u32, Arc<dyn CommandSuggestionHandler>>,
> = Mutex::new(BTreeMap::new());

/// Convenience methods on [`CommandSender`].
pub trait CommandSenderExt {
    /// Sends a formatted message back to this command sender (console or player).
    fn reply(&self, message: impl AsRef<str>);

    /// Returns `true` if the command was invoked from the proxy console.
    fn is_console(&self) -> bool;
}

impl CommandSenderExt for CommandSender {
    fn reply(&self, message: impl AsRef<str>) {
        command_wit::reply(self, message.as_ref());
    }

    fn is_console(&self) -> bool {
        self.sender_type == CommandSenderType::Console
    }
}

/// Handles execution of a registered proxy command.
pub trait CommandHandler: Send + Sync {
    /// Executes the command and returns an exit code (or an error message string).
    fn handle(&self, sender: CommandSender, args: Vec<String>) -> Result<i32, String>;
}

impl<F> CommandHandler for F
where
    F: Fn(CommandSender, Vec<String>) -> Result<i32, String> + Send + Sync,
{
    fn handle(&self, sender: CommandSender, args: Vec<String>) -> Result<i32, String> {
        (self)(sender, args)
    }
}

/// Computes tab-completion suggestions for a registered proxy command.
pub trait CommandSuggestionHandler: Send + Sync {
    /// Returns completion candidates for the current command `args`.
    fn suggest(&self, sender: CommandSender, args: Vec<String>) -> Vec<String>;
}

impl<F> CommandSuggestionHandler for F
where
    F: Fn(CommandSender, Vec<String>) -> Vec<String> + Send + Sync,
{
    fn suggest(&self, sender: CommandSender, args: Vec<String>) -> Vec<String> {
        (self)(sender, args)
    }
}

/// Builder for registering a proxy command with aliases, permission node, execution handler,
/// and optional tab-completion suggestions.
pub struct CommandSpec {
    name: String,
    description: String,
    aliases: Vec<String>,
    permission: Option<String>,
    handler: Option<Arc<dyn CommandHandler>>,
    suggester: Option<Arc<dyn CommandSuggestionHandler>>,
}

impl CommandSpec {
    /// Starts building a new command with `name` and `description`.
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            aliases: Vec::new(),
            permission: None,
            handler: None,
            suggester: None,
        }
    }

    /// Adds command aliases (e.g. `["hub", "l"]`).
    #[must_use]
    pub fn aliases<I, S>(mut self, aliases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }

    /// Requires a permission node (e.g. `"myplugin.admin"`) to execute this command.
    #[must_use]
    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.permission = Some(permission.into());
        self
    }

    /// Attaches the execution handler for this command.
    #[must_use]
    pub fn executes<H: CommandHandler + 'static>(mut self, handler: H) -> Self {
        self.handler = Some(Arc::new(handler));
        self
    }

    /// Attaches an optional tab-completion handler for this command.
    #[must_use]
    pub fn suggests<S: CommandSuggestionHandler + 'static>(mut self, suggester: S) -> Self {
        self.suggester = Some(Arc::new(suggester));
        self
    }
}

/// Extension trait on [`Context`] for registering [`CommandSpec`]s.
pub trait CommandContextExt {
    /// Registers `command` on the Vine proxy command dispatcher and returns its command ID.
    fn register_command_spec(&self, command: CommandSpec) -> u32;
}

impl CommandContextExt for Context {
    fn register_command_spec(&self, command: CommandSpec) -> u32 {
        let id = NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed);
        if let Some(handler) = command.handler {
            COMMAND_HANDLERS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(id, handler);
        }
        if let Some(suggester) = command.suggester {
            COMMAND_SUGGESTION_HANDLERS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(id, suggester);
        }
        self.register_command(
            id,
            &command.name,
            &command.description,
            &command.aliases,
            command.permission.as_deref(),
        );
        id
    }
}
