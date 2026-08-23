//! Immutable artifact installation: bundles, bindings, uid identities, session secrets.

use super::*;

pub(crate) struct InstalledBinding {
    pub(crate) seal: SealedBinding,
    pub(crate) bundle_path: PathBuf,
    pub(crate) identity: Option<ToolIdentity>,
}

/// Per-generation registry for the kernel identity assigned to each immutable binding. A hash
/// collision is rejected instead of aliasing two secret subsets onto one uid. The very large uid
/// range makes a collision vanishingly unlikely, while the explicit binding cap keeps the registry
/// and collision analysis bounded.
pub(crate) struct BindingIdentityRegistry {
    pub(crate) by_ref: HashMap<String, Option<ToolIdentity>>,
    pub(crate) by_uid: HashMap<u32, String>,
    pub(crate) uid_min: u32,
    pub(crate) uid_span: u32,
    pub(crate) max_bindings: usize,
}

impl BindingIdentityRegistry {
    pub(crate) fn production() -> Self {
        Self::with_bounds(
            MANAGED_BINDING_UID_MIN,
            MANAGED_BINDING_UID_SPAN,
            MAX_PREPARED_BINDINGS,
        )
    }

    pub(crate) fn with_bounds(uid_min: u32, uid_span: u32, max_bindings: usize) -> Self {
        Self {
            by_ref: HashMap::new(),
            by_uid: HashMap::new(),
            uid_min,
            uid_span,
            max_bindings,
        }
    }

    pub(crate) fn allocate(
        &mut self,
        binding_ref: &str,
        sandbox_identity: Option<ToolIdentity>,
    ) -> Result<Option<ToolIdentity>, EnvironmentError> {
        if let Some(identity) = self.by_ref.get(binding_ref) {
            return Ok(*identity);
        }
        if self.by_ref.len() >= self.max_bindings {
            return Err(environment_error(
                EnvironmentErrorCode::ResourceExhausted,
                false,
                "physical generation has reached the prepared-binding limit",
            ));
        }
        let Some(sandbox_identity) = sandbox_identity else {
            self.by_ref.insert(binding_ref.to_owned(), None);
            return Ok(None);
        };
        if self.uid_span == 0 {
            return Err(environment_error(
                EnvironmentErrorCode::ResourceExhausted,
                false,
                "managed-binding uid range is empty",
            ));
        }
        let digest = Sha256::digest(binding_ref.as_bytes());
        let hash = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix"));
        let uid = self.uid_min + (hash % u64::from(self.uid_span)) as u32;
        if self.by_uid.contains_key(&uid) {
            return Err(environment_error(
                EnvironmentErrorCode::BindingConflict,
                false,
                "managed-binding uid collision",
            ));
        }
        let identity = ToolIdentity {
            uid,
            gid: sandbox_identity.gid,
            supervisor_uid: sandbox_identity.supervisor_uid,
        };
        self.by_uid.insert(uid, binding_ref.to_owned());
        self.by_ref.insert(binding_ref.to_owned(), Some(identity));
        Ok(Some(identity))
    }
}

/// Deliberately cannot be serialized or formatted. Values are zeroized when a generation exits.
pub(crate) struct SessionSecrets {
    pub(crate) generation: String,
    pub(crate) declared: BTreeSet<String>,
    pub(crate) values: HashMap<String, String>,
}

impl Drop for SessionSecrets {
    fn drop(&mut self) {
        for value in self.values.values_mut() {
            value.zeroize();
        }
        self.values.clear();
    }
}

