use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::bedrock::config::BedrockConfig;

pub const RAKNET_UNCONNECTED_PING_ID: u8 = 0x01;
pub const RAKNET_UNCONNECTED_PING_OPEN_ID: u8 = 0x02;
pub const RAKNET_UNCONNECTED_PONG_ID: u8 = 0x1c;

/// Magic byte sequence in RakNet offline message headers.
///
/// Pumpkin dropped RakNet in favour of NetherNet, so Vine keeps the few bytes
/// of the unconnected ping/pong format it needs for the server list itself.
pub const OFFLINE_MESSAGE_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

/// Reads the client time and magic from an unconnected ping payload
/// (`time: u64 BE`, `magic: [u8; 16]`, `client_guid: u64 BE`).
fn read_unconnected_ping(payload: &[u8]) -> Option<(u64, [u8; 16])> {
    let time = u64::from_be_bytes(payload.get(0..8)?.try_into().ok()?);
    let magic: [u8; 16] = payload.get(8..24)?.try_into().ok()?;
    payload.get(24..32)?;
    Some((time, magic))
}

/// Writes an unconnected pong payload
/// (`time: u64 BE`, `server_guid: u64 BE`, magic, `u16 BE` length, advertisement).
fn write_unconnected_pong(out: &mut Vec<u8>, time: u64, server_guid: u64, ad: &str) -> Option<()> {
    let length = u16::try_from(ad.len()).ok()?;
    out.extend_from_slice(&time.to_be_bytes());
    out.extend_from_slice(&server_guid.to_be_bytes());
    out.extend_from_slice(&OFFLINE_MESSAGE_MAGIC);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(ad.as_bytes());
    Some(())
}

/// Handler for Bedrock RakNet status pings.
pub struct BedrockStatusHandler {
    config: BedrockConfig,
    online_players: Arc<AtomicUsize>,
    server_guid: u64,
    ipv4_port: u16,
    ipv6_port: u16,
}

impl BedrockStatusHandler {
    pub fn new(config: BedrockConfig, online_players: Arc<AtomicUsize>, ipv4_port: u16) -> Self {
        let server_guid = if config.server_guid != 0 {
            config.server_guid
        } else {
            rand::random()
        };
        let ipv6_port = ipv4_port.saturating_add(1);

        Self {
            config,
            online_players,
            server_guid,
            ipv4_port,
            ipv6_port,
        }
    }

    /// Formats the official Bedrock MCPE advertisement string
    pub fn build_advertisement_string(&self) -> String {
        let online = self.online_players.load(Ordering::Relaxed) as i32;
        let motd = format!("{}\n{}", self.config.motd, self.config.sub_motd);

        format!(
            "MCPE;{};{};{};{};{};{};{};{};{};{};{};0;",
            motd,
            self.config.protocol_version,
            self.config.version_name,
            online,
            self.config.max_players,
            self.server_guid,
            self.config.level_name,
            self.config.game_mode,
            self.config.game_mode_id,
            self.ipv4_port,
            self.ipv6_port,
        )
    }

    /// Inspects an incoming UDP datagram and, if it is an unconnected ping, builds the pong response.
    pub fn handle_packet(&self, data: &[u8]) -> Option<Vec<u8>> {
        let (&packet_id, payload) = data.split_first()?;

        let (time, magic) = match packet_id {
            RAKNET_UNCONNECTED_PING_ID | RAKNET_UNCONNECTED_PING_OPEN_ID => {
                read_unconnected_ping(payload)?
            }
            _ => return None,
        };

        if magic != OFFLINE_MESSAGE_MAGIC {
            return None;
        }

        let ad = self.build_advertisement_string();
        let mut response = vec![RAKNET_UNCONNECTED_PONG_ID];
        write_unconnected_pong(&mut response, time, self.server_guid, &ad)?;
        Some(response)
    }

    #[must_use]
    pub const fn server_guid(&self) -> u64 {
        self.server_guid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_advertisement_string() {
        let config = BedrockConfig {
            enabled: true,
            bind_address: "0.0.0.0:19132".to_string(),
            motd: "Vine Server".to_string(),
            sub_motd: "Bedrock Proxy".to_string(),
            protocol_version: 2169,
            version_name: "1.26.45".to_string(),
            level_name: "Lobby".to_string(),
            game_mode: "Survival".to_string(),
            game_mode_id: 0,
            max_players: 50,
            server_guid: 12345,
        };

        let online = Arc::new(AtomicUsize::new(3));
        let handler = BedrockStatusHandler::new(config, online, 19132);
        let ad = handler.build_advertisement_string();

        assert!(ad.starts_with("MCPE;Vine Server\nBedrock Proxy;2169;1.26.45;3;50;12345;Lobby;Survival;0;19132;19133;0;"));
    }

    #[test]
    fn test_handle_unconnected_ping() {
        let config = BedrockConfig::default();
        let online = Arc::new(AtomicUsize::new(0));
        let handler = BedrockStatusHandler::new(config, online, 19132);

        let mut packet = vec![RAKNET_UNCONNECTED_PING_ID];
        packet.extend_from_slice(&1000u64.to_be_bytes());
        packet.extend_from_slice(&OFFLINE_MESSAGE_MAGIC);
        packet.extend_from_slice(&9999u64.to_be_bytes());

        let response = handler.handle_packet(&packet);
        assert!(response.is_some());
        let res_bytes = response.unwrap();
        assert_eq!(res_bytes[0], RAKNET_UNCONNECTED_PONG_ID);
        assert_eq!(&res_bytes[1..9], &1000u64.to_be_bytes());
        assert_eq!(&res_bytes[17..33], &OFFLINE_MESSAGE_MAGIC);
    }

    #[test]
    fn test_ignore_invalid_magic() {
        let config = BedrockConfig::default();
        let online = Arc::new(AtomicUsize::new(0));
        let handler = BedrockStatusHandler::new(config, online, 19132);

        let mut packet = vec![RAKNET_UNCONNECTED_PING_ID];
        packet.extend_from_slice(&1000u64.to_be_bytes());
        packet.extend_from_slice(&[0u8; 16]);
        packet.extend_from_slice(&9999u64.to_be_bytes());

        let response = handler.handle_packet(&packet);
        assert!(response.is_none());
    }
}
