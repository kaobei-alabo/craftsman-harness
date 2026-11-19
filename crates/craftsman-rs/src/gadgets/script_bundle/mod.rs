//! Immutable admission of supplied, preprepared script bundle DATA.
//!
//! Neither a candidate nor checked data grants source consent, an image grant,
//! task/root authority, a VM call or real file operations. No discovery, resolver,
//! filesystem, compiler, dispatcher, catalog or session is used here.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use unicode_normalization::UnicodeNormalization;

mod encoding;
mod metadata;
#[cfg(test)]
mod tests;

const CODE_FILES: usize = 64; // Includes the entry, also for python.
const CODE_BYTES: usize = 256 * 1024;
const ASSETS: usize = 64;
const ASSET_BYTES: usize = 256 * 1024;
const ASSET_FILE_BYTES: usize = 64 * 1024;
const ARTIFACTS: usize = 32;
const ARTIFACT_BYTES: usize = 256 * 1024;
const METADATA_BYTES: usize = 64 * 1024;
const PATH_BYTES: usize = 1024;
const COMPONENT_BYTES: usize = 255;
const NAME_BYTES: usize = 16 * 1024;
const KEY_BYTES: usize = 64 * 1024;
const IDENTIFIER_BYTES: usize = 128;
const PATH_DEPTH: usize = 8;
const SOURCES: usize = 64;
const PACKAGES: usize = 32;
const DEPENDENCIES: usize = 256; // Shared code and package edge work.
const DEPENDENCY_DEPTH: usize = 8;
const ENCODED_BYTES: usize = 1024 * 1024;
const IMAGE_BYTES: usize = 32 * 1024 * 1024;

const JS_IMAGE: &str = "244f18978b7a0165660974d558f5d25d04e3e79b6dc2692de10884a49682802a";
const PY_IMAGE: &str = "47b3d888c28d2618fa9c0ac9c7afa062e4dcea1733f36c99fca531c35f02faaf";
const JS_LOADER: &str = "10efb52de693a7e634ddb9765f580c9e41fd84caeff4dc73a04cdd7393b5cac1";
const PY_LOADER: &str = "757e239434130144b76c4ef1ac1498fde6804e399a797d4db892a9aa4a4f6815";
const SUCRASE: &str = "7c89971b57bbf87c548f7a213fe776361ba3d0637faf03302194a11c92ccc2a2";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdmissionError {
    Bound,
    Utf8,
    Metadata,
    Profile,
    Name,
    Collision,
    Closure,
    Digest,
    Image,
}
impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Script bundle data admission refused.")
    }
}
impl std::error::Error for AdmissionError {}
type Result<T> = std::result::Result<T, AdmissionError>;

/// Borrowed original data, never a file/path/consent authority. No IO occurs.
pub(crate) struct SuppliedFile<'a> {
    name: &'a [u8],
    bytes: &'a [u8],
}
impl<'a> SuppliedFile<'a> {
    pub(crate) fn new(name: &'a [u8], bytes: &'a [u8]) -> Self {
        Self { name, bytes }
    }
}

