use std::{
  cmp::Ordering,
  error::Error,
  fmt, fs, io,
  path::{Path, PathBuf},
};

use quick_xml::{Reader, XmlVersion, events::Event};

use crate::{FrameworkReference, ProjectSpec, RuntimeRollForward, SdkInventory, package::global_packages_directory};

const BUNDLED_VERSIONS_FILE: &str = "Microsoft.NETCoreSdk.BundledVersions.props";
const IMPLICIT_FRAMEWORK_REFERENCE: &str = "Microsoft.NETCore.App";
const NO_TEXT: TextSpan = TextSpan { start: u32::MAX, len: 0 };

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextSpan {
  start: u32,
  len: u32,
}

const _: () = assert!(size_of::<TextSpan>() == 8);
const _: () = assert!(align_of::<TextSpan>() == 4);

/// One resolved framework row consumed linearly by runtime-config and compiler planning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedFrameworkReference {
  reference: TextSpan,
  runtime_name: TextSpan,
  requested_version: TextSpan,
  selected_version: TextSpan,
  shared_root: TextSpan,
  targeting_pack_id: TextSpan,
  targeting_pack_version: TextSpan,
  targeting_pack_root: TextSpan,
  profile: TextSpan,
}

// ASSUMPTION: the benchmark machine has 64-byte cache lines. This 72-byte
// immutable record intentionally remains naturally aligned rather than adding
// 56 bytes of padding solely to isolate read-only rows.
const _: () = assert!(size_of::<ResolvedFrameworkReference>() == 72);
const _: () = assert!(align_of::<ResolvedFrameworkReference>() == 4);

/// Framework-reference and installed shared-framework selections for one project.
///
/// All variable text has one owner and one allocation. The framework records
/// form one contiguous immutable batch with sequential access in downstream
/// runtime-config and reference-pack transforms.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameworkReferencePlan {
  text: Box<str>,
  frameworks: Box<[ResolvedFrameworkReference]>,
  project: TextSpan,
  sdk_version: TextSpan,
  manifest: TextSpan,
  target_framework: TextSpan,
  roll_forward: RuntimeRollForward,
  self_contained: bool,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<FrameworkReferencePlan>() == 72);
const _: () = assert!(align_of::<FrameworkReferencePlan>() == align_of::<usize>());

impl FrameworkReferencePlan {
  /// Returns the project which owns this framework batch.
  pub fn project(&self) -> &str {
    self.get(self.project)
  }

  /// Returns the selected SDK version.
  pub fn sdk_version(&self) -> &str {
    self.get(self.sdk_version)
  }

  /// Returns the selected SDK framework manifest.
  pub fn manifest(&self) -> &str {
    self.get(self.manifest)
  }

  /// Returns the evaluated target framework.
  pub fn target_framework(&self) -> &str {
    self.get(self.target_framework)
  }

  /// Returns the runtime-host roll-forward policy.
  pub fn roll_forward(&self) -> RuntimeRollForward {
    self.roll_forward
  }

  /// Returns whether the project carries its runtime instead of using shared frameworks.
  pub fn self_contained(&self) -> bool {
    self.self_contained
  }

  /// Returns the contiguous resolved-framework batch.
  pub fn frameworks(&self) -> &[ResolvedFrameworkReference] {
    &self.frameworks
  }

  /// Returns the project-facing `FrameworkReference` identity.
  pub fn reference(&self, framework: ResolvedFrameworkReference) -> &str {
    self.get(framework.reference)
  }

  /// Returns the runtimeconfig/shared-directory framework name.
  pub fn runtime_name(&self, framework: ResolvedFrameworkReference) -> &str {
    self.get(framework.runtime_name)
  }

  /// Returns the minimum runtime version selected from project and SDK data.
  pub fn requested_version(&self, framework: ResolvedFrameworkReference) -> &str {
    self.get(framework.requested_version)
  }

  /// Returns the installed shared-framework version selected by roll-forward.
  pub fn selected_version(&self, framework: ResolvedFrameworkReference) -> Option<&str> {
    self.optional(framework.selected_version)
  }

  /// Returns the selected installed shared-framework directory.
  pub fn shared_root(&self, framework: ResolvedFrameworkReference) -> Option<&str> {
    self.optional(framework.shared_root)
  }

  /// Returns the targeting-pack NuGet identity.
  pub fn targeting_pack_id(&self, framework: ResolvedFrameworkReference) -> &str {
    self.get(framework.targeting_pack_id)
  }

