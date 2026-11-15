//! Per-version packet ids.
//!
//! `pumpkin-protocol` packets know how to write their bodies for older versions, but their ids
//! come from `pumpkin-data`, which only has the current version's table: `to_id(version)`
//! ignores `version`. Vine talks to clients of many versions, so every id it matches or emits
//! goes through this module, which maps the current id to the client's id with the table in
//! [`legacy_ids`].

pub mod legacy_ids;

use bytes::Bytes;
use pumpkin_data::packet::{CURRENT_MC_VERSION, clientbound};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::packet::MultiVersionJavaPacket;
use pumpkin_protocol::ser::{NetworkWriteExt, WritingError};
use pumpkin_protocol::{ClientPacket, ConnectionState};
use pumpkin_util::version::JavaMinecraftVersion;

/// The current-version id of the packet `P` uses at `version`. This is usually one fixed id,
/// but a few packets pick a different row for older versions (for example
/// `CSystemChatMessage` is `CHAT` before 1.19).
fn current_id<P: MultiVersionJavaPacket + ?Sized>(version: JavaMinecraftVersion) -> Option<i32> {
    let id = P::to_id(version);
    (id >= 0).then_some(id)
}

/// The id a client on `version` uses for the clientbound packet `P`, or `None` when that
/// version has no such packet. Unknown versions get the current id.
#[must_use]
pub fn clientbound_id<P: MultiVersionJavaPacket + ?Sized>(
    state: ConnectionState,
    version: JavaMinecraftVersion,
) -> Option<i32> {
    let current = current_id::<P>(version)?;
    if version == JavaMinecraftVersion::Unknown {
        return Some(current);
    }
    // Before 1.19 system messages went out as CHAT, which the current table has no row for
    // (the current CHAT is an alias of SYSTEM_CHAT).
    if state == ConnectionState::Play
        && current == clientbound::play::SYSTEM_CHAT.to_id(CURRENT_MC_VERSION)
        && version < JavaMinecraftVersion::V_1_19
    {
        let id = legacy_ids::clientbound::play::CHAT.to_id(version);
        return (id >= 0).then_some(id);
    }
    legacy_ids::clientbound_id(state, current, version)
}

/// The id a client (or a backend speaking the client's version) on `version` uses for the
/// serverbound packet `P`, or `None` when that version has no such packet. Unknown versions get
/// the current id.
#[must_use]
pub fn serverbound_id<P: MultiVersionJavaPacket + ?Sized>(
    state: ConnectionState,
    version: JavaMinecraftVersion,
) -> Option<i32> {
    let current = current_id::<P>(CURRENT_MC_VERSION)?;
    if version == JavaMinecraftVersion::Unknown {
        return Some(current);
    }
    legacy_ids::serverbound_id(state, current, version)
}

fn frame<P: ClientPacket + ?Sized>(
    packet: &P,
    id: Option<i32>,
    version: JavaMinecraftVersion,
) -> Result<Bytes, WritingError> {
    let id = id.ok_or(WritingError::UnsupportedVersion(version))?;
    let mut buf = Vec::new();
    buf.write_var_int(&VarInt(id))?;
    packet.write_packet_data(&mut buf, &version)?;
    Ok(buf.into())
}

/// Encodes `pumpkin-protocol` packets with the id of the version they are sent to.
///
/// Use these instead of `ClientPacket::serialize_packet`, which always writes the current id.
pub trait VersionedPacket: ClientPacket {
    /// Id and body of a clientbound packet for a client on `version`. Fails with
    /// `UnsupportedVersion` when that version has no such packet.
    fn serialize_for_client(
        &self,
        state: ConnectionState,
        version: JavaMinecraftVersion,
    ) -> Result<Bytes, WritingError> {
        frame(self, clientbound_id::<Self>(state, version), version)
    }

    /// Id and body of a serverbound packet for a server expecting `version`. Fails with
    /// `UnsupportedVersion` when that version has no such packet.
    fn serialize_for_server(
        &self,
        state: ConnectionState,
        version: JavaMinecraftVersion,
    ) -> Result<Bytes, WritingError> {
        frame(self, serverbound_id::<Self>(state, version), version)
    }
}

impl<P: ClientPacket> VersionedPacket for P {}

