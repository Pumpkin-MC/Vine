use std::fmt::Write as _;

use pumpkin_util::text::click::ClickEvent;
use pumpkin_util::text::color::{Color, NamedColor, RGBColor};
use pumpkin_util::text::style::Style;
use pumpkin_util::text::{TextComponent, TextComponentBase, TextContent};
use pumpkin_util::translation::{Locale, get_translation, reorder_substitutions};

/// Determines whether ANSI color and style escape sequences should be emitted to console logs.
///
/// Matches `tracing-subscriber`'s ANSI behavior while honoring `CLICOLOR_FORCE`, `FORCE_COLOR`,
/// `NO_COLOR`, and `CLICOLOR`.
#[must_use]
pub fn console_ansi_enabled() -> bool {
    resolve_ansi_enabled(
        std::env::var("CLICOLOR_FORCE").ok().as_deref(),
        std::env::var("FORCE_COLOR").ok().as_deref(),
        std::env::var("NO_COLOR").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
        std::env::var("CLICOLOR").ok().as_deref(),
    )
}

#[must_use]
fn resolve_ansi_enabled(
    clicolor_force: Option<&str>,
    force_color: Option<&str>,
    no_color: Option<&str>,
    _term: Option<&str>,
    clicolor: Option<&str>,
) -> bool {
    if clicolor_force.is_some_and(|v| !v.is_empty() && v != "0") {
        return true;
    }
    if force_color.is_some_and(|v| !v.is_empty() && v != "0") {
        return true;
    }
    if no_color.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if clicolor.is_some_and(|v| v == "0") {
        return false;
    }
    true
}

/// Translates legacy `&` formatting codes (`&0`-`&9`, `&a`-`&f`, `&k`-`&o`, `&r`, `&x`, and
/// hex shorthand `&#RRGGBB`) into Minecraft `§` formatting codes.
#[must_use]
pub fn translate_color_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '&' {
            if let Some(&next) = chars.peek() {
                if next == '#' {
                    let mut clone = chars.clone();
                    let _ = clone.next(); // '#'
                    let mut hex_chars = ['\0'; 6];
                    let mut valid = true;
                    for slot in &mut hex_chars {
                        if let Some(hc) = clone.next()
                            && hc.is_ascii_hexdigit()
                        {
                            *slot = hc.to_ascii_lowercase();
                        } else {
                            valid = false;
                            break;
                        }
                    }
                    if valid {
                        chars = clone;
                        out.push_str("§x");
                        for hc in hex_chars {
                            out.push('§');
                            out.push(hc);
                        }
                        continue;
                    }
                } else if matches!(
                    next.to_ascii_lowercase(),
                    '0'..='9' | 'a'..='f' | 'k'..='o' | 'r' | 'x'
                ) {
                    chars.next();
                    out.push('§');
                    out.push(next.to_ascii_lowercase());
                    continue;
                }
            }
            out.push(ch);
        } else {
            out.push(ch);
        }
    }
    out
}

/// Strips Minecraft `§` color and formatting codes from a string.
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

/// Builds a [`TextComponent`] from a string, parsing any legacy Minecraft `§` formatting codes
/// into structured component styles and colors.
#[must_use]
pub fn legacy_component(input: impl AsRef<str>) -> TextComponent {
    let s = input.as_ref();
    if s.contains('§') {
        TextComponent::from_legacy_string(s)
    } else {
        TextComponent::text(s.to_string())
    }
}

/// Formats a [`TextComponent`] for console output, rendering structured component styles and any
/// embedded Minecraft `§` codes as ANSI escape sequences when console colors are enabled.
#[must_use]
pub fn format_console_component(component: &TextComponent) -> String {
    format_console_component_with_ansi(component, console_ansi_enabled())
}

/// Formats a [`TextComponent`] for console output with explicit ANSI toggle control.
#[must_use]
pub fn format_console_component_with_ansi(component: &TextComponent, ansi: bool) -> String {
    let mut out = String::new();
    render_component_base(&component.0, &EffectiveStyle::default(), ansi, &mut out);
    out
}

/// Formats a string containing legacy Minecraft `§` color/formatting codes for console output.
#[must_use]
pub fn format_console_text(input: &str) -> String {
    format_console_text_with_ansi(input, console_ansi_enabled())
}

/// Formats a string containing legacy Minecraft `§` color/formatting codes with explicit ANSI control.
#[must_use]
pub fn format_console_text_with_ansi(input: &str, ansi: bool) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    render_legacy_str(input, &EffectiveStyle::default(), ansi, &mut out);
    out
}

