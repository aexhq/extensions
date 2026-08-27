//! Narrow operator CLI for immutable image publication and explicit lifecycle inspection.

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use environment_lambda::REGION;
use environment_lambda::canary::{
    NetworkBoundaryCanaryConfig, NoRespawnCanaryConfig, run_network_boundary_canary,
    run_no_respawn_canary,
};
use environment_lambda::control::{Control, ControlError, is_terminated};
use environment_lambda::image::{self, PublishConfig};

#[derive(Parser)]
#[command(
    name = "environment-lambda",
    about = "Aex Environment Lambda MicroVM operator"
)]
struct Cli {
    #[arg(long, default_value = REGION)]
    region: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Image {
        #[command(subcommand)]
        command: ImageCommand,
    },
    List,
    Get {
        id: String,
    },
    Suspend {
        id: String,
    },
    Resume {
        id: String,
    },
    Terminate {
        id: String,
    },
    TerminateImage {
        #[arg(long)]
        image_arn: String,
        #[arg(long, required = true, action = clap::ArgAction::SetTrue)]
        confirm_terminate_image: bool,
    },
}

#[derive(Subcommand)]
enum ImageCommand {
    Publish {
        #[arg(long)]
        source_sha: String,
        #[arg(long)]
        binary: std::path::PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        bucket: String,
        #[arg(long)]
        build_role: String,
        #[arg(long)]
        log_group: String,
        #[arg(long = "egress-connector", required = true)]
        egress_connectors: Vec<String>,
    },
    Status {
        name: String,
    },
    /// Exits non-zero when the version's recorded MicroVM base is no longer the newest managed
    /// base version (AWS retires bases on its own calendar; an EXPIRED base cannot run).
    RebuildDue {
        #[arg(long)]
        image_arn: String,
        #[arg(long)]
        image_version: String,
    },
    Dockerfile,
    /// Destructive dev release gate: launches and always terminates one exact image version.
    Canary {
        #[arg(long)]
        image_arn: String,
        #[arg(long)]
        image_version: String,
        #[arg(long)]
        none_connector: String,
        #[arg(long, required = true, action = clap::ArgAction::SetTrue)]
        confirm_dev_image_canary: bool,
    },
    /// Destructive dev release gate: proves all three connector classes on the exact image.
    NetworkCanary {
        #[arg(long)]
        image_arn: String,
        #[arg(long)]
        image_version: String,
        #[arg(long)]
        none_connector: String,
        #[arg(long)]
        allowlist_connector: String,
        #[arg(long)]
        public_connector: String,
        #[arg(long)]
        gateway_authority: String,
        #[arg(long, required = true, action = clap::ArgAction::SetTrue)]
        confirm_dev_network_canary: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("environment_lambda=info".parse()?),
        )
        .with_target(false)
        .init();
    let cli = Cli::parse();
    let aws = environment_lambda::aws_config(&cli.region).await;
    let control = Control::from_sdk_config(&aws, &cli.region)?;
    match cli.command {
        Command::Image { command } => image_command(&control, &aws, command).await,
        Command::List => {
            for vm in control.list().await? {
                println!("{}\t{:?}", vm.id, vm.state);
            }
            Ok(())
        }
        Command::Get { id } => {
            let vm = control.get(&id).await?;
            println!(
                "{}\t{:?}\t{}",
                vm.id,
                vm.state,
                vm.endpoint.unwrap_or_default()
            );
            Ok(())
        }
        Command::Suspend { id } => control.suspend(&id).await.map_err(Into::into),
        Command::Resume { id } => control.resume(&id).await.map_err(Into::into),
        Command::Terminate { id } => control.terminate(&id).await.map_err(Into::into),
        Command::TerminateImage {
            image_arn,
            confirm_terminate_image,
        } => {
            debug_assert!(confirm_terminate_image, "clap enforces the flag");
            terminate_image(&control, &image_arn).await
        }
    }
}