  /// Returns the targeting-pack version.
  pub fn targeting_pack_version(&self, framework: ResolvedFrameworkReference) -> &str {
    self.get(framework.targeting_pack_version)
  }

  /// Returns the installed or restored targeting-pack directory.
  pub fn targeting_pack_root(&self, framework: ResolvedFrameworkReference) -> &str {
    self.get(framework.targeting_pack_root)
  }

  /// Returns the optional framework profile.
  pub fn profile(&self, framework: ResolvedFrameworkReference) -> Option<&str> {
    self.optional(framework.profile)
  }

  fn get(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }

  fn optional(&self, span: TextSpan) -> Option<&str> {
    (span != NO_TEXT).then(|| self.get(span))
  }
}

/// Stable framework-reference planning failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameworkReferenceErrorKind {
  /// Required SDK, pack, or shared-framework data could not be read.
  Io,
  /// The selected SDK framework manifest is malformed or incomplete.
  InvalidManifest,
  /// A project framework reference is unknown to the selected SDK and TFM.
  UnknownFramework,
  /// A runtime version does not follow the supported three-part format.
  InvalidVersion,
  /// A required targeting pack is not installed or restored.
  TargetingPackNotFound,
  /// No installed shared framework satisfies the requested roll-forward policy.
  SharedFrameworkNotFound,
  /// NuGet configuration could not provide a global-packages directory.
  Configuration,
  /// A path cannot be represented in the UTF-8 plan table.
  NonUnicodePath,
  /// Compact plan storage exceeded its 32-bit index space.
  TextOverflow,
}

/// A framework-reference planning failure with stable path context.
#[derive(Debug)]
pub struct FrameworkReferenceError {
  kind: FrameworkReferenceErrorKind,
  path: PathBuf,
  message: String,
}

impl FrameworkReferenceError {
  fn new(kind: FrameworkReferenceErrorKind, path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
    Self {
      kind,
      path: path.into(),
      message: message.into(),
    }
  }

  /// Returns the stable failure category.
  pub fn kind(&self) -> FrameworkReferenceErrorKind {
    self.kind
  }

  /// Returns the project, manifest, pack, or shared root associated with the failure.
  pub fn path(&self) -> &Path {
    &self.path
  }
}

impl fmt::Display for FrameworkReferenceError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl Error for FrameworkReferenceError {}

struct KnownFramework {
  target_framework: String,
  name: String,
  runtime_name: String,
  default_runtime_version: String,
  latest_runtime_version: String,
  targeting_pack_id: String,
  targeting_pack_version: String,
  profile: Option<String>,
}

struct RequestedFramework<'a> {
  name: &'a str,
  source: Option<FrameworkReference>,
}

struct MaterializedFramework {
  reference: String,
  runtime_name: String,
  requested_version: String,
  selected_version: Option<String>,
  shared_root: Option<PathBuf>,
  targeting_pack_id: String,
  targeting_pack_version: String,
  targeting_pack_root: PathBuf,
  profile: Option<String>,
}

/// Resolves framework references for a project batch using one SDK-manifest read.
///
/// The selected SDK/root is a genuine singleton for one command invocation.
/// Project rows retain input order and each project's framework rows retain the
/// implicit-core-first order used by SDK-style projects.
pub fn plan_framework_references(
  projects: &[&ProjectSpec],
  inventory: &SdkInventory,
  packages_directory: Option<&Path>,
) -> Result<Box<[FrameworkReferencePlan]>, FrameworkReferenceError> {
  if projects.is_empty() {
    return Ok(Box::new([]));
  }

  let selected = inventory.selected();
  let sdk_root = inventory.installation_path(selected);
  let manifest = sdk_root.join(BUNDLED_VERSIONS_FILE);
  let definitions = read_framework_definitions(&manifest, projects)?;
  let dotnet_root = inventory.root(selected);
  let mut plans = Vec::with_capacity(projects.len());
  for project in projects {
    plans.push(plan_project(
      project,
      selected.version.as_str(),
      &manifest,
      &definitions,
      dotnet_root,
      packages_directory,
    )?);
  }
  Ok(plans.into_boxed_slice())
}

