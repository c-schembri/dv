use std::{
  error::Error,
  fmt, fs, io,
  mem::{align_of, size_of},
  path::{Component, Path, PathBuf},
  sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
  time::UNIX_EPOCH,
};

use quick_xml::{Reader, XmlVersion, events::Event};
use sha2::{Digest, Sha512};

use crate::{
  PackAcquisition, PackKind, PackRequirement, ProjectSpec, RuntimeIdentifierGraph, SdkInventory, load_portable_runtime_graph,
  package::global_packages_directory,
};

const BUNDLED_VERSIONS_FILE: &str = "Microsoft.NETCoreSdk.BundledVersions.props";
const RUNTIME_LIST_FILE: &str = "data/RuntimeList.xml";
const IMPLICIT_FRAMEWORK_REFERENCE: &str = "Microsoft.NETCore.App";
const RID_PLACEHOLDER: &str = "**RID**";
const PORTABLE_GRAPH_FILE: &str = "PortableRuntimeIdentifierGraph.json";
const PACK_INVENTORY_CACHE_DIRECTORY: &str = ".dv/sdk-pack-inventories/v2";
const PACK_INVENTORY_MAGIC: &[u8; 8] = b"DVPKINV\0";
const PACK_INVENTORY_SCHEMA: u32 = 2;
const PACK_FINGERPRINT_BYTES: usize = 64;
const PACK_CHECKSUM_BYTES: usize = 64;
const CACHE_PAYLOAD_OFFSET: usize = PACK_INVENTORY_MAGIC.len() + size_of::<u32>() + PACK_FINGERPRINT_BYTES + PACK_CHECKSUM_BYTES;
const CACHE_HEADER_BYTES: usize = CACHE_PAYLOAD_OFFSET + 4 * size_of::<u32>();
static CACHE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextSpan {
  start: u32,
  len: u32,
}

const _: () = assert!(size_of::<TextSpan>() == 8);
const _: () = assert!(align_of::<TextSpan>() == 4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IndexRange {
  start: u32,
  len: u32,
}

const _: () = assert!(size_of::<IndexRange>() == 8);
const _: () = assert!(align_of::<IndexRange>() == 4);

/// Runtime and apphost inputs selected for one project runtime dimension.
///
/// Variable text lives in one allocation. The two asset families share one
/// contiguous eight-byte span batch so downstream copy planning walks memory
/// linearly without per-asset objects or strings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimePackPlan {
  text: Box<str>,
  assets: Box<[TextSpan]>,
  project: TextSpan,
  sdk_version: TextSpan,
  manifest: TextSpan,
  target_framework: TextSpan,
  requested_runtime_identifier: TextSpan,
  runtime_identifier: TextSpan,
  runtime_pack_id: TextSpan,
  runtime_pack_version: TextSpan,
  runtime_pack_root: TextSpan,
  host_runtime_identifier: TextSpan,
  host_pack_id: TextSpan,
  host_pack_version: TextSpan,
  host_pack_root: TextSpan,
  apphost_template: TextSpan,
  managed_assets: IndexRange,
  native_assets: IndexRange,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<RuntimePackPlan>() == 160);
const _: () = assert!(align_of::<RuntimePackPlan>() == align_of::<usize>());

impl RuntimePackPlan {
  /// Returns the project which requested this runtime dimension.
  pub fn project(&self) -> &str {
    self.get(self.project)
  }

  /// Returns the selected SDK version.
  pub fn sdk_version(&self) -> &str {
    self.get(self.sdk_version)
  }

  /// Returns the selected SDK pack manifest.
  pub fn manifest(&self) -> &str {
    self.get(self.manifest)
  }

  /// Returns the evaluated target framework.
  pub fn target_framework(&self) -> &str {
    self.get(self.target_framework)
  }

  /// Returns the exact RID requested by the project.
  pub fn requested_runtime_identifier(&self) -> &str {
    self.get(self.requested_runtime_identifier)
  }

  /// Returns the nearest compatible RID supplied by the runtime pack.
  pub fn runtime_identifier(&self) -> &str {
    self.get(self.runtime_identifier)
  }

  /// Returns the runtime-pack NuGet identity.
  pub fn runtime_pack_id(&self) -> &str {
    self.get(self.runtime_pack_id)
  }

  /// Returns the runtime-pack version selected by the SDK manifest.
  pub fn runtime_pack_version(&self) -> &str {
    self.get(self.runtime_pack_version)
  }

  /// Returns the resolved runtime-pack directory.
  pub fn runtime_pack_root(&self) -> &str {
    self.get(self.runtime_pack_root)
  }

  /// Returns the nearest compatible RID supplied by the host pack.
  pub fn host_runtime_identifier(&self) -> &str {
    self.get(self.host_runtime_identifier)
  }

  /// Returns the host-pack identity.
  pub fn host_pack_id(&self) -> &str {
    self.get(self.host_pack_id)
  }

  /// Returns the host-pack version selected by the SDK manifest.
  pub fn host_pack_version(&self) -> &str {
    self.get(self.host_pack_version)
  }

  /// Returns the resolved host-pack directory.
  pub fn host_pack_root(&self) -> &str {
    self.get(self.host_pack_root)
  }

  /// Returns the platform apphost template which will be patched at output time.
  pub fn apphost_template(&self) -> &str {
    self.get(self.apphost_template)
  }

  /// Iterates managed runtime assets in runtime-manifest order.
  pub fn managed_assets(&self) -> impl ExactSizeIterator<Item = &str> {
    self.values(self.managed_assets)
  }

  /// Iterates native runtime assets in runtime-manifest order.
  pub fn native_assets(&self) -> impl ExactSizeIterator<Item = &str> {
    self.values(self.native_assets)
  }

  fn values(&self, range: IndexRange) -> impl ExactSizeIterator<Item = &str> {
    self.assets[index_range(range)].iter().map(|span| self.get(*span))
  }

  fn get(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }
}

/// Stable runtime-pack planning failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimePackErrorKind {
  /// Required SDK or package data could not be read.
  Io,
  /// The selected SDK pack manifest is malformed or incomplete.
  InvalidManifest,
  /// The project has no selected runtime dimension.
  RuntimeRequired,
  /// No SDK-declared pack RID is compatible with the requested RID.
  UnsupportedRuntime,
  /// A required runtime or host pack is not installed or restored.
  PackNotFound,
  /// A selected pack points to a missing asset.
  MissingAsset,
  /// NuGet configuration could not provide a global-packages directory.
  Configuration,
  /// A path cannot be represented in the UTF-8 plan table.
  NonUnicodePath,
  /// Compact plan storage exceeded its 32-bit index space.
  TextOverflow,
}

/// A runtime-pack planning failure with stable source-path context.
#[derive(Debug)]
pub struct RuntimePackError {
  kind: RuntimePackErrorKind,
  path: PathBuf,
  message: String,
  requirement: Option<PackRequirement>,
}

