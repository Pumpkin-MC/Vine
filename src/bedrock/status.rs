use std::io::{Error, ErrorKind};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use aes::{
    Aes256, Block,
    cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit},
};
use hmac::{Hmac, Mac};
use pumpkin_protocol::{
    codec::{var_int::VarInt, var_uint::VarUInt},
    serial::PacketWrite,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bedrock::config::BedrockConfig;

/// Size in bytes of the leading HMAC-SHA256 checksum on NetherNet UDP discovery packets.
pub const NETHERNET_CHECKSUM_SIZE: usize = 32;

/// Size in bytes of the fixed NetherNet discovery packet header (excluding the 2-byte length prefix).
pub const NETHERNET_HEADER_SIZE: usize = 18;

/// NetherNet discovery request packet ID.
pub const NETHERNET_REQUEST_PACKET: u16 = 0;

/// NetherNet discovery advertisement response packet ID.
pub const NETHERNET_RESPONSE_PACKET: u16 = 1;

/// NetherNet LAN signaling / Ping-Pong message packet ID.
pub const NETHERNET_MESSAGE_PACKET: u16 = 2;

/// Current Bedrock NetherNet server advertisement binary schema version.
pub const NETHERNET_SERVER_DATA_VERSION: u8 = 7;

static NETHERNET_KEY: LazyLock<[u8; 32]> =
    LazyLock::new(|| Sha256::digest(0xdeadbeefu64.to_le_bytes()).into());

/// Decoded inbound NetherNet UDP discovery or signaling packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetherNetPacket {
    /// Broadcast server discovery probe (`REQUEST_PACKET = 0`).
    Request,
    /// Addressed LAN signaling or `Ping`/`Pong` message (`MESSAGE_PACKET = 2`).
    Message {
        sender_id: u64,
        recipient_id: u64,
        data: String,
    },
}

/// Server description returned by NetherNet HTTP endpoints (`GET /v1/join`, `/v1/ping`, `/status`, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetherNetStatus<'a> {
    pub name: &'a str,
    pub protocol: u32,
    pub version: &'a str,
    pub level: &'a str,
    pub players: u32,
    pub max_players: u32,
    pub game_type: u8,
}

/// Handler for Bedrock `NetherNet` HTTP status queries and encrypted UDP discovery/ping packets.
pub struct NetherNetStatusHandler {
    config: BedrockConfig,
    online_players: Arc<AtomicUsize>,
    network_id: u64,
    advertisement_id: u64,
}

impl NetherNetStatusHandler {
    #[must_use]
    pub fn new(config: BedrockConfig, online_players: Arc<AtomicUsize>) -> Self {
        let network_id = if config.network_id != 0 {
            config.network_id
        } else {
            rand::random()
        };
        let advertisement_id = rand::random();

        Self {
            config,
            online_players,
            network_id,
            advertisement_id,
        }
    }

    #[must_use]
    pub const fn config(&self) -> &BedrockConfig {
        &self.config
    }

    #[must_use]
    pub const fn network_id(&self) -> u64 {
        self.network_id
    }

    #[must_use]
    pub const fn advertisement_id(&self) -> u64 {
        self.advertisement_id
    }

    /// Builds the JSON status payload served on `GET /v1/join`, `/join`, `/v1/ping`, `/ping`, and `/status`.
    #[must_use]
    pub fn build_http_status(&self) -> NetherNetStatus<'_> {
        let players =
            u32::try_from(self.online_players.load(Ordering::Relaxed)).unwrap_or(u32::MAX);
        NetherNetStatus {
            name: &self.config.motd,
            protocol: self.config.protocol_version,
            version: &self.config.version_name,
            level: &self.config.level_name,
            players,
            max_players: self.config.max_players,
            game_type: self.config.game_mode_id,
        }
    }

    /// Encodes an encrypted NetherNet UDP `RESPONSE_PACKET` advertising this proxy.
    pub fn encode_discovery_response(&self) -> Result<Vec<u8>, Error> {
        let players =
            u32::try_from(self.online_players.load(Ordering::Relaxed)).unwrap_or(u32::MAX);
        encode_response(
            self.network_id,
            self.advertisement_id,
            &self.config.motd,
            &self.config.level_name,
            self.config.protocol_version,
            &self.config.version_name,
            self.config.game_mode_id,
            players,
            self.config.max_players,
            self.config.hardcore,
            !self.config.online_mode,
        )
    }

    /// Inspects an incoming UDP datagram and returns an encrypted NetherNet response if it is a discovery request or `Ping`.
    #[must_use]
    pub fn handle_packet(&self, data: &[u8]) -> Option<Vec<u8>> {
        match decode_packet(data)? {
            NetherNetPacket::Request => self.encode_discovery_response().ok(),
            NetherNetPacket::Message {
                sender_id,
                recipient_id,
                data,
            } if recipient_id == self.network_id && data.eq_ignore_ascii_case("ping") => {
                encode_message(self.network_id, sender_id, "Pong").ok()
            }
            NetherNetPacket::Message { .. } => None,
        }
    }
}

