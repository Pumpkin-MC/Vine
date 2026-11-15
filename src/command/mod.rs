pub mod alert;
pub mod end;
pub mod find;
pub mod glist;
pub mod kick;
pub mod send;
pub mod server;
pub mod vine_cmd;

use std::sync::{Arc, RwLock, Weak};
use std::time::Instant;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use pumpkin_command::Requirement;
use pumpkin_command::argument_builder::{ArgumentBuilder, argument, command};
use pumpkin_command::argument_types::core::string::StringArgumentType;
use pumpkin_command::context::command_context::CommandContext;
use pumpkin_command::context::string_range::StringRange;
pub use pumpkin_command::dispatcher::CommandDispatcher as RawCommandDispatcher;
pub use pumpkin_command::errors::command_syntax_error::CommandSyntaxError;
pub use pumpkin_command::errors::error_types::LiteralCommandErrorType;
pub use pumpkin_command::node::{CommandExecutor, CommandExecutorResult};
pub use pumpkin_command::source::CommandSource;
use pumpkin_command::suggestion::Suggestion;
use pumpkin_command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
pub use pumpkin_command::suggestion::suggestions::Suggestions;
use pumpkin_command::suggestion::suggestions::SuggestionsBuilder;
use pumpkin_protocol::java::client::play::CommandSuggestion;
use pumpkin_util::text::TextComponent;

use crate::config::Config;
use crate::plugin::{PluginManager, PluginMetadata};
use crate::session::{PlayerAction, SessionManager};

pub type ReloadCallback = Arc<dyn Fn() -> Result<Arc<Config>, String> + Send + Sync>;

pub static PROXY_COMMAND_ERROR: LiteralCommandErrorType =
    LiteralCommandErrorType::new("Proxy command execution failed");

pub fn command_error(msg: impl Into<String>) -> CommandSyntaxError {
    CommandSyntaxError::create_without_context(
        &PROXY_COMMAND_ERROR,
        TextComponent::text(msg.into()),
    )
}

/// Translates legacy `&` formatting codes (`&0`-`&9`, `&a`-`&f`, `&k`-`&o`, `&r`) into Minecraft `§` codes.
#[must_use]
pub fn translate_color_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '&'
            && let Some(&next) = chars.peek()
            && matches!(
                next.to_ascii_lowercase(),
                '0'..='9' | 'a'..='f' | 'k'..='o' | 'r'
            )
        {
            chars.next();
            out.push('§');
            out.push(next.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Strips Minecraft `§` color/formatting codes from a string.
#[must_use]
pub fn strip_formatting_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '§' {
            let _ = chars.next();
        } else {
            out.push(ch);
        }
    }
    out
}