impl RuntimePackError {
  fn new(kind: RuntimePackErrorKind, path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
    Self {
      kind,
      path: path.into(),
      message: message.into(),
      requirement: None,
    }
  }

  fn with_requirement(mut self, requirement: PackRequirement) -> Self {
    self.requirement = Some(requirement);
    self
  }

  /// Returns the stable failure category.
  pub fn kind(&self) -> RuntimePackErrorKind {
    self.kind
  }

  /// Returns the manifest, pack, project, or asset associated with the failure.
  pub fn path(&self) -> &Path {
    &self.path
  }

  /// Returns the exact unavailable-pack requirement when selection reached one.
  pub fn requirement(&self) -> Option<&PackRequirement> {
    self.requirement.as_ref()
  }
}

impl fmt::Display for RuntimePackError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl Error for RuntimePackError {}

struct PackDefinitions {
  runtime_pattern: String,
  default_runtime_version: String,
  runtime_version: String,
  runtime_identifiers: String,
  host_pattern: String,
  host_version: String,
  host_identifiers: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AssetKind {
  Managed,
  Native,
}

#[derive(Debug)]
struct RuntimeAsset {
  kind: AssetKind,
  path: TextSpan,
}

const _: () = assert!(size_of::<RuntimeAsset>() == 12);
const _: () = assert!(align_of::<RuntimeAsset>() == 4);

#[derive(Debug)]
struct RuntimePackInventory {
  text: Box<str>,
  assets: Box<[RuntimeAsset]>,
  apphost_template: TextSpan,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<RuntimePackInventory>() == 40);
const _: () = assert!(align_of::<RuntimePackInventory>() == align_of::<usize>());

impl RuntimePackInventory {
  fn asset_relative_path(&self, asset: &RuntimeAsset) -> &str {
    let start = asset.path.start as usize;
    &self.text[start..start + asset.path.len as usize]
  }

  fn apphost_relative_path(&self) -> &str {
    let start = self.apphost_template.start as usize;
    &self.text[start..start + self.apphost_template.len as usize]
  }
}

#[derive(Clone, Copy)]
struct PackSelection<'a> {
  label: &'static str,
  kind: PackKind,
  pattern: &'a str,
  version: &'a str,
  target_framework: &'a str,
}

struct PackInventoryInputs<'a> {
  sdk_path: &'a Path,
  sdk_version: &'a str,
  sdk_manifest: &'a Path,
  runtime_graph: &'a Path,
  target_framework: &'a str,
  framework_version: &'a str,
  requested_runtime_identifier: &'a str,
  runtime_identifier: &'a str,
  runtime_pack_id: &'a str,
  runtime_pack_version: &'a str,
  runtime_pack_root: &'a Path,
  runtime_manifest: &'a Path,
  host_runtime_identifier: &'a str,
  host_pack_id: &'a str,
  host_pack_version: &'a str,
  host_pack_root: &'a Path,
}

/// Selects runtime and host packs for a project's active runtime identifier.
///
/// Pack identities, versions, and supported RIDs come from the selected SDK.
/// Compatibility comes from its portable RID graph. No SDK version, package
/// version, pack identity, or RID fallback is synthesized in code.
pub fn plan_runtime_packs(project: &ProjectSpec, inventory: &SdkInventory, packages_directory: Option<&Path>) -> Result<RuntimePackPlan, RuntimePackError> {
  let requested_runtime_identifier = project.runtime_identifier().ok_or_else(|| {
    RuntimePackError::new(
      RuntimePackErrorKind::RuntimeRequired,
      project.project_path(),
      "runtime-pack planning requires one selected RuntimeIdentifier",
    )
  })?;
  let selected = inventory.selected();
  let sdk_root = inventory.installation_path(selected);
  let manifest = sdk_root.join(BUNDLED_VERSIONS_FILE);
  let definitions = read_pack_definitions(&manifest, project.target_framework())?;
  let runtime_pack_version = selected_runtime_pack_version(project, &definitions);
  let graph = load_portable_runtime_graph(inventory).map_err(|error| {
    RuntimePackError::new(
      RuntimePackErrorKind::InvalidManifest,
      error.path(),
      format!("failed to load the selected SDK RID graph: {error}"),
    )
  })?;

  let runtime_identifier = select_pack_runtime_identifier(
    &graph,
    requested_runtime_identifier,
    &definitions.runtime_identifiers,
    &manifest,
    PackSelection {
      label: "runtime pack",
      kind: PackKind::Runtime,
      pattern: &definitions.runtime_pattern,
      version: runtime_pack_version,
      target_framework: project.target_framework(),
    },
  )?;
  let host_runtime_identifier = select_pack_runtime_identifier(
    &graph,
    requested_runtime_identifier,
    &definitions.host_identifiers,
    &manifest,
    PackSelection {
      label: "host pack",
      kind: PackKind::Host,
      pattern: &definitions.host_pattern,
      version: &definitions.host_version,
      target_framework: project.target_framework(),
    },
  )?;
  let runtime_pack_id = expand_pack_pattern(&definitions.runtime_pattern, runtime_identifier, &manifest, "runtime pack")?;
  let host_pack_id = expand_pack_pattern(&definitions.host_pattern, host_runtime_identifier, &manifest, "host pack")?;
  let global_packages = global_packages_directory(project.project_directory(), packages_directory).map_err(|error| {
    RuntimePackError::new(
      RuntimePackErrorKind::Configuration,
      project.project_directory(),
      format!("failed to resolve the global-packages directory: {error}"),
    )
  })?;
  let dotnet_root = inventory.root(selected);
  let runtime_pack_root = locate_pack(
    dotnet_root,
    &global_packages,
    &runtime_pack_id,
    runtime_pack_version,
    PackKind::Runtime,
    project.target_framework(),
    requested_runtime_identifier,
  )?;
  let host_pack_root = locate_pack(
    dotnet_root,
    &global_packages,
    &host_pack_id,
    &definitions.host_version,
    PackKind::Host,
    project.target_framework(),
    requested_runtime_identifier,
  )?;
  let runtime_manifest = runtime_pack_root.join(RUNTIME_LIST_FILE);
  let framework_version = project.target().framework_version();
  let (pack_inventory, _) = load_or_build_pack_inventory(
    &global_packages.join(PACK_INVENTORY_CACHE_DIRECTORY),
    PackInventoryInputs {
      sdk_path: &sdk_root,
      sdk_version: selected.version.as_str(),
      sdk_manifest: &manifest,
      runtime_graph: &sdk_root.join(PORTABLE_GRAPH_FILE),
      target_framework: project.target_framework(),
      framework_version: &framework_version,
      requested_runtime_identifier,
      runtime_identifier,
      runtime_pack_id: &runtime_pack_id,
      runtime_pack_version,
      runtime_pack_root: &runtime_pack_root,
      runtime_manifest: &runtime_manifest,
      host_runtime_identifier,
      host_pack_id: &host_pack_id,
      host_pack_version: &definitions.host_version,
      host_pack_root: &host_pack_root,
    },
  )?;

  materialize_plan(
    project,
    selected.version.as_str(),
    &manifest,
    requested_runtime_identifier,
    runtime_identifier,
    &runtime_pack_id,
    runtime_pack_version,
    &runtime_pack_root,
    host_runtime_identifier,
    &host_pack_id,
    &definitions.host_version,
    &host_pack_root,
    &pack_inventory,
  )
}

