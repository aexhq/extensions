//! Pure request/seal validation and network-ceiling mapping.

use crate::*;

pub(crate) fn target_spec(
    cfg: &AwsEnvironmentConfig,
    resources: &ResourceCeiling,
    network: &NetworkCeiling,
    resource_class: &str,
) -> EnvironmentResult<TargetSpec> {
    if resources.max_output_bytes.get() > environment_wire::MAX_TOOL_TERMINAL_INLINE_BYTES as u64
        || resources.timeout_ms.get() > TARGET_LIFETIME_MS
    {
        return Err(error(
            EnvironmentErrorCode::CapabilityUnavailable,
            false,
            "the selected target cannot enforce the requested resource ceiling",
        ));
    }
    TargetSpec::new(
        connector_class(network),
        format!("{}@{}", cfg.image, cfg.image_version),
        resource_class,
        TARGET_MEMORY_MIB,
        canonical_digest(resources)
            .map_err(|_| invalid("resource seal cannot be canonicalized"))?
            .to_string(),
        canonical_digest(network)
            .map_err(|_| invalid("network seal cannot be canonicalized"))?
            .to_string(),
    )
    .map_err(materialization_error)
}

pub(crate) fn validate_resource_ceiling_subset(
    request: &ResourceCeiling,
    physical: &ResourceCeiling,
) -> EnvironmentResult<()> {
    if request.timeout_ms > physical.timeout_ms
        || request.max_output_bytes > physical.max_output_bytes
    {
        return Err(error(
            EnvironmentErrorCode::GenerationConflict,
            false,
            "sandbox resources widen the immutable root target seal",
        ));
    }
    Ok(())
}

pub(crate) fn validate_operation_root_seal(
    envelope: &environment_wire::OperationEnvelope,
    preparation: &PrepareSessionRequest,
) -> EnvironmentResult<()> {
    validate_resource_ceiling_subset(&envelope.resources, &preparation.resources)?;
    if !network_ceiling_is_subset(&envelope.network, &preparation.network) {
        return Err(error(
            EnvironmentErrorCode::GenerationConflict,
            false,
            "operation network policy widens the immutable root target seal",
        ));
    }
    Ok(())
}

pub(crate) fn require_exact_root_seal(
    request: &CreateSandboxRequest,
    preparation: &PrepareSessionRequest,
) -> EnvironmentResult<()> {
    if request.resource_class.as_str() != RESOURCE_CLASS
        || canonical_digest(&request.resources)
            .map_err(|_| invalid("resource seal cannot be canonicalized"))?
            != canonical_digest(&preparation.resources)
                .map_err(|_| invalid("prepared resource seal cannot be canonicalized"))?
        || canonical_digest(&request.network)
            .map_err(|_| invalid("network seal cannot be canonicalized"))?
            != canonical_digest(&preparation.network)
                .map_err(|_| invalid("prepared network seal cannot be canonicalized"))?
    {
        return Err(error(
            EnvironmentErrorCode::GenerationConflict,
            false,
            "environment must use the immutable prepared root seal",
        ));
    }
    Ok(())
}

pub(crate) fn validate_inline_input(
    input: &environment_wire::OperationInput,
) -> EnvironmentResult<()> {
    if input.kind != serde_json::Value::String("inline".into()) {
        return Err(invalid("managed Tool input kind must be inline"));
    }
    let encoded = serde_jcs::to_vec(input)
        .map_err(|_| invalid("managed Tool input cannot be canonicalized"))?;
    if encoded.len() > environment_wire::MAX_MANAGED_TOOL_INPUT_BYTES {
        return Err(invalid(format!(
            "managed Tool input exceeds the {}-byte canonical bound",
            environment_wire::MAX_MANAGED_TOOL_INPUT_BYTES
        )));
    }
    Ok(())
}

pub(crate) fn validate_prepared_binding_projection(
    prepared: &PreparedBindingBundles,
    binding: &SealedBinding,
    root_id: &str,
    session_id: &str,
) -> EnvironmentResult<Vec<ValidatedPreparedBundle>> {
    if binding.root_id.as_str() != root_id || binding.session_id.as_str() != session_id {
        return Err(binding_error(
            "prepared binding is outside the exact root/session scope",
        ));
    }
    let descriptor = validate_managed_binding(binding)?;
    let expected = descriptor
        .layers
        .iter()
        .map(|layer| layer.digest.as_str())
        .collect::<HashSet<_>>();
    let supplied = prepared
        .bundle_digests
        .iter()
        .map(|digest| digest.as_str())
        .collect::<HashSet<_>>();
    if supplied.len() != prepared.bundle_digests.len() || supplied != expected {
        return Err(binding_error(
            "prepared bundle digests do not match the immutable binding descriptor",
        ));
    }
    Ok(descriptor
        .layers
        .iter()
        .map(|layer| ValidatedPreparedBundle {
            bytes: layer.bytes.get(),
            digest: layer.digest.to_string(),
        })
        .collect())
}