fn plan_project(
  project: &ProjectSpec,
  sdk_version: &str,
  manifest: &Path,
  definitions: &[KnownFramework],
  dotnet_root: &Path,
  packages_directory: Option<&Path>,
) -> Result<FrameworkReferencePlan, FrameworkReferenceError> {
  let explicit_core = project
    .framework_references()
    .iter()
    .copied()
    .find(|reference| project.framework_reference_id(*reference).eq_ignore_ascii_case(IMPLICIT_FRAMEWORK_REFERENCE));
  let mut requested = Vec::with_capacity(project.framework_references().len() + usize::from(explicit_core.is_none()));
  requested.push(RequestedFramework {
    name: IMPLICIT_FRAMEWORK_REFERENCE,
    source: explicit_core,
  });
  for reference in project.framework_references() {
    let name = project.framework_reference_id(*reference);
    if !name.eq_ignore_ascii_case(IMPLICIT_FRAMEWORK_REFERENCE) {
      requested.push(RequestedFramework {
        name,
        source: Some(*reference),
      });
    }
  }

  let mut rows = Vec::with_capacity(requested.len());
  let mut global_packages = None;
  for request in requested {
    let definition = definitions
      .iter()
      .find(|definition| definition.target_framework == project.target_framework() && definition.name.eq_ignore_ascii_case(request.name))
      .ok_or_else(|| {
        FrameworkReferenceError::new(
          FrameworkReferenceErrorKind::UnknownFramework,
          manifest,
          format!("selected SDK has no framework reference {:?} for {}", request.name, project.target_framework()),
        )
      })?;
    let requested_version = requested_runtime_version(project, request.source, definition);
    let requested_parsed = RuntimeVersion::parse(requested_version).ok_or_else(|| {
      FrameworkReferenceError::new(
        FrameworkReferenceErrorKind::InvalidVersion,
        project.project_path(),
        format!("runtime framework version {requested_version:?} is not a three-part .NET runtime version"),
      )
    })?;
    let targeting_pack_version = request
      .source
      .and_then(|source| project.framework_targeting_pack_version(source))
      .unwrap_or(&definition.targeting_pack_version);
    RuntimeVersion::parse(targeting_pack_version).ok_or_else(|| {
      FrameworkReferenceError::new(
        FrameworkReferenceErrorKind::InvalidVersion,
        project.project_path(),
        format!("targeting pack version {targeting_pack_version:?} is not a three-part .NET version"),
      )
    })?;
    let targeting_pack_root = locate_targeting_pack(
      dotnet_root,
      project.project_directory(),
      packages_directory,
      &mut global_packages,
      &definition.targeting_pack_id,
      targeting_pack_version,
    )?;
    let (selected_version, shared_root) = if project.self_contained() {
      (None, None)
    } else {
      let shared_base = dotnet_root.join("shared").join(&definition.runtime_name);
      let selected = select_installed_runtime(&shared_base, &requested_parsed, project.roll_forward())?;
      let root = shared_base.join(&selected.text);
      (Some(selected.text), Some(root))
    };
    rows.push(MaterializedFramework {
      reference: request.name.to_owned(),
      runtime_name: definition.runtime_name.clone(),
      requested_version: requested_version.to_owned(),
      selected_version,
      shared_root,
      targeting_pack_id: definition.targeting_pack_id.clone(),
      targeting_pack_version: targeting_pack_version.to_owned(),
      targeting_pack_root,
      profile: definition.profile.clone(),
    });
  }

  materialize_plan(project, sdk_version, manifest, rows)
}

fn requested_runtime_version<'a>(project: &'a ProjectSpec, reference: Option<FrameworkReference>, definition: &'a KnownFramework) -> &'a str {
  if let Some(version) = reference.and_then(|reference| project.framework_runtime_version(reference)) {
    return version;
  }
  if let Some(version) = project.runtime_framework_version() {
    return version;
  }
  let use_latest = reference
    .and_then(|reference| project.framework_target_latest_runtime_patch(reference))
    .or(project.target_latest_runtime_patch())
    .unwrap_or(false);
  if use_latest {
    &definition.latest_runtime_version
  } else {
    &definition.default_runtime_version
  }
}