fn read_pack_definitions(path: &Path, target_framework: &str) -> Result<PackDefinitions, RuntimePackError> {
  let bytes = fs::read(path).map_err(|error| io_error("read SDK pack manifest", path, error))?;
  let mut reader = Reader::from_reader(bytes.as_slice());
  reader.config_mut().trim_text(true);
  let mut runtime = None;
  let mut host = None;

  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) | Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"KnownFrameworkReference" => {
        if xml_attribute(&reader, &element, b"Include", path)?.as_deref() == Some(IMPLICIT_FRAMEWORK_REFERENCE)
          && xml_attribute(&reader, &element, b"TargetFramework", path)?.as_deref() == Some(target_framework)
        {
          if runtime.is_some() {
            return Err(invalid_manifest(
              path,
              format!("multiple {IMPLICIT_FRAMEWORK_REFERENCE} runtime definitions match {target_framework}"),
            ));
          }
          runtime = Some((
            required_attribute(&reader, &element, b"RuntimePackNamePatterns", path)?,
            required_attribute(&reader, &element, b"DefaultRuntimeFrameworkVersion", path)?,
            required_attribute(&reader, &element, b"LatestRuntimeFrameworkVersion", path)?,
            required_attribute(&reader, &element, b"RuntimePackRuntimeIdentifiers", path)?,
          ));
        }
      },
      Ok(Event::Start(element)) | Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"KnownAppHostPack" => {
        if xml_attribute(&reader, &element, b"Include", path)?.as_deref() == Some(IMPLICIT_FRAMEWORK_REFERENCE)
          && xml_attribute(&reader, &element, b"TargetFramework", path)?.as_deref() == Some(target_framework)
        {
          if host.is_some() {
            return Err(invalid_manifest(
              path,
              format!("multiple {IMPLICIT_FRAMEWORK_REFERENCE} host definitions match {target_framework}"),
            ));
          }
          host = Some((
            required_attribute(&reader, &element, b"AppHostPackNamePattern", path)?,
            required_attribute(&reader, &element, b"AppHostPackVersion", path)?,
            required_attribute(&reader, &element, b"AppHostRuntimeIdentifiers", path)?,
          ));
        }
      },
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(invalid_manifest(path, format!("invalid SDK pack manifest XML: {error}"))),
    }
  }

  let (runtime_pattern, default_runtime_version, runtime_version, runtime_identifiers) = runtime.ok_or_else(|| {
    invalid_manifest(
      path,
      format!("SDK pack manifest has no {IMPLICIT_FRAMEWORK_REFERENCE} runtime definition for {target_framework}"),
    )
  })?;
  let (host_pattern, host_version, host_identifiers) = host.ok_or_else(|| {
    invalid_manifest(
      path,
      format!("SDK pack manifest has no {IMPLICIT_FRAMEWORK_REFERENCE} host definition for {target_framework}"),
    )
  })?;
  Ok(PackDefinitions {
    runtime_pattern,
    default_runtime_version,
    runtime_version,
    runtime_identifiers,
    host_pattern,
    host_version,
    host_identifiers,
  })
}

fn selected_runtime_pack_version<'a>(project: &'a ProjectSpec, definitions: &'a PackDefinitions) -> &'a str {
  let core_reference = project
    .framework_references()
    .iter()
    .copied()
    .find(|reference| project.framework_reference_id(*reference).eq_ignore_ascii_case(IMPLICIT_FRAMEWORK_REFERENCE));
  if let Some(version) = core_reference.and_then(|reference| project.framework_runtime_version(reference)) {
    return version;
  }
  if let Some(version) = project.runtime_framework_version() {
    return version;
  }
  match core_reference
    .and_then(|reference| project.framework_target_latest_runtime_patch(reference))
    .or(project.target_latest_runtime_patch())
  {
    Some(false) => &definitions.default_runtime_version,
    Some(true) | None => &definitions.runtime_version,
  }
}

fn select_pack_runtime_identifier<'a>(
  graph: &'a RuntimeIdentifierGraph,
  requested: &'a str,
  supported: &str,
  manifest: &Path,
  pack: PackSelection<'_>,
) -> Result<&'a str, RuntimePackError> {
  if let Some(selected) = graph
    .compatible_rids(requested)
    .find(|candidate| supported.split(';').any(|supported| supported == *candidate))
  {
    return Ok(selected);
  }
  let identity = expand_pack_pattern(pack.pattern, requested, manifest, pack.label)?;
  Err(
    RuntimePackError::new(
      RuntimePackErrorKind::UnsupportedRuntime,
      manifest,
      format!("selected SDK provides no {} compatible with runtime identifier {requested:?}", pack.label),
    )
    .with_requirement(PackRequirement::new(
      pack.kind,
      &identity,
      Some(pack.version),
      pack.target_framework,
      Some(requested),
      PackAcquisition::ChooseRuntimeIdentifier,
    )),
  )
}

fn expand_pack_pattern(pattern: &str, runtime_identifier: &str, manifest: &Path, pack_kind: &str) -> Result<String, RuntimePackError> {
  let Some((prefix, suffix)) = pattern.split_once(RID_PLACEHOLDER) else {
    return Err(invalid_manifest(
      manifest,
      format!("{pack_kind} pattern {pattern:?} has no {RID_PLACEHOLDER} placeholder"),
    ));
  };
  if suffix.contains(RID_PLACEHOLDER) || pattern.contains(';') {
    return Err(invalid_manifest(
      manifest,
      format!("{pack_kind} pattern {pattern:?} is not one supported pack pattern"),
    ));
  }
  let mut identity = String::with_capacity(prefix.len() + runtime_identifier.len() + suffix.len());
  identity.push_str(prefix);
  identity.push_str(runtime_identifier);
  identity.push_str(suffix);
  Ok(identity)
}

fn locate_pack(
  dotnet_root: &Path,
  global_packages: &Path,
  package_id: &str,
  version: &str,
  kind: PackKind,
  target_framework: &str,
  requested_runtime_identifier: &str,
) -> Result<PathBuf, RuntimePackError> {
  let installed = dotnet_root.join("packs").join(package_id).join(version);
  let cached = global_packages.join(package_id.to_ascii_lowercase()).join(version.to_ascii_lowercase());
  let candidates = [&installed, &cached];
  candidates.into_iter().find(|candidate| candidate.is_dir()).cloned().ok_or_else(|| {
    RuntimePackError::new(
      RuntimePackErrorKind::PackNotFound,
      &cached,
      format!(
        "required pack {package_id} {version} is not installed under {} or restored under {}",
        installed.display(),
        cached.display()
      ),
    )
    .with_requirement(PackRequirement::new(
      kind,
      package_id,
      Some(version),
      target_framework,
      Some(requested_runtime_identifier),
      PackAcquisition::RestorePackage,
    ))
  })
}

