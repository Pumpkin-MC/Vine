use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tracing::{debug, error, info, trace};

use crate::bedrock::config::BedrockConfig;
use crate::bedrock::status::BedrockStatusHandler;

/// Runs the Bedrock UDP listener task.
pub async fn run_bedrock_listener(
    config: BedrockConfig,
    online_players: Arc<AtomicUsize>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bind_addr: SocketAddr = config.bind_address.parse()?;
    let socket = UdpSocket::bind(bind_addr).await?;
    let local_addr = socket.local_addr()?;

    info!("=====================================================");
    info!("  Vine Bedrock Listener is active on UDP {}", local_addr);
    info!(
        "  Bedrock Version: {} (Protocol {})",
        config.version_name, config.protocol_version
    );
    info!("=====================================================");

    let status_handler = BedrockStatusHandler::new(config, online_players, local_addr.port());
    let mut buffer = [0u8; 2048];

    loop {
        tokio::select! {
            result = socket.recv_from(&mut buffer) => {
                match result {
                    Ok((length, peer_addr)) => {
                        let packet = &buffer[..length];
                        if let Some(pong_bytes) = status_handler.handle_packet(packet) {
                            trace!("[{}] Received Bedrock status ping, sending pong ({} bytes)", peer_addr, pong_bytes.len());
                            if let Err(e) = socket.send_to(&pong_bytes, peer_addr).await {
                                debug!("[{}] Failed to send Bedrock pong: {}", peer_addr, e);
                            }
                        } else {
                            trace!("[{}] Received unrecognized Bedrock datagram ({} bytes)", peer_addr, length);
                        }
                    }
                    Err(e) => {
                        error!("Bedrock UDP receive error: {}", e);
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                info!("Bedrock listener received shutdown signal, stopping...");
                break;
            }
        }
    }

    Ok(())
}