fn locate_targeting_pack(
  dotnet_root: &Path,
  project_directory: &Path,
  packages_directory: Option<&Path>,
  global_packages: &mut Option<PathBuf>,
  id: &str,
  version: &str,
) -> Result<PathBuf, FrameworkReferenceError> {
  let installed = dotnet_root.join("packs").join(id).join(version);
  if installed.is_dir() {
    return Ok(installed);
  }
  if global_packages.is_none() {
    *global_packages = Some(global_packages_directory(project_directory, packages_directory).map_err(|error| {
      FrameworkReferenceError::new(
        FrameworkReferenceErrorKind::Configuration,
        project_directory,
        format!("failed to resolve the global-packages directory: {error}"),
      )
    })?);
  }
  let global_packages = global_packages.as_deref().expect("global packages was initialized");
  let cached = global_packages.join(id.to_ascii_lowercase()).join(version.to_ascii_lowercase());
  if cached.is_dir() {
    return Ok(cached);
  }
  Err(FrameworkReferenceError::new(
    FrameworkReferenceErrorKind::TargetingPackNotFound,
    &cached,
    format!(
      "required targeting pack {id} {version} is not installed under {} or restored under {}",
      installed.display(),
      cached.display()
    ),
  ))
}

fn read_framework_definitions(path: &Path, projects: &[&ProjectSpec]) -> Result<Vec<KnownFramework>, FrameworkReferenceError> {
  let bytes = fs::read(path).map_err(|error| io_error("read SDK framework manifest", path, error))?;
  let mut reader = Reader::from_reader(bytes.as_slice());
  reader.config_mut().trim_text(true);
  let mut definitions = Vec::new();

  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) | Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"KnownFrameworkReference" => {
        let target_framework = required_attribute(&reader, &element, b"TargetFramework", path)?;
        if projects.iter().any(|project| project.target_framework() == target_framework) {
          definitions.push(KnownFramework {
            target_framework,
            name: required_attribute(&reader, &element, b"Include", path)?,
            runtime_name: required_attribute(&reader, &element, b"RuntimeFrameworkName", path)?,
            default_runtime_version: required_attribute(&reader, &element, b"DefaultRuntimeFrameworkVersion", path)?,
            latest_runtime_version: required_attribute(&reader, &element, b"LatestRuntimeFrameworkVersion", path)?,
            targeting_pack_id: required_attribute(&reader, &element, b"TargetingPackName", path)?,
            targeting_pack_version: required_attribute(&reader, &element, b"TargetingPackVersion", path)?,
            profile: xml_attribute(&reader, &element, b"Profile", path)?,
          });
        }
      },
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(invalid_manifest(path, format!("invalid SDK framework manifest XML: {error}"))),
    }
  }
  Ok(definitions)
}

fn required_attribute(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  name: &[u8],
  path: &Path,
) -> Result<String, FrameworkReferenceError> {
  xml_attribute(reader, element, name, path)?.filter(|value| !value.is_empty()).ok_or_else(|| {
    invalid_manifest(
      path,
      format!(
        "{} element requires a non-empty {} attribute",
        String::from_utf8_lossy(local_name(element.name().as_ref())),
        String::from_utf8_lossy(name)
      ),
    )
  })
}

fn xml_attribute(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  name: &[u8],
  path: &Path,
) -> Result<Option<String>, FrameworkReferenceError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| invalid_manifest(path, format!("invalid SDK framework manifest attribute: {error}")))?;
    if local_name(attribute.key.as_ref()) == name {
      return attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
        .map(|value| Some(value.into_owned()))
        .map_err(|error| invalid_manifest(path, format!("invalid SDK framework manifest attribute value: {error}")));
    }
  }
  Ok(None)
}

fn local_name(name: &[u8]) -> &[u8] {
  name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn invalid_manifest(path: &Path, message: impl Into<String>) -> FrameworkReferenceError {
  FrameworkReferenceError::new(FrameworkReferenceErrorKind::InvalidManifest, path, message)
}

#[derive(Clone, Debug)]
struct RuntimeVersion {
  text: String,
  major: u32,
  minor: u32,
  patch: u32,
  prerelease: Option<String>,
}

impl RuntimeVersion {
  fn parse(value: &str) -> Option<Self> {
    let (precedence, build) = value.split_once('+').map_or((value, None), |(precedence, build)| (precedence, Some(build)));
    if build.is_some_and(|build| !valid_identifiers(build)) {
      return None;
    }
    let (numbers, prerelease) = match precedence.split_once('-') {
      Some((numbers, prerelease)) if valid_identifiers(prerelease) => (numbers, Some(prerelease)),
      Some(_) => return None,
      None => (precedence, None),
    };
    let mut parts = numbers.split('.');
    let major = numeric_part(parts.next()?)?;
    let minor = numeric_part(parts.next()?)?;
    let patch = numeric_part(parts.next()?)?;
    if parts.next().is_some() {
      return None;
    }
    Some(Self {
      text: value.to_owned(),
      major,
      minor,
      patch,
      prerelease: prerelease.map(str::to_owned),
    })
  }

  fn is_prerelease(&self) -> bool {
    self.prerelease.is_some()
  }
}

impl PartialEq for RuntimeVersion {
  fn eq(&self, other: &Self) -> bool {
    self.cmp(other) == Ordering::Equal
  }
}

impl Eq for RuntimeVersion {}

impl PartialOrd for RuntimeVersion {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    Some(self.cmp(other))
  }
}