async fn terminate_image(control: &Control, image_arn: &str) -> anyhow::Result<()> {
    let targets = control
        .list()
        .await?
        .into_iter()
        .filter(|target| target.image_arn == image_arn)
        .collect::<Vec<_>>();
    for target in &targets {
        if !is_terminated(&target.state) {
            match control.terminate(&target.id).await {
                Ok(()) | Err(ControlError::Gone(_) | ControlError::Unknown(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        for attempt in 0..60 {
            match control.get(&target.id).await {
                Ok(current) if is_terminated(&current.state) => break,
                Err(ControlError::Gone(_)) => break,
                Ok(_) | Err(ControlError::Retryable(_) | ControlError::Throttled(_))
                    if attempt < 59 =>
                {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                Ok(current) => anyhow::bail!(
                    "MicroVM {} remained {:?} after termination",
                    current.id,
                    current.state
                ),
                Err(error) => return Err(error.into()),
            }
        }
    }
    println!(
        "Terminated {} MicroVM(s) for image {image_arn}",
        targets.len()
    );
    Ok(())
}

async fn image_command(
    control: &Control,
    aws: &aws_config::SdkConfig,
    command: ImageCommand,
) -> anyhow::Result<()> {
    match command {
        ImageCommand::Dockerfile => {
            print!("{}", image::dockerfile());
            Ok(())
        }
        ImageCommand::Canary {
            image_arn,
            image_version,
            none_connector,
            confirm_dev_image_canary,
        } => {
            debug_assert!(confirm_dev_image_canary, "clap enforces the flag");
            let http = environment_lambda::endpoint_http_client_builder().build()?;
            run_no_respawn_canary(
                control,
                &http,
                NoRespawnCanaryConfig {
                    image_arn,
                    image_version,
                    none_connector: environment_core::connector::ConnectorRef::parse(
                        none_connector,
                    )?,
                },
            )
            .await
        }
        ImageCommand::NetworkCanary {
            image_arn,
            image_version,
            none_connector,
            allowlist_connector,
            public_connector,
            gateway_authority,
            confirm_dev_network_canary,
        } => {
            debug_assert!(confirm_dev_network_canary, "clap enforces the flag");
            run_network_boundary_canary(
                control,
                NetworkBoundaryCanaryConfig {
                    image_arn,
                    image_version,
                    connectors: environment_core::connector::ConnectorCatalog::new(
                        environment_core::connector::ConnectorRef::parse(none_connector)?,
                        environment_core::connector::ConnectorRef::parse(public_connector)?,
                        environment_core::connector::ConnectorRef::parse(allowlist_connector)?,
                    ),
                    gateway_authority: environment_core::connector::GatewayAuthority::parse(
                        &gateway_authority,
                    )?,
                },
            )
            .await
        }
        ImageCommand::Publish {
            source_sha,
            binary,
            name,
            bucket,
            build_role,
            log_group,
            egress_connectors,
        } => {
            let bytes =
                std::fs::read(&binary).with_context(|| format!("reading {}", binary.display()))?;
            image::validate_aarch64_elf(&bytes)
                .with_context(|| format!("validating {}", binary.display()))?;
            let zip = image::pack_zip(&bytes)?;
            let s3 = aws_sdk_s3::Client::new(aws);
            let published = image::publish(
                control,
                &s3,
                &PublishConfig {
                    source_sha,
                    name,
                    bucket,
                    build_role_arn: build_role,
                    log_group,
                    egress_connectors,
                },
                zip,
            )
            .await?;
            println!("{}\t{}", published.image_arn, published.image_version);
            Ok(())
        }
        ImageCommand::RebuildDue {
            image_arn,
            image_version,
        } => match image::rebuild_due(control, &image_arn, &image_version).await? {
            Some(newest) => bail!(
                "rebuild due: managed base version {newest} is newer than the recorded base of \
                 {image_arn}@{image_version}"
            ),
            None => {
                println!("current");
                Ok(())
            }
        },
        ImageCommand::Status { name } => {
            let Some(arn) = image::find_image_arn(control, &name).await? else {
                bail!("no image named {name}");
            };
            for version in image::version_summaries(control, &arn).await? {
                println!(
                    "{}\t{}\t{}\tbase={}",
                    version.version, version.state, version.status, version.base_version
                );
            }
            Ok(())
        }
    }
}
