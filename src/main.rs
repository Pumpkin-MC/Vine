use std::path::PathBuf;
use tokio::sync::broadcast;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use vine::config::Config;
use vine::proxy::ProxyServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,vine=debug")),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let (config_path, init_settings) = parse_cli_args(std::env::args().skip(1));

    info!("Loading configuration from '{}'...", config_path.display());
    let config = match Config::load_or_create(&config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!(
                "Failed to load configuration file '{}': {}",
                config_path.display(),
                e
            );
            std::process::exit(1);
        }
    };

    if init_settings {
        std::process::exit(0);
    }

    let server = ProxyServer::with_config_path(config, config_path)?;

    let (shutdown_tx, shutdown_rx) = broadcast::channel(4);
    let signal_shutdown_tx = shutdown_tx.clone();

    tokio::spawn(async move {
        if let Err(err) = tokio::signal::ctrl_c().await {
            error!("Failed to listen for shutdown signal: {}", err);
        } else {
            info!("Received shutdown signal (Ctrl+C). Initiating graceful shutdown...");
            let _ = signal_shutdown_tx.send(());
        }
    });

    if let Err(e) = server.run_with_shutdown(shutdown_tx, shutdown_rx).await {
        error!("Fatal proxy error: {}", e);
        std::process::exit(1);
    }

    info!("Vine proxy shut down successfully.");
    std::process::exit(0);
}

fn parse_cli_args(args: impl Iterator<Item = String>) -> (PathBuf, bool) {
    let mut init_settings = false;
    let mut config_path = None;

    for arg in args {
        if arg == "--init-settings" || arg == "--initSettings" {
            init_settings = true;
        } else if config_path.is_none() {
            config_path = Some(PathBuf::from(arg));
        }
    }

    (
        config_path.unwrap_or_else(|| PathBuf::from("vine.toml")),
        init_settings,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cli_args_defaults() {
        let (path, init) = parse_cli_args(std::iter::empty());
        assert_eq!(path, PathBuf::from("vine.toml"));
        assert!(!init);
    }

    #[test]
    fn test_parse_cli_args_init_settings() {
        let (path, init) = parse_cli_args(["--init-settings".to_string()].into_iter());
        assert_eq!(path, PathBuf::from("vine.toml"));
        assert!(init);

        let (path2, init2) =
            parse_cli_args(["custom.toml".to_string(), "--initSettings".to_string()].into_iter());
        assert_eq!(path2, PathBuf::from("custom.toml"));
        assert!(init2);
    }
}
