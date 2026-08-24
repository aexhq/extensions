use environment_egress_gateway::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("environment_egress_gateway=info".parse()?),
        )
        .with_target(false)
        .init();
    let config = Config::from_env()?;
    environment_egress_gateway::serve(config).await?;
    Ok(())
}