/// Immutable borrowed candidate. Construction labels nothing checked or trusted.
pub(crate) struct CandidateBundle<'a> {
    entry: SuppliedFile<'a>,
    modules: &'a [SuppliedFile<'a>],
    assets: &'a [SuppliedFile<'a>],
    artifacts: &'a [SuppliedFile<'a>],
    metadata: &'a [u8],
    image: Option<&'a [u8]>,
}
impl<'a> CandidateBundle<'a> {
    pub(crate) fn new(
        entry: SuppliedFile<'a>,
        modules: &'a [SuppliedFile<'a>],
        assets: &'a [SuppliedFile<'a>],
        artifacts: &'a [SuppliedFile<'a>],
        metadata: &'a [u8],
        image: Option<&'a [u8]>,
    ) -> Self {
        Self {
            entry,
            modules,
            assets,
            artifacts,
            metadata,
            image,
        }
    }

    pub(crate) fn admit(self) -> Result<CheckedBundle> {
        // All original count/byte bounds precede UTF8/metadata decoding, name
        // keys, hashing, encoding and owned copies. Escaped metadata is scanned
        // without heap copies before Deserialize may allocate bounded strings.
        self.bounds()?;
        metadata::preflight(self.metadata)?;
        let declaration: Declaration =
            serde_json::from_slice(self.metadata).map_err(|_| AdmissionError::Metadata)?;
        declaration.bounds()?;
        let runtime = RuntimeProfile::validate(&declaration.runtime)?;
        let files = self.files(runtime)?;
        declaration.validate(&files, runtime)?;

        // Real JSON escaping/base64/framing work, not a forecast or clipped
        // prefix. The code-only guest envelope and complete data view must both
        // fit. Future work/input still need their own full call admission.
        let encoded_bytes = encoding::measure(&self, runtime)?;
        let guest_code_bytes = encoding::measure_guest_code(&self, runtime)?;
        let digests = files
            .iter()
            .map(|file| {
                let actual = sha256(file.file.bytes);
                let record = declaration.file(file.kind, file.name)?;
                if parse_digest(&record.sha256)? != actual {
                    return Err(AdmissionError::Digest);
                }
                Ok(actual)
            })
            .collect::<Result<Vec<_>>>()?;
        if let Some(image) = self.image {
            if image.len() != runtime.image_len()
                || sha256(image) != parse_digest(runtime.image_sha256())?
            {
                return Err(AdmissionError::Image);
            }
        }

        let mut binding = Sha256::new();
        framed(&mut binding, b"craftsman-script-bundle-data-v1");
        framed(&mut binding, runtime.name().as_bytes());
        framed(&mut binding, runtime.engine().as_bytes());
        framed(&mut binding, runtime.image_sha256().as_bytes());
        framed(&mut binding, runtime.loader_sha256().as_bytes());
        framed(
            &mut binding,
            runtime.transform_sha256().unwrap_or("").as_bytes(),
        );
        binding.update((runtime.image_len() as u64).to_be_bytes());
        binding.update([u8::from(self.image.is_some())]);
        framed(&mut binding, self.metadata);
        // Preserve supplied order/entry semantics. Duplicates were refused;
        // there is no map overwrite or sorting that can erase a conflict.
        for (file, digest) in files.iter().zip(&digests) {
            framed(&mut binding, file.kind.name().as_bytes());
            framed(&mut binding, file.file.name);
            framed(&mut binding, file.file.bytes);
            binding.update(digest);
        }
        let digest = binding.finalize().into();

        // Only after *every* validation/hash/complete encoding bound passed do
        // original bytes get copied into the immutable snapshot.
        let mut owned = files
            .into_iter()
            .zip(digests)
            .map(|(file, digest)| CheckedFile {
                name: file.name.into(),
                bytes: file.file.bytes.into(),
                digest,
            });
        let entry = owned.next().ok_or(AdmissionError::Closure)?;
        let modules = owned.by_ref().take(self.modules.len()).collect();
        let assets = owned.by_ref().take(self.assets.len()).collect();
        let artifacts = owned.collect();
        Ok(CheckedBundle {
            runtime,
            entry,
            modules,
            assets,
            artifacts,
            metadata: self.metadata.into(),
            image: self.image.map(Into::into),
            digest,
            encoded_bytes,
            guest_code_bytes,
        })
    }

    fn bounds(&self) -> Result<()> {
        if self.modules.len() >= CODE_FILES
            || self.assets.len() > ASSETS
            || self.artifacts.len() > ARTIFACTS
            || self.metadata.len() > METADATA_BYTES
            || self.image.is_some_and(|image| image.len() > IMAGE_BYTES)
        {
            return Err(AdmissionError::Bound);
        }
        let mut code = 0;
        let mut names = 0;
        for file in std::iter::once(&self.entry).chain(self.modules) {
            add(&mut code, file.bytes.len(), CODE_BYTES)?;
            file_bounds(file, CODE_BYTES, &mut names)?;
        }
        for (files, total_cap, file_cap) in [
            (self.assets, ASSET_BYTES, ASSET_FILE_BYTES),
            (self.artifacts, ARTIFACT_BYTES, ARTIFACT_BYTES),
        ] {
            let mut total = 0;
            for file in files {
                add(&mut total, file.bytes.len(), total_cap)?;
                file_bounds(file, file_cap, &mut names)?;
            }
        }
        Ok(())
    }

    fn files(&self, runtime: RuntimeProfile) -> Result<Vec<File<'_>>> {
        let mut files = Vec::new();
        let mut namespace = BTreeSet::new();
        let mut artifact_keys = BTreeSet::new();
        let mut keys = 0;
        let mut guest_bytes = 0;
        for (kind, supplied) in std::iter::once((FileKind::Entry, &self.entry))
            .chain(self.modules.iter().map(|f| (FileKind::Module, f)))
            .chain(self.assets.iter().map(|f| (FileKind::Asset, f)))
            .chain(self.artifacts.iter().map(|f| (FileKind::Artifact, f)))
        {
            let name = text(supplied.name)?;
            let logical = logical_name(name, kind, runtime)?;
            let key = name_key(&logical)?;
            add(&mut keys, key.len(), KEY_BYTES)?;
            let set = if kind == FileKind::Artifact {
                &mut artifact_keys
            } else {
                &mut namespace
            };
            if !set.insert(key.clone())
                || set.iter().any(|other| {
                    other != &key && (parent_of(other, &key) || parent_of(&key, other))
                })
            {
                return Err(AdmissionError::Collision);
            }
            if matches!(kind, FileKind::Entry | FileKind::Module) {
                let source = text(supplied.bytes)?;
                if source.contains('\0') {
                    return Err(AdmissionError::Utf8);
                }
                // Actual JS run counts original map keys as well as source.
                if runtime != RuntimeProfile::Cpython {
                    add(&mut guest_bytes, source.len() + name.len(), CODE_BYTES)?;
                }
            }
            files.push(File {
                kind,
                name,
                file: supplied,
            });
        }
        Ok(files)
    }
}

