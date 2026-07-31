use std::{
  error::Error,
  fmt, fs, io,
  mem::{align_of, size_of},
  path::{Component, Path, PathBuf},
};

use quick_xml::{Reader, XmlVersion, events::Event};

use crate::{ProjectSpec, RuntimeIdentifierGraph, SdkInventory, load_portable_runtime_graph, package::global_packages_directory};

const BUNDLED_VERSIONS_FILE: &str = "Microsoft.NETCoreSdk.BundledVersions.props";
const RUNTIME_LIST_FILE: &str = "data/RuntimeList.xml";
const IMPLICIT_FRAMEWORK_REFERENCE: &str = "Microsoft.NETCore.App";
const RID_PLACEHOLDER: &str = "**RID**";

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
}

impl RuntimePackError {
  fn new(kind: RuntimePackErrorKind, path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
    Self {
      kind,
      path: path.into(),
      message: message.into(),
    }
  }

  /// Returns the stable failure category.
  pub fn kind(&self) -> RuntimePackErrorKind {
    self.kind
  }

  /// Returns the manifest, pack, project, or asset associated with the failure.
  pub fn path(&self) -> &Path {
    &self.path
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
  path: PathBuf,
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
    "runtime pack",
  )?;
  let host_runtime_identifier = select_pack_runtime_identifier(&graph, requested_runtime_identifier, &definitions.host_identifiers, &manifest, "host pack")?;
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
  let runtime_pack_root = locate_pack(dotnet_root, &global_packages, &runtime_pack_id, runtime_pack_version)?;
  let host_pack_root = locate_pack(dotnet_root, &global_packages, &host_pack_id, &definitions.host_version)?;
  let runtime_manifest = runtime_pack_root.join(RUNTIME_LIST_FILE);
  let framework_version = project.target().framework_version();
  let runtime_assets = read_runtime_assets(&runtime_manifest, &runtime_pack_root, project.target_framework(), &framework_version)?;
  let apphost_template = find_apphost_template(&host_pack_root, host_runtime_identifier)?;

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
    &apphost_template,
    &runtime_assets,
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
  pack_kind: &str,
) -> Result<&'a str, RuntimePackError> {
  graph
    .compatible_rids(requested)
    .find(|candidate| supported.split(';').any(|supported| supported == *candidate))
    .ok_or_else(|| {
      RuntimePackError::new(
        RuntimePackErrorKind::UnsupportedRuntime,
        manifest,
        format!("selected SDK provides no {pack_kind} compatible with runtime identifier {requested:?}"),
      )
    })
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

fn locate_pack(dotnet_root: &Path, global_packages: &Path, package_id: &str, version: &str) -> Result<PathBuf, RuntimePackError> {
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
  })
}

fn read_runtime_assets(path: &Path, pack_root: &Path, target_framework: &str, expected_version: &str) -> Result<Vec<RuntimeAsset>, RuntimePackError> {
  let bytes = fs::read(path).map_err(|error| io_error("read runtime-pack manifest", path, error))?;
  let mut reader = Reader::from_reader(bytes.as_slice());
  reader.config_mut().trim_text(true);
  let mut root_seen = false;
  let mut assets = Vec::with_capacity(192);

  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) | Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"FileList" => {
        if root_seen {
          return Err(invalid_manifest(path, "runtime-pack manifest contains multiple FileList roots"));
        }
        let framework_name = xml_attribute(&reader, &element, b"FrameworkName", path)?;
        let framework_version = xml_attribute(&reader, &element, b"TargetFrameworkVersion", path)?;
        if framework_name.as_deref() != Some(IMPLICIT_FRAMEWORK_REFERENCE) || framework_version.as_deref() != Some(expected_version) {
          return Err(invalid_manifest(
            path,
            format!("runtime-pack manifest target does not match {target_framework}"),
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
        if relative_path.is_absolute() || relative_path.components().any(|component| matches!(component, Component::ParentDir)) {
          return Err(invalid_manifest(path, "runtime asset path escapes the selected pack"));
        }
        let asset = join_relative_path(pack_root, relative_path);
        if !asset.is_file() {
          return Err(RuntimePackError::new(
            RuntimePackErrorKind::MissingAsset,
            &asset,
            format!("runtime-pack asset {} is missing", asset.display()),
          ));
        }
        assets.push(RuntimeAsset { kind, path: asset });
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
  Ok(assets)
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
  apphost_template: &Path,
  runtime_assets: &[RuntimeAsset],
) -> Result<RuntimePackPlan, RuntimePackError> {
  let estimated_text = runtime_assets.iter().map(|asset| asset.path.as_os_str().len()).sum::<usize>() + 2048;
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
  let apphost_template_span = table.push_path(apphost_template)?;

  let mut assets = Vec::with_capacity(runtime_assets.len());
  let managed_start = compact_len(assets.len(), "runtime asset span batch")?;
  for asset in runtime_assets.iter().filter(|asset| asset.kind == AssetKind::Managed) {
    assets.push(table.push_path(&asset.path)?);
  }
  let managed_end = compact_len(assets.len(), "runtime asset span batch")?;
  let native_start = managed_end;
  for asset in runtime_assets.iter().filter(|asset| asset.kind == AssetKind::Native) {
    assets.push(table.push_path(&asset.path)?);
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
  }

  #[test]
  fn unknown_runtime_does_not_gain_a_string_inferred_fallback() {
    let graph_path = TempDirectory::new();
    let path = graph_path.write("graph.json", r##"{"runtimes":{"linux-x64":{"#import":[]}}}"##);
    let graph = RuntimeIdentifierGraph::load(&path).unwrap();
    let error = select_pack_runtime_identifier(&graph, "linux-custom-x64", "linux-x64", &path, "runtime pack").unwrap_err();
    assert_eq!(error.kind(), RuntimePackErrorKind::UnsupportedRuntime);
  }

  #[test]
  fn rejects_pack_asset_paths_which_escape_the_pack() {
    let temp = TempDirectory::new();
    let root = temp.0.join("pack");
    let manifest = temp.write(
      "pack/data/RuntimeList.xml",
      r#"<FileList TargetFrameworkVersion="10.0" FrameworkName="Microsoft.NETCore.App"><File Type="Managed" Path="../outside.dll"/></FileList>"#,
    );
    let error = read_runtime_assets(&manifest, &root, "net10.0", "10.0").unwrap_err();
    assert_eq!(error.kind(), RuntimePackErrorKind::InvalidManifest);
  }
}
