use pumpkin_command::Requirement;
use pumpkin_command::argument_builder::{ArgumentBuilder, argument, command};
use pumpkin_command::argument_types::core::string::StringArgumentType;
use pumpkin_command::context::command_context::CommandContext;
use pumpkin_command::dispatcher::CommandDispatcher as RawCommandDispatcher;
use pumpkin_command::node::{CommandExecutor, CommandExecutorResult};
use pumpkin_command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use pumpkin_command::suggestion::suggestions::SuggestionsBuilder;
use tracing::info;

use super::{ProxyCommandSource, command_error, translate_color_codes};
use crate::session::PlayerAction;

const DEFAULT_KICK_REASON: &str = "Kicked from proxy.";

struct OnlinePlayerSuggestionProvider;

impl SuggestionProvider<ProxyCommandSource> for OnlinePlayerSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext<ProxyCommandSource>,
        builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult {
        let mut builder = builder;
        let remaining = builder.remaining_lowercase().to_string();
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

fn execute_kick(
    context: &CommandContext<ProxyCommandSource>,
    raw_reason: Option<&str>,
) -> CommandExecutorResult {
    if !context.source.has_permission("vine.command.kick") {
        return Err(command_error(
            "You do not have permission to execute this command (required: vine.command.kick).",
        ));
    }

    let player_name = StringArgumentType::get(context, "player")?;
    let session = context
        .source
        .session_manager
        .get_session(player_name)
        .ok_or_else(|| command_error(format!("Player '{player_name}' is not online.")))?;

    let reason = raw_reason
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map_or_else(|| DEFAULT_KICK_REASON.to_string(), translate_color_codes);

    let _ = session
        .action_tx
        .send(PlayerAction::Disconnect(reason.clone()));
    info!(
        "Player '{}' was kicked from proxy by {} (reason: {})",
        session.username,
        context.source.sender.name(),
        crate::console::format_console_text(&reason)
    );

    context
        .source
        .send_message(crate::console::legacy_component(format!(
            "Kicked '{}' from the proxy ({reason}§r).",
            session.username
        )));
    Ok(1)
}

struct KickDefaultExecutor;

impl CommandExecutor<ProxyCommandSource> for KickDefaultExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        execute_kick(context, None)
    }
}

struct KickReasonExecutor;

impl CommandExecutor<ProxyCommandSource> for KickReasonExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        let reason = StringArgumentType::get(context, "reason")?;
        execute_kick(context, Some(reason))
    }
}

pub fn register(dispatcher: &mut RawCommandDispatcher<ProxyCommandSource>) {
    dispatcher.register(
        command("kick", "Disconnects an online player from the proxy")
            .requires(Requirement::from("vine.command.kick"))
            .then(
                argument("player", StringArgumentType::SingleWord)
                    .suggests(OnlinePlayerSuggestionProvider)
                    .executes(KickDefaultExecutor)
                    .then(
                        argument("reason", StringArgumentType::GreedyPhrase)
                            .executes(KickReasonExecutor),
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
    use crate::config::Config;
    use crate::session::{PlayerSession, SessionManager};

    #[tokio::test]
    async fn kick_disconnects_target_with_default_and_custom_reason() {
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

        let dispatcher = CommandDispatcher::new();
        assert!(dispatcher.execute_input("kick Steve", &source).is_ok());
        assert_eq!(
            rx.recv().await,
            Some(PlayerAction::Disconnect(DEFAULT_KICK_REASON.to_string()))
        );

        assert!(
            dispatcher
                .execute_input("kick Steve &cRule violation", &source)
                .is_ok()
        );
        assert_eq!(
            rx.recv().await,
            Some(PlayerAction::Disconnect("§cRule violation".to_string()))
        );
    }
}
