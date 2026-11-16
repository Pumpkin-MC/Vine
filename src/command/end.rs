use std::sync::Arc;

use pumpkin_command::Requirement;
use pumpkin_command::argument_builder::{ArgumentBuilder, argument, command};
use pumpkin_command::argument_types::core::string::StringArgumentType;
use pumpkin_command::context::command_context::CommandContext;
use pumpkin_command::dispatcher::CommandDispatcher as RawCommandDispatcher;
use pumpkin_command::node::{CommandExecutor, CommandExecutorResult};
use tracing::info;

use super::{CommandRuntime, ProxyCommandSource, command_error, translate_color_codes};
use crate::session::PlayerAction;

const DEFAULT_SHUTDOWN_REASON: &str = "Proxy is shutting down.";

fn execute_shutdown(
    context: &CommandContext<ProxyCommandSource>,
    runtime: &CommandRuntime,
    raw_reason: Option<&str>,
) -> CommandExecutorResult {
    if !context.source.has_permission("vine.command.end") {
        return Err(command_error(
            "You do not have permission to execute this command (required: vine.command.end).",
        ));
    }

    let reason = raw_reason
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map_or_else(
            || DEFAULT_SHUTDOWN_REASON.to_string(),
            translate_color_codes,
        );

    let sessions = context.source.session_manager.list_sessions();
    for session in &sessions {
        let _ = session
            .action_tx
            .send(PlayerAction::Disconnect(reason.clone()));
    }

    info!(
        "Proxy shutdown initiated by {} (disconnected {} player(s), reason: {})",
        context.source.sender.name(),
        sessions.len(),
        crate::console::format_console_text(&reason)
    );

    context
        .source
        .send_message(crate::console::legacy_component(format!(
            "§6[Vine] §eShutting down proxy ({reason}§e)..."
        )));

    let _ = runtime.trigger_shutdown();
    Ok(1)
}

struct EndDefaultExecutor {
    runtime: Arc<CommandRuntime>,
}

impl CommandExecutor<ProxyCommandSource> for EndDefaultExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        execute_shutdown(context, &self.runtime, None)
    }
}

struct EndReasonExecutor {
    runtime: Arc<CommandRuntime>,
}

impl CommandExecutor<ProxyCommandSource> for EndReasonExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        let reason = StringArgumentType::get(context, "reason")?;
        execute_shutdown(context, &self.runtime, Some(reason))
    }
}

pub fn register(
    dispatcher: &mut RawCommandDispatcher<ProxyCommandSource>,
    runtime: Arc<CommandRuntime>,
) {
    dispatcher.register_with_aliases(
        command("end", "Gracefully shuts down the Vine proxy")
            .requires(Requirement::from("vine.command.end"))
            .executes(EndDefaultExecutor {
                runtime: runtime.clone(),
            })
            .then(
                argument("reason", StringArgumentType::GreedyPhrase)
                    .executes(EndReasonExecutor { runtime }),
            ),
        &["stop"],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{broadcast, mpsc};
    use uuid::Uuid;

    use crate::command::{CommandDispatcher, CommandSender};
    use crate::config::Config;
    use crate::session::{PlayerSession, SessionManager};

    #[tokio::test]
    async fn end_and_stop_disconnect_players_and_signal_shutdown() {
        let dispatcher = CommandDispatcher::new();
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(2);
        dispatcher.runtime().set_shutdown_sender(shutdown_tx);

        let session_manager = Arc::new(SessionManager::new());
        let (tx, mut rx) = mpsc::unbounded_channel();
        session_manager.register(PlayerSession {
            username: "Steve".to_string(),
            uuid: Uuid::new_v4(),
            current_server: "lobby".to_string(),
            action_tx: tx,
            client_ip: "127.0.0.1".to_string(),
            protocol_version: 776,
            client_brand: None,
            locale: None,
            view_distance: None,
        });

        let source = ProxyCommandSource {
            sender: CommandSender::Console,
            config: Arc::new(Config::default()),
            session_manager,
        };

        assert!(
            dispatcher
                .execute_input("stop &eScheduled maintenance", &source)
                .is_ok()
        );
        assert_eq!(
            rx.recv().await,
            Some(PlayerAction::Disconnect(
                "§eScheduled maintenance".to_string()
            ))
        );
        assert!(shutdown_rx.try_recv().is_ok());
    }
}