#[cfg(test)]
mod tests {
    use super::*;
    use JavaMinecraftVersion as V;
    use pumpkin_protocol::java::client::config::CConfigDisconnect;
    use pumpkin_protocol::java::client::login::{
        CLoginDisconnect, CLoginPluginRequest, CLoginSuccess, CSetCompression,
    };
    use pumpkin_protocol::java::client::play::{
        CCommandSuggestions, CCommands, CCustomPayload, CKeepAlive, CPlayDisconnect,
        CStartConfiguration, CSystemChatMessage, CTransfer,
    };
    use pumpkin_protocol::java::server::login::{SLoginPluginResponse, SLoginStart};
    use pumpkin_protocol::java::server::play::{
        SChatCommand, SChatCommandSigned, SChatMessage, SCommandSuggestion,
        SConfigurationAcknowledged, SCustomPayload, SKeepAlive,
    };
    use pumpkin_protocol::ser::NetworkReadExt;
    use pumpkin_util::text::TextComponent;

    /// A packet name, how to look its id up, and the expected id per version.
    type IdCase = (
        &'static str,
        fn(V) -> Option<i32>,
        &'static [(V, Option<i32>)],
    );

    const PLAY: ConnectionState = ConnectionState::Play;
    const LOGIN: ConnectionState = ConnectionState::Login;
    const CONFIG: ConnectionState = ConnectionState::Config;

    // Expected ids are taken from minecraft-data (PrismarineJS, 3.119.0) `protocol.json`:
    // pc/1.7 (1.7.10), pc/1.8 (1.8.9), pc/1.12.2, pc/1.16.2 (1.16.5), pc/1.20.3 (1.20.4),
    // pc/1.21.11 and pc/26.1. `None` means the version has no such packet.

    #[test]
    fn clientbound_play_ids_match_minecraft_data() {
        let cases: &[IdCase] = &[
            (
                "disconnect",
                |v| clientbound_id::<CPlayDisconnect>(PLAY, v),
                &[
                    (V::V_1_7_6, Some(0x40)),
                    (V::V_1_8, Some(0x40)),
                    (V::V_1_12_2, Some(0x1a)),
                    (V::V_1_16_4, Some(0x19)),
                    (V::V_1_20_3, Some(0x1b)),
                    (V::V_1_21_11, Some(0x20)),
                    (V::V_26_1, Some(0x20)),
                ],
            ),
            (
                "system chat (chat before 1.19)",
                |v| clientbound_id::<CSystemChatMessage>(PLAY, v),
                &[
                    (V::V_1_7_6, Some(0x02)),
                    (V::V_1_8, Some(0x02)),
                    (V::V_1_12_2, Some(0x0f)),
                    (V::V_1_16_4, Some(0x0e)),
                    (V::V_1_20_3, Some(0x69)),
                    (V::V_1_21_11, Some(0x77)),
                    (V::V_26_1, Some(0x79)),
                ],
            ),
            (
                "custom payload",
                |v| clientbound_id::<CCustomPayload>(PLAY, v),
                &[
                    (V::V_1_7_6, Some(0x3f)),
                    (V::V_1_8, Some(0x3f)),
                    (V::V_1_12_2, Some(0x18)),
                    (V::V_1_16_4, Some(0x17)),
                    (V::V_1_20_3, Some(0x18)),
                    (V::V_1_21_11, Some(0x18)),
                    (V::V_26_1, Some(0x18)),
                ],
            ),
            (
                "command suggestions",
                |v| clientbound_id::<CCommandSuggestions>(PLAY, v),
                &[
                    (V::V_1_7_6, Some(0x3a)),
                    (V::V_1_8, Some(0x3a)),
                    (V::V_1_12_2, Some(0x0e)),
                    (V::V_1_16_4, Some(0x0f)),
                    (V::V_1_20_3, Some(0x10)),
                    (V::V_1_21_11, Some(0x0f)),
                    (V::V_26_1, Some(0x0f)),
                ],
            ),
            (
                "commands",
                |v| clientbound_id::<CCommands>(PLAY, v),
                &[
                    (V::V_1_7_6, None),
                    (V::V_1_8, None),
                    (V::V_1_12_2, None),
                    (V::V_1_16_4, Some(0x10)),
                    (V::V_1_20_3, Some(0x11)),
                    (V::V_1_21_11, Some(0x10)),
                    (V::V_26_1, Some(0x10)),
                ],
            ),
            (
                "keep alive",
                |v| clientbound_id::<CKeepAlive>(PLAY, v),
                &[
                    (V::V_1_7_6, Some(0x00)),
                    (V::V_1_8, Some(0x00)),
                    (V::V_1_12_2, Some(0x1f)),
                    (V::V_1_16_4, Some(0x1f)),
                    (V::V_1_20_3, Some(0x24)),
                    (V::V_1_21_11, Some(0x2b)),
                    (V::V_26_1, Some(0x2c)),
                ],
            ),
            (
                "start configuration",
                |v| clientbound_id::<CStartConfiguration>(PLAY, v),
                &[
                    (V::V_1_7_6, None),
                    (V::V_1_8, None),
                    (V::V_1_12_2, None),
                    (V::V_1_16_4, None),
                    (V::V_1_20_3, Some(0x67)),
                    (V::V_1_21_11, Some(0x74)),
                    (V::V_26_1, Some(0x76)),
                ],
            ),
            (
                "transfer",
                |v| clientbound_id::<CTransfer>(PLAY, v),
                &[
                    (V::V_1_7_6, None),
                    (V::V_1_8, None),
                    (V::V_1_12_2, None),
                    (V::V_1_16_4, None),
                    (V::V_1_20_3, None),
                    (V::V_1_21_11, Some(0x7f)),
                    (V::V_26_1, Some(0x81)),
                ],
            ),
        ];
        for (name, id, rows) in cases {
            for (version, expected) in *rows {
                assert_eq!(id(*version), *expected, "{name} on {version:?}");
            }
        }
    }