fn load_or_build_pack_inventory(cache_directory: &Path, inputs: PackInventoryInputs<'_>) -> Result<(RuntimePackInventory, bool), RuntimePackError> {
  let fingerprint = pack_inventory_fingerprint(&inputs);
  if let Some(fingerprint) = fingerprint {
    let cache_path = cache_directory.join(format!("{}.bin", hex_fingerprint(&fingerprint)));
    if let Ok(bytes) = fs::read(&cache_path) {
      if let Some(inventory) = decode_pack_inventory(&bytes, &fingerprint)
        && join_relative_path(inputs.host_pack_root, Path::new(inventory.apphost_relative_path())).is_file()
      {
        return Ok((inventory, true));
      }
      let _ = fs::remove_file(&cache_path);
    }

    let inventory = build_pack_inventory(&inputs)?;
    publish_pack_inventory(cache_directory, &cache_path, &fingerprint, &inventory);
    return Ok((inventory, false));
  }

  build_pack_inventory(&inputs).map(|inventory| (inventory, false))
}

fn pack_inventory_fingerprint(inputs: &PackInventoryInputs<'_>) -> Option<[u8; PACK_FINGERPRINT_BYTES]> {
  let mut hasher = Sha512::new();
  hasher.update(PACK_INVENTORY_MAGIC);
  hasher.update(PACK_INVENTORY_SCHEMA.to_le_bytes());
  for value in [
    inputs.sdk_version,
    inputs.target_framework,
    inputs.framework_version,
    inputs.requested_runtime_identifier,
    inputs.runtime_identifier,
    inputs.runtime_pack_id,
    inputs.runtime_pack_version,
    inputs.host_runtime_identifier,
    inputs.host_pack_id,
    inputs.host_pack_version,
  ] {
    hash_bytes(&mut hasher, value.as_bytes());
  }
  for path in [
    inputs.sdk_path,
    inputs.sdk_manifest,
    inputs.runtime_graph,
    inputs.runtime_pack_root,
    inputs.runtime_manifest,
    inputs.host_pack_root,
  ] {
    hash_path(&mut hasher, path)?;
  }
  for path in [inputs.sdk_manifest, inputs.runtime_graph, inputs.runtime_manifest] {
    hash_metadata(&mut hasher, path)?;
  }
  let host_native = inputs.host_pack_root.join("runtimes").join(inputs.host_runtime_identifier).join("native");
  hash_path(&mut hasher, &host_native)?;
  hash_metadata(&mut hasher, &host_native)?;
  for pack_root in [inputs.runtime_pack_root, inputs.host_pack_root] {
    let completion = pack_root.join(".nupkg.metadata");
    let completion_metadata = fs::metadata(&completion).ok().filter(|metadata| metadata.is_file());
    hasher.update([u8::from(completion_metadata.is_some())]);
    if let Some(metadata) = completion_metadata {
      hash_path(&mut hasher, &completion)?;
      hash_metadata_value(&mut hasher, &metadata)?;
    }
  }
  Some(hasher.finalize().into())
}

fn hash_path(hasher: &mut Sha512, path: &Path) -> Option<()> {
  let text = path.to_str()?;
  hasher.update((text.len() as u64).to_le_bytes());
  for byte in text.bytes() {
    let normalized = if cfg!(windows) && byte == b'\\' { b'/' } else { byte };
    hasher.update([normalized]);
  }
  Some(())
}

fn hash_metadata(hasher: &mut Sha512, path: &Path) -> Option<()> {
  let metadata = fs::metadata(path).ok()?;
  hash_metadata_value(hasher, &metadata)
}

fn hash_metadata_value(hasher: &mut Sha512, metadata: &fs::Metadata) -> Option<()> {
  let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
  hasher.update(metadata.len().to_le_bytes());
  hasher.update(modified.as_secs().to_le_bytes());
  hasher.update(modified.subsec_nanos().to_le_bytes());
  hasher.update([u8::from(metadata.is_dir())]);
  Some(())
}

fn hash_bytes(hasher: &mut Sha512, bytes: &[u8]) {
  hasher.update((bytes.len() as u64).to_le_bytes());
  hasher.update(bytes);
}

fn hex_fingerprint(fingerprint: &[u8; PACK_FINGERPRINT_BYTES]) -> String {
  const HEX: &[u8; 16] = b"0123456789abcdef";
  let mut output = String::with_capacity(PACK_FINGERPRINT_BYTES * 2);
  for byte in fingerprint {
    output.push(HEX[usize::from(byte >> 4)] as char);
    output.push(HEX[usize::from(byte & 0x0f)] as char);
  }
  output
}

fn decode_pack_inventory(bytes: &[u8], expected_fingerprint: &[u8; PACK_FINGERPRINT_BYTES]) -> Option<RuntimePackInventory> {
  if bytes.len() < CACHE_HEADER_BYTES || bytes.get(..PACK_INVENTORY_MAGIC.len())? != PACK_INVENTORY_MAGIC {
    return None;
  }
  let mut cursor = PACK_INVENTORY_MAGIC.len();
  if read_u32(bytes, &mut cursor)? != PACK_INVENTORY_SCHEMA {
    return None;
  }
  if bytes.get(cursor..cursor + PACK_FINGERPRINT_BYTES)? != expected_fingerprint {
    return None;
  }
  cursor += PACK_FINGERPRINT_BYTES;
  let checksum = bytes.get(cursor..cursor + PACK_CHECKSUM_BYTES)?;
  cursor += PACK_CHECKSUM_BYTES;
  if Sha512::digest(bytes.get(cursor..)?).as_slice() != checksum {
    return None;
  }
  let text_len = read_u32(bytes, &mut cursor)?;
  let asset_count = read_u32(bytes, &mut cursor)?;
  let apphost_template = TextSpan {
    start: read_u32(bytes, &mut cursor)?,
    len: read_u32(bytes, &mut cursor)?,
  };
  let text_end = cursor.checked_add(text_len as usize)?;
  let text = std::str::from_utf8(bytes.get(cursor..text_end)?).ok()?;
  let apphost_relative_path = span_text(text, apphost_template)?;
  if !is_safe_pack_relative_path(apphost_relative_path) {
    return None;
  }
  cursor = text_end;
  let record_bytes = (asset_count as usize).checked_mul(1 + 2 * size_of::<u32>())?;
  if cursor.checked_add(record_bytes)? != bytes.len() {
    return None;
  }
  let mut assets = Vec::with_capacity(asset_count as usize);
  for _ in 0..asset_count {
    let kind = match *bytes.get(cursor)? {
      0 => AssetKind::Managed,
      1 => AssetKind::Native,
      _ => return None,
    };
    cursor += 1;
    let path = TextSpan {
      start: read_u32(bytes, &mut cursor)?,
      len: read_u32(bytes, &mut cursor)?,
    };
    let relative_path = span_text(text, path)?;
    if !is_safe_pack_relative_path(relative_path) {
      return None;
    }
    assets.push(RuntimeAsset { kind, path });
  }
  Some(RuntimePackInventory {
    text: text.into(),
    assets: assets.into_boxed_slice(),
    apphost_template,
  })
}

