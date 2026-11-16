use std::time::{SystemTime, UNIX_EPOCH};

use pumpkin_command::Requirement;
use pumpkin_command::argument_builder::{ArgumentBuilder, argument, command};
use pumpkin_command::argument_types::core::string::StringArgumentType;
use pumpkin_command::context::command_context::CommandContext;
use pumpkin_command::dispatcher::CommandDispatcher as RawCommandDispatcher;
use pumpkin_command::node::{CommandExecutor, CommandExecutorResult};
use pumpkin_command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use pumpkin_command::suggestion::suggestions::SuggestionsBuilder;
use pumpkin_util::version::JavaMinecraftVersion;

use super::{ProxyCommandSource, command_error, format_duration_secs};

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

struct FindCommandExecutor;

impl CommandExecutor<ProxyCommandSource> for FindCommandExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.find") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.find).",
            ));
        }

        let player_name = StringArgumentType::get(context, "player")?;
        let session = context
            .source
            .session_manager
            .get_session(player_name)
            .ok_or_else(|| command_error(format!("Player '{player_name}' is not online.")))?;

        let mc_ver = JavaMinecraftVersion::from_protocol(session.protocol_version);
        let version_label = if mc_ver == JavaMinecraftVersion::Unknown {
            format!("Protocol {}", session.protocol_version)
        } else {
            format!("{mc_ver} (proto {})", session.protocol_version)
        };

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let joined_at = context
            .source
            .session_manager
            .telemetry_sessions()
            .joined_at(session.uuid);
        let online_for = format_duration_secs(now_secs.saturating_sub(joined_at));

        let brand = session.client_brand.as_deref().unwrap_or("unknown");
        let locale = session.locale.as_deref().unwrap_or("unknown");
        let view_dist = session
            .view_distance
            .map_or_else(|| "unknown".to_string(), |vd| format!("{vd} chunks"));

        let info_msg = format!(
            "§6[Vine] §fPlayer §a{} §7({})\n  §7Server: §e{}\n  §7Address: §f{}\n  §7Version: §f{}\n  §7Client: §f{} §8(locale: {}, view: {})\n  §7Online for: §f{}",
            session.username,
            session.uuid,
            session.current_server,
            session.client_ip,
            version_label,
            brand,
            locale,
            view_dist,
            online_for,
        );

        context
            .source
            .send_message(crate::console::legacy_component(info_msg));
        Ok(1)
    }
}

pub fn register(dispatcher: &mut RawCommandDispatcher<ProxyCommandSource>) {
    dispatcher.register(
        command("find", "Locates an online player and shows session details")
            .requires(Requirement::from("vine.command.find"))
            .then(
                argument("player", StringArgumentType::SingleWord)
                    .suggests(OnlinePlayerSuggestionProvider)
                    .executes(FindCommandExecutor),
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
    use crate::session::{PlayerAction, PlayerSession, SessionManager};

    #[tokio::test]
    async fn find_reports_online_player_details_and_errors_for_offline() {
        let session_manager = Arc::new(SessionManager::new());
        let (p_tx, _p_rx) = mpsc::unbounded_channel();
        let uuid = Uuid::new_v4();
        session_manager.register(PlayerSession {
            username: "Steve".to_string(),
            uuid,
            current_server: "survival".to_string(),
            action_tx: p_tx,
            client_ip: "198.51.100.12:54321".to_string(),
            protocol_version: 776,
            client_brand: Some("fabric".to_string()),
            locale: Some("en_us".to_string()),
            view_distance: Some(12),
        });

        let mut admin_cfg = Config::default();
        admin_cfg.permissions.users.insert(
            "AdminUser".to_string(),
            crate::config::UserPermissionConfig::Groups(vec!["admin".to_string()]),
        );

        let (admin_tx, mut admin_rx) = mpsc::unbounded_channel();
        let source = ProxyCommandSource {
            sender: CommandSender::Player {
                username: "AdminUser".to_string(),
                action_tx: admin_tx,
            },
            config: Arc::new(admin_cfg),
            session_manager,
        };

        let dispatcher = CommandDispatcher::new();
        assert!(dispatcher.execute_input("find Steve", &source).is_ok());

        match admin_rx.recv().await.unwrap() {
            PlayerAction::Message(msg) => {
                let s = msg.to_pretty_console();
                assert!(s.contains("Steve"));
                assert!(s.contains(&uuid.to_string()));
                assert!(s.contains("survival"));
                assert!(s.contains("198.51.100.12:54321"));
                assert!(s.contains("fabric"));
                assert!(s.contains("en_us"));
            }
            other => panic!("Expected Message action, got {other:?}"),
        }

        assert!(dispatcher.execute_input("find Nobody", &source).is_err());
    }
}
