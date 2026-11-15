use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::{
        HeaderMap, HeaderValue, StatusCode, Uri,
        header::{CONTENT_TYPE, HOST},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tokio::net::{TcpListener, UdpSocket, lookup_host};
use tokio::sync::broadcast;
use tracing::{debug, error, info, trace, warn};

use crate::bedrock::config::{BedrockConfig, NETHERNET_DISCOVERY_PORT};
use crate::bedrock::status::{
    NetherNetPacket, NetherNetStatusHandler, decode_packet, encode_message, proxy_answer,
    proxy_offer, stun_username,
};

const MAX_SDP_SIZE: usize = 1 << 20;
const ICE_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const HTTP_BACKEND_TIMEOUT: Duration = Duration::from_secs(10);

struct IceSession {
    ufrag: String,
    backend_socket: Arc<UdpSocket>,
    backend_udp_addr: SocketAddr,
    client_candidates: Vec<SocketAddr>,
    client_remote: RwLock<Option<SocketAddr>>,
    last_active_secs: AtomicU64,
}

impl IceSession {
    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn touch(&self) {
        self.last_active_secs
            .store(Self::now_secs(), Ordering::Relaxed);
    }

    fn is_idle(&self, timeout_secs: u64) -> bool {
        let last = self.last_active_secs.load(Ordering::Relaxed);
        Self::now_secs().saturating_sub(last) > timeout_secs
    }
}

#[derive(Default)]
struct IceRelayTable {
    by_ufrag: RwLock<HashMap<String, Arc<IceSession>>>,
    by_client_remote: RwLock<HashMap<SocketAddr, Arc<IceSession>>>,
}

impl IceRelayTable {
    fn insert(&self, session: Arc<IceSession>) {
        if let Ok(mut by_ufrag) = self.by_ufrag.write() {
            by_ufrag.insert(session.ufrag.clone(), session);
        }
    }

    fn remove(&self, session: &Arc<IceSession>) {
        if let Ok(mut by_ufrag) = self.by_ufrag.write()
            && by_ufrag
                .get(&session.ufrag)
                .is_some_and(|existing| Arc::ptr_eq(existing, session))
        {
            by_ufrag.remove(&session.ufrag);
        }
        if let Ok(mut by_remote) = self.by_client_remote.write() {
            by_remote.retain(|_, existing| !Arc::ptr_eq(existing, session));
        }
    }

    fn resolve_inbound(&self, packet: &[u8], remote: SocketAddr) -> Option<Arc<IceSession>> {
        let matched_by_ufrag = stun_username(packet).and_then(|username| {
            let by_ufrag = self.by_ufrag.read().ok()?;
            username
                .split(':')
                .find_map(|part| by_ufrag.get(part).cloned())
        });

        if let Some(session) = matched_by_ufrag {
            if let Ok(mut client_remote) = session.client_remote.write() {
                *client_remote = Some(remote);
            }
            if let Ok(mut by_remote) = self.by_client_remote.write() {
                by_remote.insert(remote, session.clone());
            }
            return Some(session);
        }

        self.by_client_remote
            .read()
            .ok()
            .and_then(|by_remote| by_remote.get(&remote).cloned())
    }
}

#[derive(Clone)]
struct SignalingState {
    status_handler: Arc<NetherNetStatusHandler>,
    public_udp: Arc<UdpSocket>,
    ice_public_port: u16,
    relay_table: Arc<IceRelayTable>,
    http_client: Option<reqwest::Client>,
    shutdown_tx: broadcast::Sender<()>,
}

/// Runs the Bedrock `NetherNet` HTTP signaling, UDP ICE relay, and UDP LAN discovery listeners.
pub async fn run_bedrock_listener(
    config: BedrockConfig,
    online_players: Arc<AtomicUsize>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bind_addr: SocketAddr = config.bind_address.parse()?;
    let tcp_listener = TcpListener::bind(bind_addr).await?;
    let tcp_local_addr = tcp_listener.local_addr()?;

    let udp_socket = Arc::new(UdpSocket::bind(bind_addr).await?);
    let udp_local_addr = udp_socket.local_addr()?;

    let status_handler = Arc::new(NetherNetStatusHandler::new(config.clone(), online_players));
    let relay_table = Arc::new(IceRelayTable::default());

    let http_client = pumpkin_auth::client_builder()
        .timeout(HTTP_BACKEND_TIMEOUT)
        .user_agent(format!("Vine-Proxy/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .ok();

    let (internal_shutdown_tx, _) = broadcast::channel::<()>(1);

    info!("=====================================================");
    info!(
        "  Vine Bedrock NetherNet Signaling active on TCP {}",
        tcp_local_addr
    );
    info!(
        "  Vine Bedrock NetherNet ICE active on UDP {}",
        udp_local_addr
    );
    info!(
        "  Bedrock Version: {} (Protocol {}, Network ID: {:016x})",
        config.version_name,
        config.protocol_version,
        status_handler.network_id()
    );
    if let Some(ref backend) = config.backend_address {
        info!("  Bedrock Backend Relay: {}", backend);
    }
    info!("=====================================================");

    let state = SignalingState {
        status_handler: status_handler.clone(),
        public_udp: udp_socket.clone(),
        ice_public_port: udp_local_addr.port(),
        relay_table: relay_table.clone(),
        http_client,
        shutdown_tx: internal_shutdown_tx.clone(),
    };

    if config.lan_discovery {
        let lan_ip = match tcp_local_addr.ip() {
            IpAddr::V4(ip) => ip,
            IpAddr::V6(_) => Ipv4Addr::UNSPECIFIED,
        };
        match UdpSocket::bind((lan_ip, NETHERNET_DISCOVERY_PORT)).await {
            Ok(lan_socket) => {
                if let Ok(addr) = lan_socket.local_addr() {
                    info!("Bedrock NetherNet LAN discovery listening on UDP {}", addr);
                }
                let lan_socket = Arc::new(lan_socket);
                let lan_state = state.clone();
                let mut lan_shutdown = internal_shutdown_tx.subscribe();
                tokio::spawn(async move {
                    run_lan_discovery_loop(lan_socket, lan_state, &mut lan_shutdown).await;
                });
            }
            Err(err) => {
                warn!(
                    "Failed to bind Bedrock NetherNet LAN discovery on UDP port {}: {}",
                    NETHERNET_DISCOVERY_PORT, err
                );
            }
        }
    }

    let router = build_signaling_router(state.clone());
    let mut http_shutdown = internal_shutdown_tx.subscribe();
    let http_task = tokio::spawn(async move {
        let result = axum::serve(
            tcp_listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = http_shutdown.recv().await;
        })
        .await;
        if let Err(err) = result {
            warn!("Bedrock NetherNet HTTP signaling server stopped: {}", err);
        }
    });

    let mut buffer = vec![0u8; usize::from(u16::MAX)];
    loop {
        tokio::select! {
            result = udp_socket.recv_from(&mut buffer) => {
                match result {
                    Ok((length, peer_addr)) => {
                        let packet = &buffer[..length];
                        if let Some(session) = relay_table.resolve_inbound(packet, peer_addr) {
                            session.touch();
                            if let Err(err) = session
                                .backend_socket
                                .send_to(packet, session.backend_udp_addr)
                                .await
                            {
                                trace!("[{}] Failed to relay inbound NetherNet ICE packet: {}", peer_addr, err);
                            }
                        } else if let Some(response) = status_handler.handle_packet(packet) {
                            trace!("[{}] Responding to direct NetherNet UDP probe ({} bytes)", peer_addr, response.len());
                            if let Err(err) = udp_socket.send_to(&response, peer_addr).await {
                                debug!("[{}] Failed to send NetherNet UDP response: {}", peer_addr, err);
                            }
                        } else {
                            trace!("[{}] Ignored unroutable Bedrock UDP datagram ({} bytes)", peer_addr, length);
                        }
                    }
                    Err(err) => {
                        error!("Bedrock NetherNet UDP receive error: {}", err);
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                info!("Bedrock NetherNet listener received shutdown signal, stopping...");
                break;
            }
        }
    }

    let _ = internal_shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), http_task).await;

    Ok(())
}

fn build_signaling_router(state: SignalingState) -> Router {
    Router::new()
        .route("/v1/join", get(http_status))
        .route("/join", get(http_status))
        .route("/v1/ping", get(http_status))
        .route("/ping", get(http_status))
        .route("/status", get(http_status))
        .route("/v1/join/{network_id}", post(http_join))
        .fallback(http_unknown_request)
        .layer(DefaultBodyLimit::max(MAX_SDP_SIZE))
        .with_state(state)
}

async fn http_unknown_request(ConnectInfo(address): ConnectInfo<SocketAddr>, uri: Uri) -> Response {
    debug!(%address, %uri, "Unhandled NetherNet signaling request");
    (StatusCode::NOT_FOUND, "Unknown NetherNet endpoint").into_response()
}

async fn http_status(
    State(state): State<SignalingState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
) -> Response {
    let status = state.status_handler.build_http_status();
    let Ok(body) = serde_json::to_vec(&status) else {
        warn!(%address, "Failed to serialize NetherNet server status");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut response = (StatusCode::OK, body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    trace!(
        %address,
        players = status.players,
        max_players = status.max_players,
        protocol = status.protocol,
        "Sent NetherNet HTTP server status"
    );
    response
}

async fn http_join(
    State(state): State<SignalingState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(network_id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    offer: Bytes,
) -> Response {
    trace!(%address, %network_id, length = offer.len(), "Received NetherNet SDP offer");
    if offer.is_empty() {
        return (StatusCode::BAD_REQUEST, "Missing SDP offer").into_response();
    }
    let Ok(offer_sdp) = String::from_utf8(offer.to_vec()) else {
        return (StatusCode::BAD_REQUEST, "SDP offer must be UTF-8").into_response();
    };

    let host_header = headers.get(HOST).and_then(|h| h.to_str().ok());
    let uri_authority = uri.authority().map(axum::http::uri::Authority::as_str);
    let request_host_ip = host_header
        .or(uri_authority)
        .and_then(|h| h.parse::<axum::http::uri::Authority>().ok())
        .and_then(|auth| auth.host().parse::<IpAddr>().ok());
    let advertised_ip = state
        .status_handler
        .config()
        .external_ip
        .or(request_host_ip);

    let Some(backend_address) = state.status_handler.config().backend_address.as_deref() else {
        debug!(%address, "Rejected NetherNet join: no Bedrock backend_address configured");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "No Bedrock NetherNet backend configured",
        )
            .into_response();
    };

    match negotiate_with_backend(
        &state,
        backend_address,
        &network_id,
        &offer_sdp,
        advertised_ip,
    )
    .await
    {
        Ok(answer_sdp) => {
            let mut response = (StatusCode::OK, answer_sdp).into_response();
            response
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/sdp"));
            response
        }
        Err(err) => {
            warn!(%address, "Bedrock NetherNet backend negotiation failed: {}", err);
            (StatusCode::BAD_GATEWAY, err).into_response()
        }
    }
}

async fn negotiate_with_backend(
    state: &SignalingState,
    backend_address: &str,
    network_id: &str,
    offer_sdp: &str,
    advertised_ip: Option<IpAddr>,
) -> Result<String, String> {
    let client = state
        .http_client
        .as_ref()
        .ok_or_else(|| "HTTP client unavailable for NetherNet signaling".to_string())?;

    let backend_sock_addr = lookup_host(backend_address)
        .await
        .map_err(|e| format!("Failed to resolve Bedrock backend '{backend_address}': {e}"))?
        .next()
        .ok_or_else(|| format!("No addresses resolved for Bedrock backend '{backend_address}'"))?;

    let (bind_addr, local_ip) = resolve_local_bind_for_backend(backend_sock_addr).await?;
    let backend_socket = Arc::new(
        UdpSocket::bind(bind_addr)
            .await
            .map_err(|e| format!("Failed to bind session UDP relay socket: {e}"))?,
    );
    let session_local_port = backend_socket
        .local_addr()
        .map_err(|e| format!("Failed to inspect session UDP relay socket: {e}"))?
        .port();
    let proxy_candidate_addr = SocketAddr::new(local_ip, session_local_port);

    let (rewritten_offer, client_candidates) = proxy_offer(offer_sdp, proxy_candidate_addr);
    let backend_url = format!(
        "http://{}/v1/join/{}",
        backend_address.trim_end_matches('/'),
        network_id
    );

    let response = client
        .post(&backend_url)
        .header("Content-Type", "application/sdp")
        .body(rewritten_offer)
        .send()
        .await
        .map_err(|e| format!("Failed to contact Bedrock backend '{backend_url}': {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Bedrock backend returned HTTP {status}: {body}"));
    }

    let raw_answer = response
        .text()
        .await
        .map_err(|e| format!("Failed to read SDP answer from Bedrock backend: {e}"))?;

    let (rewritten_answer, server_ufrag, backend_udp_addr) =
        proxy_answer(&raw_answer, advertised_ip, state.ice_public_port)?;

    let session = Arc::new(IceSession {
        ufrag: server_ufrag,
        backend_socket: backend_socket.clone(),
        backend_udp_addr,
        client_candidates,
        client_remote: RwLock::new(None),
        last_active_secs: AtomicU64::new(IceSession::now_secs()),
    });

    state.relay_table.insert(session.clone());
    spawn_backend_ice_pump(
        session,
        state.public_udp.clone(),
        state.relay_table.clone(),
        state.shutdown_tx.subscribe(),
    );

    Ok(rewritten_answer)
}

async fn resolve_local_bind_for_backend(
    backend: SocketAddr,
) -> Result<(SocketAddr, IpAddr), String> {
    if backend.ip().is_loopback() {
        return Ok(match backend {
            SocketAddr::V4(_) => (
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ),
            SocketAddr::V6(_) => (
                SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ),
        });
    }

    let unspec = match backend {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    };
    let probe = UdpSocket::bind(unspec)
        .await
        .map_err(|e| format!("Failed to probe outbound interface: {e}"))?;
    probe
        .connect(backend)
        .await
        .map_err(|e| format!("Failed to connect probe socket to {backend}: {e}"))?;
    let local_ip = probe
        .local_addr()
        .map_err(|e| format!("Failed to read probe local address: {e}"))?
        .ip();
    Ok((unspec, local_ip))
}

fn spawn_backend_ice_pump(
    session: Arc<IceSession>,
    public_udp: Arc<UdpSocket>,
    relay_table: Arc<IceRelayTable>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; usize::from(u16::MAX)];
        let mut cleanup_interval = tokio::time::interval(Duration::from_secs(15));
        cleanup_interval.tick().await;

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                _ = cleanup_interval.tick() => {
                    if session.is_idle(ICE_SESSION_IDLE_TIMEOUT.as_secs()) {
                        trace!(ufrag = %session.ufrag, "Closing idle NetherNet ICE relay session");
                        break;
                    }
                }
                res = session.backend_socket.recv_from(&mut buf) => {
                    match res {
                        Ok((len, _from)) => {
                            session.touch();
                            let target = session
                                .client_remote
                                .read()
                                .ok()
                                .and_then(|r| *r)
                                .or_else(|| session.client_candidates.first().copied());
                            if let Some(client_addr) = target
                                && let Err(err) = public_udp.send_to(&buf[..len], client_addr).await
                            {
                                trace!(%client_addr, "Failed to relay outbound NetherNet ICE packet: {}", err);
                            }
                        }
                        Err(err) => {
                            trace!(ufrag = %session.ufrag, "Backend ICE socket closed/errored: {}", err);
                            break;
                        }
                    }
                }
            }
        }

        relay_table.remove(&session);
    });
}

