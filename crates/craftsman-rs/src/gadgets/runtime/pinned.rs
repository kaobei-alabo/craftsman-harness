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

fn refused(message: &'static str) -> GadgetError {
    GadgetError::new(ErrorKind::InvalidGadget, message)
}

fn identity(metadata: &cap_std::fs::Metadata) -> (u64, u64) {
    use cap_fs_ext::MetadataExt;
    // Taken from an open handle, so the by-handle fields are populated on
    // every platform.
    (metadata.dev(), metadata.ino())
}

/// Every ancestor of `root`, and `root` itself, is a directory and not a link.
fn real_directories(root: &Path) -> Result<(), GadgetError> {
    if !root.is_absolute()
        || root.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(refused(
            "the user configuration root must be an absolute path",
        ));
    }
    for ancestor in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor).map_err(io_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(refused(
                "the user configuration root must not be reached through a link",
            ));
        }
    }
    Ok(())
}

/// The user configuration root and one gadget directory below it, both held
/// by handle. Nothing here resolves a link.
struct Anchored {
    root: cap_std::fs::Dir,
    root_identity: (u64, u64),
    gadgets_identity: (u64, u64),
    directory: cap_std::fs::Dir,
    directory_identity: (u64, u64),
}

impl Anchored {
    fn open(config: &Path, id: &str) -> Result<Self, GadgetError> {
        use cap_fs_ext::DirExt;
        real_directories(config)?;
        let root = cap_std::fs::Dir::open_ambient_dir(config, cap_std::ambient_authority())
            .map_err(io_error)?;
        let gadgets = root
            .open_dir_nofollow("gadgets")
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => GadgetError::new(
                    ErrorKind::NotFound,
                    "the pinned user gadget is not installed",
                ),
                _ => refused("the user gadget root must be a real directory"),
            })?;
        let directory = gadgets
            .open_dir_nofollow(id)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => GadgetError::new(
                    ErrorKind::NotFound,
                    "the pinned user gadget is not installed",
                ),
                _ => refused("gadget directories must not be symlinks"),
            })?;
        Ok(Self {
            root_identity: identity(&root.dir_metadata().map_err(io_error)?),
            gadgets_identity: identity(&gadgets.dir_metadata().map_err(io_error)?),
            root,
            directory_identity: identity(&directory.dir_metadata().map_err(io_error)?),
            directory,
        })
    }

    /// One regular, single-link file of at most `maximum` bytes, read through
    /// the held directory and unchanged across the read.
    fn read(&self, name: &str, maximum: usize) -> Result<Vec<u8>, GadgetError> {
        use cap_fs_ext::MetadataExt;
        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        if self
            .directory
            .symlink_metadata(name)
            .map_err(io_error)?
            .file_type()
            .is_symlink()
        {
            return Err(refused(
                "pinned gadget inputs must be regular single-link files",
            ));
        }
        let file = self.directory.open_with(name, &options).map_err(io_error)?;
        let before = file.metadata().map_err(io_error)?;
        let modified_before = before.modified().map_err(io_error)?;
        if !before.is_file() || before.nlink() != 1 {
            return Err(refused(
                "pinned gadget inputs must be regular single-link files",
            ));
        }
        if before.len() > maximum as u64 {
            return Err(refused("a pinned gadget input exceeds its size limit"));
        }
        let mut bytes = Vec::new();
        (&file)
            .take((maximum + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        let after = file.metadata().map_err(io_error)?;
        let modified_after = after.modified().map_err(io_error)?;
        if bytes.len() > maximum
            || bytes.len() as u64 != before.len()
            || after.len() != before.len()
            || after.nlink() != 1
            || identity(&after) != identity(&before)
            || modified_after != modified_before
        {
            return Err(refused("a pinned gadget input changed while it was read"));
        }
        Ok(bytes)
    }

    /// After the reads: the root and the directory reached by path are still
    /// the objects held by handle. Returns the directory's canonical path,
    /// which is the source label a stored grant carries.
    fn confirm(&self, config: &Path, id: &str) -> Result<PathBuf, GadgetError> {
        use cap_fs_ext::DirExt;
        real_directories(config)?;
        let open = |path: &Path| {
            cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())
                .and_then(|directory| directory.dir_metadata())
                .map(|metadata| identity(&metadata))
                .map_err(io_error)
        };
        // Re-open the relative entries without following links from the
        // retained root, before and after obtaining the stored source label.
        let relative = || -> Result<(), GadgetError> {
            let gadgets = self.root.open_dir_nofollow("gadgets").map_err(io_error)?;
            let directory = gadgets.open_dir_nofollow(id).map_err(io_error)?;
            if identity(&self.root.dir_metadata().map_err(io_error)?) != self.root_identity
                || identity(&gadgets.dir_metadata().map_err(io_error)?) != self.gadgets_identity
                || identity(&directory.dir_metadata().map_err(io_error)?) != self.directory_identity
            {
                return Err(refused("the pinned gadget source moved while it was read"));
            }
            Ok(())
        };
        relative()?;
        let source = config
            .join("gadgets")
            .join(id)
            .canonicalize()
            .map_err(io_error)?;
        if open(config)? != self.root_identity
            || open(&source)? != self.directory_identity
            || identity(&self.directory.dir_metadata().map_err(io_error)?)
                != self.directory_identity
        {
            return Err(refused("the pinned gadget source moved while it was read"));
        }
        relative()?;
        Ok(source)
    }
}