fn span_text(text: &str, span: TextSpan) -> Option<&str> {
  let start = span.start as usize;
  let end = start.checked_add(span.len as usize)?;
  text.get(start..end)
}

fn is_safe_pack_relative_path(value: &str) -> bool {
  !value.is_empty() && Path::new(value).components().all(|component| matches!(component, Component::Normal(_)))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Option<u32> {
  let end = cursor.checked_add(size_of::<u32>())?;
  let value = u32::from_le_bytes(bytes.get(*cursor..end)?.try_into().ok()?);
  *cursor = end;
  Some(value)
}

fn publish_pack_inventory(cache_directory: &Path, cache_path: &Path, fingerprint: &[u8; PACK_FINGERPRINT_BYTES], inventory: &RuntimePackInventory) {
  if fs::create_dir_all(cache_directory).is_err() || cache_path.is_file() {
    return;
  }
  let bytes = encode_pack_inventory(fingerprint, inventory);
  let sequence = CACHE_TEMP_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
  let temporary = cache_directory.join(format!(".pack-inventory-{}-{sequence}.tmp", std::process::id()));
  if fs::write(&temporary, bytes).is_err() {
    let _ = fs::remove_file(&temporary);
    return;
  }
  if fs::rename(&temporary, cache_path).is_err() {
    let _ = fs::remove_file(&temporary);
  }
}

fn encode_pack_inventory(fingerprint: &[u8; PACK_FINGERPRINT_BYTES], inventory: &RuntimePackInventory) -> Vec<u8> {
  let record_bytes = inventory.assets.len() * (1 + 2 * size_of::<u32>());
  let text_len = u32::try_from(inventory.text.len()).expect("inventory text was built through the compact text table");
  let asset_count = u32::try_from(inventory.assets.len()).expect("inventory asset count was checked before publication");
  let mut bytes = Vec::with_capacity(CACHE_HEADER_BYTES + inventory.text.len() + record_bytes);
  bytes.extend_from_slice(PACK_INVENTORY_MAGIC);
  bytes.extend_from_slice(&PACK_INVENTORY_SCHEMA.to_le_bytes());
  bytes.extend_from_slice(fingerprint);
  bytes.extend_from_slice(&[0; PACK_CHECKSUM_BYTES]);
  bytes.extend_from_slice(&text_len.to_le_bytes());
  bytes.extend_from_slice(&asset_count.to_le_bytes());
  bytes.extend_from_slice(&inventory.apphost_template.start.to_le_bytes());
  bytes.extend_from_slice(&inventory.apphost_template.len.to_le_bytes());
  bytes.extend_from_slice(inventory.text.as_bytes());
  for asset in &inventory.assets {
    bytes.push(match asset.kind {
      AssetKind::Managed => 0,
      AssetKind::Native => 1,
    });
    bytes.extend_from_slice(&asset.path.start.to_le_bytes());
    bytes.extend_from_slice(&asset.path.len.to_le_bytes());
  }
  let checksum: [u8; PACK_CHECKSUM_BYTES] = Sha512::digest(&bytes[CACHE_PAYLOAD_OFFSET..]).into();
  let checksum_start = CACHE_PAYLOAD_OFFSET - PACK_CHECKSUM_BYTES;
  bytes[checksum_start..CACHE_PAYLOAD_OFFSET].copy_from_slice(&checksum);
  bytes
}

fn build_pack_inventory(inputs: &PackInventoryInputs<'_>) -> Result<RuntimePackInventory, RuntimePackError> {
  let path = inputs.runtime_manifest;
  let bytes = fs::read(path).map_err(|error| io_error("read runtime-pack manifest", path, error))?;
  let mut reader = Reader::from_reader(bytes.as_slice());
  reader.config_mut().trim_text(true);
  let mut root_seen = false;
  let mut assets = Vec::with_capacity(192);
  let mut text = TextTable::with_capacity(32 * 1024);

  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) | Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"FileList" => {
        if root_seen {
          return Err(invalid_manifest(path, "runtime-pack manifest contains multiple FileList roots"));
        }
        let framework_name = xml_attribute(&reader, &element, b"FrameworkName", path)?;
        let framework_version = xml_attribute(&reader, &element, b"TargetFrameworkVersion", path)?;
        if framework_name.as_deref() != Some(IMPLICIT_FRAMEWORK_REFERENCE) || framework_version.as_deref() != Some(inputs.framework_version) {
          return Err(invalid_manifest(
            path,
            format!("runtime-pack manifest target does not match {}", inputs.target_framework),
          ));
        }
        root_seen = true;
      },
      Ok(Event::Start(element)) | Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"File" => {
        if !root_seen {
          return Err(invalid_manifest(path, "runtime asset appears before the FileList root"));
        }
        let Some(kind) = xml_attribute(&reader, &element, b"Type", path)? else {
          continue;
        };
        let kind = match kind.as_str() {
          "Managed" => AssetKind::Managed,
          "Native" => AssetKind::Native,
          _ => continue,
        };
        let relative = required_attribute(&reader, &element, b"Path", path)?;
        let relative_path = Path::new(&relative);
        if !is_safe_pack_relative_path(&relative) {
          return Err(invalid_manifest(path, "runtime asset path escapes the selected pack"));
        }
        let asset = join_relative_path(inputs.runtime_pack_root, relative_path);
        if !asset.is_file() {
          return Err(RuntimePackError::new(
            RuntimePackErrorKind::MissingAsset,
            &asset,
            format!("runtime-pack asset {} is missing", asset.display()),
          ));
        }
        assets.push(RuntimeAsset {
          kind,
          path: text.push(&relative)?,
        });
      },
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(invalid_manifest(path, format!("invalid runtime-pack manifest XML: {error}"))),
    }
  }
  if !root_seen {
    return Err(invalid_manifest(path, "runtime-pack manifest has no FileList root"));
  }
  if !assets.iter().any(|asset| asset.kind == AssetKind::Managed) || !assets.iter().any(|asset| asset.kind == AssetKind::Native) {
    return Err(invalid_manifest(path, "runtime-pack manifest must contain managed and native assets"));
  }
  compact_len(assets.len(), "runtime pack inventory")?;
  let apphost = find_apphost_template(inputs.host_pack_root, inputs.host_runtime_identifier)?;
  let apphost_relative_path = apphost
    .strip_prefix(inputs.host_pack_root)
    .map_err(|_| invalid_manifest(inputs.host_pack_root, "apphost template escapes the selected host pack"))?;
  let apphost_template = text.push_path(apphost_relative_path)?;
  Ok(RuntimePackInventory {
    text: text.text.into_boxed_str(),
    assets: assets.into_boxed_slice(),
    apphost_template,
  })
}

