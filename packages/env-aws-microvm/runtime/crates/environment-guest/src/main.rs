//! `environment-guest` binary: read config from the environment, serve the ABI until killed.

use std::sync::Arc;

use environment_guest::{Config, Environment, Server};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("environment_guest=info".parse()?),
        )
        .with_target(false)
        .init();
    let cfg = Config::from_env()?;
    let environment: Arc<Environment> = Environment::new(cfg)?;
    let server = Server::bind(environment.clone()).await?;
    let serve = tokio::spawn(server.run());
    tokio::select! {
        r = serve => { r??; }
        _ = shutdown_signal() => {
            tracing::info!("shutting down: cancelling operations");
            environment.shutdown().await;
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
