use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pumpkin_command::Requirement;
use pumpkin_command::argument_builder::{ArgumentBuilder, argument, command, literal};
use pumpkin_command::argument_types::core::string::StringArgumentType;
use pumpkin_command::context::command_context::CommandContext;
use pumpkin_command::dispatcher::CommandDispatcher as RawCommandDispatcher;
use pumpkin_command::node::{CommandExecutor, CommandExecutorResult};
use pumpkin_util::text::click::ClickEvent;
use pumpkin_util::text::hover::HoverEvent;
use pumpkin_util::text::{TextComponent, color::NamedColor};
use pumpkin_util::version::JavaMinecraftVersion;
use serde::{Deserialize, Serialize};

use super::{
    CommandRuntime, CommandSender, ProxyCommandSource, command_error, format_duration_secs,
    strip_formatting_codes,
};
use crate::telemetry::{
    HEADER_PUBLIC_KEY, HEADER_SIGNATURE, HEADER_TIMESTAMP, TelemetryClient,
    build_heartbeat_payload, resolve_identity_key, sign_telemetry_payload,
};

const DEFAULT_ACCOUNTS_URL: &str = "https://accounts.pumpkinmc.org";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct ServerLinkInitRequest {
    server_name: String,
    server_type: String,
    server_version: String,
    minecraft_version: String,
    os: String,
    arch: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ServerLinkInitResponse {
    code: String,
    link_url: String,
    expires_in_secs: u64,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    error: String,
}

fn resolve_accounts_base_url() -> String {
    std::env::var("PUMPKIN_ACCOUNTS_URL")
        .ok()
        .map(|val| val.trim().trim_end_matches('/').to_string())
        .filter(|val| !val.is_empty())
        .unwrap_or_else(|| DEFAULT_ACCOUNTS_URL.to_string())
}

fn resolve_link_server_name(
    custom_name: Option<&str>,
    configured_name: Option<&str>,
    motd: Option<&str>,
) -> String {
    let cleaned_motd = motd.map(strip_formatting_codes);
    custom_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| configured_name.map(str::trim).filter(|s| !s.is_empty()))
        .or_else(|| {
            cleaned_motd
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("Vine Proxy")
        .to_string()
}

fn detect_host_os() -> String {
    sysinfo::System::long_os_version().unwrap_or_else(|_| {
        let name = sysinfo::System::name().unwrap_or_else(|_| std::env::consts::OS.to_string());
        sysinfo::System::os_version().map_or(name.clone(), |ver| format!("{name} {ver}"))
    })
}

const fn is_console_only(source: &ProxyCommandSource) -> bool {
    matches!(source.sender, CommandSender::Console)
}

fn format_link_success_message(
    resp: &ServerLinkInitResponse,
    telemetry_enabled: bool,
) -> TextComponent {
    let expires_mins = resp.expires_in_secs.div_ceil(60);
    let mut msg = TextComponent::text("\nPumpkin Account Proxy Link\n")
        .color_named(NamedColor::Gold)
        .bold();

    msg = msg.add_child(TextComponent::text("  Code: ").color_named(NamedColor::Gray));
    msg = msg.add_child(
        TextComponent::text(resp.code.clone())
            .color_named(NamedColor::Aqua)
            .bold()
            .click_event(ClickEvent::CopyToClipboard {
                value: Cow::from(resp.code.clone()),
            })
            .hover_event(HoverEvent::show_text(TextComponent::text(
                "Click to copy link code",
            ))),
    );
    msg = msg.add_child(
        TextComponent::text(format!(" (expires in {expires_mins}m)\n"))
            .color_named(NamedColor::DarkGray),
    );

    msg = msg.add_child(TextComponent::text("  Claim: ").color_named(NamedColor::Gray));
    msg = msg.add_child(
        TextComponent::text(resp.link_url.clone())
            .color_named(NamedColor::Green)
            .bold()
            .underlined()
            .click_event(ClickEvent::OpenUrl {
                url: Cow::from(resp.link_url.clone()),
            })
            .hover_event(HoverEvent::show_text(TextComponent::text(
                "Click to open Pumpkin Account and claim this proxy",
            ))),
    );

    if !telemetry_enabled {
        msg = msg.add_child(
            TextComponent::text(
                "\n  Note: Telemetry is disabled in vine.toml ([telemetry] enabled = false). Enable it to see live stats and session analytics.",
            )
            .color_named(NamedColor::Yellow),
        );
    }

    msg
}

async fn request_server_link(
    server_name: String,
    minecraft_version: String,
) -> Result<ServerLinkInitResponse, String> {
    let signing_key = resolve_identity_key();
    let payload = ServerLinkInitRequest {
        server_name,
        server_type: "vine".to_string(),
        server_version: CARGO_PKG_VERSION.to_string(),
        minecraft_version,
        os: detect_host_os(),
        arch: std::env::consts::ARCH.to_string(),
    };

    let body_bytes =
        serde_json::to_vec(&payload).map_err(|e| format!("Failed to serialize payload: {e}"))?;
    let timestamp_str = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let (pubkey_hex, sig_hex) = sign_telemetry_payload(&signing_key, &timestamp_str, &body_bytes);

    let user_agent = format!("Vine-Proxy/{CARGO_PKG_VERSION}");
    let client = pumpkin_auth::client_builder()
        .user_agent(&user_agent)
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

    let init_url = format!("{}/api/user/servers/link/init", resolve_accounts_base_url());
    let response = client
        .post(&init_url)
        .header("Content-Type", "application/json")
        .header(HEADER_PUBLIC_KEY, pubkey_hex)
        .header(HEADER_SIGNATURE, sig_hex)
        .header(HEADER_TIMESTAMP, timestamp_str)
        .body(body_bytes)
        .send()
        .await
        .map_err(|e| format!("Could not reach Pumpkin Account ({init_url}): {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let detail = response
            .json::<ApiErrorBody>()
            .await
            .map_or_else(|_| format!("HTTP {status}"), |b| b.error);
        return Err(detail);
    }

    response
        .json::<ServerLinkInitResponse>()
        .await
        .map_err(|e| format!("Invalid response from Pumpkin Account: {e}"))
}

struct VineInfoExecutor {
    runtime: Arc<CommandRuntime>,
}

impl CommandExecutor<ProxyCommandSource> for VineInfoExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.vine") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.vine).",
            ));
        }

        let cfg = &context.source.config;
        let online = context.source.session_manager.count();
        let plugin_count = self.runtime.loaded_plugins().len();
        let uptime = format_duration_secs(self.runtime.uptime_secs());

        let bedrock_info = if cfg.bedrock.enabled {
            format!(
                "enabled (NetherNet {} / proto {} on {})",
                cfg.bedrock.version_name, cfg.bedrock.protocol_version, cfg.bedrock.bind_address
            )
        } else {
            "disabled".to_string()
        };

        let telemetry_info = if cfg.telemetry.enabled {
            if cfg.telemetry.public {
                "enabled (public directory)"
            } else {
                "enabled (anonymous)"
            }
        } else {
            "disabled"
        };

        let msg = format!(
            "§a§lVine Proxy §fv{CARGO_PKG_VERSION}\n  §7Java Support: §f1.7.2 - 26.2 (MOTD proto {})\n  §7Bedrock Support: §f{bedrock_info}\n  §7Players: §a{online}§7/§f{}\n  §7Backends: §e{} §7(default: '{}')\n  §7Plugins: §e{plugin_count} loaded\n  §7Telemetry: §f{telemetry_info}\n  §7Uptime: §f{uptime}",
            cfg.motd.protocol_version,
            cfg.motd.max_players,
            cfg.servers.len(),
            cfg.routing.default_server,
        );
        context.source.send_message(TextComponent::text(msg));
        Ok(1)
    }
}

