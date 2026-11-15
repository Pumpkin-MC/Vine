//! Rust WebAssembly Plugin SDK for the Vine Minecraft Proxy.
//!
//! Implement [`Plugin`] on your plugin struct and export it with [`register_plugin!`]:
//!
//! ```rust,no_run
//! use vine_plugin_api::prelude::*;
//!
//! struct MotdPlugin;
//!
//! impl Plugin for MotdPlugin {
//!     fn new() -> Self {
//!         Self
//!     }
//!
//!     fn metadata(&self) -> PluginMetadata {
//!         PluginMetadata::new("motd-plugin", "0.1.0")
//!             .authors(["you"])
//!             .description("Customizes status ping MOTD")
//!     }
//!
//!     fn on_load(&self, ctx: &Context) -> Result<()> {
//!         tracing::info!("motd-plugin loaded!");
//!         ctx.on_event::<ProxyPing, _>(EventPriority::Normal, |ping| {
//!             ping.motd = "§aPowered by Vine WASM!".into();
//!         });
//!         Ok(())
//!     }
//! }
//!
//! register_plugin!(MotdPlugin);
//! ```

use std::sync::OnceLock;

pub mod commands;
pub mod events;
pub mod logging;
pub mod permissions;
pub mod proxy;
pub mod scheduler;

use crate::commands::{COMMAND_HANDLERS, COMMAND_SUGGESTION_HANDLERS};
use crate::events::EVENT_HANDLERS;
use crate::logging::WitSubscriber;
use crate::scheduler::TASK_HANDLERS;

#[allow(clippy::too_many_arguments, missing_docs)]
pub mod wit {
    wit_bindgen::generate!({
        skip: ["init-plugin"],
        path: "../../wit",
        world: "plugin",
    });

    use super::Component;
    export!(Component);
}

pub use wit::vine::plugin::context::Context;
pub use wit::vine::plugin::types::{
    CommandSender, CommandSenderType, EventPriority, PlayerInfo, ServerInfo,
};

/// Convenient re-exports for writing Vine plugins.
pub mod prelude {
    pub use crate::commands::{
        CommandContextExt, CommandHandler, CommandSenderExt, CommandSpec, CommandSuggestionHandler,
    };
    pub use crate::events::{
        EventContextExt, EventHandler, EventPriority, PlayerChat, PlayerChatEvent, PlayerCommand,
        PlayerCommandEvent, PlayerDisconnect, PlayerDisconnectEvent, PlayerJoin, PlayerJoinEvent,
        PlayerLogin, PlayerLoginEvent, PlayerPreLogin, PlayerPreLoginEvent, PluginMessage,
        PluginMessageEvent, ProxyPing, ProxyPingEvent, ServerConnect, ServerConnectEvent,
        ServerConnected, ServerConnectedEvent,
    };
    pub use crate::permissions;
    pub use crate::proxy;
    pub use crate::scheduler::{SchedulerExt, cancel_task};
    pub use crate::{
        CommandSender, CommandSenderType, Context, PlayerInfo, Plugin, PluginMetadata, Result,
        ServerInfo, register_plugin,
    };
}

/// Convenience `Result` alias used across plugin lifecycle callbacks.
pub type Result<T, E = String> = core::result::Result<T, E>;

/// Metadata describing a Vine plugin to the proxy host.
#[derive(Debug, Clone)]
pub struct PluginMetadata {
    /// Unique identifier / name of the plugin.
    pub name: String,
    /// Semver version string (e.g. `"0.1.0"`).
    pub version: String,
    /// Authors of the plugin.
    pub authors: Vec<String>,
    /// Optional human-readable summary.
    pub description: Option<String>,
    /// Names of plugins that must be loaded before this plugin.
    pub dependencies: Vec<String>,
    /// Host capability permissions requested by this plugin (see [`permissions`]).
    pub permissions: Vec<String>,
}

impl PluginMetadata {
    /// Creates metadata with `name` and `version` and empty optional lists.
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            authors: Vec::new(),
            description: None,
            dependencies: Vec::new(),
            permissions: Vec::new(),
        }
    }

    /// Sets the plugin author list.
    #[must_use]
    pub fn authors<I, S>(mut self, authors: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.authors = authors.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the plugin description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sets the plugin load-order dependencies.
    #[must_use]
    pub fn dependencies<I, S>(mut self, dependencies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.dependencies = dependencies.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the WASI/host permissions requested by this plugin.
    #[must_use]
    pub fn permissions<I, S>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.permissions = permissions.into_iter().map(Into::into).collect();
        self
    }
}