/// Original checked bytes/identity are still untrusted executable data.
/// No Debug/Serialize/Clone, mutable getters or bypass constructors.
pub(crate) struct CheckedBundle {
    runtime: RuntimeProfile,
    entry: CheckedFile,
    modules: Box<[CheckedFile]>,
    assets: Box<[CheckedFile]>,
    artifacts: Box<[CheckedFile]>,
    metadata: Box<[u8]>,
    image: Option<Box<[u8]>>,
    digest: [u8; 32],
    encoded_bytes: usize,
    guest_code_bytes: usize,
}
impl CheckedBundle {
    pub(crate) fn runtime(&self) -> RuntimeProfile {
        self.runtime
    }
    pub(crate) fn entry(&self) -> &CheckedFile {
        &self.entry
    }
    pub(crate) fn modules(&self) -> &[CheckedFile] {
        &self.modules
    }
    pub(crate) fn assets(&self) -> &[CheckedFile] {
        &self.assets
    }
    pub(crate) fn artifacts(&self) -> &[CheckedFile] {
        &self.artifacts
    }
    pub(crate) fn metadata_bytes(&self) -> &[u8] {
        &self.metadata
    }
    pub(crate) fn image_bytes(&self) -> Option<&[u8]> {
        self.image.as_deref()
    }
    pub(crate) fn content_digest(&self) -> &[u8; 32] {
        &self.digest
    }
    pub(crate) fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }
    pub(crate) fn guest_code_bytes(&self) -> usize {
        self.guest_code_bytes
    }
}
pub(crate) struct CheckedFile {
    name: Box<str>,
    bytes: Box<[u8]>,
    digest: [u8; 32],
}
impl CheckedFile {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(crate) fn sha256(&self) -> &[u8; 32] {
        &self.digest
    }
}

