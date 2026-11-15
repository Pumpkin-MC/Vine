use std::collections::BTreeMap;

use pumpkin_command::Requirement;
use pumpkin_command::argument_builder::{ArgumentBuilder, argument, command};
use pumpkin_command::argument_types::core::string::StringArgumentType;
use pumpkin_command::context::command_context::CommandContext;
use pumpkin_command::dispatcher::CommandDispatcher as RawCommandDispatcher;
use pumpkin_command::node::{CommandExecutor, CommandExecutorResult};
use pumpkin_command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use pumpkin_command::suggestion::suggestions::SuggestionsBuilder;
use pumpkin_util::text::TextComponent;

use super::{ProxyCommandSource, command_error};

struct GlistSuggestionProvider;

impl SuggestionProvider<ProxyCommandSource> for GlistSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext<ProxyCommandSource>,
        builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult {
        let mut builder = builder;
        let remaining = builder.remaining_lowercase().to_string();
        if "all".starts_with(&remaining) {
            builder = builder.suggest("all");
        }
        for server in context.source.config.servers.keys() {
            if server.to_ascii_lowercase().starts_with(&remaining) {
                builder = builder.suggest(server.as_str());
            }
        }
        builder.build()
    }
}

fn execute_glist(
    context: &CommandContext<ProxyCommandSource>,
    filter: Option<&str>,
) -> CommandExecutorResult {
    if !context.source.has_permission("vine.command.glist") {
        return Err(command_error(
            "You do not have permission to execute this command (required: vine.command.glist).",
        ));
    }

    let sessions = context.source.session_manager.list_sessions();
    let total_players = sessions.len();

    let mut server_players: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for server_name in context.source.config.servers.keys() {
        server_players.entry(server_name.clone()).or_default();
    }

    for session in sessions {
        server_players
            .entry(session.current_server.clone())
            .or_default()
            .push(session.username);
    }

    if let Some(target_filter) = filter
        && !target_filter.eq_ignore_ascii_case("all")
    {
        let (canonical_name, mut players) = server_players
            .into_iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(target_filter))
            .ok_or_else(|| command_error(format!("Unknown backend server '{target_filter}'.")))?;
        players.sort_unstable();
        let count = players.len();
        let line = if players.is_empty() {
            format!("[{canonical_name}] (0):")
        } else {
            format!("[{canonical_name}] ({count}): {}", players.join(", "))
        };
        context.source.send_message(TextComponent::text(line));
        return Ok(count as i32);
    }

    let mut lines = Vec::new();
    for (server_name, mut players) in server_players {
        players.sort_unstable();
        if players.is_empty() {
            lines.push(format!("[{server_name}] (0):"));
        } else {
            lines.push(format!(
                "[{server_name}] ({}): {}",
                players.len(),
                players.join(", ")
            ));
        }
    }

    lines.push(format!("Total players online: {total_players}"));
    context
        .source
        .send_message(TextComponent::text(lines.join("\n")));
    Ok(total_players as i32)
}

/// Command to list players grouped by server across the entire proxy
pub struct GlistCommandExecutor;

impl CommandExecutor<ProxyCommandSource> for GlistCommandExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        execute_glist(context, None)
    }
}

struct GlistFilteredExecutor;

impl CommandExecutor<ProxyCommandSource> for GlistFilteredExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        let server = StringArgumentType::get(context, "server")?;
        execute_glist(context, Some(server))
    }
}