struct VinePluginsExecutor {
    runtime: Arc<CommandRuntime>,
}

impl CommandExecutor<ProxyCommandSource> for VinePluginsExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.vine") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.vine).",
            ));
        }

        let plugins = self.runtime.loaded_plugins();
        if plugins.is_empty() {
            context.source.send_message(TextComponent::text(
                "§6[Vine] §7No WASM plugins are currently loaded.",
            ));
            return Ok(0);
        }

        let mut lines = vec![format!("§6[Vine] §fLoaded plugins ({}):", plugins.len())];
        for meta in &plugins {
            let authors = if meta.authors.is_empty() {
                String::new()
            } else {
                format!(" §8by {}", meta.authors.join(", "))
            };
            let desc = meta
                .description
                .as_deref()
                .filter(|d| !d.trim().is_empty())
                .map_or_else(String::new, |d| format!(" §7- {d}"));
            lines.push(format!(
                "  §a{} §fv{}{}{}",
                meta.name, meta.version, authors, desc
            ));
        }

        context
            .source
            .send_message(TextComponent::text(lines.join("\n")));
        Ok(plugins.len() as i32)
    }
}

struct VineServersExecutor;

impl CommandExecutor<ProxyCommandSource> for VineServersExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.vine") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.vine).",
            ));
        }

        let cfg = &context.source.config;
        let sessions = context.source.session_manager.list_sessions();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for s in &sessions {
            *counts.entry(s.current_server.as_str()).or_default() += 1;
        }

        let mut names: Vec<&String> = cfg.servers.keys().collect();
        names.sort();

        let mut lines = vec![format!(
            "§6[Vine] §fConfigured backend servers ({}):",
            names.len()
        )];
        for name in names {
            if let Some(backend) = cfg.servers.get(name) {
                let online = counts.get(name.as_str()).copied().unwrap_or(0);
                let mut tags = Vec::new();
                if cfg.routing.default_server.eq_ignore_ascii_case(name) {
                    tags.push("default");
                }
                if cfg
                    .routing
                    .fallback_server
                    .as_deref()
                    .is_some_and(|fb| fb.eq_ignore_ascii_case(name))
                {
                    tags.push("fallback");
                }
                let tag_suffix = if tags.is_empty() {
                    String::new()
                } else {
                    format!(" §e[{}]", tags.join(", "))
                };
                lines.push(format!(
                    "  §a{name} §7-> §f{} §8(forwarding: {:?}, online: {online}){tag_suffix}",
                    backend.address, backend.forwarding
                ));
            }
        }

        context
            .source
            .send_message(TextComponent::text(lines.join("\n")));
        Ok(cfg.servers.len() as i32)
    }
}