/// Fixed host reference data, not a selectable/approved runtime or VM handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeProfile {
    JavaScript,
    TypeScript,
    Cpython,
}
impl RuntimeProfile {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::JavaScript => "javascript-esm-vfs-v3",
            Self::TypeScript => "typescript-sucrase-vfs-v3",
            Self::Cpython => "python-flat-v4",
        }
    }
    pub(crate) fn engine(self) -> &'static str {
        match self {
            Self::JavaScript | Self::TypeScript => "quickjs-ng-d7215b0",
            Self::Cpython => "cpython-3.14.0",
        }
    }
    pub(crate) fn image_sha256(self) -> &'static str {
        match self {
            Self::JavaScript | Self::TypeScript => JS_IMAGE,
            Self::Cpython => PY_IMAGE,
        }
    }
    pub(crate) fn image_len(self) -> usize {
        match self {
            Self::JavaScript | Self::TypeScript => 1_381_447,
            Self::Cpython => 19_741_769,
        }
    }
    pub(crate) fn loader_sha256(self) -> &'static str {
        match self {
            Self::JavaScript | Self::TypeScript => JS_LOADER,
            Self::Cpython => PY_LOADER,
        }
    }
    pub(crate) fn transform_sha256(self) -> Option<&'static str> {
        match self {
            Self::JavaScript | Self::TypeScript => Some(SUCRASE),
            Self::Cpython => None,
        }
    }
    fn validate(record: &RuntimeRecord) -> Result<Self> {
        let profile = match record.profile.as_str() {
            "javascript-esm-vfs-v3" => Self::JavaScript,
            "typescript-sucrase-vfs-v3" => Self::TypeScript,
            "python-flat-v4" => Self::Cpython,
            _ => return Err(AdmissionError::Profile),
        };
        if record.engine != profile.engine()
            || record.image_sha256 != profile.image_sha256()
            || record.image_bytes != profile.image_len() as u64
            || record.loader_sha256 != profile.loader_sha256()
        {
            return Err(AdmissionError::Profile);
        }
        match (profile.transform_sha256(), &record.transform) {
            (None, None) => {}
            (Some(digest), Some(transform))
                if transform.name == "sucrase"
                    && transform.version == "3.35.1"
                    && transform.sha256 == digest => {}
            _ => return Err(AdmissionError::Profile),
        }
        Ok(profile)
    }
    fn runtime_module(self, name: &str) -> bool {
        match self {
            Self::JavaScript | Self::TypeScript => {
                matches!(name, "node:fs" | "node:fs/promises" | "node:path")
            }
            Self::Cpython => {
                // Known image-local modules, not native-package resolution.
                matches!(
                    name,
                    "_struct"
                        | "sys"
                        | "os"
                        | "io"
                        | "json"
                        | "pathlib"
                        | "importlib"
                        | "traceback"
                        | "componentize_py_types"
                )
            }
        }
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FileKind {
    Entry,
    Module,
    Asset,
    Artifact,
}
impl FileKind {
    fn name(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Module => "module",
            Self::Asset => "asset",
            Self::Artifact => "artifact",
        }
    }
}
struct File<'a> {
    kind: FileKind,
    name: &'a str,
    file: &'a SuppliedFile<'a>,
}