impl Ord for RuntimeVersion {
  fn cmp(&self, other: &Self) -> Ordering {
    self
      .major
      .cmp(&other.major)
      .then_with(|| self.minor.cmp(&other.minor))
      .then_with(|| self.patch.cmp(&other.patch))
      .then_with(|| compare_prerelease(self.prerelease.as_deref(), other.prerelease.as_deref()))
  }
}

fn numeric_part(value: &str) -> Option<u32> {
  if value.is_empty() || (value.len() > 1 && value.starts_with('0')) || !value.bytes().all(|byte| byte.is_ascii_digit()) {
    return None;
  }
  value.parse().ok()
}

fn valid_identifiers(value: &str) -> bool {
  value
    .split('.')
    .all(|identifier| !identifier.is_empty() && identifier.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
}

fn compare_prerelease(left: Option<&str>, right: Option<&str>) -> Ordering {
  match (left, right) {
    (None, None) => Ordering::Equal,
    (None, Some(_)) => Ordering::Greater,
    (Some(_), None) => Ordering::Less,
    (Some(left), Some(right)) => {
      let mut left = left.split('.');
      let mut right = right.split('.');
      loop {
        match (left.next(), right.next()) {
          (None, None) => return Ordering::Equal,
          (None, Some(_)) => return Ordering::Less,
          (Some(_), None) => return Ordering::Greater,
          (Some(left), Some(right)) => {
            let left_numeric = left.bytes().all(|byte| byte.is_ascii_digit());
            let right_numeric = right.bytes().all(|byte| byte.is_ascii_digit());
            let ordering = match (left_numeric, right_numeric) {
              (true, true) => left.len().cmp(&right.len()).then_with(|| left.cmp(right)),
              (true, false) => Ordering::Less,
              (false, true) => Ordering::Greater,
              (false, false) => left.cmp(right),
            };
            if ordering != Ordering::Equal {
              return ordering;
            }
          },
        }
      }
    },
  }
}

fn select_installed_runtime(base: &Path, requested: &RuntimeVersion, policy: RuntimeRollForward) -> Result<RuntimeVersion, FrameworkReferenceError> {
  let entries = fs::read_dir(base).map_err(|error| {
    if error.kind() == io::ErrorKind::NotFound {
      FrameworkReferenceError::new(
        FrameworkReferenceErrorKind::SharedFrameworkNotFound,
        base,
        format!("shared framework {} is not installed", base.display()),
      )
    } else {
      io_error("enumerate shared frameworks", base, error)
    }
  })?;
  let mut installed = Vec::new();
  for entry in entries {
    let entry = entry.map_err(|error| io_error("enumerate shared frameworks", base, error))?;
    if entry
      .file_type()
      .map_err(|error| io_error("inspect shared framework", &entry.path(), error))?
      .is_dir()
    {
      let Some(text) = entry.file_name().to_str().map(str::to_owned) else {
        continue;
      };
      if let Some(version) = RuntimeVersion::parse(&text) {
        installed.push(version);
      }
    }
  }
  installed.sort_unstable();

  let selected = if requested.is_prerelease() {
    select_by_policy(&installed, requested, policy, false)
  } else {
    select_by_policy(&installed, requested, policy, true).or_else(|| select_by_policy(&installed, requested, policy, false))
  };
  selected.ok_or_else(|| {
    FrameworkReferenceError::new(
      FrameworkReferenceErrorKind::SharedFrameworkNotFound,
      base,
      format!(
        "no installed {} version satisfies {} with roll-forward {}",
        base.file_name().and_then(|name| name.to_str()).unwrap_or("shared framework"),
        requested.text,
        policy.as_str()
      ),
    )
  })
}

fn select_by_policy(installed: &[RuntimeVersion], requested: &RuntimeVersion, policy: RuntimeRollForward, stable_only: bool) -> Option<RuntimeVersion> {
  let eligible_release = |version: &&RuntimeVersion| !stable_only || !version.is_prerelease();
  match policy {
    RuntimeRollForward::Disable => installed.iter().filter(eligible_release).find(|version| *version == requested).cloned(),
    RuntimeRollForward::LatestPatch => installed
      .iter()
      .filter(eligible_release)
      .filter(|version| version.major == requested.major && version.minor == requested.minor && *version >= requested)
      .max()
      .cloned(),
    RuntimeRollForward::Minor => select_nearest_band(
      installed,
      requested,
      |version| (!stable_only || !version.is_prerelease()) && version.major == requested.major && version >= requested,
      |version| (version.major, version.minor),
    ),
    RuntimeRollForward::Major => select_nearest_band(
      installed,
      requested,
      |version| (!stable_only || !version.is_prerelease()) && version >= requested,
      |version| (version.major, version.minor),
    ),
    RuntimeRollForward::LatestMinor => installed
      .iter()
      .filter(eligible_release)
      .filter(|version| version.major == requested.major && *version >= requested)
      .max()
      .cloned(),
    RuntimeRollForward::LatestMajor => installed.iter().filter(eligible_release).filter(|version| *version >= requested).max().cloned(),
  }
}

fn select_nearest_band<K: Copy + Ord>(
  installed: &[RuntimeVersion],
  requested: &RuntimeVersion,
  eligible: impl Fn(&RuntimeVersion) -> bool,
  band: impl Fn(&RuntimeVersion) -> K,
) -> Option<RuntimeVersion> {
  let nearest = installed.iter().filter(|version| eligible(version)).map(&band).min()?;
  installed
    .iter()
    .filter(|version| eligible(version) && band(version) == nearest && *version >= requested)
    .max()
    .cloned()
}

fn materialize_plan(
  project: &ProjectSpec,
  sdk_version: &str,
  manifest: &Path,
  rows: Vec<MaterializedFramework>,
) -> Result<FrameworkReferencePlan, FrameworkReferenceError> {
  let project_text = path_text(project.project_path())?;
  let manifest_text = path_text(manifest)?;
  let estimated = project_text.len()
    + sdk_version.len()
    + manifest_text.len()
    + project.target_framework().len()
    + rows
      .iter()
      .map(|row| {
        row.reference.len()
          + row.runtime_name.len()
          + row.requested_version.len()
          + row.selected_version.as_ref().map_or(0, String::len)
          + row.shared_root.as_ref().and_then(|path| path.to_str()).map_or(0, str::len)
          + row.targeting_pack_id.len()
          + row.targeting_pack_version.len()
          + row.targeting_pack_root.to_str().map_or(0, str::len)
          + row.profile.as_ref().map_or(0, String::len)
      })
      .sum::<usize>();
  let mut table = TextTable::with_capacity(estimated);
  let project_span = table.push(project_text)?;
  let sdk_version_span = table.push(sdk_version)?;
  let manifest_span = table.push(manifest_text)?;
  let target_framework_span = table.push(project.target_framework())?;
  let mut frameworks = Vec::with_capacity(rows.len());
  for row in rows {
    frameworks.push(ResolvedFrameworkReference {
      reference: table.push(&row.reference)?,
      runtime_name: table.push(&row.runtime_name)?,
      requested_version: table.push(&row.requested_version)?,
      selected_version: match row.selected_version.as_deref() {
        Some(version) => table.push(version)?,
        None => NO_TEXT,
      },
      shared_root: match row.shared_root.as_deref() {
        Some(path) => table.push(path_text(path)?)?,
        None => NO_TEXT,
      },
      targeting_pack_id: table.push(&row.targeting_pack_id)?,
      targeting_pack_version: table.push(&row.targeting_pack_version)?,
      targeting_pack_root: table.push(path_text(&row.targeting_pack_root)?)?,
      profile: match row.profile.as_deref() {
        Some(profile) => table.push(profile)?,
        None => NO_TEXT,
      },
    });
  }
  Ok(FrameworkReferencePlan {
    text: table.text.into_boxed_str(),
    frameworks: frameworks.into_boxed_slice(),
    project: project_span,
    sdk_version: sdk_version_span,
    manifest: manifest_span,
    target_framework: target_framework_span,
    roll_forward: project.roll_forward(),
    self_contained: project.self_contained(),
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

  fn push(&mut self, value: &str) -> Result<TextSpan, FrameworkReferenceError> {
    let start = u32::try_from(self.text.len())
      .map_err(|_| FrameworkReferenceError::new(FrameworkReferenceErrorKind::TextOverflow, PathBuf::new(), "framework plan text exceeds 4 GiB"))?;
    let len = u32::try_from(value.len()).map_err(|_| {
      FrameworkReferenceError::new(
        FrameworkReferenceErrorKind::TextOverflow,
        PathBuf::new(),
        "one framework plan value exceeds 4 GiB",
      )
    })?;
    self.text.push_str(value);
    Ok(TextSpan { start, len })
  }
}

fn path_text(path: &Path) -> Result<&str, FrameworkReferenceError> {
  path.to_str().ok_or_else(|| {
    FrameworkReferenceError::new(
      FrameworkReferenceErrorKind::NonUnicodePath,
      path,
      format!("framework plan path {} is not valid Unicode", path.display()),
    )
  })
}

fn io_error(operation: &str, path: &Path, error: io::Error) -> FrameworkReferenceError {
  FrameworkReferenceError::new(
    FrameworkReferenceErrorKind::Io,
    path,
    format!("failed to {operation} {}: {error}", path.display()),
  )
}

#[cfg(test)]
mod tests {
  use std::{
    env,
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
    time::{SystemTime, UNIX_EPOCH},
  };

  use crate::{ProjectConfiguration, SdkInstallation, SdkVersion, evaluate_project_path};

  use super::*;

  static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let nonce = NEXT_TEMP.fetch_add(1, AtomicOrdering::Relaxed);
      let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let path = env::temp_dir().join(format!("dv-framework-reference-test-{}-{time}-{nonce}", std::process::id()));
      fs::create_dir_all(&path).unwrap();
      Self(path)
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
      let path = self.0.join(relative);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(&path, contents).unwrap();
      path
    }

    fn directory(&self, relative: &str) -> PathBuf {
      let path = self.0.join(relative);
      fs::create_dir_all(&path).unwrap();
      path
    }
  }

  impl Drop for TempDirectory {
    fn drop(&mut self) {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }

  fn versions(values: &[&str]) -> Vec<RuntimeVersion> {
    values.iter().map(|value| RuntimeVersion::parse(value).unwrap()).collect()
  }

  #[test]
  fn runtime_roll_forward_policies_select_documented_bands() {
    let installed = versions(&["8.0.3", "8.0.9", "8.1.2", "8.1.7", "9.0.1", "9.0.4", "10.0.0"]);
    let requested = RuntimeVersion::parse("8.0.4").unwrap();

    assert_eq!(
      select_by_policy(&installed, &requested, RuntimeRollForward::Disable, false).map(|value| value.text),
      None
    );
    assert_eq!(
      select_by_policy(&installed, &requested, RuntimeRollForward::LatestPatch, false).map(|value| value.text),
      Some("8.0.9".into())
    );
    assert_eq!(
      select_by_policy(&installed, &requested, RuntimeRollForward::Minor, false).map(|value| value.text),
      Some("8.0.9".into())
    );
    assert_eq!(
      select_by_policy(&installed, &requested, RuntimeRollForward::Major, false).map(|value| value.text),
      Some("8.0.9".into())
    );
    assert_eq!(
      select_by_policy(&installed, &requested, RuntimeRollForward::LatestMinor, false).map(|value| value.text),
      Some("8.1.7".into())
    );
    assert_eq!(
      select_by_policy(&installed, &requested, RuntimeRollForward::LatestMajor, false).map(|value| value.text),
      Some("10.0.0".into())
    );
  }

  #[test]
  fn nearest_policies_advance_only_when_the_requested_band_is_missing() {
    let installed = versions(&["8.1.2", "8.1.7", "9.0.1", "9.0.4", "9.1.3", "10.2.0"]);
    let requested = RuntimeVersion::parse("8.0.4").unwrap();

    assert_eq!(
      select_by_policy(&installed, &requested, RuntimeRollForward::Minor, false).map(|value| value.text),
      Some("8.1.7".into())
    );
    assert_eq!(
      select_by_policy(&installed, &requested, RuntimeRollForward::Major, false).map(|value| value.text),
      Some("8.1.7".into())
    );

    let requested = RuntimeVersion::parse("8.2.0").unwrap();
    assert_eq!(
      select_by_policy(&installed, &requested, RuntimeRollForward::Major, false).map(|value| value.text),
      Some("9.0.4".into())
    );
  }

  #[test]
  fn runtime_versions_use_semver_prerelease_order() {
    let preview_2 = RuntimeVersion::parse("10.0.0-preview.2").unwrap();
    let preview_10 = RuntimeVersion::parse("10.0.0-preview.10").unwrap();
    let stable = RuntimeVersion::parse("10.0.0").unwrap();

    assert!(preview_2 < preview_10);
    assert!(preview_10 < stable);
    assert!(RuntimeVersion::parse("10.0").is_none());
    assert!(RuntimeVersion::parse("10.0.0-").is_none());
    assert!(RuntimeVersion::parse("10.0.0+").is_none());
  }

  #[test]
  fn targeting_pack_cache_discovery_is_deferred_until_installed_lookup_misses() {
    let temp = TempDirectory::new();
    let dotnet = temp.directory("dotnet");
    let packages = temp.directory("packages/example.ref/10.0.0");
    let packages_root = temp.0.join("packages");
    let mut discovered = None;

    let selected = locate_targeting_pack(&dotnet, &temp.0, Some(&packages_root), &mut discovered, "Example.Ref", "10.0.0").unwrap();

    assert_eq!(selected, packages);
    assert_eq!(discovered.as_deref(), Some(packages_root.as_path()));
  }

  #[test]
  fn resolves_implicit_and_explicit_frameworks_from_sdk_and_installed_data() {
    let temp = TempDirectory::new();
    let root = temp.directory("dotnet");
    temp.write(
      "dotnet/sdk/10.0.100/Microsoft.NETCoreSdk.BundledVersions.props",
      r#"<Project><ItemGroup>
        <KnownFrameworkReference Include="Microsoft.NETCore.App" TargetFramework="net10.0" RuntimeFrameworkName="Microsoft.NETCore.App" DefaultRuntimeFrameworkVersion="10.0.0" LatestRuntimeFrameworkVersion="10.0.1" TargetingPackName="Microsoft.NETCore.App.Ref" TargetingPackVersion="10.0.1" />
        <KnownFrameworkReference Include="Microsoft.AspNetCore.App" TargetFramework="net10.0" RuntimeFrameworkName="Microsoft.AspNetCore.App" DefaultRuntimeFrameworkVersion="10.0.0" LatestRuntimeFrameworkVersion="10.0.1" TargetingPackName="Microsoft.AspNetCore.App.Ref" TargetingPackVersion="10.0.1" />
      </ItemGroup></Project>"#,
    );
    temp.directory("dotnet/packs/Microsoft.NETCore.App.Ref/10.0.1");
    temp.directory("dotnet/packs/Microsoft.AspNetCore.App.Ref/10.0.1");
    temp.directory("dotnet/shared/Microsoft.NETCore.App/10.0.7");
    temp.directory("dotnet/shared/Microsoft.AspNetCore.App/10.0.7");
    let project_path = temp.write(
      "project/App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><TargetLatestRuntimePatch>true</TargetLatestRuntimePatch><RollForward>LatestPatch</RollForward></PropertyGroup><ItemGroup><FrameworkReference Include="Microsoft.AspNetCore.App" /></ItemGroup></Project>"#,
    );
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let inventory = SdkInventory {
      roots: vec![root],
      installations: vec![SdkInstallation {
        version: SdkVersion::parse("10.0.100").unwrap(),
        root_index: 0,
      }],
      selected_index: 0,
      global_json: None,
    };
    let packages = temp.directory("packages");

    let plans = plan_framework_references(&[&project], &inventory, Some(&packages)).unwrap();
    let plan = &plans[0];

    assert_eq!(plan.frameworks().len(), 2);
    let core = plan.frameworks()[0];
    let aspnet = plan.frameworks()[1];
    assert_eq!(plan.reference(core), "Microsoft.NETCore.App");
    assert_eq!(plan.reference(aspnet), "Microsoft.AspNetCore.App");
    assert_eq!(plan.requested_version(core), "10.0.1");
    assert_eq!(plan.selected_version(core), Some("10.0.7"));
    assert_eq!(plan.selected_version(aspnet), Some("10.0.7"));
    assert_eq!(plan.targeting_pack_id(aspnet), "Microsoft.AspNetCore.App.Ref");
    assert!(Path::new(plan.targeting_pack_root(aspnet)).ends_with(Path::new("Microsoft.AspNetCore.App.Ref").join("10.0.1")));
  }
}