/// Rejects malformed or internally inconsistent immutable implementation metadata before it can
/// become a durable binding definition. The guest repeats the byte/digest checks at installation,
/// immediately before the first import of customer code.
pub(crate) fn validate_managed_binding(
    binding: &SealedBinding,
) -> EnvironmentResult<&BundleDescriptor> {
    if binding.extension.as_str() != "@aexhq/env-aws-microvm"
        || binding.protocol != "environment/v1"
        || binding.profile.kind != EnvironmentProfileKind::Computer
        || binding.profile.platform != Some(EnvironmentProfilePlatform::LinuxArm64)
        || binding.profile.network != EnvironmentProfileNetwork::Allowlist
        || binding.profile.recovery != EnvironmentProfileRecovery::Retained
    {
        return Err(error(
            EnvironmentErrorCode::CapabilityUnavailable,
            false,
            "the AWS environment declaration does not match this provider's authoritative profile",
        ));
    }
    if binding.configuration.keys().any(|key| key != "region")
        || binding.configuration.get("region").is_some_and(|region| {
            region.as_str().is_none_or(|requested| {
                std::env::var("AWS_REGION")
                    .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
                    .map_or(true, |actual| actual != requested)
            })
        })
    {
        return Err(error(
            EnvironmentErrorCode::CapabilityUnavailable,
            false,
            "the AWS environment configuration is unsupported by this provider instance",
        ));
    }
    let descriptor = binding.bundle.as_ref().ok_or_else(|| {
        error(
            EnvironmentErrorCode::CapabilityUnavailable,
            false,
            "the AWS environment accepts only immutable computer artifacts",
        )
    })?;
    if descriptor.target != ArtifactTarget::LinuxArm64
        || descriptor.contract_digest != binding.contract_digest
        || descriptor.environment_name != binding.environment_name
    {
        return Err(error(
            EnvironmentErrorCode::CapabilityUnavailable,
            false,
            "the AWS environment accepts only linux-arm64 artifacts with an exact contract and environment seal",
        ));
    }
    let mut layer_bytes = 0_u64;
    let mut layer_digests = HashSet::with_capacity(descriptor.layers.len());
    for layer in &descriptor.layers {
        layer_bytes = layer_bytes.saturating_add(layer.bytes.get());
        if layer.bytes.get() > environment_wire::MAX_TOOL_BUNDLE_BYTES as u64
            || layer.object.bytes != layer.bytes.get()
            || layer.object.sha256 != layer.digest
            || !layer_digests.insert(layer.digest.as_str())
        {
            return Err(binding_error(
                "artifact layer size or object digest conflicts with its immutable seal",
            ));
        }
    }
    if layer_bytes != descriptor.bytes.get()
        || descriptor.bytes.get() > environment_wire::MAX_SESSION_BUNDLE_BYTES as u64
    {
        return Err(binding_error(
            "artifact manifest size conflicts with its immutable layers",
        ));
    }
    if descriptor.required_env.len() > environment_wire::MAX_SESSION_SECRET_NAMES {
        return Err(binding_error(
            "bundle descriptor exceeds the required environment-name bound",
        ));
    }
    let mut env_names = HashSet::with_capacity(descriptor.required_env.len());
    if descriptor.required_env.iter().any(|name| {
        !environment_name_is_valid(name.as_str())
            || reserved_tool_environment(name.as_str())
            || !env_names.insert(name.as_str())
    }) {
        return Err(binding_error(
            "bundle descriptor has invalid, reserved, or repeated environment names",
        ));
    }
    let mut capabilities = HashSet::with_capacity(binding.required_capabilities.len());
    if binding
        .required_capabilities
        .iter()
        .any(|capability| !capabilities.insert(*capability))
    {
        return Err(binding_error("binding repeats a required capability"));
    }
    Ok(descriptor)
}

pub(crate) fn merge_validated_prepared_bundle(
    required: &mut HashMap<String, ValidatedPreparedBundle>,
    bundle: ValidatedPreparedBundle,
) -> EnvironmentResult<()> {
    if let Some(existing) = required.get(&bundle.digest)
        && existing.bytes != bundle.bytes
    {
        return Err(binding_error(
            "one artifact layer digest is sealed with conflicting byte lengths",
        ));
    }
    required.insert(bundle.digest.clone(), bundle);
    Ok(())
}

pub(crate) fn required_bundle_digests(
    request: &PrepareSessionRequest,
) -> EnvironmentResult<HashSet<String>> {
    let mut required = HashSet::new();
    for binding in &request.bindings {
        for digest in &binding.bundle_digests {
            required.insert(digest.to_string());
            if required.len() > MAX_PREPARED_BUNDLES {
                return Err(invalid("preparation exceeds the unique bundle bound"));
            }
        }
    }
    Ok(required)
}

pub(crate) fn connector_class(network: &NetworkCeiling) -> ConnectorClass {
    match network {
        NetworkCeiling::None => ConnectorClass::None,
        NetworkCeiling::Public => ConnectorClass::Public,
        NetworkCeiling::Allowlist(_) => ConnectorClass::Allowlist,
    }
}

pub(crate) fn capability_destinations(
    items: &[NetworkCeilingDestinationsItem],
) -> EnvironmentResult<Vec<CapabilityDestination>> {
    items
        .iter()
        .map(|item| match item {
            NetworkCeilingDestinationsItem::Tls { host, .. } => Ok(CapabilityDestination {
                host: Some(host.as_str().into()),
                cidr: None,
                ports: vec![443],
                protocol: DestinationProtocol::Tls,
            }),
            NetworkCeilingDestinationsItem::Tcp { cidr, ports } => Ok(CapabilityDestination {
                host: None,
                cidr: Some(
                    cidr.as_str()
                        .parse::<Ipv4Net>()
                        .map_err(|_| invalid("allowlist CIDR is invalid"))?,
                ),
                ports: ports
                    .iter()
                    .map(|port| {
                        u16::try_from(port.get()).map_err(|_| invalid("allowlist port is invalid"))
                    })
                    .collect::<EnvironmentResult<Vec<_>>>()?,
                protocol: DestinationProtocol::Tcp,
            }),
        })
        .collect()
}