struct VineReloadExecutor {
    runtime: Arc<CommandRuntime>,
}

impl CommandExecutor<ProxyCommandSource> for VineReloadExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        if !context.source.has_permission("vine.command.reload") {
            return Err(command_error(
                "You do not have permission to execute this command (required: vine.command.reload).",
            ));
        }

        match self.runtime.reload_config() {
            Ok(new_cfg) => {
                context.source.send_message(TextComponent::text(format!(
                    "§6[Vine] §aConfiguration reloaded successfully ({} backend server(s), default: '{}').",
                    new_cfg.servers.len(),
                    new_cfg.routing.default_server
                )));
                Ok(1)
            }
            Err(err) => Err(command_error(format!(
                "Failed to reload configuration: {err}"
            ))),
        }
    }
}

fn execute_link(context: &CommandContext<ProxyCommandSource>, custom_name: Option<&str>) -> i32 {
    if !is_console_only(&context.source) {
        context.source.send_message(
            TextComponent::text(
                "For security, `vine link` can only be executed from the proxy console.",
            )
            .color_named(NamedColor::Red),
        );
        return 0;
    }

    let cfg = &context.source.config;
    let server_name = resolve_link_server_name(
        custom_name,
        cfg.telemetry.server_name.as_deref(),
        Some(cfg.motd.line1.as_str()),
    );

    let mc_ver = JavaMinecraftVersion::from_protocol(cfg.motd.protocol_version as u32);
    let minecraft_version = if mc_ver == JavaMinecraftVersion::Unknown {
        "26.2".to_string()
    } else {
        mc_ver.to_string()
    };

    let telemetry_enabled = cfg.telemetry.enabled;
    let immediate_heartbeat = telemetry_enabled.then(|| {
        (
            cfg.telemetry.endpoint.clone(),
            build_heartbeat_payload(cfg, context.source.session_manager.count()),
        )
    });

    let Ok(rt_handle) = tokio::runtime::Handle::try_current() else {
        context.source.send_message(
            TextComponent::text("Unable to start async runtime for /vine link.")
                .color_named(NamedColor::Red),
        );
        return 0;
    };

    context.source.send_message(
        TextComponent::text("Generating Pumpkin Account proxy link code...")
            .color_named(NamedColor::Gray),
    );

    let source = context.source.clone();
    rt_handle.spawn(async move {
        if let Some((endpoint, hb_payload)) = immediate_heartbeat
            && let Ok(http_client) = pumpkin_auth::client_builder()
                .user_agent(format!("Vine-Proxy/{CARGO_PKG_VERSION}"))
                .timeout(HTTP_TIMEOUT)
                .build()
        {
            let client = TelemetryClient::new(resolve_identity_key(), http_client, endpoint);
            let _ = client.send_heartbeat(&hb_payload).await;
        }

        match request_server_link(server_name, minecraft_version).await {
            Ok(resp) => {
                source.send_message(format_link_success_message(&resp, telemetry_enabled));
            }
            Err(err) => {
                source.send_message(
                    TextComponent::text(format!("Failed to generate proxy link code: {err}"))
                        .color_named(NamedColor::Red),
                );
            }
        }
    });

    1
}

struct VineLinkExecutor;

impl CommandExecutor<ProxyCommandSource> for VineLinkExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        Ok(execute_link(context, None))
    }
}

struct VineLinkNamedExecutor;