fn find_apphost_template(pack_root: &Path, runtime_identifier: &str) -> Result<PathBuf, RuntimePackError> {
  let directory = pack_root.join("runtimes").join(runtime_identifier).join("native");
  let candidates = [directory.join("apphost"), directory.join("apphost.exe")];
  let mut found = candidates.into_iter().filter(|candidate| candidate.is_file());
  let template = found.next().ok_or_else(|| {
    RuntimePackError::new(
      RuntimePackErrorKind::MissingAsset,
      &directory,
      format!("host pack contains no apphost template in {}", directory.display()),
    )
  })?;
  if found.next().is_some() {
    return Err(RuntimePackError::new(
      RuntimePackErrorKind::InvalidManifest,
      &directory,
      format!("host pack contains ambiguous apphost templates in {}", directory.display()),
    ));
  }
  Ok(template)
}

fn join_relative_path(root: &Path, relative: &Path) -> PathBuf {
  let mut path = root.to_owned();
  for component in relative.components() {
    if let Component::Normal(component) = component {
      path.push(component);
    }
  }
  path
}

#[allow(clippy::too_many_arguments)]
fn materialize_plan(
  project: &ProjectSpec,
  sdk_version: &str,
  manifest: &Path,
  requested_runtime_identifier: &str,
  runtime_identifier: &str,
  runtime_pack_id: &str,
  runtime_pack_version: &str,
  runtime_pack_root: &Path,
  host_runtime_identifier: &str,
  host_pack_id: &str,
  host_pack_version: &str,
  host_pack_root: &Path,
  pack_inventory: &RuntimePackInventory,
) -> Result<RuntimePackPlan, RuntimePackError> {
  let runtime_root_length = runtime_pack_root.as_os_str().len();
  let estimated_text = pack_inventory.text.len() + pack_inventory.assets.len() * (runtime_root_length + 1) + 2048;
  let mut table = TextTable::with_capacity(estimated_text);
  let project_span = table.push_path(project.project_path())?;
  let sdk_version_span = table.push(sdk_version)?;
  let manifest_span = table.push_path(manifest)?;
  let target_framework_span = table.push(project.target_framework())?;
  let requested_runtime_identifier_span = table.push(requested_runtime_identifier)?;
  let runtime_identifier_span = table.push(runtime_identifier)?;
  let runtime_pack_id_span = table.push(runtime_pack_id)?;
  let runtime_pack_version_span = table.push(runtime_pack_version)?;
  let runtime_pack_root_span = table.push_path(runtime_pack_root)?;
  let host_runtime_identifier_span = table.push(host_runtime_identifier)?;
  let host_pack_id_span = table.push(host_pack_id)?;
  let host_pack_version_span = table.push(host_pack_version)?;
  let host_pack_root_span = table.push_path(host_pack_root)?;
  let apphost_template_span = table.push_joined_path(host_pack_root, pack_inventory.apphost_relative_path())?;

  let mut assets = Vec::with_capacity(pack_inventory.assets.len());
  let managed_start = compact_len(assets.len(), "runtime asset span batch")?;
  for asset in pack_inventory.assets.iter().filter(|asset| asset.kind == AssetKind::Managed) {
    assets.push(table.push_joined_path(runtime_pack_root, pack_inventory.asset_relative_path(asset))?);
  }
  let managed_end = compact_len(assets.len(), "runtime asset span batch")?;
  let native_start = managed_end;
  for asset in pack_inventory.assets.iter().filter(|asset| asset.kind == AssetKind::Native) {
    assets.push(table.push_joined_path(runtime_pack_root, pack_inventory.asset_relative_path(asset))?);
  }
  let native_end = compact_len(assets.len(), "runtime asset span batch")?;

  Ok(RuntimePackPlan {
    text: table.text.into_boxed_str(),
    assets: assets.into_boxed_slice(),
    project: project_span,
    sdk_version: sdk_version_span,
    manifest: manifest_span,
    target_framework: target_framework_span,
    requested_runtime_identifier: requested_runtime_identifier_span,
    runtime_identifier: runtime_identifier_span,
    runtime_pack_id: runtime_pack_id_span,
    runtime_pack_version: runtime_pack_version_span,
    runtime_pack_root: runtime_pack_root_span,
    host_runtime_identifier: host_runtime_identifier_span,
    host_pack_id: host_pack_id_span,
    host_pack_version: host_pack_version_span,
    host_pack_root: host_pack_root_span,
    apphost_template: apphost_template_span,
    managed_assets: IndexRange {
      start: managed_start,
      len: managed_end - managed_start,
    },
    native_assets: IndexRange {
      start: native_start,
      len: native_end - native_start,
    },
  })
}

struct TextTable {
  text: String,
}

impl TextTable {
  fn with_capacity(capacity: usize) -> Self {
    Self {
      text: String::with_capacity(capacity),
    }
  }

  fn push(&mut self, value: &str) -> Result<TextSpan, RuntimePackError> {
    let start = compact_len(self.text.len(), "runtime-pack plan text")?;
    let len = compact_len(value.len(), "runtime-pack plan value")?;
    start.checked_add(len).ok_or_else(text_overflow)?;
    self.text.push_str(value);
    Ok(TextSpan { start, len })
  }

  fn push_path(&mut self, path: &Path) -> Result<TextSpan, RuntimePackError> {
    let value = path.to_str().ok_or_else(|| {
      RuntimePackError::new(
        RuntimePackErrorKind::NonUnicodePath,
        path,
        format!("runtime-pack plan path {} is not valid Unicode", path.display()),
      )
    })?;
    self.push(value)
  }

  fn push_joined_path(&mut self, root: &Path, relative: &str) -> Result<TextSpan, RuntimePackError> {
    let root = root.to_str().ok_or_else(|| {
      RuntimePackError::new(
        RuntimePackErrorKind::NonUnicodePath,
        root,
        format!("runtime-pack path {} is not valid Unicode", root.display()),
      )
    })?;
    let start = compact_len(self.text.len(), "runtime-pack plan text")?;
    self.text.push_str(root);
    if !root.ends_with(std::path::MAIN_SEPARATOR) {
      self.text.push(std::path::MAIN_SEPARATOR);
    }
    if cfg!(windows) {
      for character in relative.chars() {
        self.text.push(if matches!(character, '/' | '\\') {
          std::path::MAIN_SEPARATOR
        } else {
          character
        });
      }
    } else {
      self.text.push_str(relative);
    }
    let len = compact_len(self.text.len() - start as usize, "runtime-pack plan value")?;
    Ok(TextSpan { start, len })
  }
}

fn compact_len(value: usize, meaning: &str) -> Result<u32, RuntimePackError> {
  u32::try_from(value).map_err(|_| {
    RuntimePackError::new(
      RuntimePackErrorKind::TextOverflow,
      PathBuf::new(),
      format!("{meaning} exceeds the compact 32-bit index space"),
    )
  })
}

fn text_overflow() -> RuntimePackError {
  RuntimePackError::new(
    RuntimePackErrorKind::TextOverflow,
    PathBuf::new(),
    "runtime-pack plan text exceeds the compact 32-bit index space",
  )
}