#[derive(Clone, Default)]
struct EffectiveStyle {
    color: Option<Color>,
    bold: bool,
    italic: bool,
    underlined: bool,
    strikethrough: bool,
    link_url: Option<String>,
}

impl EffectiveStyle {
    fn from_component_style(style: &Style, parent: &Self) -> Self {
        let color = match style.color {
            Some(Color::Reset) => None,
            Some(c) => Some(c),
            None => parent.color,
        };
        let bold = style.bold.unwrap_or(parent.bold);
        let italic = style.italic.unwrap_or(parent.italic);
        let underlined = style.underlined.unwrap_or(parent.underlined);
        let strikethrough = style.strikethrough.unwrap_or(parent.strikethrough);
        let link_url = match style.click_event.as_ref() {
            Some(ClickEvent::OpenUrl { url }) if is_safe_osc8_url(url) => Some(url.to_string()),
            Some(ClickEvent::OpenFile { path }) => {
                let candidate = format!("file://{path}");
                is_safe_osc8_url(&candidate).then_some(candidate)
            }
            _ => parent.link_url.clone(),
        };

        Self {
            color,
            bold,
            italic,
            underlined,
            strikethrough,
            link_url,
        }
    }

    fn write_sgr(&self, out: &mut String) -> bool {
        let mut codes = String::new();

        if self.bold {
            push_sgr_param(&mut codes, "1");
        }
        if self.italic {
            push_sgr_param(&mut codes, "3");
        }
        if self.underlined {
            push_sgr_param(&mut codes, "4");
        }
        if self.strikethrough {
            push_sgr_param(&mut codes, "9");
        }

        match self.color {
            Some(Color::Named(named)) => {
                push_sgr_param(&mut codes, named_color_sgr(named));
            }
            Some(Color::Rgb(RGBColor { red, green, blue })) => {
                if !codes.is_empty() {
                    codes.push(';');
                }
                let _ = write!(codes, "38;2;{red};{green};{blue}");
            }
            Some(Color::Reset) | None => {}
        }

        if codes.is_empty() {
            false
        } else {
            out.push_str("\x1b[");
            out.push_str(&codes);
            out.push('m');
            true
        }
    }
}

fn push_sgr_param(buf: &mut String, param: &str) {
    if !buf.is_empty() {
        buf.push(';');
    }
    buf.push_str(param);
}

const fn named_color_sgr(color: NamedColor) -> &'static str {
    match color {
        NamedColor::Black => "30",
        NamedColor::DarkBlue => "34",
        NamedColor::DarkGreen => "32",
        NamedColor::DarkAqua => "36",
        NamedColor::DarkRed => "31",
        NamedColor::DarkPurple => "35",
        NamedColor::Gold => "33",
        NamedColor::Gray => "37",
        NamedColor::DarkGray => "90",
        NamedColor::Blue => "94",
        NamedColor::Green => "92",
        NamedColor::Aqua => "96",
        NamedColor::Red => "91",
        NamedColor::LightPurple => "95",
        NamedColor::Yellow => "93",
        NamedColor::White => "97",
    }
}

fn is_safe_osc8_url(url: &str) -> bool {
    !url.is_empty() && !url.chars().any(char::is_control)
}

fn emit_styled_segment(segment: &str, style: &EffectiveStyle, ansi: bool, out: &mut String) {
    if segment.is_empty() {
        return;
    }

    if !ansi {
        out.push_str(segment);
        return;
    }

    for (idx, line) in segment.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            continue;
        }
        if let Some(ref url) = style.link_url {
            out.push_str("\x1b]8;;");
            out.push_str(url);
            out.push_str("\x1b\\");
        }
        let has_sgr = style.write_sgr(out);
        out.push_str(line);
        if has_sgr {
            out.push_str("\x1b[0m");
        }
        if style.link_url.is_some() {
            out.push_str("\x1b]8;;\x1b\\");
        }
    }
}

