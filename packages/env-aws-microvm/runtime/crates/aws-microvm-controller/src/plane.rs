//! AWS client composition for one Environment plane.

use crate::*;

pub struct AwsEnvironmentPlane {
    pub(crate) cfg: AwsEnvironmentConfig,
    pub(crate) control: Control,
    pub(crate) registry: DynamoTargetRegistry,
    pub(crate) definitions: DynamoDefinitionRegistry,
    pub(crate) guest: GuestClient,
    pub(crate) kms: aws_sdk_kms::Client,
    pub(crate) image_arn: tokio::sync::OnceCell<String>,
}

impl AwsEnvironmentPlane {
    pub async fn from_env(cfg: AwsEnvironmentConfig) -> anyhow::Result<Self> {
        let aws = environment_lambda::aws_config(&cfg.region).await;
        let control = Control::with_pacing(
            aws_sdk_lambdamicrovms::Client::new(&aws),
            cfg.region.clone(),
            ControlPacingConfig::from_env()?,
        );
        let http = environment_lambda::endpoint_http_client_builder()
            .pool_max_idle_per_host(64)
            .build()
            .expect("HTTP client configuration");
        let db = aws_sdk_dynamodb::Client::new(&aws);
        Ok(Self {
            registry: DynamoTargetRegistry::new(
                db.clone(),
                &cfg.registry_table,
                cfg.max_materialized_mib,
            ),
            definitions: DynamoDefinitionRegistry::new(db, &cfg.registry_table),
            guest: GuestClient::new(control.clone(), http),
            kms: aws_sdk_kms::Client::new(&aws),
            control,
            cfg,
            image_arn: tokio::sync::OnceCell::new(),
        })
    }

    pub(crate) async fn image_arn(&self) -> EnvironmentResult<String> {
        self.image_arn
            .get_or_try_init(|| async {
                environment_lambda::image::find_image_arn(&self.control, &self.cfg.image)
                    .await
                    .map_err(|error| temporary_from("MicroVM image lookup failed", error))?
                    .ok_or_else(|| {
                        error(
                            EnvironmentErrorCode::CapabilityUnavailable,
                            false,
                            "configured MicroVM image does not exist",
                        )
                    })
            })
            .await
            .cloned()
    }

    pub(crate) async fn sign_capability(&self, capability: &Capability) -> EnvironmentResult<String> {
        use aws_sdk_kms::primitives::Blob;
        use aws_sdk_kms::types::{MessageType, SigningAlgorithmSpec};
        let payload = unsigned_capability_bytes(capability)
            .map_err(|_| invalid("network capability is invalid"))?;
        let digest = Sha256::digest(&payload);
        let response = self
            .kms
            .sign()
            .key_id(&self.cfg.capability_signing_key_id)
            .message(Blob::new(digest.to_vec()))
            .message_type(MessageType::Digest)
            .signing_algorithm(SigningAlgorithmSpec::EcdsaSha256)
            .send()
            .await
            .map_err(|error| temporary_from("network capability signing failed", error))?;
        let signature = response
            .signature()
            .ok_or_else(|| temporary("network capability signature is absent"))?;
        encode_signed_token(&payload, signature.as_ref())
            .map_err(|_| invalid("sealed network policy cannot fit the gateway transport bound"))
    }
}