/// Decodes and verifies an AES-256-ECB + HMAC-SHA256 NetherNet UDP packet.
#[must_use]
pub fn decode_packet(data: &[u8]) -> Option<NetherNetPacket> {
    let encrypted = data.get(NETHERNET_CHECKSUM_SIZE..)?;
    if encrypted.is_empty() || !encrypted.len().is_multiple_of(16) {
        return None;
    }

    let mut payload = encrypted.to_vec();
    decrypt(&mut payload)?;
    let padding = usize::from(*payload.last()?);
    if padding == 0
        || padding > 16
        || payload.len() < padding
        || payload[payload.len() - padding..]
            .iter()
            .any(|byte| usize::from(*byte) != padding)
    {
        return None;
    }
    payload.truncate(payload.len() - padding);

    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(NETHERNET_KEY.as_slice()).ok()?;
    mac.update(&payload);
    mac.verify_slice(data.get(..NETHERNET_CHECKSUM_SIZE)?)
        .ok()?;

    let declared_length = usize::from(u16::from_le_bytes(payload.get(..2)?.try_into().ok()?));
    if ![payload.len(), payload.len() - 2].contains(&declared_length)
        || payload.get(12..20)? != [0; 8]
    {
        return None;
    }

    let packet_id = u16::from_le_bytes(payload.get(2..4)?.try_into().ok()?);
    let sender_id = u64::from_le_bytes(payload.get(4..12)?.try_into().ok()?);
    let body = payload.get(20..)?;
    match packet_id {
        NETHERNET_REQUEST_PACKET if body.is_empty() => Some(NetherNetPacket::Request),
        NETHERNET_MESSAGE_PACKET => {
            let recipient_id = u64::from_le_bytes(body.get(..8)?.try_into().ok()?);
            let length = u32::from_le_bytes(body.get(8..12)?.try_into().ok()?) as usize;
            let data = body.get(12..12usize.checked_add(length)?)?;
            if body.len() != 12 + length {
                return None;
            }
            Some(NetherNetPacket::Message {
                sender_id,
                recipient_id,
                data: String::from_utf8(data.to_vec()).ok()?,
            })
        }
        _ => None,
    }
}

/// Encodes an encrypted NetherNet `RESPONSE_PACKET` (`SERVER_DATA_VERSION = 7`).
#[allow(clippy::too_many_arguments)]
pub fn encode_response(
    network_id: u64,
    advertisement_id: u64,
    server_name: &str,
    level_name: &str,
    protocol_version: u32,
    version_name: &str,
    game_mode: u8,
    player_count: u32,
    max_player_count: u32,
    hardcore: bool,
    accepts_self_signed: bool,
) -> Result<Vec<u8>, Error> {
    let mut server_data = vec![NETHERNET_SERVER_DATA_VERSION];
    push_string(&mut server_data, server_name)?;
    VarInt(protocol_version as i32).write(&mut server_data)?;
    push_string(&mut server_data, version_name)?;
    push_string(&mut server_data, level_name)?;
    VarInt(i32::try_from(player_count).unwrap_or(i32::MAX)).write(&mut server_data)?;
    VarInt(i32::try_from(max_player_count).unwrap_or(i32::MAX)).write(&mut server_data)?;
    VarInt(i32::from(game_mode)).write(&mut server_data)?;
    server_data.extend_from_slice(&[0, u8::from(hardcore), 1, u8::from(accepts_self_signed)]);
    push_string(&mut server_data, &format!("{advertisement_id:016x}"))?;
    VarInt(4).write(&mut server_data)?;

    let application_data = hex::encode(server_data);
    let application_length = u32::try_from(application_data.len())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "NetherNet MOTD is too long"))?;
    let mut body = Vec::with_capacity(size_of::<u32>() + application_data.len());
    body.extend_from_slice(&application_length.to_le_bytes());
    body.extend_from_slice(application_data.as_bytes());
    encode_packet(NETHERNET_RESPONSE_PACKET, network_id, &body)
}