fn index_range(range: IndexRange) -> std::ops::Range<usize> {
  range.start as usize..(range.start + range.len) as usize
}

fn required_attribute(reader: &Reader<&[u8]>, element: &quick_xml::events::BytesStart<'_>, name: &[u8], path: &Path) -> Result<String, RuntimePackError> {
  xml_attribute(reader, element, name, path)?.ok_or_else(|| {
    invalid_manifest(
      path,
      format!(
        "{} element requires {}",
        String::from_utf8_lossy(local_name(element.name().as_ref())),
        String::from_utf8_lossy(name)
      ),
    )
  })
}

fn xml_attribute(reader: &Reader<&[u8]>, element: &quick_xml::events::BytesStart<'_>, name: &[u8], path: &Path) -> Result<Option<String>, RuntimePackError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| invalid_manifest(path, format!("invalid manifest attribute: {error}")))?;
    if local_name(attribute.key.as_ref()) == name {
      return attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
        .map(|value| Some(value.into_owned()))
        .map_err(|error| invalid_manifest(path, format!("invalid manifest attribute value: {error}")));
    }
  }
  Ok(None)
}

fn local_name(name: &[u8]) -> &[u8] {
  name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn invalid_manifest(path: &Path, message: impl Into<String>) -> RuntimePackError {
  RuntimePackError::new(RuntimePackErrorKind::InvalidManifest, path, message)
}

fn io_error(operation: &str, path: &Path, error: io::Error) -> RuntimePackError {
  RuntimePackError::new(RuntimePackErrorKind::Io, path, format!("failed to {operation} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
  use std::{
    env,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
  };

  use crate::{ProjectConfiguration, SdkInstallation, SdkVersion, evaluate_project_path};

  use super::*;

  static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
      let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let path = env::temp_dir().join(format!("dv-runtime-pack-test-{}-{time}-{nonce}", std::process::id()));
      fs::create_dir_all(&path).unwrap();
      Self(path)
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
      let path = self.0.join(relative);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(&path, contents).unwrap();
      path
    }
  }

  impl Drop for TempDirectory {
    fn drop(&mut self) {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }

  #[test]
  fn selects_manifest_defined_packs_and_materializes_assets_contiguously() {
    let temp = TempDirectory::new();
    let project_path = temp.write(
      "project/App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><RuntimeIdentifier>child-x64</RuntimeIdentifier></PropertyGroup></Project>"#,
    );
    temp.write("project/Program.cs", "class Program { static void Main() {} }");
    temp.write(
      "dotnet/sdk/10.0.100/Microsoft.NETCoreSdk.BundledVersions.props",
      r#"<Project><ItemGroup>
        <KnownFrameworkReference Include="Microsoft.NETCore.App" TargetFramework="net10.0" DefaultRuntimeFrameworkVersion="10.0.0" LatestRuntimeFrameworkVersion="10.2.3" RuntimePackNamePatterns="Runtime.**RID**" RuntimePackRuntimeIdentifiers="base-x64" />
        <KnownAppHostPack Include="Microsoft.NETCore.App" TargetFramework="net10.0" AppHostPackNamePattern="Host.**RID**" AppHostPackVersion="10.4.5" AppHostRuntimeIdentifiers="base-x64" />
      </ItemGroup></Project>"#,
    );
    temp.write(
      "dotnet/sdk/10.0.100/PortableRuntimeIdentifierGraph.json",
      r##"{"runtimes":{"base-x64":{"#import":[]},"child-x64":{"#import":["base-x64"]}}}"##,
    );
    let cache = temp.0.join("packages");
    let runtime_root = cache.join("runtime.base-x64").join("10.2.3");
    temp.write(
      "packages/runtime.base-x64/10.2.3/data/RuntimeList.xml",
      r#"<FileList TargetFrameworkVersion="10.0" FrameworkName="Microsoft.NETCore.App"><File Type="Managed" Path="runtimes/base-x64/lib/net10.0/Core.dll"/><File Type="Native" Path="runtimes/base-x64/native/core.so"/></FileList>"#,
    );
    temp.write("packages/runtime.base-x64/10.2.3/runtimes/base-x64/lib/net10.0/Core.dll", "managed");
    temp.write("packages/runtime.base-x64/10.2.3/runtimes/base-x64/native/core.so", "native");
    let host_root = temp.0.join("dotnet").join("packs").join("Host.base-x64").join("10.4.5");
    temp.write("dotnet/packs/Host.base-x64/10.4.5/runtimes/base-x64/native/apphost", "host");

    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let inventory = SdkInventory {
      roots: vec![temp.0.join("dotnet")],
      installations: vec![SdkInstallation {
        version: SdkVersion::parse("10.0.100").unwrap(),
        root_index: 0,
      }],
      selected_index: 0,
      global_json: None,
    };
    let plan = plan_runtime_packs(&project, &inventory, Some(&cache)).unwrap();

    assert_eq!(plan.requested_runtime_identifier(), "child-x64");
    assert_eq!(plan.runtime_identifier(), "base-x64");
    assert_eq!(plan.runtime_pack_id(), "Runtime.base-x64");
    assert_eq!(plan.runtime_pack_version(), "10.2.3");
    assert_eq!(plan.runtime_pack_root(), runtime_root.to_str().unwrap());
    assert_eq!(plan.host_runtime_identifier(), "base-x64");
    assert_eq!(plan.host_pack_id(), "Host.base-x64");
    assert_eq!(plan.host_pack_version(), "10.4.5");
    assert_eq!(plan.host_pack_root(), host_root.to_str().unwrap());
    let managed = runtime_root.join("runtimes").join("base-x64").join("lib").join("net10.0").join("Core.dll");
    let native = runtime_root.join("runtimes").join("base-x64").join("native").join("core.so");
    let apphost = host_root.join("runtimes").join("base-x64").join("native").join("apphost");
    assert_eq!(plan.managed_assets().collect::<Vec<_>>(), [managed.to_str().unwrap()]);
    assert_eq!(plan.native_assets().collect::<Vec<_>>(), [native.to_str().unwrap()]);
    assert_eq!(plan.apphost_template(), apphost.to_str().unwrap());

    let cache_directory = cache.join(PACK_INVENTORY_CACHE_DIRECTORY);
    let sdk_path = temp.0.join("dotnet").join("sdk").join("10.0.100");
    let sdk_manifest = sdk_path.join(BUNDLED_VERSIONS_FILE);
    let graph_path = sdk_path.join(PORTABLE_GRAPH_FILE);
    let runtime_manifest = runtime_root.join(RUNTIME_LIST_FILE);
    let inputs = || PackInventoryInputs {
      sdk_path: &sdk_path,
      sdk_version: "10.0.100",
      sdk_manifest: &sdk_manifest,
      runtime_graph: &graph_path,
      target_framework: "net10.0",
      framework_version: "10.0",
      requested_runtime_identifier: "child-x64",
      runtime_identifier: "base-x64",
      runtime_pack_id: "Runtime.base-x64",
      runtime_pack_version: "10.2.3",
      runtime_pack_root: &runtime_root,
      runtime_manifest: &runtime_manifest,
      host_runtime_identifier: "base-x64",
      host_pack_id: "Host.base-x64",
      host_pack_version: "10.4.5",
      host_pack_root: &host_root,
    };
    let (cached, cache_hit) = load_or_build_pack_inventory(&cache_directory, inputs()).unwrap();
    assert!(cache_hit);
    assert_eq!(cached.assets.len(), 2);
    assert_eq!(cached.asset_relative_path(&cached.assets[0]), "runtimes/base-x64/lib/net10.0/Core.dll");
    assert_eq!(Path::new(cached.apphost_relative_path()), Path::new("runtimes/base-x64/native/apphost"));
    let cache_file = fs::read_dir(&cache_directory).unwrap().next().unwrap().unwrap().path();
    let mut corrupt = fs::read(&cache_file).unwrap();
    *corrupt.last_mut().unwrap() ^= 0xff;
    fs::write(&cache_file, corrupt).unwrap();
    let (rebuilt, cache_hit) = load_or_build_pack_inventory(&cache_directory, inputs()).unwrap();
    assert!(!cache_hit);
    assert_eq!(rebuilt.assets.len(), 2);
    let (_, cache_hit) = load_or_build_pack_inventory(&cache_directory, inputs()).unwrap();
    assert!(cache_hit);

    temp.write("packages/runtime.base-x64/10.2.3/runtimes/base-x64/lib/net10.0/Extra.dll", "extra");
    temp.write(
      "packages/runtime.base-x64/10.2.3/data/RuntimeList.xml",
      r#"<FileList TargetFrameworkVersion="10.0" FrameworkName="Microsoft.NETCore.App"><File Type="Managed" Path="runtimes/base-x64/lib/net10.0/Core.dll"/><File Type="Managed" Path="runtimes/base-x64/lib/net10.0/Extra.dll"/><File Type="Native" Path="runtimes/base-x64/native/core.so"/></FileList>"#,
    );
    let (changed, cache_hit) = load_or_build_pack_inventory(&cache_directory, inputs()).unwrap();
    assert!(!cache_hit);
    assert_eq!(changed.assets.len(), 3);
    let (_, cache_hit) = load_or_build_pack_inventory(&cache_directory, inputs()).unwrap();
    assert!(cache_hit);

    let changed_sdk = PackInventoryInputs {
      sdk_version: "10.0.101",
      ..inputs()
    };
    let (_, cache_hit) = load_or_build_pack_inventory(&cache_directory, changed_sdk).unwrap();
    assert!(!cache_hit);

    fs::remove_file(&apphost).unwrap();
    let error = load_or_build_pack_inventory(&cache_directory, inputs()).unwrap_err();
    assert_eq!(error.kind(), RuntimePackErrorKind::MissingAsset);
  }

  #[test]
  fn rejects_checksum_valid_cache_paths_outside_the_selected_pack() {
    let fingerprint = [0x5a; PACK_FINGERPRINT_BYTES];
    let text = "../outside.dllruntimes/base-x64/native/apphost";
    let inventory = RuntimePackInventory {
      text: text.into(),
      assets: vec![RuntimeAsset {
        kind: AssetKind::Managed,
        path: TextSpan { start: 0, len: 14 },
      }]
      .into_boxed_slice(),
      apphost_template: TextSpan { start: 14, len: 32 },
    };

    let bytes = encode_pack_inventory(&fingerprint, &inventory);
    assert!(decode_pack_inventory(&bytes, &fingerprint).is_none());
  }

  #[test]
  fn unknown_runtime_does_not_gain_a_string_inferred_fallback() {
    let graph_path = TempDirectory::new();
    let path = graph_path.write("graph.json", r##"{"runtimes":{"linux-x64":{"#import":[]}}}"##);
    let graph = RuntimeIdentifierGraph::load(&path).unwrap();
    let error = select_pack_runtime_identifier(
      &graph,
      "linux-custom-x64",
      "linux-x64",
      &path,
      PackSelection {
        label: "runtime pack",
        kind: PackKind::Runtime,
        pattern: "Runtime.**RID**",
        version: "10.0.0",
        target_framework: "net10.0",
      },
    )
    .unwrap_err();
    assert_eq!(error.kind(), RuntimePackErrorKind::UnsupportedRuntime);
    let requirement = error.requirement().unwrap();
    assert_eq!(requirement.identity(), "Runtime.linux-custom-x64");
    assert_eq!(requirement.runtime_identifier(), Some("linux-custom-x64"));
    assert_eq!(requirement.acquisition(), PackAcquisition::ChooseRuntimeIdentifier);
  }

  #[test]
  fn missing_host_pack_keeps_exact_identity_version_dimensions_and_action() {
    let temp = TempDirectory::new();
    let error = locate_pack(
      &temp.0.join("dotnet"),
      &temp.0.join("packages"),
      "Microsoft.NETCore.App.Host.win-x64",
      "10.0.0",
      PackKind::Host,
      "net10.0",
      "win-x64",
    )
    .unwrap_err();

    assert_eq!(error.kind(), RuntimePackErrorKind::PackNotFound);
    let requirement = error.requirement().unwrap();
    assert_eq!(requirement.kind(), PackKind::Host);
    assert_eq!(requirement.identity(), "Microsoft.NETCore.App.Host.win-x64");
    assert_eq!(requirement.version(), Some("10.0.0"));
    assert_eq!(requirement.target_framework(), "net10.0");
    assert_eq!(requirement.runtime_identifier(), Some("win-x64"));
    assert_eq!(requirement.acquisition(), PackAcquisition::RestorePackage);
  }

  #[test]
  fn rejects_pack_asset_paths_which_escape_the_pack() {
    let temp = TempDirectory::new();
    let root = temp.0.join("pack");
    let manifest = temp.write(
      "pack/data/RuntimeList.xml",
      r#"<FileList TargetFrameworkVersion="10.0" FrameworkName="Microsoft.NETCore.App"><File Type="Managed" Path="../outside.dll"/></FileList>"#,
    );
    let error = build_pack_inventory(&PackInventoryInputs {
      sdk_path: &root,
      sdk_version: "10.0.100",
      sdk_manifest: &manifest,
      runtime_graph: &manifest,
      target_framework: "net10.0",
      framework_version: "10.0",
      requested_runtime_identifier: "win-x64",
      runtime_identifier: "win-x64",
      runtime_pack_id: "Runtime.win-x64",
      runtime_pack_version: "10.0.0",
      runtime_pack_root: &root,
      runtime_manifest: &manifest,
      host_runtime_identifier: "win-x64",
      host_pack_id: "Host.win-x64",
      host_pack_version: "10.0.0",
      host_pack_root: &root,
    })
    .unwrap_err();
    assert_eq!(error.kind(), RuntimePackErrorKind::InvalidManifest);
  }
}