/// Formats a duration in seconds as a compact human-readable string (e.g. `"2h 14m 5s"`).
#[must_use]
pub fn format_duration_secs(total_secs: u64) -> String {
    let days = total_secs / 86_400;
    let hours = (total_secs % 86_400) / 3_600;
    let mins = (total_secs % 3_600) / 60;
    let secs = total_secs % 60;

    if days > 0 {
        format!("{days}d {hours}h {mins}m {secs}s")
    } else if hours > 0 {
        format!("{hours}h {mins}m {secs}s")
    } else if mins > 0 {
        format!("{mins}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

/// Shared runtime hooks bound by `ProxyServer` so administrative commands (`/end`, `/vine`)
/// can inspect loaded plugins, reload configuration, and trigger graceful proxy shutdown.
pub struct CommandRuntime {
    started_at: Instant,
    shutdown_tx: RwLock<Option<broadcast::Sender<()>>>,
    plugin_manager: RwLock<Option<Weak<PluginManager>>>,
    reload_handler: RwLock<Option<ReloadCallback>>,
}

impl Default for CommandRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            shutdown_tx: RwLock::new(None),
            plugin_manager: RwLock::new(None),
            reload_handler: RwLock::new(None),
        }
    }

    #[must_use]
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub fn set_shutdown_sender(&self, tx: broadcast::Sender<()>) {
        if let Ok(mut guard) = self.shutdown_tx.write() {
            *guard = Some(tx);
        }
    }

    pub fn trigger_shutdown(&self) -> bool {
        if let Ok(guard) = self.shutdown_tx.read()
            && let Some(ref tx) = *guard
        {
            let _ = tx.send(());
            return true;
        }
        false
    }

    pub fn set_plugin_manager(&self, manager: &Arc<PluginManager>) {
        if let Ok(mut guard) = self.plugin_manager.write() {
            *guard = Some(Arc::downgrade(manager));
        }
    }

    #[must_use]
    pub fn plugin_manager(&self) -> Option<Arc<PluginManager>> {
        self.plugin_manager
            .read()
            .ok()
            .and_then(|g| g.as_ref().and_then(Weak::upgrade))
    }

    #[must_use]
    pub fn loaded_plugins(&self) -> Vec<PluginMetadata> {
        self.plugin_manager()
            .map(|pm| pm.list_plugins())
            .unwrap_or_default()
    }

    pub fn set_reload_handler(&self, handler: ReloadCallback) {
        if let Ok(mut guard) = self.reload_handler.write() {
            *guard = Some(handler);
        }
    }

    pub fn reload_config(&self) -> Result<Arc<Config>, String> {
        let handler = self
            .reload_handler
            .read()
            .map_err(|_| "Reload lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "Configuration reload is not bound in this context.".to_string())?;
        handler()
    }
}

/// Represents the origin that invoked a command
#[derive(Clone)]
pub enum CommandSender {
    /// Command invoked via proxy console (stdin)
    Console,
    /// Command invoked by an in-game player
    Player {
        username: String,
        action_tx: mpsc::UnboundedSender<PlayerAction>,
    },
}

impl CommandSender {
    /// Sends response feedback to the command sender
    pub fn send_message(&self, message: TextComponent) {
        match self {
            Self::Console => {
                info!("{}", message.to_pretty_console());
            }
            Self::Player { action_tx, .. } => {
                let _ = action_tx.send(PlayerAction::Message(message));
            }
        }
    }

    /// Sends a plain text message to the command sender
    pub fn send_text(&self, text: impl Into<String>) {
        self.send_message(TextComponent::text(text.into()));
    }

    /// Returns the name of the sender
    pub fn name(&self) -> &str {
        match self {
            Self::Console => "CONSOLE",
            Self::Player { username, .. } => username,
        }
    }
}

/// Execution context passed to commands
#[derive(Clone)]
pub struct ProxyCommandSource {
    pub sender: CommandSender,
    pub config: Arc<Config>,
    pub session_manager: Arc<SessionManager>,
}

impl ProxyCommandSource {
    /// Sends feedback to the command sender without requiring the `CommandSource` trait in scope.
    pub fn send_message(&self, message: TextComponent) {
        self.sender.send_message(message);
    }

    /// Evaluates if the command source has the specified permission node.
    /// Console always returns `true`.
    pub fn has_permission(&self, node: &str) -> bool {
        match &self.sender {
            CommandSender::Console => true,
            CommandSender::Player { username, .. } => {
                self.config.permissions.has_permission(username, node)
            }
        }
    }

    /// Returns true if the command sender has administrative privileges (node `vine.admin` or `*`)
    pub fn is_admin(&self) -> bool {
        self.has_permission("vine.admin")
    }
}

impl CommandSource for ProxyCommandSource {
    fn send_message(&self, message: TextComponent) {
        self.sender.send_message(message);
    }

    fn send_error(&self, error: TextComponent) {
        self.send_message(error);
    }

    fn has_permission(&self, permission: &str) -> bool {
        self.has_permission(permission)
    }
}

fn builtin_command_usage(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "server" => Some((
            "/server [server]",
            "View your current server or switch to another backend",
        )),
        "send" => Some((
            "/send <player|all|current> <server|host:port>",
            "Send players to another backend or address (Admin)",
        )),
        "list" => Some(("/list", "List online players connected to the proxy")),
        "glist" => Some((
            "/glist [all|<server>]",
            "List online players grouped by backend server",
        )),
        "alert" => Some((
            "/alert <message>",
            "Broadcast a formatted alert to all connected players (Admin)",
        )),
        "find" => Some((
            "/find <player>",
            "Inspect an online player's server, IP, version, and session (Admin)",
        )),
        "kick" => Some((
            "/kick <player> [reason]",
            "Disconnect an online player from the proxy (Admin)",
        )),
        "end" | "stop" => Some((
            "/end [reason] (alias: /stop)",
            "Disconnect players and gracefully stop the proxy (Admin)",
        )),
        "vine" => Some((
            "/vine [info|plugins|servers|reload|link]",
            "Display Vine proxy status, plugins, backends, reload, or link account",
        )),
        "help" => Some((
            "/help [command]",
            "Show available commands or detailed help for a command",
        )),
        _ => None,
    }
}