/// Encodes an encrypted NetherNet `MESSAGE_PACKET` (used for `Ping`/`Pong` and LAN WebRTC signaling).
pub fn encode_message(network_id: u64, recipient_id: u64, data: &str) -> Result<Vec<u8>, Error> {
    let length = u32::try_from(data.len())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "NetherNet signal is too long"))?;
    let mut body = Vec::with_capacity(size_of::<u64>() + size_of::<u32>() + data.len());
    body.extend_from_slice(&recipient_id.to_le_bytes());
    body.extend_from_slice(&length.to_le_bytes());
    body.extend_from_slice(data.as_bytes());
    encode_packet(NETHERNET_MESSAGE_PACKET, network_id, &body)
}

fn encode_packet(packet_id: u16, network_id: u64, body: &[u8]) -> Result<Vec<u8>, Error> {
    let packet_length = u16::try_from(size_of::<u16>() + NETHERNET_HEADER_SIZE + body.len())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "NetherNet packet is too long"))?;
    let mut payload = Vec::with_capacity(usize::from(packet_length) + 16);
    payload.extend_from_slice(&packet_length.to_le_bytes());
    payload.extend_from_slice(&packet_id.to_le_bytes());
    payload.extend_from_slice(&network_id.to_le_bytes());
    payload.extend_from_slice(&[0; 8]);
    payload.extend_from_slice(body);

    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(NETHERNET_KEY.as_slice())
        .map_err(|_| Error::other("invalid NetherNet discovery key"))?;
    mac.update(&payload);
    let checksum = mac.finalize().into_bytes();

    let padding = 16 - payload.len() % 16;
    payload.resize(payload.len() + padding, padding as u8);
    encrypt(&mut payload);

    let mut response = Vec::with_capacity(NETHERNET_CHECKSUM_SIZE + payload.len());
    response.extend_from_slice(&checksum);
    response.extend_from_slice(&payload);
    Ok(response)
}

fn push_string(buffer: &mut Vec<u8>, value: &str) -> Result<(), Error> {
    let length = u32::try_from(value.len())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "NetherNet MOTD field is too long"))?;
    VarUInt(length).write(buffer)?;
    buffer.extend_from_slice(value.as_bytes());
    Ok(())
}

fn encrypt(data: &mut [u8]) {
    let (blocks, remainder) = Block::slice_as_chunks_mut(data);
    debug_assert!(remainder.is_empty());
    Aes256::new((&*NETHERNET_KEY).into()).encrypt_blocks(blocks);
}

fn decrypt(data: &mut [u8]) -> Option<()> {
    let (blocks, remainder) = Block::slice_as_chunks_mut(data);
    if !remainder.is_empty() {
        return None;
    }
    Aes256::new((&*NETHERNET_KEY).into()).decrypt_blocks(blocks);
    Some(())
}

/// Extracts the STUN `USERNAME` (`0x0006`) attribute (`<server_ufrag>:<client_ufrag>`) from a STUN packet.
#[must_use]
pub fn stun_username(packet: &[u8]) -> Option<&str> {
    if packet.len() < 20 || packet.get(4..8)? != [0x21, 0x12, 0xa4, 0x42] {
        return None;
    }
    let attributes_length = usize::from(u16::from_be_bytes(packet.get(2..4)?.try_into().ok()?));
    let end = 20usize.checked_add(attributes_length)?;
    if end > packet.len() {
        return None;
    }

    let mut offset = 20usize;
    while offset.checked_add(4)? <= end {
        let attribute = u16::from_be_bytes(packet.get(offset..offset + 2)?.try_into().ok()?);
        let length = usize::from(u16::from_be_bytes(
            packet.get(offset + 2..offset + 4)?.try_into().ok()?,
        ));
        let value_start = offset + 4;
        let value_end = value_start.checked_add(length)?;
        if value_end > end {
            return None;
        }
        if attribute == 0x0006 {
            return std::str::from_utf8(packet.get(value_start..value_end)?).ok();
        }
        offset = value_start.checked_add(length.next_multiple_of(4))?;
    }
    None
}

