use std::net::SocketAddr;

use pumpkin_command::Requirement;
use pumpkin_command::argument_builder::{ArgumentBuilder, argument, command};
use pumpkin_command::argument_types::core::string::StringArgumentType;
use pumpkin_command::context::command_context::CommandContext;
use pumpkin_command::dispatcher::CommandDispatcher as RawCommandDispatcher;
use pumpkin_command::node::{CommandExecutor, CommandExecutorResult};
use pumpkin_command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use pumpkin_command::suggestion::suggestions::SuggestionsBuilder;
use pumpkin_util::text::TextComponent;

use super::{CommandSender, ProxyCommandSource, command_error};
use crate::session::PlayerAction;

/// Parses a `host:port` address string for direct client transfer packets.
#[must_use]
pub fn parse_address(addr: &str) -> Option<(String, u16)> {
    if let Ok(socket_addr) = addr.parse::<SocketAddr>() {
        return Some((socket_addr.ip().to_string(), socket_addr.port()));
    }
    if let Some((host, port_str)) = addr.rsplit_once(':')
        && !host.is_empty()
        && let Ok(port) = port_str.parse::<u16>()
    {
        return Some((host.to_string(), port));
    }
    None
}

struct TargetSuggestionProvider;

impl SuggestionProvider<ProxyCommandSource> for TargetSuggestionProvider {
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
        if matches!(context.source.sender, CommandSender::Player { .. })
            && "current".starts_with(&remaining)
        {
            builder = builder.suggest("current");
        }
        for session in context.source.session_manager.list_sessions() {
            if session
                .username
                .to_ascii_lowercase()
                .starts_with(&remaining)
            {
                builder = builder.suggest(session.username);
            }
        }
        builder.build()
    }
}

struct ServerSuggestionProvider;

impl SuggestionProvider<ProxyCommandSource> for ServerSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext<ProxyCommandSource>,
        builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult {
        let mut builder = builder;
        let remaining = builder.remaining_lowercase().to_string();
        for server in context.source.config.servers.keys() {
            if server.to_ascii_lowercase().starts_with(&remaining) {
                builder = builder.suggest(server.as_str());
            }
        }
        builder.build()
    }
}

enum Destination {
    Backend(String),
    Transfer { host: String, port: u16 },
}

impl Destination {
    fn to_action(&self) -> PlayerAction {
        match self {
            Self::Backend(server_name) => PlayerAction::Connect {
                server_name: server_name.clone(),
            },
            Self::Transfer { host, port } => PlayerAction::Transfer {
                host: host.clone(),
                port: *port,
            },
        }
    }

    fn display_name(&self) -> String {
        match self {
            Self::Backend(name) => name.clone(),
            Self::Transfer { host, port } => format!("{host}:{port}"),
        }
    }
}

struct SendCommandExecutor;