impl Environment {
    pub async fn install_bundle(
        &self,
        metadata: InstallBundleMetadata,
        bytes: &[u8],
    ) -> Result<InstallReceipt, EnvironmentError> {
        if metadata.descriptor.target != ArtifactTarget::LinuxArm64 {
            return Err(invalid("the AWS environment accepts linux-arm64 artifacts"));
        }
        if metadata.descriptor.bytes.get() > brain_protocol::MAX_TOOL_BUNDLE_BYTES as u64
            || metadata.descriptor.bytes.get() != bytes.len() as u64
            || metadata.descriptor.object.bytes != bytes.len() as u64
            || metadata.descriptor.object.sha256 != metadata.descriptor.bundle_digest
            || hex::encode(Sha256::digest(bytes)) != metadata.descriptor.bundle_digest.as_str()
        {
            return Err(invalid(
                "bundle bytes do not match the immutable descriptor",
            ));
        }
        let required_env = metadata
            .descriptor
            .required_env
            .iter()
            .map(|name| name.as_str())
            .collect::<BTreeSet<_>>();
        if metadata.descriptor.required_env.len() > brain_protocol::MAX_SESSION_SECRET_NAMES
            || required_env.len() != metadata.descriptor.required_env.len()
            || metadata.descriptor.required_env.iter().any(|name| {
                !environment_name_is_valid(name.as_str())
                    || reserved_tool_environment(name.as_str())
            })
        {
            return Err(invalid(
                "bundle descriptor contains an invalid or reserved environment name",
            ));
        }
        let digest = metadata.descriptor.bundle_digest.to_string();
        let mut bundles = self.artifacts.bundles.write().await;
        if let Some((existing, _)) = bundles.get(&digest) {
            return if canonical_equal(existing, &metadata.descriptor)? {
                Ok(InstallReceipt {
                    installed: true,
                    replayed: true,
                })
            } else {
                Err(environment_error(
                    EnvironmentErrorCode::BindingConflict,
                    false,
                    "bundle digest is already installed with a different descriptor",
                ))
            };
        }
        let path = self.cfg.tool_dir.join(format!("{digest}.mjs"));
        let temporary = self.cfg.tool_dir.join(format!(".{digest}.install"));
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            // Every managed binding may read the verified module through the shared Tool group,
            // but no untrusted Tool process may rewrite code after digest verification.
            options.mode(0o640);
        }
        let mut file = options
            .open(&temporary)
            .await
            .map_err(|_| unavailable("could not stage the Tool bundle"))?;
        if file.write_all(bytes).await.is_err()
            || file.flush().await.is_err()
            || file.sync_all().await.is_err()
        {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(unavailable("could not stage the Tool bundle"));
        }
        drop(file);
        tokio::fs::rename(&temporary, &path)
            .await
            .map_err(|_| unavailable("could not install the Tool bundle"))?;
        bundles.insert(digest, (metadata.descriptor, path));
        Ok(InstallReceipt {
            installed: true,
            replayed: false,
        })
    }

    pub async fn install_binding(
        &self,
        request: InstallBindingRequest,
    ) -> Result<InstallReceipt, EnvironmentError> {
        let target = self.require_target().await?;
        if request.binding.root_id.as_str() != target.root_id {
            return Err(environment_error(
                EnvironmentErrorCode::BindingConflict,
                false,
                "binding is outside this target root",
            ));
        }
        let descriptor = request.binding.bundle.as_ref().ok_or_else(|| {
            environment_error(
                EnvironmentErrorCode::CapabilityUnavailable,
                false,
                "managed execution requires an immutable Tool bundle",
            )
        })?;
        if descriptor.contract_digest != request.binding.contract_digest {
            return Err(environment_error(
                EnvironmentErrorCode::BindingConflict,
                false,
                "bundle and binding contract digests differ",
            ));
        }
        if descriptor.environment_name != request.binding.environment_name {
            return Err(environment_error(
                EnvironmentErrorCode::BindingConflict,
                false,
                "bundle and binding environment names differ",
            ));
        }
        let bundles = self.artifacts.bundles.read().await;
        let bundle_path = match bundles.get(descriptor.bundle_digest.as_str()) {
            Some((installed, path)) if canonical_equal(installed, descriptor)? => path.clone(),
            _ => return Err(invalid("binding references a bundle that is not installed")),
        };
        drop(bundles);
        let requires_undeclared_secret = self
            .artifacts
            .secrets
            .read()
            .await
            .get(request.binding.session_id.as_str())
            .is_some_and(|secrets| {
                descriptor
                    .required_env
                    .iter()
                    .any(|name| !secrets.declared.contains(name.as_str()))
            });
        if requires_undeclared_secret {
            return Err(environment_error(
                EnvironmentErrorCode::BindingConflict,
                false,
                "binding requires environment outside the prepared session secret union",
            ));
        }
        let mut bindings = self.artifacts.bindings.write().await;
        if let Some(existing) = bindings.get(request.binding_ref.as_str()) {
            return if canonical_equal(&existing.seal, &request.binding)? {
                Ok(InstallReceipt {
                    installed: true,
                    replayed: true,
                })
            } else {
                Err(environment_error(
                    EnvironmentErrorCode::BindingConflict,
                    false,
                    "binding_ref is already installed with a different seal",
                ))
            };
        }
        let identity = self
            .artifacts
            .identities
            .lock()
            .await
            .allocate(request.binding_ref.as_str(), self.cfg.sandboxing.identity())?;
        bindings.insert(
            request.binding_ref.to_string(),
            InstalledBinding {
                seal: request.binding,
                bundle_path,
                identity,
            },
        );
        Ok(InstallReceipt {
            installed: true,
            replayed: false,
        })
    }

    pub async fn install_object_file(
        &self,
        metadata: InstallObjectMetadata,
        temporary: PathBuf,
        actual_bytes: u64,
        actual_sha256: &str,
    ) -> Result<InstallReceipt, EnvironmentError> {
        if metadata.object.bytes != actual_bytes || actual_sha256 != metadata.object.sha256.as_str()
        {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(invalid("object bytes do not match the immutable reference"));
        }
        let digest = metadata.object.sha256.as_str();
        let path = self.cfg.object_dir.join(digest);
        if path.exists() {
            let existing = tokio::fs::metadata(&path)
                .await
                .map_err(|_| unavailable("installed object is unavailable"))?;
            let _ = tokio::fs::remove_file(&temporary).await;
            return if existing.is_file() && existing.len() == actual_bytes {
                Ok(InstallReceipt {
                    installed: true,
                    replayed: true,
                })
            } else {
                Err(invalid("object digest is installed with different bytes"))
            };
        }
        if tokio::fs::rename(&temporary, &path).await.is_err() {
            let _ = tokio::fs::remove_file(&temporary).await;
            let existing = tokio::fs::metadata(&path)
                .await
                .map_err(|_| unavailable("could not atomically install object input"))?;
            if !existing.is_file() || existing.len() != actual_bytes {
                return Err(unavailable("could not atomically install object input"));
            }
            return Ok(InstallReceipt {
                installed: true,
                replayed: true,
            });
        }
        Ok(InstallReceipt {
            installed: true,
            replayed: false,
        })
    }

    pub async fn open_file_export(
        &self,
        request: SandboxFileRequest,
    ) -> Result<(FileEntry, std::fs::File), EnvironmentError> {
        self.fence(&request.target, request.expected_generation.as_str())
            .await?;
        let files = self.workspace_files()?;
        let path = request.path.to_string();
        let reader = blocking_file(move || files.open_reader(&path)).await?;
        Ok((file_entry(&reader.entry)?, reader.file))
    }

    pub async fn install_secrets(
        &self,
        request: InstallSecretsRequest,
    ) -> Result<InstallReceipt, EnvironmentError> {
        let target = self.require_target().await?;
        if request.generation != target.generation {
            return Err(generation_conflict());
        }
        let declared = request.env_names.iter().cloned().collect::<BTreeSet<_>>();
        if let Err(refusal) = secret_material_fits(&request.env_names, &request.values) {
            return Err(invalid(format!(
                "secret material is outside the canonical bounded environment union: {refusal}"
            )));
        }
        if declared.iter().any(|name| reserved_tool_environment(name)) {
            return Err(invalid(
                "secret environment name conflicts with the trusted Tool runtime boundary",
            ));
        }
        let installed_requirements_are_declared = self
            .artifacts
            .bindings
            .read()
            .await
            .values()
            .filter(|binding| binding.seal.session_id.as_str() == request.session_id)
            .flat_map(|binding| {
                binding
                    .seal
                    .bundle
                    .iter()
                    .flat_map(|bundle| bundle.required_env.iter())
            })
            .all(|name| declared.contains(name.as_str()));
        if !installed_requirements_are_declared {
            return Err(invalid(
                "prepared environment-name union omits an installed binding requirement",
            ));
        }
        let mut secrets = self.artifacts.secrets.write().await;
        if let Some(existing) = secrets.get(&request.session_id) {
            return if existing.generation == request.generation
                && existing.declared == declared
                && existing.values == request.values
            {
                Ok(InstallReceipt {
                    installed: true,
                    replayed: true,
                })
            } else {
                Err(environment_error(
                    EnvironmentErrorCode::GenerationConflict,
                    false,
                    "secret material conflicts with the installed generation",
                ))
            };
        }
        secrets.insert(
            request.session_id,
            SessionSecrets {
                generation: request.generation,
                declared,
                values: request.values,
            },
        );
        Ok(InstallReceipt {
            installed: true,
            replayed: false,
        })
    }
}