/// Rewrites component-1 UDP host candidates in a client SDP offer to point to `proxy_addr`,
/// returning `(rewritten_sdp, original_client_candidates)`.
#[must_use]
pub fn proxy_offer(sdp: &str, proxy_addr: SocketAddr) -> (String, Vec<SocketAddr>) {
    let mut candidates = Vec::new();
    let rewritten = rewrite_sdp_candidates(sdp, |fields| {
        if is_component_one_udp(fields) {
            if let Some(candidate) = candidate_address(fields) {
                candidates.push(candidate);
            }
            fields[4] = proxy_addr.ip().to_string();
            fields[5] = proxy_addr.port().to_string();
        }
    });
    (rewritten, candidates)
}

/// Rewrites component-1 UDP host candidates in a backend SDP answer to advertise Vine's `public_ip`/`public_port`,
/// returning `(rewritten_sdp, server_ice_ufrag, backend_udp_candidate)`.
pub fn proxy_answer(
    sdp: &str,
    public_ip: Option<IpAddr>,
    public_port: u16,
) -> Result<(String, String, SocketAddr), String> {
    let ufrag = sdp
        .lines()
        .find_map(|line| line.strip_prefix("a=ice-ufrag:"))
        .map(str::to_owned)
        .ok_or_else(|| "WebRTC answer has no ICE username fragment".to_string())?;
    let backend_candidate = sdp
        .lines()
        .filter_map(|line| line.strip_prefix("a=candidate:"))
        .map(|candidate| {
            candidate
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .find(|fields| is_component_one_udp(fields))
        .and_then(|fields| candidate_address(&fields))
        .ok_or_else(|| "WebRTC answer has no UDP host candidate".to_string())?;

    let answer = rewrite_sdp_candidates(sdp, |fields| {
        if is_component_one_udp(fields) {
            if let Some(ip) = public_ip {
                fields[4] = ip.to_string();
            }
            fields[5] = public_port.to_string();
        }
    });
    Ok((answer, ufrag, backend_candidate))
}

fn rewrite_sdp_candidates(sdp: &str, mut rewrite: impl FnMut(&mut Vec<String>)) -> String {
    let mut rewritten = String::with_capacity(sdp.len());
    for line in sdp.lines() {
        if let Some(candidate) = line.strip_prefix("a=candidate:") {
            let mut fields = candidate
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            rewrite(&mut fields);
            rewritten.push_str("a=candidate:");
            rewritten.push_str(&fields.join(" "));
        } else {
            rewritten.push_str(line);
        }
        rewritten.push_str("\r\n");
    }
    rewritten
}

fn is_component_one_udp(fields: &[String]) -> bool {
    fields.len() >= 8 && fields[1] == "1" && fields[2].eq_ignore_ascii_case("udp")
}

fn candidate_address(fields: &[String]) -> Option<SocketAddr> {
    Some(SocketAddr::new(
        fields.get(4)?.parse().ok()?,
        fields.get(5)?.parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bedrock::config::{CURRENT_BEDROCK_MC_PROTOCOL, CURRENT_BEDROCK_MC_VERSION};

    #[test]
    fn encodes_multibyte_string_lengths() {
        for (value, prefix) in [
            ("a".repeat(127), vec![127]),
            ("é".repeat(64), vec![128, 1]),
            ("a".repeat(255), vec![255, 1]),
            ("é".repeat(128), vec![128, 2]),
        ] {
            let mut encoded = Vec::new();
            push_string(&mut encoded, &value).unwrap();
            assert_eq!(&encoded[..prefix.len()], prefix);
            assert_eq!(&encoded[prefix.len()..], value.as_bytes());
        }
    }

    #[test]
    fn decodes_reference_discovery_requests() {
        for hex_pkt in [
            "b3eca3eb83a6fcb079faf2eae2bf8abbaadb5906bc42bd63a0056274a26e013f\
             6b9027ddcd144fe3ada066f65b76e8faff0f8709e13bf6858e2051c321db615e",
            "ba539dab685df1c59ba406ec900f48e8249d28821c6e7149309d3fc289002201\
             ee5a9a7b4fec49402ac54510cb744a96ff0f8709e13bf6858e2051c321db615e",
        ] {
            let request = hex::decode(hex_pkt).unwrap();
            assert_eq!(decode_packet(&request), Some(NetherNetPacket::Request));
        }
    }

    #[test]
    fn encodes_current_server_data() {
        for (accepts_self_signed, authentication_flags) in [(false, "0100"), (true, "0101")] {
            let response = encode_response(
                99,
                0x9bb64bcf14727bdb,
                "Dedicated Server",
                "Creative level",
                CURRENT_BEDROCK_MC_PROTOCOL,
                CURRENT_BEDROCK_MC_VERSION,
                1,
                0,
                10,
                false,
                accepts_self_signed,
            )
            .unwrap();
            let mut payload = response[NETHERNET_CHECKSUM_SIZE..].to_vec();
            decrypt(&mut payload).unwrap();
            let padding = usize::from(*payload.last().unwrap());
            payload.truncate(payload.len() - padding);
            assert_eq!(
                usize::from(u16::from_le_bytes(payload[..2].try_into().unwrap())),
                payload.len()
            );
            let application_length =
                u32::from_le_bytes(payload[20..24].try_into().unwrap()) as usize;
            let server_data = hex::decode(&payload[24..24 + application_length]).unwrap();
            assert_eq!(
                hex::encode(server_data),
                format!(
                    "071044656469636174656420536572766572e22207312e32362e3633\
                     0e4372656174697665206c6576656c0014020000{authentication_flags}103962623634626366313437323762646208"
                )
            );
        }
    }

    #[test]
    fn message_and_ping_pong_roundtrip() {
        let message = encode_message(99, 42, "CONNECTREQUEST 7 offer").unwrap();
        assert_eq!(
            decode_packet(&message),
            Some(NetherNetPacket::Message {
                sender_id: 99,
                recipient_id: 42,
                data: "CONNECTREQUEST 7 offer".to_owned(),
            })
        );

        let config = BedrockConfig {
            network_id: 99,
            ..BedrockConfig::default()
        };
        let online = Arc::new(AtomicUsize::new(4));
        let handler = NetherNetStatusHandler::new(config, online);

        let ping = encode_message(42, 99, "Ping").unwrap();
        let pong = handler.handle_packet(&ping).expect("expected Pong packet");
        assert_eq!(
            decode_packet(&pong),
            Some(NetherNetPacket::Message {
                sender_id: 99,
                recipient_id: 42,
                data: "Pong".to_owned(),
            })
        );

        // Message addressed to another server should be ignored
        let other_ping = encode_message(42, 777, "Ping").unwrap();
        assert!(handler.handle_packet(&other_ping).is_none());
    }

    #[test]
    fn http_status_camel_case_json() {
        let config = BedrockConfig {
            motd: "Vine Hub".to_string(),
            level_name: "Lobby".to_string(),
            max_players: 250,
            game_mode_id: 1,
            ..BedrockConfig::default()
        };
        let online = Arc::new(AtomicUsize::new(12));
        let handler = NetherNetStatusHandler::new(config, online);
        let json = serde_json::to_value(handler.build_http_status()).unwrap();
        assert_eq!(json["name"], "Vine Hub");
        assert_eq!(json["protocol"], CURRENT_BEDROCK_MC_PROTOCOL);
        assert_eq!(json["version"], CURRENT_BEDROCK_MC_VERSION);
        assert_eq!(json["level"], "Lobby");
        assert_eq!(json["players"], 12);
        assert_eq!(json["maxPlayers"], 250);
        assert_eq!(json["gameType"], 1);
    }

    #[test]
    fn reads_stun_username_and_rewrites_sdp_candidates() {
        let username = b"server:client";
        let mut packet = vec![0, 1, 0, 20, 0x21, 0x12, 0xa4, 0x42];
        packet.extend_from_slice(&[0; 12]);
        packet.extend_from_slice(&0x0006u16.to_be_bytes());
        packet.extend_from_slice(&(username.len() as u16).to_be_bytes());
        packet.extend_from_slice(username);
        packet.resize(packet.len().next_multiple_of(4), 0);
        assert_eq!(stun_username(&packet), Some("server:client"));

        let offer =
            "v=0\r\na=ice-ufrag:client\r\na=candidate:1 1 udp 123 192.168.1.7 50000 typ host\r\n";
        let proxy: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let (rewritten_offer, candidates) = proxy_offer(offer, proxy);
        assert!(rewritten_offer.contains("127.0.0.1 40000 typ host"));
        assert_eq!(candidates, vec!["192.168.1.7:50000".parse().unwrap()]);

        let answer =
            "v=0\r\na=ice-ufrag:server\r\na=candidate:2 1 udp 456 10.0.0.5 19133 typ host\r\n";
        let (rewritten_answer, ufrag, backend_addr) =
            proxy_answer(answer, Some("203.0.113.7".parse().unwrap()), 19132).unwrap();
        assert!(rewritten_answer.contains("203.0.113.7 19132 typ host"));
        assert_eq!(ufrag, "server");
        assert_eq!(backend_addr, "10.0.0.5:19133".parse().unwrap());
    }
}