impl CommandExecutor<ProxyCommandSource> for SendCommandExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.send") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.send).",
            ));
        }

        let target = StringArgumentType::get(context, "target")?;
        let server_arg = StringArgumentType::get(context, "server")?;

        let destination = if let Some(name) = context
            .source
            .config
            .servers
            .keys()
            .find(|k| k.eq_ignore_ascii_case(server_arg))
        {
            Destination::Backend(name.clone())
        } else if let Some((host, port)) = parse_address(server_arg) {
            Destination::Transfer { host, port }
        } else {
            let mut available: Vec<&String> = context.source.config.servers.keys().collect();
            available.sort();
            return Err(command_error(format!(
                "Unknown backend server '{}'. Available servers: [{}]",
                server_arg,
                available
                    .into_iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };

        let action = destination.to_action();
        let dest_label = destination.display_name();

        if target.eq_ignore_ascii_case("all") {
            let sessions = context.source.session_manager.list_sessions();
            if sessions.is_empty() {
                return Err(command_error("No players currently online to send."));
            }

            let count = sessions.len();
            for session in &sessions {
                let _ = session.action_tx.send(action.clone());
            }

            context.source.send_message(TextComponent::text(format!(
                "Sent {count} player(s) to '{dest_label}'"
            )));
            Ok(count as i32)
        } else if target.eq_ignore_ascii_case("current") {
            let sender_username = match &context.source.sender {
                CommandSender::Player { username, .. } => username,
                CommandSender::Console => {
                    return Err(command_error(
                        "Console is not connected to a backend server; cannot use 'current'.",
                    ));
                }
            };
            let sender_server = context
                .source
                .session_manager
                .get_session(sender_username)
                .map(|s| s.current_server)
                .ok_or_else(|| command_error("Could not determine your current server."))?;

            let matching: Vec<_> = context
                .source
                .session_manager
                .list_sessions()
                .into_iter()
                .filter(|s| s.current_server.eq_ignore_ascii_case(&sender_server))
                .collect();

            if matching.is_empty() {
                return Err(command_error(format!(
                    "No players currently on '{sender_server}' to send."
                )));
            }

            let count = matching.len();
            for session in &matching {
                let _ = session.action_tx.send(action.clone());
            }

            context.source.send_message(TextComponent::text(format!(
                "Sent {count} player(s) from '{sender_server}' to '{dest_label}'"
            )));
            Ok(count as i32)
        } else {
            let session = context
                .source
                .session_manager
                .get_session(target)
                .ok_or_else(|| command_error(format!("Player '{target}' is not online.")))?;

            let _ = session.action_tx.send(action);

            context.source.send_message(TextComponent::text(format!(
                "Sent '{}' to '{}'",
                session.username, dest_label
            )));
            Ok(1)
        }
    }
}

pub fn register(dispatcher: &mut RawCommandDispatcher<ProxyCommandSource>) {
    dispatcher.register(
        command(
            "send",
            "Sends a player, current server, or all players to another backend or address",
        )
        .requires(Requirement::from("vine.command.send"))
        .then(
            argument("target", StringArgumentType::SingleWord)
                .suggests(TargetSuggestionProvider)
                .then(
                    argument("server", StringArgumentType::GreedyPhrase)
                        .suggests(ServerSuggestionProvider)
                        .executes(SendCommandExecutor),
                ),
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
    use crate::session::{PlayerSession, SessionManager};

    #[test]
    fn test_parse_address() {
        assert_eq!(
            parse_address("127.0.0.1:25565"),
            Some(("127.0.0.1".to_string(), 25565))
        );
        assert_eq!(
            parse_address("mc.example.com:25577"),
            Some(("mc.example.com".to_string(), 25577))
        );
        assert_eq!(parse_address("invalid"), None);
    }

    #[tokio::test]
    async fn test_send_command_execution_current_and_transfer() {
        let mut config = Config::default();
        config.permissions.users.insert(
            "Alex".to_string(),
            crate::config::UserPermissionConfig::Groups(vec!["admin".to_string()]),
        );
        config.servers.insert(
            "survival".to_string(),
            BackendConfig {
                address: "127.0.0.1:25567".to_string(),
                forwarding: ForwardingMode::None,
                secret: None,
                public_key: None,
                motd_line1: None,
                motd_line2: None,
                icon_path: None,
            },
        );

        let session_manager = Arc::new(SessionManager::new());
        let (tx, mut rx) = mpsc::unbounded_channel();
        session_manager.register(PlayerSession {
            username: "Steve".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "lobby".to_string(),
            action_tx: tx,
            client_ip: "127.0.0.1".to_string(),
            protocol_version: 765,
            client_brand: None,
            locale: None,
            view_distance: None,
        });

        let (alex_tx, mut alex_rx) = mpsc::unbounded_channel();
        session_manager.register(PlayerSession {
            username: "Alex".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "lobby".to_string(),
            action_tx: alex_tx.clone(),
            client_ip: "127.0.0.1".to_string(),
            protocol_version: 776,
            client_brand: None,
            locale: None,
            view_distance: None,
        });

        let cfg_arc = Arc::new(config);
        let source = ProxyCommandSource {
            sender: CommandSender::Console,
            config: cfg_arc.clone(),
            session_manager: session_manager.clone(),
        };

        let dispatcher = CommandDispatcher::new();
        assert!(
            dispatcher
                .execute_input("send Steve survival", &source)
                .is_ok()
        );
        assert_eq!(
            rx.recv().await,
            Some(PlayerAction::Connect {
                server_name: "survival".to_string(),
            })
        );

        // Direct host:port transfer
        assert!(
            dispatcher
                .execute_input("send Steve play.example.org:25577", &source)
                .is_ok()
        );
        assert_eq!(
            rx.recv().await,
            Some(PlayerAction::Transfer {
                host: "play.example.org".to_string(),
                port: 25577,
            })
        );

        // `current` target from in-game admin Alex sends everyone on `lobby`
        let alex_source = ProxyCommandSource {
            sender: CommandSender::Player {
                username: "Alex".to_string(),
                action_tx: alex_tx,
            },
            config: cfg_arc,
            session_manager,
        };
        let sent_count = dispatcher
            .execute_input("send current survival", &alex_source)
            .unwrap();
        assert_eq!(sent_count, 2);
        assert_eq!(
            rx.recv().await,
            Some(PlayerAction::Connect {
                server_name: "survival".to_string(),
            })
        );
        assert_eq!(
            alex_rx.recv().await,
            Some(PlayerAction::Connect {
                server_name: "survival".to_string(),
            })
        );

        // Non-admin player does not get `send` permitted or executable
        let (non_admin_tx, _) = mpsc::unbounded_channel();
        let non_admin_source = ProxyCommandSource {
            sender: CommandSender::Player {
                username: "RegularPlayer".to_string(),
                action_tx: non_admin_tx,
            },
            config: Arc::new(Config::default()),
            session_manager: Arc::new(SessionManager::new()),
        };
        assert!(
            !dispatcher
                .get_all_permitted_commands(&non_admin_source)
                .iter()
                .any(|(cmd, _)| cmd == "send")
        );
        assert!(
            dispatcher
                .execute_input("send Steve survival", &non_admin_source)
                .is_err()
        );
    }

    #[test]
    fn test_send_command_unknown_server() {
        let config = Config::default();
        let session_manager = Arc::new(SessionManager::new());
        let source = ProxyCommandSource {
            sender: CommandSender::Console,
            config: Arc::new(config),
            session_manager,
        };

        let dispatcher = CommandDispatcher::new();
        let res = dispatcher.execute_input("send Steve nonexistent", &source);
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .message
                .to_pretty_console()
                .contains("Unknown backend server")
        );
    }

    #[test]
    fn test_send_command_offline_player() {
        let mut config = Config::default();
        config.permissions.users.insert(
            "Alex".to_string(),
            crate::config::UserPermissionConfig::Groups(vec!["admin".to_string()]),
        );
        config.servers.insert(
            "survival".to_string(),
            BackendConfig {
                address: "127.0.0.1:25567".to_string(),
                forwarding: ForwardingMode::None,
                secret: None,
                public_key: None,
                motd_line1: None,
                motd_line2: None,
                icon_path: None,
            },
        );

        let session_manager = Arc::new(SessionManager::new());
        let source = ProxyCommandSource {
            sender: CommandSender::Console,
            config: Arc::new(config),
            session_manager,
        };

        let dispatcher = CommandDispatcher::new();
        let res = dispatcher.execute_input("send Alex survival", &source);
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .message
                .to_pretty_console()
                .contains("Player 'Alex' is not online")
        );
    }

    #[test]
    fn test_send_command_suggestions() {
        let mut config = Config::default();
        config.permissions.users.insert(
            "Alex".to_string(),
            crate::config::UserPermissionConfig::Groups(vec!["admin".to_string()]),
        );
        config.servers.insert(
            "survival".to_string(),
            BackendConfig {
                address: "127.0.0.1:25567".to_string(),
                forwarding: ForwardingMode::None,
                secret: None,
                public_key: None,
                motd_line1: None,
                motd_line2: None,
                icon_path: None,
            },
        );
        config.servers.insert(
            "lobby".to_string(),
            BackendConfig {
                address: "127.0.0.1:25565".to_string(),
                forwarding: ForwardingMode::None,
                secret: None,
                public_key: None,
                motd_line1: None,
                motd_line2: None,
                icon_path: None,
            },
        );

        let session_manager = Arc::new(SessionManager::new());
        let (tx, _rx) = mpsc::unbounded_channel();
        session_manager.register(PlayerSession {
            username: "Steve".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "lobby".to_string(),
            action_tx: tx,
            client_ip: "127.0.0.1".to_string(),
            protocol_version: 765,
            client_brand: None,
            locale: None,
            view_distance: None,
        });

        let source = ProxyCommandSource {
            sender: CommandSender::Console,
            config: Arc::new(config),
            session_manager,
        };

        let dispatcher = CommandDispatcher::new();
        let target_suggestions = dispatcher.suggest("send ", &source);
        let names: Vec<&str> = target_suggestions
            .iter()
            .map(|s| s.suggestion.as_str())
            .collect();
        assert!(names.contains(&"all"));
        assert!(names.contains(&"Steve"));

        let server_suggestions = dispatcher.suggest("send Steve ", &source);
        let servers: Vec<&str> = server_suggestions
            .iter()
            .map(|s| s.suggestion.as_str())
            .collect();
        assert!(servers.contains(&"survival"));
        assert!(servers.contains(&"lobby"));
    }
}