fn render_legacy_str(input: &str, base_style: &EffectiveStyle, ansi: bool, out: &mut String) {
    let mut cur = base_style.clone();
    let mut segment = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '§' {
            let Some(code_raw) = chars.next() else {
                break;
            };
            emit_styled_segment(&segment, &cur, ansi, out);
            segment.clear();

            let code = code_raw.to_ascii_lowercase();
            match code {
                '0'..='9' | 'a'..='f' => {
                    cur.color = Color::from_legacy_code(code);
                    cur.bold = false;
                    cur.italic = false;
                    cur.underlined = false;
                    cur.strikethrough = false;
                }
                'x' => {
                    let mut clone = chars.clone();
                    let mut hex = [0u8; 6];
                    let mut valid = true;
                    for slot in &mut hex {
                        if clone.next() == Some('§')
                            && let Some(hc) = clone.next()
                            && hc.is_ascii_hexdigit()
                        {
                            *slot = hc.to_ascii_lowercase() as u8;
                        } else {
                            valid = false;
                            break;
                        }
                    }
                    if valid && let Ok(hex_str) = std::str::from_utf8(&hex) {
                        chars = clone;
                        cur.color = Color::from_hex_str(hex_str);
                        cur.bold = false;
                        cur.italic = false;
                        cur.underlined = false;
                        cur.strikethrough = false;
                    }
                }
                'l' => cur.bold = true,
                'o' => cur.italic = true,
                'n' => cur.underlined = true,
                'm' => cur.strikethrough = true,
                'k' => {}
                'r' => {
                    cur = EffectiveStyle {
                        link_url: base_style.link_url.clone(),
                        ..EffectiveStyle::default()
                    };
                }
                _ => {}
            }
        } else if ch == '\n' || ch == '\t' || !ch.is_control() {
            segment.push(ch);
        }
    }

    emit_styled_segment(&segment, &cur, ansi, out);
}

fn render_translation(
    namespaced_key: &str,
    locale: Locale,
    with: &[TextComponentBase],
    style: &EffectiveStyle,
    ansi: bool,
    out: &mut String,
) {
    let translation = get_translation(namespaced_key, locale);
    if with.is_empty() || !translation.contains('%') {
        render_legacy_str(&translation, style, ansi, out);
        return;
    }

    let (substitutions, indices) = reorder_substitutions(&translation, with.to_vec());
    let mut pos = 0;

    for (idx, range) in indices.iter().enumerate() {
        render_legacy_str(&translation[pos..range.start], style, ansi, out);
        if !substitutions.is_empty() {
            let sub_idx = idx.min(substitutions.len() - 1);
            render_component_base(&substitutions[sub_idx], style, ansi, out);
        }
        pos = range.end + 1;
    }

    render_legacy_str(&translation[pos..], style, ansi, out);
}