/// Blocking work of the pinned path, run on the runtime's blocking pool and
/// awaited. Every use goes through one erased closure and result type, so
/// the runtime's task code is compiled once for all of them instead of once
/// per closure. Where it runs, what is awaited and how a failure is reported
/// are the same as a direct `spawn_blocking` of the closure.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, GadgetError> + Send + 'static,
) -> Result<T, GadgetError> {
    type Erased = Box<dyn FnOnce() -> Result<Box<dyn std::any::Any + Send>, GadgetError> + Send>;
    let erased: Erased =
        Box::new(move || work().map(|value| Box::new(value) as Box<dyn std::any::Any + Send>));
    let value = tokio::task::spawn_blocking(erased)
        .await
        .map_err(io_error)??;
    value
        .downcast::<T>()
        .map(|value| *value)
        .map_err(|_| io_error("pinned blocking work returned another type"))
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
        let granted = blocking(move || {
            grants.approved_exact(
                &copy.manifest.id,
                &copy.source,
                copy.hash.as_ref().unwrap(),
                &copy.manifest.capabilities,
            )
        })
        .await?;
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
        // The guest's return is handed back byte for byte. It is not parsed,
        // re-serialized or escaped here: the host caller applies its own size
        // and shape checks to exactly what the guest wrote, including
        // repeated members and padding that a re-serialization would drop.
        let (logs, logs_truncated) = store.data().capture.contents();
        Ok(CallResult {
            json,
            duration: started.elapsed(),
            network_calls: store.data().network_calls,
            logs,
            logs_truncated,
        })
    }

    /// The checks of `call_pinned` that precede compilation, and nothing
    /// after them: the installed bytes are the pinned ones, declare no
    /// capability and have a stored grant for exactly those bytes at that
    /// source. No guest is compiled, described or run, and nothing is
    /// approved here.
    pub(crate) async fn pinned_source_approved(
        &self,
        pin: &PinnedSource,
    ) -> Result<(), GadgetError> {
        if !valid_id(&pin.id)
            || !hash_shape(&pin.component_sha256)
            || !hash_shape(&pin.manifest_sha256)
        {
            return Err(GadgetError::new(
                ErrorKind::InvalidArgs,
                "invalid pinned gadget source",
            ));
        }
        let (bundle, _) = self.pinned_bundle(pin).await?;
        let grants = self.grants.clone();
        let granted = blocking(move || {
            grants.approved_exact(
                &bundle.manifest.id,
                &bundle.source,
                bundle.hash.as_ref().unwrap(),
                &bundle.manifest.capabilities,
            )
        })
        .await?;
        if !granted {
            return Err(GadgetError::new(
                ErrorKind::NeedsApproval,
                "the pinned gadget has no stored approval for these exact bytes",
            ));
        }
        Ok(())
    }

    /// The pin of the user-level gadget as it is installed now: its id and
    /// the SHA-256 of both files, read through the same held directory and
    /// with the same file rules as the pinned call. It selects a source and
    /// approves nothing; `pinned_source_approved` still has to accept it.
    pub(crate) async fn installed_pin(&self, id: &str) -> Result<PinnedSource, GadgetError> {
        if !valid_id(id) {
            return Err(GadgetError::new(
                ErrorKind::InvalidArgs,
                "invalid pinned gadget source",
            ));
        }
        let config = self.config.clone();
        let id = id.to_owned();
        blocking(move || {
            let anchored = Anchored::open(&config, &id)?;
            let manifest_sha256 = digest(&anchored.read("gadget.toml", MAX_MANIFEST_BYTES)?);
            let component_sha256 = digest(&anchored.read("gadget.wasm", MAX_COMPONENT_BYTES)?);
            anchored.confirm(&config, &id)?;
            Ok(PinnedSource {
                id,
                component_sha256,
                manifest_sha256,
            })
        })
        .await
    }

    /// Reads the user-level source afresh, without the scan cache and without
    /// resolving any link, and returns it only if it is exactly the pinned one.
    ///
    /// The directory is reached by handle from the user configuration root:
    /// every ancestor of that root is a real directory, and `gadgets` and the
    /// gadget's own directory are each opened without following a link. Both
    /// files are read through that one directory handle. Each must be a
    /// regular file with a single link, within its size bound, and unchanged
    /// in identity, size and modification time across the read. The component
    /// bytes returned are the bytes that were hashed.
    ///
    /// The ancestor checks are observations made before and after the read.
    /// They are not an operating-system guarantee against another process of
    /// the same user renaming a directory at the same moment.
    async fn pinned_bundle(&self, pin: &PinnedSource) -> Result<(Bundle, Vec<u8>), GadgetError> {
        let config = self.config.clone();
        let shadow = self.workspace.join(".craftsman/gadgets").join(&pin.id);
        let pin = pin.clone();
        blocking(move || {
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
            let anchored = Anchored::open(&config, &pin.id)?;
            let manifest_bytes = anchored.read("gadget.toml", MAX_MANIFEST_BYTES)?;
            let text = std::str::from_utf8(&manifest_bytes).map_err(io_error)?;
            let manifest = Manifest::parse(text, &pin.id)?;
            let manifest_hash = digest(&manifest_bytes);
            let bytes = anchored.read("gadget.wasm", MAX_COMPONENT_BYTES)?;
            let hash = digest(&bytes);
            // The root and the directory are still the ones that were opened.
            let source = anchored.confirm(&config, &pin.id)?;
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
            blocking(move || Component::new(&engine, bytes).map_err(wasm_error)).await?;
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