type SharedRawDispatcher = Weak<RwLock<RawCommandDispatcher<ProxyCommandSource>>>;

fn permitted_commands_from_weak(
    dispatcher: &SharedRawDispatcher,
    source: &ProxyCommandSource,
) -> Vec<(String, String)> {
    dispatcher
        .upgrade()
        .and_then(|arc| {
            arc.read().ok().map(|lock| {
                lock.get_all_permitted_commands(source)
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Help command listing permitted registered commands
pub struct HelpCommandExecutor {
    dispatcher: SharedRawDispatcher,
}

impl CommandExecutor<ProxyCommandSource> for HelpCommandExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.help") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.help).",
            ));
        }

        let mut permitted = permitted_commands_from_weak(&self.dispatcher, &context.source);
        permitted.sort_by(|a, b| a.0.cmp(&b.0));

        let mut lines = vec!["Available commands:".to_string()];
        for (cmd_name, description) in permitted {
            if cmd_name == "stop" {
                // Documented alongside `/end` to keep `/help` concise
                continue;
            }
            if let Some((usage, summary)) = builtin_command_usage(&cmd_name) {
                lines.push(format!("  - {usage:<44} : {summary}"));
            } else {
                let usage = format!("/{cmd_name}");
                lines.push(format!("  - {usage:<44} : {description}"));
            }
        }

        context
            .source
            .send_message(TextComponent::text(lines.join("\n")));
        Ok(1)
    }
}

/// Suggestion provider for `/help [command]`
struct HelpSuggestionProvider {
    dispatcher: SharedRawDispatcher,
}

impl SuggestionProvider<ProxyCommandSource> for HelpSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext<ProxyCommandSource>,
        builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult {
        let mut builder = builder;
        let remaining = builder.remaining_lowercase().to_string();

        for (cmd_name, _) in permitted_commands_from_weak(&self.dispatcher, &context.source) {
            if cmd_name.starts_with(&remaining) {
                builder = builder.suggest(cmd_name);
            }
        }
        builder.build()
    }
}

/// Detailed help executor for `/help <command>`
struct CommandSpecificHelpExecutor {
    dispatcher: SharedRawDispatcher,
}