async fn run_lan_discovery_loop(
    socket: Arc<UdpSocket>,
    state: SignalingState,
    shutdown_rx: &mut broadcast::Receiver<()>,
) {
    let mut buffer = vec![0u8; 65_535];
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,
            result = socket.recv_from(&mut buffer) => {
                let (length, address) = match result {
                    Ok(pair) => pair,
                    Err(err) => {
                        debug!("NetherNet LAN discovery recv failed: {}", err);
                        continue;
                    }
                };
                let packet = &buffer[..length];
                match decode_packet(packet) {
                    Some(NetherNetPacket::Request) => {
                        trace!("Received NetherNet LAN discovery request from {}", address);
                        if let Ok(response) = state.status_handler.encode_discovery_response()
                            && let Err(err) = socket.send_to(&response, address).await
                        {
                            debug!("Failed to send NetherNet LAN discovery response to {}: {}", address, err);
                        }
                    }
                    Some(NetherNetPacket::Message {
                        sender_id,
                        recipient_id,
                        data,
                    }) if recipient_id == state.status_handler.network_id() => {
                        handle_lan_signal(&socket, &state, address, sender_id, data).await;
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn handle_lan_signal(
    socket: &Arc<UdpSocket>,
    state: &SignalingState,
    address: SocketAddr,
    sender_id: u64,
    data: String,
) {
    let network_id = state.status_handler.network_id();
    if data.eq_ignore_ascii_case("ping") {
        if let Ok(pong) = encode_message(network_id, sender_id, "Pong") {
            let _ = socket.send_to(&pong, address).await;
        }
        return;
    }

    let mut parts = data.splitn(3, ' ');
    let (Some(signal_type), Some(conn_id_str), Some(payload)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return;
    };
    let Ok(connection_id) = conn_id_str.parse::<u64>() else {
        return;
    };

    if signal_type == "CONNECTREQUEST" {
        let socket = socket.clone();
        let state = state.clone();
        let offer = payload.to_owned();
        tokio::spawn(async move {
            let advertised_ip = state.status_handler.config().external_ip;
            let signal = match state.status_handler.config().backend_address.as_deref() {
                Some(backend_addr) => match negotiate_with_backend(
                    &state,
                    backend_addr,
                    &network_id.to_string(),
                    &offer,
                    advertised_ip,
                )
                .await
                {
                    Ok(answer) => format!("CONNECTRESPONSE {connection_id} {answer}"),
                    Err(err) => {
                        warn!(%address, "NetherNet LAN backend negotiation failed: {}", err);
                        format!("CONNECTERROR {connection_id} 11")
                    }
                },
                None => format!("CONNECTERROR {connection_id} 11"),
            };

            if let Ok(encoded) = encode_message(network_id, sender_id, &signal) {
                let _ = socket.send_to(&encoded, address).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bedrock::config::{CURRENT_BEDROCK_MC_PROTOCOL, CURRENT_BEDROCK_MC_VERSION};

    #[tokio::test]
    async fn nethernet_listener_serves_http_status_and_udp_ping() {
        let config = BedrockConfig {
            enabled: true,
            bind_address: "127.0.0.1:0".to_string(),
            lan_discovery: false,
            motd: "Vine NetherNet Test".to_string(),
            level_name: "ProxyWorld".to_string(),
            max_players: 42,
            network_id: 0x1234_5678_9abc_def0,
            ..BedrockConfig::default()
        };

        // Pre-allocate an ephemeral port available on loopback
        let probe_tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = probe_tcp.local_addr().unwrap();
        drop(probe_tcp);

        let mut cfg = config;
        cfg.bind_address = bound_addr.to_string();

        let online = Arc::new(AtomicUsize::new(7));
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let listener_handle = tokio::spawn(run_bedrock_listener(cfg, online, shutdown_rx));
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 1. Test HTTP GET /v1/join
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{bound_addr}/v1/join"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["name"], "Vine NetherNet Test");
        assert_eq!(body["protocol"], CURRENT_BEDROCK_MC_PROTOCOL);
        assert_eq!(body["version"], CURRENT_BEDROCK_MC_VERSION);
        assert_eq!(body["level"], "ProxyWorld");
        assert_eq!(body["players"], 7);
        assert_eq!(body["maxPlayers"], 42);

        // 2. Test UDP Ping -> Pong on ICE socket
        let client_udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let ping = encode_message(999, 0x1234_5678_9abc_def0, "Ping").unwrap();
        client_udp.send_to(&ping, bound_addr).await.unwrap();

        let mut recv_buf = [0u8; 512];
        let (len, _) =
            tokio::time::timeout(Duration::from_secs(2), client_udp.recv_from(&mut recv_buf))
                .await
                .unwrap()
                .unwrap();

        assert_eq!(
            decode_packet(&recv_buf[..len]),
            Some(NetherNetPacket::Message {
                sender_id: 0x1234_5678_9abc_def0,
                recipient_id: 999,
                data: "Pong".to_string(),
            })
        );

        let _ = shutdown_tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), listener_handle).await;
    }
}