impl CommandExecutor<ProxyCommandSource> for VineLinkNamedExecutor {
    fn execute(&self, context: &CommandContext<ProxyCommandSource>) -> CommandExecutorResult {
        let name = StringArgumentType::get(context, "name")?;
        Ok(execute_link(context, Some(name)))
    }
}

pub fn register(
    dispatcher: &mut RawCommandDispatcher<ProxyCommandSource>,
    runtime: Arc<CommandRuntime>,
) {
    dispatcher.register(
        command("vine", "Displays Vine proxy information and admin tools")
            .requires(Requirement::from("vine.command.vine"))
            .executes(VineInfoExecutor {
                runtime: runtime.clone(),
            })
            .then(literal("info").executes(VineInfoExecutor {
                runtime: runtime.clone(),
            }))
            .then(literal("version").executes(VineInfoExecutor {
                runtime: runtime.clone(),
            }))
            .then(literal("plugins").executes(VinePluginsExecutor {
                runtime: runtime.clone(),
            }))
            .then(literal("servers").executes(VineServersExecutor))
            .then(
                literal("reload")
                    .requires(Requirement::from("vine.command.reload"))
                    .executes(VineReloadExecutor { runtime }),
            )
            .then(
                literal("link")
                    .requires(is_console_only)
                    .executes(VineLinkExecutor)
                    .then(
                        argument("name", StringArgumentType::GreedyPhrase)
                            .executes(VineLinkNamedExecutor),
                    ),
            ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    use crate::command::CommandDispatcher;
    use crate::config::Config;
    use crate::session::{PlayerAction, SessionManager};

    #[test]
    fn link_server_name_precedence_and_motd_stripping() {
        assert_eq!(
            resolve_link_server_name(Some("  My Network "), Some("Telemetry"), Some("§aMOTD")),
            "My Network"
        );
        assert_eq!(
            resolve_link_server_name(Some("   "), Some("Telemetry Hub"), Some("§aMOTD")),
            "Telemetry Hub"
        );
        assert_eq!(
            resolve_link_server_name(None, None, Some("§a§lVine §7» §fProxy")),
            "Vine » Proxy"
        );
        assert_eq!(
            resolve_link_server_name(None, Some(""), Some("")),
            "Vine Proxy"
        );
    }

    #[test]
    fn link_payload_signing_verifies_with_telemetry_verifier() {
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let req = ServerLinkInitRequest {
            server_name: "Vine Hub 1".to_string(),
            server_type: "vine".to_string(),
            server_version: CARGO_PKG_VERSION.to_string(),
            minecraft_version: "26.2".to_string(),
            os: "Linux".to_string(),
            arch: "x86_64".to_string(),
        };
        let body = serde_json::to_vec(&req).unwrap();
        let ts = "1700000000";
        let (pub_hex, sig_hex) = sign_telemetry_payload(&signing_key, ts, &body);

        assert!(
            crate::telemetry::verify_telemetry_request(
                &pub_hex,
                &sig_hex,
                ts,
                &body,
                1_700_000_010
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn vine_subcommands_and_reload_work() {
        let dispatcher = CommandDispatcher::new();
        dispatcher.runtime().set_reload_handler(Arc::new(|| {
            let mut cfg = Config::default();
            cfg.routing.default_server = "survival".to_string();
            Ok(Arc::new(cfg))
        }));

        let (tx, mut rx) = mpsc::unbounded_channel();
        let admin_player_source = ProxyCommandSource {
            sender: CommandSender::Player {
                username: "Alex".to_string(),
                action_tx: tx,
            },
            config: Arc::new(Config::default()),
            session_manager: Arc::new(SessionManager::new()),
        };

        assert!(
            dispatcher
                .execute_input("vine", &admin_player_source)
                .is_ok()
        );
        assert!(
            dispatcher
                .execute_input("vine servers", &admin_player_source)
                .is_ok()
        );
        assert!(
            dispatcher
                .execute_input("vine plugins", &admin_player_source)
                .is_ok()
        );
        assert!(
            dispatcher
                .execute_input("vine reload", &admin_player_source)
                .is_ok()
        );

        let mut received = Vec::new();
        while let Ok(PlayerAction::Message(m)) = rx.try_recv() {
            received.push(m.to_pretty_console());
        }
        assert!(received.iter().any(|m| m.contains("Vine Proxy")));
        assert!(
            received
                .iter()
                .any(|m| m.contains("Configured backend servers"))
        );
        assert!(
            received.iter().any(
                |m| m.contains("Configuration reloaded successfully") && m.contains("survival")
            )
        );

        // Players cannot execute `vine link` (console-only)
        assert!(!is_console_only(&admin_player_source));
        assert!(
            dispatcher
                .execute_input("vine link", &admin_player_source)
                .is_err()
        );
    }
}