impl CommandExecutor<ProxyCommandSource> for CommandSpecificHelpExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.help") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.help).",
            ));
        }

        let raw_cmd = StringArgumentType::get(context, "command")?;
        let cmd = raw_cmd.trim_start_matches('/').to_ascii_lowercase();
        let msg = match cmd.as_str() {
            "server" => {
                "Command /server:\n  Usage: /server [server]\n  Permission: vine.command.server\n  Description: View your current server or switch to another backend server.".to_string()
            }
            "send" => {
                "Command /send:\n  Usage: /send <player|all|current> <server|host:port>\n  Permission: vine.command.send\n  Description: Send a player, your current server's players, or all online players to another backend or transfer address.".to_string()
            }
            "list" => {
                "Command /list:\n  Usage: /list\n  Permission: vine.command.list\n  Description: List all online players connected to the proxy.".to_string()
            }
            "glist" => {
                "Command /glist:\n  Usage: /glist [all|<server>]\n  Permission: vine.command.glist\n  Description: List online players grouped by backend server (or filtered to one server).".to_string()
            }
            "alert" => {
                "Command /alert:\n  Usage: /alert <message>\n  Permission: vine.command.alert\n  Description: Broadcast a formatted announcement across all connected players (supports & color codes).".to_string()
            }
            "find" => {
                "Command /find:\n  Usage: /find <player>\n  Permission: vine.command.find\n  Description: Show an online player's backend server, UUID, IP, client version/brand, and session duration.".to_string()
            }
            "kick" => {
                "Command /kick:\n  Usage: /kick <player> [reason]\n  Permission: vine.command.kick\n  Description: Kick an online player from the proxy with an optional reason.".to_string()
            }
            "end" | "stop" => {
                "Command /end (alias: /stop):\n  Usage: /end [reason]\n  Permission: vine.command.end\n  Description: Disconnect all online players with an optional message and gracefully shut down the proxy.".to_string()
            }
            "vine" => {
                "Command /vine:\n  Usage: /vine [info|version|plugins|servers|reload|link [name]]\n  Permission: vine.command.vine (reload: vine.command.reload, link: console-only)\n  Description: View proxy runtime status, loaded WASM plugins, configured backends, reload vine.toml, or link to Pumpkin Account.".to_string()
            }
            "help" => {
                "Command /help:\n  Usage: /help [command]\n  Permission: vine.command.help\n  Description: Show help information for available proxy commands.".to_string()
            }
            other => {
                if let Some((_, desc)) =
                    permitted_commands_from_weak(&self.dispatcher, &context.source)
                        .into_iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(other))
                {
                    format!("Command /{other}:\n  Usage: /{other} [args]\n  Description: {desc}")
                } else {
                    "Unknown command. Use /help to see all available proxy commands.".to_string()
                }
            }
        };
        context.source.send_message(TextComponent::text(msg));
        Ok(1)
    }
}

/// List command displaying online players
pub struct ListCommandExecutor;

impl CommandExecutor<ProxyCommandSource> for ListCommandExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.list") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.list).",
            ));
        }

        let sessions = context.source.session_manager.list_sessions();
        if sessions.is_empty() {
            context
                .source
                .send_message(TextComponent::text("There are 0 players online."));
            return Ok(0);
        }

        let mut players_info: Vec<String> = sessions
            .into_iter()
            .map(|s| format!("{} ({})", s.username, s.current_server))
            .collect();
        players_info.sort_unstable();

        context.source.send_message(TextComponent::text(format!(
            "Online players ({}): {}",
            players_info.len(),
            players_info.join(", ")
        )));
        Ok(players_info.len() as i32)
    }
}