pub fn register(dispatcher: &mut RawCommandDispatcher<ProxyCommandSource>) {
    dispatcher.register(
        command(
            "glist",
            "Lists online players grouped by server across the proxy",
        )
        .requires(Requirement::from("vine.command.glist"))
        .executes(GlistCommandExecutor)
        .then(
            argument("server", StringArgumentType::SingleWord)
                .suggests(GlistSuggestionProvider)
                .executes(GlistFilteredExecutor),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use crate::command::{CommandDispatcher, CommandSender};
    use crate::config::{BackendConfig, Config, ForwardingMode};
    use crate::session::{PlayerAction, PlayerSession, SessionManager};

    #[tokio::test]
    async fn test_glist_empty() {
        let mut config = Config::default();
        config.servers.insert(
            "lobby".to_string(),
            BackendConfig {
                address: "127.0.0.1:25565".to_string(),
                forwarding: ForwardingMode::None,
                secret: None,
                motd_line1: None,
                motd_line2: None,
                icon_path: None,
            },
        );

        let session_manager = Arc::new(SessionManager::new());
        let (action_tx, mut action_rx) = mpsc::unbounded_channel();
        let source = ProxyCommandSource {
            sender: CommandSender::Player {
                username: "TestPlayer".to_string(),
                action_tx,
            },
            config: Arc::new(config),
            session_manager,
        };

        let dispatcher = CommandDispatcher::new();
        let res = dispatcher.execute_input("glist", &source);
        assert!(res.is_ok());

        let msg = action_rx.recv().await.unwrap();
        match msg {
            PlayerAction::Message(text) => {
                let s = text.to_pretty_console();
                assert!(s.contains("[lobby] (0):"));
                assert!(s.contains("Total players online: 0"));
            }
            _ => panic!("Expected Message action"),
        }
    }

    #[tokio::test]
    async fn test_glist_with_players_and_server_filter() {
        let mut config = Config::default();
        config.servers.insert(
            "lobby".to_string(),
            BackendConfig {
                address: "127.0.0.1:25565".to_string(),
                forwarding: ForwardingMode::None,
                secret: None,
                motd_line1: None,
                motd_line2: None,
                icon_path: None,
            },
        );
        config.servers.insert(
            "survival".to_string(),
            BackendConfig {
                address: "127.0.0.1:25566".to_string(),
                forwarding: ForwardingMode::None,
                secret: None,
                motd_line1: None,
                motd_line2: None,
                icon_path: None,
            },
        );

        let session_manager = Arc::new(SessionManager::new());
        let (tx1, _) = mpsc::unbounded_channel();
        let (tx2, _) = mpsc::unbounded_channel();

        session_manager.register(PlayerSession {
            username: "Steve".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "lobby".to_string(),
            action_tx: tx1,
            client_ip: "127.0.0.1".to_string(),
            protocol_version: 765,
            client_brand: None,
            locale: None,
            view_distance: None,
        });
        session_manager.register(PlayerSession {
            username: "Alex".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "survival".to_string(),
            action_tx: tx2,
            client_ip: "127.0.0.1".to_string(),
            protocol_version: 765,
            client_brand: None,
            locale: None,
            view_distance: None,
        });

        let (action_tx, mut action_rx) = mpsc::unbounded_channel();
        let source = ProxyCommandSource {
            sender: CommandSender::Player {
                username: "Admin".to_string(),
                action_tx,
            },
            config: Arc::new(config),
            session_manager,
        };

        let dispatcher = CommandDispatcher::new();
        assert!(dispatcher.execute_input("glist", &source).is_ok());

        let msg = action_rx.recv().await.unwrap();
        match msg {
            PlayerAction::Message(text) => {
                let s = text.to_pretty_console();
                assert!(s.contains("[lobby] (1): Steve"));
                assert!(s.contains("[survival] (1): Alex"));
                assert!(s.contains("Total players online: 2"));
            }
            _ => panic!("Expected Message action"),
        }

        assert_eq!(
            dispatcher.execute_input("glist survival", &source).unwrap(),
            1
        );
        match action_rx.recv().await.unwrap() {
            PlayerAction::Message(text) => {
                let s = text.to_pretty_console();
                assert!(s.contains("[survival] (1): Alex"));
                assert!(!s.contains("[lobby]"));
            }
            _ => panic!("Expected Message action"),
        }
    }
}