    #[test]
    fn serverbound_play_ids_match_minecraft_data() {
        let cases: &[IdCase] = &[
            (
                "chat",
                |v| serverbound_id::<SChatMessage>(PLAY, v),
                &[
                    (V::V_1_7_6, Some(0x01)),
                    (V::V_1_8, Some(0x01)),
                    (V::V_1_12_2, Some(0x02)),
                    (V::V_1_16_4, Some(0x03)),
                    (V::V_1_20_3, Some(0x05)),
                    (V::V_1_21_11, Some(0x08)),
                    (V::V_26_1, Some(0x09)),
                ],
            ),
            (
                "chat command",
                |v| serverbound_id::<SChatCommand>(PLAY, v),
                &[
                    (V::V_1_7_6, None),
                    (V::V_1_8, None),
                    (V::V_1_12_2, None),
                    (V::V_1_16_4, None),
                    (V::V_1_20_3, Some(0x04)),
                    (V::V_1_21_11, Some(0x06)),
                    (V::V_26_1, Some(0x07)),
                ],
            ),
            (
                "signed chat command",
                |v| serverbound_id::<SChatCommandSigned>(PLAY, v),
                &[
                    (V::V_1_8, None),
                    (V::V_1_12_2, None),
                    (V::V_1_16_4, None),
                    (V::V_1_21_11, Some(0x07)),
                    (V::V_26_1, Some(0x08)),
                ],
            ),
            (
                "command suggestion request",
                |v| serverbound_id::<SCommandSuggestion>(PLAY, v),
                &[
                    (V::V_1_7_6, Some(0x14)),
                    (V::V_1_8, Some(0x14)),
                    (V::V_1_12_2, Some(0x01)),
                    (V::V_1_16_4, Some(0x06)),
                    (V::V_1_20_3, Some(0x0a)),
                    (V::V_1_21_11, Some(0x0e)),
                    (V::V_26_1, Some(0x0f)),
                ],
            ),
            (
                "custom payload",
                |v| serverbound_id::<SCustomPayload>(PLAY, v),
                &[
                    (V::V_1_7_6, Some(0x17)),
                    (V::V_1_8, Some(0x17)),
                    (V::V_1_12_2, Some(0x09)),
                    (V::V_1_16_4, Some(0x0b)),
                    (V::V_1_20_3, Some(0x10)),
                    (V::V_1_21_11, Some(0x15)),
                    (V::V_26_1, Some(0x16)),
                ],
            ),
            (
                "configuration acknowledged",
                |v| serverbound_id::<SConfigurationAcknowledged>(PLAY, v),
                &[
                    (V::V_1_8, None),
                    (V::V_1_16_4, None),
                    (V::V_1_20_3, Some(0x0b)),
                    (V::V_1_21_11, Some(0x0f)),
                    (V::V_26_1, Some(0x10)),
                ],
            ),
            (
                "keep alive",
                |v| serverbound_id::<SKeepAlive>(PLAY, v),
                &[
                    (V::V_1_7_6, Some(0x00)),
                    (V::V_1_8, Some(0x00)),
                    (V::V_1_12_2, Some(0x0b)),
                    (V::V_1_16_4, Some(0x10)),
                    (V::V_1_20_3, Some(0x15)),
                    (V::V_1_21_11, Some(0x1b)),
                    (V::V_26_1, Some(0x1c)),
                ],
            ),
        ];
        for (name, id, rows) in cases {
            for (version, expected) in *rows {
                assert_eq!(id(*version), *expected, "{name} on {version:?}");
            }
        }
    }