/// Central registry and dispatcher for commands using `pumpkin-command`
pub struct CommandDispatcher {
    inner: Arc<RwLock<RawCommandDispatcher<ProxyCommandSource>>>,
    runtime: Arc<CommandRuntime>,
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandDispatcher {
    /// Creates a new `CommandDispatcher` with all built-in proxy commands registered.
    pub fn new() -> Self {
        let runtime = Arc::new(CommandRuntime::new());
        let inner = Arc::new(RwLock::new(RawCommandDispatcher::new()));
        let weak_inner = Arc::downgrade(&inner);

        if let Ok(mut dispatcher) = inner.write() {
            server::register(&mut dispatcher);
            send::register(&mut dispatcher);
            glist::register(&mut dispatcher);
            alert::register(&mut dispatcher);
            find::register(&mut dispatcher);
            kick::register(&mut dispatcher);
            end::register(&mut dispatcher, runtime.clone());
            vine_cmd::register(&mut dispatcher, runtime.clone());
            dispatcher.register(
                command("list", "Lists online players connected to the proxy")
                    .requires(Requirement::from("vine.command.list"))
                    .executes(ListCommandExecutor),
            );
            dispatcher.register(
                command("help", "Displays list of available proxy commands")
                    .requires(Requirement::from("vine.command.help"))
                    .executes(HelpCommandExecutor {
                        dispatcher: weak_inner.clone(),
                    })
                    .then(
                        argument("command", StringArgumentType::SingleWord)
                            .suggests(HelpSuggestionProvider {
                                dispatcher: weak_inner.clone(),
                            })
                            .executes(CommandSpecificHelpExecutor {
                                dispatcher: weak_inner,
                            }),
                    ),
            );
        }

        Self { inner, runtime }
    }

    /// Returns a reference to the shared command runtime hooks.
    #[must_use]
    pub const fn runtime(&self) -> &Arc<CommandRuntime> {
        &self.runtime
    }

    /// Registers a new command builder into the dispatcher
    pub fn register(
        &self,
        builder: pumpkin_command::argument_builder::CommandArgumentBuilder<ProxyCommandSource>,
    ) {
        if let Ok(mut lock) = self.inner.write() {
            lock.register(builder);
        }
    }

    /// Returns true if a command with the given name is registered
    pub fn has_command(&self, name: &str) -> bool {
        let clean = name.trim_start_matches('/').to_ascii_lowercase();
        if let Ok(lock) = self.inner.read() {
            lock.has_command(&clean)
        } else {
            false
        }
    }

    /// Dispatches a command for execution, reporting any error messages back to the source
    pub fn handle_command(&self, source: &ProxyCommandSource, input: &str) {
        let trimmed = input.trim();
        let clean = trimmed.strip_prefix('/').unwrap_or(trimmed);
        if let Ok(lock) = self.inner.read() {
            lock.handle_command(source, clean);
        }
    }

    /// Executes raw input directly, returning the execution result
    pub fn execute_input(
        &self,
        input: &str,
        source: &ProxyCommandSource,
    ) -> Result<i32, CommandSyntaxError> {
        let trimmed = input.trim();
        let clean = trimmed.strip_prefix('/').unwrap_or(trimmed);
        if let Ok(lock) = self.inner.read() {
            lock.execute_input(clean, source)
        } else {
            Err(command_error("Command dispatcher lock poisoned"))
        }
    }

    /// Returns all permitted commands for the given source
    pub fn get_all_permitted_commands(&self, source: &ProxyCommandSource) -> Vec<(String, String)> {
        if let Ok(lock) = self.inner.read() {
            lock.get_all_permitted_commands(source)
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Returns suggestions with range information for any command input.
    /// If input contains arguments (i.e. has a space), it evaluates argument suggestions.
    /// If input is typing the root command name, it suggests permitted root commands starting with the prefix.
    pub fn suggest_command(&self, input: &str, source: &ProxyCommandSource) -> Suggestions {
        let clean = input.strip_prefix('/').unwrap_or(input);
        if clean.contains(' ') {
            let first_word = clean.split_whitespace().next().unwrap_or(clean);
            if self.has_command(first_word)
                && let Ok(lock) = self.inner.read()
            {
                return lock.suggest_with_range(clean, source);
            }
            return Suggestions::empty();
        }

        let clean_lower = clean.to_ascii_lowercase();
        let mut permitted = self.get_all_permitted_commands(source);
        permitted.sort_by(|a, b| a.0.cmp(&b.0));
        let mut suggestions = Vec::new();
        for (cmd_name, description) in permitted {
            if cmd_name.starts_with(&clean_lower) {
                suggestions.push(Suggestion::with_tooltip(
                    StringRange::between(0, clean.len()),
                    cmd_name,
                    TextComponent::text(description),
                ));
            }
        }
        Suggestions::new(StringRange::between(0, clean.len()), suggestions)
    }

    /// Returns suggestions for the command string
    pub fn suggest(&self, input: &str, source: &ProxyCommandSource) -> Vec<CommandSuggestion> {
        let suggestions = self.suggest_command(input, source);
        suggestions
            .suggestions
            .into_iter()
            .map(|s| CommandSuggestion {
                suggestion: s.text.cached_text().clone(),
                tooltip: s.tooltip,
            })
            .collect()
    }

    /// Returns suggestions with range information
    pub fn suggest_with_range(&self, input: &str, source: &ProxyCommandSource) -> Suggestions {
        self.suggest_command(input, source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_code_helpers_translate_and_strip() {
        assert_eq!(
            translate_color_codes("&cAlert &7&lNow & Later"),
            "§cAlert §7§lNow & Later"
        );
        assert_eq!(
            strip_formatting_codes("§a§lVine §7» §fProxy"),
            "Vine » Proxy"
        );
    }

    #[test]
    fn test_dispatcher_registration_and_dispatch() {
        let dispatcher = CommandDispatcher::new();
        for cmd in [
            "server", "send", "help", "list", "glist", "alert", "find", "kick", "end", "stop",
            "vine",
        ] {
            assert!(dispatcher.has_command(cmd), "missing command {cmd}");
            assert!(dispatcher.has_command(&format!("/{cmd}")));
        }
        assert!(!dispatcher.has_command("unknown"));

        let config = Arc::new(Config::default());
        let session_manager = Arc::new(SessionManager::new());
        let source = ProxyCommandSource {
            sender: CommandSender::Console,
            config,
            session_manager,
        };

        assert!(dispatcher.execute_input("help", &source).is_ok());
        assert!(dispatcher.execute_input("/help vine", &source).is_ok());
        assert!(dispatcher.execute_input("list", &source).is_ok());
        assert!(dispatcher.execute_input("unknown_cmd", &source).is_err());
    }

    #[test]
    fn test_suggest_various_inputs_and_permission_filtering() {
        let dispatcher = CommandDispatcher::new();
        let config = Arc::new(Config::default());
        let session_manager = Arc::new(SessionManager::new());
        let source = ProxyCommandSource {
            sender: CommandSender::Console,
            config: config.clone(),
            session_manager: session_manager.clone(),
        };

        let s_empty = dispatcher.suggest_with_range("", &source);
        let names_empty: Vec<&str> = s_empty
            .suggestions
            .iter()
            .map(|s| s.text.cached_text().as_str())
            .collect();
        for expected in [
            "alert", "end", "find", "glist", "help", "kick", "list", "send", "server", "stop",
            "vine",
        ] {
            assert!(names_empty.contains(&expected), "missing {expected}");
        }

        let s_s = dispatcher.suggest_with_range("s", &source);
        let names_s: Vec<&str> = s_s
            .suggestions
            .iter()
            .map(|s| s.text.cached_text().as_str())
            .collect();
        assert_eq!(names_s, vec!["send", "server", "stop"]);

        let s_ser = dispatcher.suggest_with_range("ser", &source);
        let names_ser: Vec<&str> = s_ser
            .suggestions
            .iter()
            .map(|s| s.text.cached_text().as_str())
            .collect();
        assert_eq!(names_ser, vec!["server"]);

        let s_server_space = dispatcher.suggest_with_range("server ", &source);
        let names_server_space: Vec<&str> = s_server_space
            .suggestions
            .iter()
            .map(|s| s.text.cached_text().as_str())
            .collect();
        assert!(names_server_space.contains(&"lobby"));

        let s_help_space = dispatcher.suggest_with_range("help ", &source);
        let names_help_space: Vec<&str> = s_help_space
            .suggestions
            .iter()
            .map(|s| s.text.cached_text().as_str())
            .collect();
        assert!(names_help_space.contains(&"server"));
        assert!(names_help_space.contains(&"vine"));

        // Non-admin player only sees default-group commands
        let (tx, _) = mpsc::unbounded_channel();
        let regular_source = ProxyCommandSource {
            sender: CommandSender::Player {
                username: "RegularPlayer".to_string(),
                action_tx: tx,
            },
            config,
            session_manager,
        };
        let regular_cmds: Vec<String> = dispatcher
            .get_all_permitted_commands(&regular_source)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(regular_cmds.contains(&"server".to_string()));
        assert!(regular_cmds.contains(&"list".to_string()));
        assert!(regular_cmds.contains(&"glist".to_string()));
        assert!(regular_cmds.contains(&"help".to_string()));
        assert!(!regular_cmds.contains(&"send".to_string()));
        assert!(!regular_cmds.contains(&"kick".to_string()));
        assert!(!regular_cmds.contains(&"end".to_string()));
        assert!(!regular_cmds.contains(&"vine".to_string()));
    }
}