/// Core trait implemented by every Vine WebAssembly plugin.
pub trait Plugin: Send + Sync {
    /// Instantiates the plugin before [`Plugin::metadata`] and [`Plugin::on_load`].
    fn new() -> Self
    where
        Self: Sized;

    /// Returns static metadata describing this plugin, its dependencies, and its permissions.
    fn metadata(&self) -> PluginMetadata;

    /// Called when the plugin is loaded by the Vine proxy.
    fn on_load(&self, _context: &Context) -> Result<()> {
        Ok(())
    }

    /// Called when the plugin is unloaded by the Vine proxy.
    fn on_unload(&self, _context: &Context) -> Result<()> {
        Ok(())
    }

    /// Called when another loaded plugin sends an IPC message to this plugin.
    fn handle_ipc_message(&self, _sender: String, _message: Vec<u8>) -> Result<Vec<u8>, String> {
        Err("This plugin does not handle IPC messages.".to_string())
    }
}

struct Component;

impl wit::exports::vine::plugin::metadata::Guest for Component {
    fn get_metadata() -> wit::exports::vine::plugin::metadata::PluginMetadata {
        let metadata = plugin().metadata();
        wit::exports::vine::plugin::metadata::PluginMetadata {
            name: metadata.name,
            version: metadata.version,
            authors: metadata.authors,
            description: metadata.description,
            dependencies: metadata.dependencies,
            permissions: metadata.permissions,
        }
    }
}

impl wit::Guest for Component {
    fn on_load(context: Context) -> Result<(), String> {
        plugin().on_load(&context)
    }

    fn on_unload(context: Context) -> Result<(), String> {
        plugin().on_unload(&context)
    }

    fn handle_event(handler_id: u32, event: events::Event) -> events::Event {
        let handler = EVENT_HANDLERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&handler_id)
            .cloned();
        if let Some(handler) = handler {
            handler.handle_erased(event)
        } else {
            event
        }
    }

    fn handle_command(
        command_id: u32,
        sender: CommandSender,
        args: Vec<String>,
    ) -> Result<i32, String> {
        let handler = COMMAND_HANDLERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&command_id)
            .cloned();
        handler.map_or_else(
            || Err(format!("no handler registered for command id {command_id}")),
            |h| h.handle(sender, args),
        )
    }

    fn handle_command_suggestion(
        command_id: u32,
        sender: CommandSender,
        args: Vec<String>,
    ) -> Vec<String> {
        let handler = COMMAND_SUGGESTION_HANDLERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&command_id)
            .cloned();
        handler.map_or_else(Vec::new, |h| h.suggest(sender, args))
    }

    fn handle_task(handler_id: u32) {
        let handler = TASK_HANDLERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(handler_id);
        if let Some(handler) = handler {
            handler();
        }
    }

    fn handle_ipc_message(sender_plugin: String, message: Vec<u8>) -> Result<Vec<u8>, String> {
        plugin().handle_ipc_message(sender_plugin, message)
    }
}

static PLUGIN: OnceLock<Box<dyn Plugin>> = OnceLock::new();

#[doc(hidden)]
pub fn register_plugin(build_plugin: fn() -> Box<dyn Plugin>) {
    let _ = tracing::subscriber::set_global_default(WitSubscriber::new());
    assert!(
        PLUGIN.set(build_plugin()).is_ok(),
        "register_plugin must only be called once"
    );
}

fn plugin() -> &'static dyn Plugin {
    PLUGIN
        .get()
        .map(Box::as_ref)
        .expect("PLUGIN must be initialized with register_plugin before use")
}

/// Exports `plugin_type` as the WebAssembly plugin entry point for the Vine proxy.
#[macro_export]
macro_rules! register_plugin {
    ($plugin_type:ty) => {
        #[unsafe(export_name = "init-plugin")]
        pub extern "C" fn __init_plugin() {
            $crate::register_plugin(|| Box::new(<$plugin_type as $crate::Plugin>::new()));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_builder_populates_fields() {
        let meta = PluginMetadata::new("hub-router", "1.2.3")
            .authors(["Alex", "Pumpkin-MC"])
            .description("Routes players to hub")
            .dependencies(["core-perms"])
            .permissions([permissions::FS_READ_DATA, permissions::FS_WRITE_DATA]);

        assert_eq!(meta.name, "hub-router");
        assert_eq!(meta.version, "1.2.3");
        assert_eq!(meta.authors, vec!["Alex", "Pumpkin-MC"]);
        assert_eq!(meta.description.as_deref(), Some("Routes players to hub"));
        assert_eq!(meta.dependencies, vec!["core-perms"]);
        assert_eq!(
            meta.permissions,
            vec![permissions::FS_READ_DATA, permissions::FS_WRITE_DATA]
        );
    }
}