    #[test]
    fn login_and_config_ids_match_minecraft_data() {
        for v in [
            V::V_1_7_6,
            V::V_1_8,
            V::V_1_12_2,
            V::V_1_16_4,
            V::V_1_20_3,
            V::V_26_1,
        ] {
            assert_eq!(clientbound_id::<CLoginDisconnect>(LOGIN, v), Some(0x00));
            assert_eq!(clientbound_id::<CLoginSuccess>(LOGIN, v), Some(0x02));
            assert_eq!(serverbound_id::<SLoginStart>(LOGIN, v), Some(0x00));
        }
        // Set Compression arrived in 1.8 and login plugin messages in 1.13, but the vendored
        // table gives every login packet the same id in every version. Code that cares checks
        // the version itself (see `client.rs` for Set Compression).
        assert_eq!(
            clientbound_id::<CSetCompression>(LOGIN, V::V_1_8),
            Some(0x03)
        );
        assert_eq!(
            clientbound_id::<CSetCompression>(LOGIN, V::V_26_1),
            Some(0x03)
        );
        assert_eq!(
            clientbound_id::<CLoginPluginRequest>(LOGIN, V::V_1_16_4),
            Some(0x04)
        );
        assert_eq!(
            serverbound_id::<SLoginPluginResponse>(LOGIN, V::V_1_16_4),
            Some(0x02)
        );
        assert_eq!(
            clientbound_id::<CConfigDisconnect>(CONFIG, V::V_1_20_3),
            Some(0x01)
        );
        assert_eq!(
            clientbound_id::<CConfigDisconnect>(CONFIG, V::V_26_1),
            Some(0x02)
        );
    }

    #[test]
    fn unknown_versions_use_the_current_ids() {
        assert_eq!(
            clientbound_id::<CPlayDisconnect>(PLAY, V::Unknown),
            clientbound_id::<CPlayDisconnect>(PLAY, CURRENT_MC_VERSION)
        );
        assert_eq!(
            serverbound_id::<SChatCommand>(PLAY, V::Unknown),
            serverbound_id::<SChatCommand>(PLAY, CURRENT_MC_VERSION)
        );
    }

    #[test]
    fn serialize_for_client_writes_the_client_id() {
        let text = TextComponent::text("hi");
        let bytes = CSystemChatMessage::new(&text, false)
            .serialize_for_client(PLAY, V::V_1_12_2)
            .unwrap();
        let mut cur = &bytes[..];
        assert_eq!(cur.get_var_int().unwrap().0, 0x0f);

        let err = CStartConfiguration
            .serialize_for_client(PLAY, V::V_1_16_4)
            .unwrap_err();
        assert!(matches!(err, WritingError::UnsupportedVersion(V::V_1_16_4)));
    }

    // Regressions for the failures seen with real clients before ids were per version.

    #[test]
    fn chunk_data_is_not_disconnect_on_1_12_2_or_1_16_5() {
        // 0x20 (the current disconnect id) is chunk data on 1.12.2 and 1.16.5, so every chunk
        // used to look like a kick.
        for v in [V::V_1_12_2, V::V_1_16_4] {
            assert_ne!(clientbound_id::<CPlayDisconnect>(PLAY, v), Some(0x20));
        }
    }

    #[test]
    fn start_configuration_on_26_1_is_0x76() {
        assert_eq!(
            clientbound_id::<CStartConfiguration>(PLAY, V::V_26_1),
            Some(0x76)
        );
    }
}
