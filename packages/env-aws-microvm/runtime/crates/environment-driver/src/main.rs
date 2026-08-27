use std::sync::Arc;
use std::time::Duration;

use environment_driver::{AwsDriver, Driver, HttpRelayDriver};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "environment_driver=info".into()),
        )
        .init();
    let required = |name: &str| -> anyhow::Result<String> {
        let value = std::env::var(name).map_err(|_| anyhow::anyhow!("{name} is required"))?;
        anyhow::ensure!(!value.is_empty(), "{name} cannot be empty");
        Ok(value)
    };
    let listen = std::env::var("ENVIRONMENT_DRIVER_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8790".into())
        .parse::<std::net::SocketAddr>()?;
    anyhow::ensure!(
        listen.ip().is_loopback(),
        "Environment driver must bind loopback"
    );
    let mut drivers = Vec::<(String, Arc<dyn Driver>)>::new();
    if std::env::var("ENVIRONMENT_AWS_DRIVER_ENABLED").as_deref() == Ok("true") {
        drivers.push((
            "aws-microvm".into(),
            Arc::new(AwsDriver::from_env().await?) as Arc<dyn Driver>,
        ));
    }
    if let Ok(url) = std::env::var("ENVIRONMENT_CUSTOMER_DRIVER_URL") {
        let token = std::env::var("ENVIRONMENT_CUSTOMER_DRIVER_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());
        drivers.push((
            "customer".into(),
            Arc::new(HttpRelayDriver::new(url, token, Duration::from_secs(30))?),
        ));
    }
    let tool_directory = std::env::var("ENVIRONMENT_TOOL_DIRECTORY")
        .unwrap_or_else(|_| "/usr/local/share/aex-tools".into());
    let app = environment_driver::router(
        required("ENVIRONMENT_DRIVER_TOKEN")?,
        drivers,
        tool_directory,
    )?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "Environment driver listening");
    axum::serve(listener, app).await?;
    Ok(())
}