// These are bounded parsed metadata DATA. No Serialize/Debug or arbitrary
// fields/authority flags. All original metadata bytes are retained separately.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    version: u8,
    runtime: RuntimeRecord,
    files: Vec<FileRecord>,
    sources: Vec<SourceRecord>,
    direct_packages: Vec<Pin>,
    dependencies: Vec<Edge>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeRecord {
    profile: String,
    engine: String,
    image_sha256: String,
    image_bytes: u64,
    loader_sha256: String,
    transform: Option<TransformRecord>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformRecord {
    name: String,
    version: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    kind: FileKind,
    name: String,
    sha256: String,
    source: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRecord {
    id: String,
    origin: String,
    license: String,
    distribution: Distribution,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Distribution {
    Local,
    Package {
        name: String,
        version: String,
        artifact: String,
        requires: Vec<Pin>,
    },
    Stdlib {
        name: String,
        version: String,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    name: String,
    version: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edge {
    from: String,
    target: Target,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Target {
    Module { name: String },
    Asset { name: String },
    RuntimeModule { name: String },
}
impl Target {
    fn name(&self) -> &str {
        match self {
            Self::Module { name } | Self::Asset { name } | Self::RuntimeModule { name } => name,
        }
    }
}

impl Declaration {
    fn bounds(&self) -> Result<()> {
        if self.files.len() > CODE_FILES + ASSETS + ARTIFACTS
            || self.sources.len() > SOURCES
            || self.direct_packages.len() > PACKAGES
            || self.dependencies.len() > DEPENDENCIES
        {
            return Err(AdmissionError::Bound);
        }
        let mut names = 0;
        let mut edges = self.dependencies.len();
        let mut packages = 0;
        for file in &self.files {
            bounded_name(&file.name, PATH_BYTES, &mut names)?;
            identifier(&file.source)?;
            parse_digest(&file.sha256)?;
        }
        for source in &self.sources {
            identifier(&source.id)?;
            prose(&source.origin, 512)?;
            prose(&source.license, 1024)?;
            match &source.distribution {
                Distribution::Local => {}
                Distribution::Package {
                    name,
                    version,
                    artifact,
                    requires,
                } => {
                    add(&mut packages, 1, PACKAGES)?;
                    package_name(name)?;
                    exact_version(version)?;
                    bounded_name(artifact, COMPONENT_BYTES, &mut names)?;
                    if requires.len() > PACKAGES {
                        return Err(AdmissionError::Bound);
                    }
                    add(&mut edges, requires.len(), DEPENDENCIES)?;
                    for pin in requires {
                        pin.validate()?;
                    }
                }
                Distribution::Stdlib { name, version } => {
                    identifier(name)?;
                    exact_version(version)?;
                }
            }
        }
        for pin in &self.direct_packages {
            pin.validate()?;
        }
        for edge in &self.dependencies {
            bounded_name(&edge.from, PATH_BYTES, &mut names)?;
            bounded_name(edge.target.name(), PATH_BYTES, &mut names)?;
        }
        Ok(())
    }

    fn file(&self, kind: FileKind, name: &str) -> Result<&FileRecord> {
        self.files
            .iter()
            .find(|file| file.kind == kind && file.name == name)
            .ok_or(AdmissionError::Closure)
    }
    fn source(&self, id: &str) -> Result<usize> {
        self.sources
            .iter()
            .position(|source| source.id == id)
            .ok_or(AdmissionError::Closure)
    }
    fn package(&self, pin: &Pin) -> Result<usize> {
        self.sources
            .iter()
            .position(|source| {
                matches!(&source.distribution,
                Distribution::Package { name, version, .. }
                    if name == &pin.name && version == &pin.version)
            })
            .ok_or(AdmissionError::Closure)
    }
    fn validate(&self, files: &[File<'_>], runtime: RuntimeProfile) -> Result<()> {
        if self.version != 1 || self.files.len() != files.len() || self.sources.is_empty() {
            return Err(AdmissionError::Closure);
        }
        let mut record_names = BTreeSet::new();
        for record in &self.files {
            if !record_names.insert((record.kind.name(), record.name.as_str()))
                || !files
                    .iter()
                    .any(|f| f.kind == record.kind && f.name == record.name)
            {
                return Err(AdmissionError::Closure);
            }
            self.source(&record.source)?;
        }
        let mut source_ids = BTreeSet::new();
        let mut package_names = BTreeSet::new();
        let mut package_graph = vec![Vec::new(); self.sources.len()];
        for (index, source) in self.sources.iter().enumerate() {
            if !source_ids.insert(name_key(&source.id)?)
                || !self.files.iter().any(|file| file.source == source.id)
            {
                return Err(AdmissionError::Closure);
            }
            match &source.distribution {
                Distribution::Local => {
                    if self
                        .files
                        .iter()
                        .any(|f| f.source == source.id && f.kind == FileKind::Artifact)
                    {
                        return Err(AdmissionError::Closure);
                    }
                }
                Distribution::Package {
                    name,
                    artifact,
                    requires,
                    ..
                } => {
                    if !package_names.insert(package_key(name)) {
                        return Err(AdmissionError::Collision);
                    }
                    let artifact_record = self.file(FileKind::Artifact, artifact)?;
                    if artifact_record.source != source.id
                        || self
                            .files
                            .iter()
                            .filter(|f| f.source == source.id && f.kind == FileKind::Artifact)
                            .count()
                            != 1
                        || !self
                            .files
                            .iter()
                            .any(|f| f.source == source.id && f.kind != FileKind::Artifact)
                    {
                        return Err(AdmissionError::Closure);
                    }
                    let mut pins = BTreeSet::new();
                    for pin in requires {
                        if !pins.insert(package_key(&pin.name)) {
                            return Err(AdmissionError::Collision);
                        }
                        package_graph[index].push(self.package(pin)?);
                    }
                }
                Distribution::Stdlib { name, version } => {
                    // First supported supplied stdlib wrapper is cpython struct.
                    // Its declared provenance remains data; actual bytes are
                    // hashed, and no ambient stdlib/package lookup takes place.
                    if runtime != RuntimeProfile::Cpython || name != "struct" || version != "3.14.0"
                        || self.files.iter().filter(|f| f.source == source.id).count() != 1
                        || !self.files.iter().any(|f| f.source == source.id && f.kind == FileKind::Module && f.name == "struct")
                        || !self.dependencies.iter().any(|edge| edge.from == "struct"
                            && matches!(&edge.target, Target::RuntimeModule { name } if name == "_struct"))
                    {
                        return Err(AdmissionError::Profile);
                    }
                }
            }
        }
        let mut roots = BTreeSet::new();
        let mut package_roots = Vec::new();
        for pin in &self.direct_packages {
            if !roots.insert(package_key(&pin.name)) {
                return Err(AdmissionError::Collision);
            }
            package_roots.push(self.package(pin)?);
        }
        let reachable = closure(&package_graph, &package_roots)?;
        if self.sources.iter().enumerate().any(|(index, s)| {
            matches!(s.distribution, Distribution::Package { .. }) && !reachable[index]
        }) {
            return Err(AdmissionError::Closure);
        }

        let code_count = files
            .iter()
            .take_while(|f| matches!(f.kind, FileKind::Entry | FileKind::Module))
            .count();
        let visible_count = files
            .iter()
            .take_while(|f| f.kind != FileKind::Artifact)
            .count();
        let mut graph = vec![Vec::new(); visible_count];
        let mut edge_keys = BTreeSet::new();
        let mut actual_package_edges = BTreeSet::new();
        for edge in &self.dependencies {
            let from = files[..code_count]
                .iter()
                .position(|f| f.name == edge.from)
                .ok_or(AdmissionError::Closure)?;
            let target_kind = match edge.target {
                Target::Module { .. } => "module",
                Target::Asset { .. } => "asset",
                Target::RuntimeModule { .. } => "runtime_module",
            };
            if !edge_keys.insert((from, target_kind, edge.target.name())) {
                return Err(AdmissionError::Collision);
            }
            if let Target::RuntimeModule { name } = &edge.target {
                if !runtime.runtime_module(name) {
                    return Err(AdmissionError::Profile);
                }
                continue;
            }
            let to = files[..visible_count]
                .iter()
                .position(|f| {
                    f.name == edge.target.name()
                        && match edge.target {
                            Target::Module { .. } => {
                                matches!(f.kind, FileKind::Entry | FileKind::Module)
                            }
                            Target::Asset { .. } => f.kind == FileKind::Asset,
                            Target::RuntimeModule { .. } => false,
                        }
                })
                .ok_or(AdmissionError::Closure)?;
            graph[from].push(to);
            let source_from =
                self.source(&self.file(files[from].kind, files[from].name)?.source)?;
            let source_to = self.source(&self.file(files[to].kind, files[to].name)?.source)?;
            if source_from != source_to
                && let Distribution::Package { name, version, .. } =
                    &self.sources[source_to].distribution
            {
                let pins = match &self.sources[source_from].distribution {
                    Distribution::Package { requires, .. } => requires,
                    Distribution::Local | Distribution::Stdlib { .. } => &self.direct_packages,
                };
                if !pins
                    .iter()
                    .any(|pin| pin.name == *name && pin.version == *version)
                {
                    return Err(AdmissionError::Closure);
                }
                actual_package_edges.insert((source_from, source_to));
            }
        }
        if closure(&graph, &[0])?.iter().any(|reachable| !reachable) {
            return Err(AdmissionError::Closure);
        }
        for (from, targets) in package_graph.iter().enumerate() {
            if targets
                .iter()
                .any(|to| !actual_package_edges.contains(&(from, *to)))
            {
                return Err(AdmissionError::Closure);
            }
        }
        Ok(())
    }
}
impl Pin {
    fn validate(&self) -> Result<()> {
        package_name(&self.name)?;
        exact_version(&self.version)
    }
}

fn closure(graph: &[Vec<usize>], roots: &[usize]) -> Result<Vec<bool>> {
    fn visit(
        node: usize,
        graph: &[Vec<usize>],
        path: &mut [bool],
        heights: &mut [usize],
        depth: usize,
    ) -> Result<usize> {
        if depth > DEPENDENCY_DEPTH || path[node] {
            return Err(AdmissionError::Closure);
        }
        if heights[node] != 0 {
            return Ok(heights[node]);
        }
        path[node] = true;
        let mut height = 1;
        for &next in &graph[node] {
            height = height.max(visit(next, graph, path, heights, depth + 1)? + 1);
            if height > DEPENDENCY_DEPTH {
                return Err(AdmissionError::Closure);
            }
        }
        path[node] = false;
        heights[node] = height;
        Ok(height)
    }
    let mut heights = vec![0; graph.len()];
    let mut path = vec![false; graph.len()];
    // Memoized longest paths: <=nodes+edges work; no exponential diamond
    // expansion, and a later longer route still checks the child's full height.
    for &root in roots {
        visit(root, graph, &mut path, &mut heights, 1)?;
    }
    Ok(heights.into_iter().map(|height| height != 0).collect())
}

fn logical_name(name: &str, kind: FileKind, runtime: RuntimeProfile) -> Result<String> {
    if runtime == RuntimeProfile::Cpython && matches!(kind, FileKind::Entry | FileKind::Module) {
        if kind == FileKind::Entry {
            if name != "main.py" {
                return Err(AdmissionError::Name);
            }
            return Ok(name.into());
        }
        identifier(name)?;
        // Deliberately narrower than arbitrary sys.modules: known unloaded
        // six/struct fixture names, or the private bundle_ source namespace.
        // Never repair an original helper/package-directory name into an alias.
        if !(name.starts_with("bundle_") && name.len() > "bundle_".len())
            && !matches!(name, "six" | "struct")
        {
            return Err(AdmissionError::Name);
        }
        return Ok(format!("{name}.py"));
    }
    if runtime == RuntimeProfile::Cpython && !name.is_ascii() {
        return Err(AdmissionError::Name);
    }
    path_name(name)?;
    if kind == FileKind::Artifact && name.contains('/') {
        return Err(AdmissionError::Name);
    }
    if matches!(kind, FileKind::Entry | FileKind::Module) {
        let supported = name.ends_with(".js")
            || name.ends_with(".mjs")
            || (runtime == RuntimeProfile::TypeScript
                && name.ends_with(".ts")
                && !name.ends_with(".d.ts"));
        if !supported {
            return Err(AdmissionError::Profile);
        }
    } else if kind == FileKind::Asset && executable_extension(&name_key(name)?) {
        return Err(AdmissionError::Name);
    }
    Ok(name.into())
}
fn path_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > PATH_BYTES
        || name.starts_with('/')
        || forbidden_name_characters(name)
    {
        return Err(AdmissionError::Name);
    }
    let mut depth = 0;
    for component in name.split('/') {
        depth += 1;
        if depth > PATH_DEPTH
            || component.is_empty()
            || component.len() > COMPONENT_BYTES
            || component == "."
            || component == ".."
            || component.starts_with(char::is_whitespace)
            || component.ends_with(char::is_whitespace)
            || component.ends_with('.')
        {
            return Err(AdmissionError::Name);
        }
        let key = name_key(component)?;
        if key.contains('/')
            || forbidden_name_characters(&key)
            || key == "."
            || key == ".."
            || key.starts_with(char::is_whitespace)
            || key.ends_with(char::is_whitespace)
            || key.ends_with('.')
            || device(&key)
            || matches!(key.as_str(), "node_modules" | "__pycache__" | ".craftsman")
            || key.starts_with(".craftsman-")
        {
            return Err(AdmissionError::Name);
        }
    }
    Ok(())
}
// The same original/canonical denial rules apply to every path component.
// Keys only refuse ambiguous data; they never replace accepted spellings.
fn forbidden_name_characters(name: &str) -> bool {
    name.contains(['\\', '\0', ':', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(|c| c.is_control()
            || matches!(c, '\u{feff}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}
fn name_key(name: &str) -> Result<String> {
    let mut bytes = 0;
    for c in name.nfkc().flat_map(char::to_lowercase) {
        add(&mut bytes, c.len_utf8(), PATH_BYTES * 4)?;
    }
    Ok(name.nfkc().flat_map(char::to_lowercase).collect())
}
fn device(key: &str) -> bool {
    let base = key.split('.').next().unwrap_or("").trim_end_matches(' ');
    matches!(
        base,
        "con" | "prn" | "aux" | "nul" | "clock$" | "conin$" | "conout$"
    ) || ["com", "lpt"].iter().any(|prefix| {
        base.strip_prefix(prefix)
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
    })
}
fn executable_extension(name: &str) -> bool {
    let extension = name.rsplit('.').next().unwrap_or("");
    [
        "js", "mjs", "cjs", "ts", "jsx", "tsx", "py", "pyc", "pyo", "so", "pyd", "dll", "node",
        "wasm",
    ]
    .iter()
    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}
fn identifier(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    if name.len() > IDENTIFIER_BYTES
        || !bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(AdmissionError::Name);
    }
    Ok(())
}
fn package_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > IDENTIFIER_BYTES
        || !name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
    {
        return Err(AdmissionError::Name);
    }
    Ok(())
}
fn package_key(name: &str) -> String {
    let mut key = String::new();
    let mut separator = false;
    for c in name.chars() {
        if matches!(c, '.' | '_' | '-') {
            if !separator {
                key.push('-');
            }
            separator = true;
        } else {
            key.push(c);
            separator = false;
        }
    }
    key
}
fn exact_version(version: &str) -> Result<()> {
    if version.len() > 32 {
        return Err(AdmissionError::Metadata);
    }
    let mut count = 0;
    for part in version.split('.') {
        count += 1;
        if count > 3
            || part.is_empty()
            || part.len() > 10
            || !part.bytes().all(|b| b.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return Err(AdmissionError::Metadata);
        }
    }
    if count != 3 {
        return Err(AdmissionError::Metadata);
    }
    Ok(())
}
fn prose(value: &str, cap: usize) -> Result<()> {
    if value.is_empty()
        || value.len() > cap
        || value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(AdmissionError::Metadata);
    }
    Ok(())
}
fn file_bounds(file: &SuppliedFile<'_>, bytes: usize, names: &mut usize) -> Result<()> {
    if file.name.len() > PATH_BYTES || file.bytes.len() > bytes {
        return Err(AdmissionError::Bound);
    }
    add(names, file.name.len(), NAME_BYTES)
}
fn bounded_name(name: &str, cap: usize, names: &mut usize) -> Result<()> {
    if name.len() > cap {
        return Err(AdmissionError::Bound);
    }
    add(names, name.len(), NAME_BYTES * 4)
}
fn add(total: &mut usize, amount: usize, cap: usize) -> Result<()> {
    *total = total
        .checked_add(amount)
        .filter(|sum| *sum <= cap)
        .ok_or(AdmissionError::Bound)?;
    Ok(())
}
fn text(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| AdmissionError::Utf8)
}
fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn parse_digest(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return Err(AdmissionError::Metadata);
    }
    let mut digest = [0; 32];
    for (target, pair) in digest.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let hex = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
        *target = hex(pair[0]) * 16 + hex(pair[1]);
    }
    Ok(digest)
}
fn framed(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}
fn parent_of(parent: &str, child: &str) -> bool {
    child
        .strip_prefix(parent)
        .is_some_and(|suffix| suffix.starts_with('/'))
}