fn render_component_base(
    base: &TextComponentBase,
    inherited: &EffectiveStyle,
    ansi: bool,
    out: &mut String,
) {
    let effective = EffectiveStyle::from_component_style(&base.style, inherited);

    match &*base.content {
        TextContent::Text { text } => {
            render_legacy_str(text, &effective, ansi, out);
        }
        TextContent::Translate {
            translate, with, ..
        } => {
            let key = format!("minecraft:{translate}");
            render_translation(&key, Locale::EnUs, with, &effective, ansi, out);
        }
        TextContent::EntityNames { selector, .. } => {
            render_legacy_str(selector, &effective, ansi, out);
        }
        TextContent::Keybind { keybind } => {
            render_legacy_str(keybind, &effective, ansi, out);
        }
        TextContent::Custom { key, with, locale } => {
            render_translation(key, *locale, with, &effective, ansi, out);
        }
        TextContent::PlayerSprite { profile, .. } => {
            let label = profile.0.get_string("name").unwrap_or("player_sprite");
            render_legacy_str(label, &effective, ansi, out);
        }
    }

    for child in &base.extra {
        render_component_base(child, &effective, ansi, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_resolution_respects_standard_env_vars() {
        assert!(resolve_ansi_enabled(None, None, None, None, None));
        assert!(resolve_ansi_enabled(None, None, None, Some("dumb"), None));
        assert!(!resolve_ansi_enabled(None, None, Some("1"), None, None));
        assert!(resolve_ansi_enabled(None, None, Some(""), None, None));
        assert!(!resolve_ansi_enabled(None, None, None, None, Some("0")));
        assert!(resolve_ansi_enabled(
            Some("1"),
            None,
            Some("1"),
            None,
            Some("0")
        ));
        assert!(resolve_ansi_enabled(
            None,
            Some("true"),
            Some("1"),
            None,
            None
        ));
    }

    #[test]
    fn translate_and_strip_color_codes_support_legacy_and_hex() {
        assert_eq!(
            translate_color_codes("&cAlert &7&lNow & Later"),
            "§cAlert §7§lNow & Later"
        );
        assert_eq!(
            translate_color_codes("&#FF5500Hex &x&0&0&a&a&f&fCode"),
            "§x§f§f§5§5§0§0Hex §x§0§0§a§a§f§fCode"
        );
        assert_eq!(
            strip_formatting_codes("§x§f§f§5§5§0§0Hex §a§lVine §7» §fProxy"),
            "Hex Vine » Proxy"
        );
    }

    #[test]
    fn console_formatting_renders_legacy_and_structured_components() {
        let legacy = "§6[Vine] §a§lOnline §7(1/100)\n  §eLobby";
        let ansi = format_console_text_with_ansi(legacy, true);
        assert_eq!(
            ansi,
            "\x1b[33m[Vine] \x1b[0m\x1b[1;92mOnline \x1b[0m\x1b[37m(1/100)\x1b[0m\n\x1b[37m  \x1b[0m\x1b[93mLobby\x1b[0m"
        );

        let plain = format_console_text_with_ansi(legacy, false);
        assert_eq!(plain, "[Vine] Online (1/100)\n  Lobby");

        let hex = format_console_text_with_ansi("§x§f§f§8§0§0§0Orange", true);
        assert_eq!(hex, "\x1b[38;2;255;128;0mOrange\x1b[0m");

        let structured = TextComponent::text("Header: ")
            .color_named(NamedColor::Gold)
            .add_child(
                TextComponent::text("Click")
                    .color_named(NamedColor::Green)
                    .bold()
                    .underlined()
                    .click_event(ClickEvent::OpenUrl {
                        url: "https://pumpkinmc.org".into(),
                    }),
            );
        assert_eq!(
            format_console_component_with_ansi(&structured, true),
            "\x1b[33mHeader: \x1b[0m\x1b]8;;https://pumpkinmc.org\x1b\\\x1b[1;4;92mClick\x1b[0m\x1b]8;;\x1b\\"
        );
        assert_eq!(
            format_console_component_with_ansi(&structured, false),
            "Header: Click"
        );
    }

    #[test]
    fn console_formatting_strips_injected_escape_sequences() {
        let untrusted =
            TextComponent::text("Safe \x1b]0;PWNED\x07\x1b[2JText").color_named(NamedColor::Red);
        let rendered = format_console_component_with_ansi(&untrusted, true);
        assert_eq!(rendered, "\x1b[91mSafe ]0;PWNED[2JText\x1b[0m");
        assert!(!rendered.contains('\x07'));
        assert!(!rendered.contains("\x1b]0;"));
    }

    #[test]
    fn end_to_end_console_command_dispatch_emits_unescaped_ansi() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;
        use tracing_subscriber::layer::SubscriberExt;

        use crate::command::{CommandDispatcher, CommandSender, ProxyCommandSource};
        use crate::config::Config;
        use crate::session::SessionManager;

        #[derive(Clone, Default)]
        struct BufWriter(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for BufWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'a> MakeWriter<'a> for BufWriter {
            type Writer = Self;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let writer = BufWriter::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer.clone())
                .with_ansi(true)
                .with_ansi_sanitization(false)
                .without_time()
                .with_target(false),
        );

        let dispatcher = CommandDispatcher::new();
        let source = ProxyCommandSource {
            sender: CommandSender::Console,
            config: Arc::new(Config::default()),
            session_manager: Arc::new(SessionManager::new()),
        };

        tracing::subscriber::with_default(subscriber, || {
            dispatcher.handle_command(&source, "vine");
            dispatcher.handle_command(&source, "vine servers");
            dispatcher.handle_command(&source, "alert &cReboot &eSoon");
            dispatcher.handle_command(&source, "unknown_cmd");
        });

        let out = String::from_utf8(writer.0.lock().unwrap().clone()).unwrap();
        assert!(
            !out.contains('§'),
            "raw section signs leaked to console: {out}"
        );
        assert!(
            !out.contains("\\x1b"),
            "ANSI escapes were sanitized instead of rendered: {out}"
        );
        assert!(
            out.contains("\x1b[1;92mVine Proxy "),
            "missing bold green header: {out}"
        );
        assert!(
            out.contains("\x1b[33m[Vine] "),
            "missing gold [Vine] prefix: {out}"
        );
        assert!(
            out.contains("\x1b[91mReboot ") && out.contains("\x1b[93mSoon"),
            "missing alert color codes: {out}"
        );
        assert!(
            out.contains("\x1b[91mUnknown or incomplete command"),
            "missing red command error output: {out}"
        );
    }
}
