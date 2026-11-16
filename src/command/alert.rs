use pumpkin_command::Requirement;
use pumpkin_command::argument_builder::{ArgumentBuilder, argument, command};
use pumpkin_command::argument_types::core::string::StringArgumentType;
use pumpkin_command::context::command_context::CommandContext;
use pumpkin_command::dispatcher::CommandDispatcher as RawCommandDispatcher;
use pumpkin_command::node::{CommandExecutor, CommandExecutorResult};
use pumpkin_util::text::TextComponent;
use tracing::info;

use super::{CommandSender, ProxyCommandSource, command_error, translate_color_codes};
use crate::session::PlayerAction;

struct AlertCommandExecutor;

impl CommandExecutor<ProxyCommandSource> for AlertCommandExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.alert") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.alert).",
            ));
        }

        let raw_message = StringArgumentType::get(context, "message")?.trim();
        if raw_message.is_empty() {
            return Err(command_error("Alert message cannot be empty."));
        }

        let formatted_body = translate_color_codes(raw_message);
        let formatted_alert = format!("§8[§c§lAlert§8] §r{formatted_body}");
        let component = crate::console::legacy_component(&formatted_alert);

        let sessions = context.source.session_manager.list_sessions();
        let recipient_count = sessions.len();
        for session in sessions {
            let _ = session
                .action_tx
                .send(PlayerAction::Message(component.clone()));
        }

        info!(
            "[Alert by {}] {}",
            context.source.sender.name(),
            crate::console::format_console_component(&component)
        );

        if matches!(context.source.sender, CommandSender::Console) {
            context.source.send_message(TextComponent::text(format!(
                "Broadcasted alert to {recipient_count} online player(s)."
            )));
        }

        Ok(recipient_count as i32)
    }
}

pub fn register(dispatcher: &mut RawCommandDispatcher<ProxyCommandSource>) {
    dispatcher.register(
        command("alert", "Broadcasts an alert message across the proxy")
            .requires(Requirement::from("vine.command.alert"))
            .then(
                argument("message", StringArgumentType::GreedyPhrase)
                    .executes(AlertCommandExecutor),
            ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use crate::command::CommandDispatcher;
    use crate::config::Config;
    use crate::session::{PlayerSession, SessionManager};

    #[tokio::test]
    async fn alert_broadcasts_formatted_message_to_all_sessions() {
        let session_manager = Arc::new(SessionManager::new());
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        for (username, tx) in [("Steve", tx1), ("Alex", tx2)] {
            session_manager.register(PlayerSession {
                username: username.to_string(),
                uuid: Uuid::new_v4(),
                current_server: "lobby".to_string(),
                action_tx: tx,
                client_ip: "127.0.0.1".to_string(),
                protocol_version: 776,
                client_brand: None,
                locale: None,
                view_distance: None,
            });
        }

        let source = ProxyCommandSource {
            sender: CommandSender::Console,
            config: Arc::new(Config::default()),
            session_manager,
        };

        let dispatcher = CommandDispatcher::new();
        let res = dispatcher.execute_input("alert &eProxy reboot in 5 minutes!", &source);
        assert_eq!(res.unwrap(), 2);

        for rx in [&mut rx1, &mut rx2] {
            match rx.recv().await.unwrap() {
                PlayerAction::Message(msg) => {
                    let rendered = msg.to_pretty_console();
                    assert!(rendered.contains("Alert"));
                    assert!(rendered.contains("Proxy reboot in 5 minutes!"));
                }
                other => panic!("Expected Message action, got {other:?}"),
            }
        }
    }
}
