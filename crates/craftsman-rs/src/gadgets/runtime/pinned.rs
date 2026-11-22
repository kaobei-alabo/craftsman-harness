//! Host-selected call of one exact user-level gadget source.
//!
//! This path is separate from ordinary gadget calls. It never prompts, never
//! reads a project gadget, and never accepts a rebuild under an older grant.
//! Every check below finishes before any compile, `describe` or guest call.
//! The pin selects a source; it is not an approval. Approval is the stored
//! grant for the bytes actually read.

use super::*;

/// The exact identity a host selected: a gadget id and the SHA-256 of its
/// component and manifest files, as lowercase hex.
#[derive(Clone)]
pub(crate) struct PinnedSource {
    pub(crate) id: String,
    pub(crate) component_sha256: String,
    pub(crate) manifest_sha256: String,
}

fn hash_shape(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl GadgetRuntime {
    /// Calls `function` of the pinned user gadget with empty capabilities.
    ///
    /// Refuses unless, in this order: the pin is well formed; no project
    /// gadget has the same id; the user-level directory and both files are
    /// regular and unlinked; the bytes just read hash to the pinned component
    /// and manifest values; the manifest declares no read, write or HTTP
    /// capability; and a stored grant exists for these exact component bytes
    /// at this source. Only then are those same bytes compiled, described and
    /// run. `allowed_gadgets`, workspace trust and the approval prompt are not
    /// consulted.
    pub(crate) async fn call_pinned(
        &self,
        pin: &PinnedSource,
        function: &str,
        args: serde_json::Value,
    ) -> Result<CallResult, GadgetError> {
        let started = Instant::now();
        if !craftsman_gadget::valid_function_name(function) {
            return Err(GadgetError::new(
                ErrorKind::InvalidArgs,
                "invalid gadget function name",
            ));
        }
        let json = craftsman_gadget::__private::encode(Ok(&args)).map_err(|_| {
            GadgetError::new(ErrorKind::InvalidArgs, "gadget arguments exceed 1 MiB")
        })?;
        if json.len() > MAX_JSON_BYTES {
            return Err(GadgetError::new(
                ErrorKind::InvalidArgs,
                "gadget arguments exceed 1 MiB",
            ));
        }
        if !valid_id(&pin.id)
            || !hash_shape(&pin.component_sha256)
            || !hash_shape(&pin.manifest_sha256)
        {
            return Err(GadgetError::new(
                ErrorKind::InvalidArgs,
                "invalid pinned gadget source",
            ));
        }
        let (bundle, bytes) = self.pinned_bundle(pin).await?;
        let grants = self.grants.clone();
        let copy = bundle.clone();
        let granted = tokio::task::spawn_blocking(move || {
            grants.approved_exact(
                &copy.manifest.id,
                &copy.source,
                copy.hash.as_ref().unwrap(),
                &copy.manifest.capabilities,
            )
        })
        .await
        .map_err(io_error)??;
        if !granted {
            return Err(GadgetError::new(
                ErrorKind::NeedsApproval,
                "the pinned gadget has no stored approval for these exact bytes",
            ));
        }
        let component = self.pinned_component(&bundle, bytes).await?;
        let description = self.describe(&bundle, &component).await?;
        let definition = description
            .functions
            .iter()
            .find(|f| f.name == function)
            .ok_or_else(|| {
                GadgetError::new(ErrorKind::NotFound, "pinned gadget function not found")
            })?;
        let validator = jsonschema::options()
            .with_pattern_options(jsonschema::PatternOptions::fancy_regex().backtrack_limit(10_000))
            .build(&definition.input_schema)
            .map_err(io_error)?;
        if let Err(error) = validator.validate(&args) {
            return Err(GadgetError::new(ErrorKind::InvalidArgs, error.to_string()));
        }
        // The same engine, linker, budget and host state as an ordinary call,
        // always with no read, write or HTTP capability.
        let mut store = self.store(Capabilities::default());
        let budget = store.data().budget.clone();
        let returned = budget
            .run(async {
                let gadget =
                    bindings::Gadget::instantiate_async(&mut store, &component, &self.linker)
                        .await
                        .map_err(wasm_error)?;
                gadget
                    .craftsman_gadget_guest()
                    .call_call(&mut store, function, &json)
                    .await
                    .map_err(wasm_error)
            })
            .await
            .map_err(|error| classify(error, store.data()))?;
        use bindings::exports::craftsman::gadget::guest::CallError;
        let json = returned.map_err(|error| {
            let error = match error {
                CallError::UnknownFunction(message) => {
                    GadgetError::new(ErrorKind::NotFound, message)
                }
                CallError::InvalidArgs(message) => {
                    GadgetError::new(ErrorKind::InvalidArgs, message)
                }
                CallError::Failed(message) => GadgetError::new(
                    store.data().last_failure.unwrap_or(
                        if message.contains("JSON output exceeds 1 MiB") {
                            ErrorKind::OutputLimit
                        } else {
                            ErrorKind::Failed
                        },
                    ),
                    message,
                ),
            };
            classify(error, store.data())
        })?;
        if json.len() > MAX_JSON_BYTES {
            return Err(GadgetError::new(
                ErrorKind::OutputLimit,
                "gadget result exceeds 1 MiB",
            ));
        }
        let value = serde_json::from_str::<serde_json::Value>(&json).map_err(|e| {
            classify(
                GadgetError::new(
                    ErrorKind::InvalidGadget,
                    format!("gadget returned invalid JSON: {e}"),
                ),
                store.data(),
            )
        })?;
        let json = serde_json::to_string(&value).map_err(io_error)?;
        if json.len() > MAX_JSON_BYTES {
            return Err(classify(
                GadgetError::new(ErrorKind::OutputLimit, "gadget result exceeds 1 MiB"),
                store.data(),
            ));
        }
        let (logs, logs_truncated) = store.data().capture.contents();
        Ok(CallResult {
            json,
            duration: started.elapsed(),
            network_calls: store.data().network_calls,
            logs,
            logs_truncated,
        })
    }

    /// Reads the user-level source afresh, without the scan cache, and returns
    /// it only if it is exactly the pinned one. The component bytes returned
    /// are the bytes that were hashed.
    async fn pinned_bundle(&self, pin: &PinnedSource) -> Result<(Bundle, Vec<u8>), GadgetError> {
        let directory = self.config.join("gadgets").join(&pin.id);
        let shadow = self.workspace.join(".craftsman/gadgets").join(&pin.id);
        let pin = pin.clone();
        tokio::task::spawn_blocking(move || {
            match fs::symlink_metadata(&shadow) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(GadgetError::new(
                        ErrorKind::InvalidGadget,
                        "a project gadget shadows the pinned user gadget",
                    ));
                }
                Err(error) => return Err(io_error(error)),
            }
            let kind = match fs::symlink_metadata(&directory) {
                Ok(metadata) => metadata.file_type(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(GadgetError::new(
                        ErrorKind::NotFound,
                        "the pinned user gadget is not installed",
                    ));
                }
                Err(error) => return Err(io_error(error)),
            };
            if !kind.is_dir() || kind.is_symlink() {
                return Err(GadgetError::new(
                    ErrorKind::InvalidGadget,
                    "gadget directories must not be symlinks",
                ));
            }
            let source = directory.canonicalize().map_err(io_error)?;
            let manifest_bytes = read_bounded(&source, "gadget.toml", MAX_MANIFEST_BYTES)?;
            let text = std::str::from_utf8(&manifest_bytes).map_err(io_error)?;
            let manifest = Manifest::parse(text, &pin.id)?;
            let manifest_hash = digest(&manifest_bytes);
            let bytes = read_bounded(&source, "gadget.wasm", MAX_COMPONENT_BYTES)?;
            let hash = digest(&bytes);
            if hash != pin.component_sha256 || manifest_hash != pin.manifest_sha256 {
                return Err(GadgetError::new(
                    ErrorKind::InvalidGadget,
                    "the pinned gadget source changed",
                ));
            }
            if manifest.capabilities != Capabilities::default() {
                return Err(GadgetError::new(
                    ErrorKind::DeniedCapability,
                    "a pinned host gadget must declare no capabilities",
                ));
            }
            Ok((
                Bundle {
                    manifest,
                    manifest_hash,
                    source,
                    project: false,
                    hash: Some(hash),
                },
                bytes,
            ))
        })
        .await
        .map_err(io_error)?
    }

    /// Compiles the bytes that were just hashed for the pin, or returns the
    /// component already compiled from bytes of that same hash.
    async fn pinned_component(
        &self,
        bundle: &Bundle,
        bytes: Vec<u8>,
    ) -> Result<Component, GadgetError> {
        let hash = bundle.hash.as_ref().unwrap();
        if let Some(component) = self.components.lock().unwrap().get(hash).cloned() {
            return Ok(component);
        }
        let engine = self.engine.clone();
        let component =
            tokio::task::spawn_blocking(move || Component::new(&engine, bytes).map_err(wasm_error))
                .await
                .map_err(io_error)??;
        let mut cache = self.components.lock().unwrap();
        if cache.len() >= 16 {
            cache.clear();
        }
        cache.insert(hash.clone(), component.clone());
        Ok(component)
    }

    #[cfg(test)]
    pub(in crate::gadgets) fn described_count(&self) -> usize {
        self.descriptions.lock().unwrap().len()
    }
}
