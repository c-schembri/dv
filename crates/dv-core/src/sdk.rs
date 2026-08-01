use std::{
  cmp::Ordering,
  env,
  error::Error,
  fmt, fs, io,
  mem::{align_of, size_of},
  path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{AncestorInputErrorKind, AncestorInputKind, AncestorInputRequest, discover_ancestor_inputs};

#[cfg(not(windows))]
use std::io::BufRead;

#[cfg(windows)]
use winreg::{
  RegKey,
  enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY},
};

const NO_PRERELEASE: u16 = u16::MAX;
const MAX_SDK_INSTALLATIONS: usize = 4_096;
const MAX_RUNTIME_INSTALLATIONS: usize = 4_096;

/// A host architecture accepted by the .NET muxer inventory commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DotnetArchitecture {
  Arm,
  Arm64,
  ArmV6,
  LoongArch64,
  Ppc64Le,
  RiscV64,
  S390X,
  X64,
  X86,
  Wasm,
}

const _: () = assert!(size_of::<DotnetArchitecture>() == 1);
const _: () = assert!(align_of::<DotnetArchitecture>() == 1);

impl DotnetArchitecture {
  /// Parses the case-insensitive architecture names accepted by .NET 10.
  pub fn parse(value: &str) -> Option<Self> {
    [
      ("arm", Self::Arm),
      ("arm64", Self::Arm64),
      ("armv6", Self::ArmV6),
      ("loongarch64", Self::LoongArch64),
      ("ppc64le", Self::Ppc64Le),
      ("riscv64", Self::RiscV64),
      ("s390x", Self::S390X),
      ("x64", Self::X64),
      ("x86", Self::X86),
      ("wasm", Self::Wasm),
    ]
    .into_iter()
    .find_map(|(name, architecture)| value.eq_ignore_ascii_case(name).then_some(architecture))
  }

  /// Returns the stable lowercase .NET architecture name.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Arm => "arm",
      Self::Arm64 => "arm64",
      Self::ArmV6 => "armv6",
      Self::LoongArch64 => "loongarch64",
      Self::Ppc64Le => "ppc64le",
      Self::RiscV64 => "riscv64",
      Self::S390X => "s390x",
      Self::X64 => "x64",
      Self::X86 => "x86",
      Self::Wasm => "wasm",
    }
  }

  /// Returns the architecture of the running `dv` process when representable.
  pub fn current() -> Option<Self> {
    match env::consts::ARCH {
      "arm" => Some(Self::Arm),
      "aarch64" => Some(Self::Arm64),
      "loongarch64" => Some(Self::LoongArch64),
      "powerpc64" => Some(Self::Ppc64Le),
      "riscv64" => Some(Self::RiscV64),
      "s390x" => Some(Self::S390X),
      "x86_64" => Some(Self::X64),
      "x86" => Some(Self::X86),
      "wasm32" | "wasm64" => Some(Self::Wasm),
      _ => None,
    }
  }
}

/// A parsed .NET SDK version with its original display text.
#[derive(Clone, Debug)]
pub struct SdkVersion {
  text: Box<str>,
  major: u32,
  minor: u32,
  patch: u32,
  prerelease_start: u16,
  prerelease_end: u16,
}

impl SdkVersion {
  /// Parses a full three-part .NET SDK version.
  pub fn parse(value: &str) -> Result<Self, SdkError> {
    Self::parse_boxed(value.into())
  }

  fn parse_owned(value: String) -> Result<Self, SdkError> {
    Self::parse_boxed(value.into_boxed_str())
  }

  fn parse_boxed(text: Box<str>) -> Result<Self, SdkError> {
    let value = text.as_ref();
    if value.is_empty() || value.len() >= usize::from(NO_PRERELEASE) {
      return Err(SdkError::new(SdkErrorKind::InvalidVersion, format!("invalid .NET SDK version {value:?}")));
    }

    let precedence_end = value.find('+').unwrap_or(value.len());
    let precedence = &value[..precedence_end];
    let (numbers, prerelease) = match precedence.split_once('-') {
      Some((numbers, prerelease)) if !prerelease.is_empty() => (numbers, Some(prerelease)),
      Some(_) => return Err(SdkError::new(SdkErrorKind::InvalidVersion, format!("invalid .NET SDK version {value:?}"))),
      None => (precedence, None),
    };
    validate_identifiers(prerelease, "prerelease", value)?;
    validate_identifiers(value.get(precedence_end + 1..), "build metadata", value)?;

    let mut parts = numbers.split('.');
    let major = parse_numeric_part(parts.next(), value)?;
    let minor = parse_numeric_part(parts.next(), value)?;
    let patch = parse_numeric_part(parts.next(), value)?;
    if parts.next().is_some() {
      return Err(SdkError::new(SdkErrorKind::InvalidVersion, format!("invalid .NET SDK version {value:?}")));
    }

    let (prerelease_start, prerelease_end) = prerelease.map_or((NO_PRERELEASE, NO_PRERELEASE), |prerelease| {
      let start = prerelease.as_ptr() as usize - value.as_ptr() as usize;
      (start as u16, (start + prerelease.len()) as u16)
    });

    Ok(Self {
      text,
      major,
      minor,
      patch,
      prerelease_start,
      prerelease_end,
    })
  }

  /// Returns the original installed version text.
  pub fn as_str(&self) -> &str {
    &self.text
  }

  /// Returns the major version.
  pub fn major(&self) -> u32 {
    self.major
  }

  /// Returns the minor version.
  pub fn minor(&self) -> u32 {
    self.minor
  }

  /// Returns the SDK feature band.
  pub fn feature_band(&self) -> u32 {
    self.patch / 100
  }

  /// Returns the patch level within the feature band.
  pub fn patch_level(&self) -> u32 {
    self.patch % 100
  }

  /// Returns whether this is a prerelease SDK.
  pub fn is_prerelease(&self) -> bool {
    self.prerelease_start != NO_PRERELEASE
  }

  fn prerelease(&self) -> Option<&str> {
    self
      .is_prerelease()
      .then(|| &self.text[usize::from(self.prerelease_start)..usize::from(self.prerelease_end)])
  }
}

impl fmt::Display for SdkVersion {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.text.fmt(formatter)
  }
}

impl PartialEq for SdkVersion {
  fn eq(&self, other: &Self) -> bool {
    self.cmp(other) == Ordering::Equal
  }
}

impl Eq for SdkVersion {}

impl PartialOrd for SdkVersion {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    Some(self.cmp(other))
  }
}

impl Ord for SdkVersion {
  fn cmp(&self, other: &Self) -> Ordering {
    self
      .major
      .cmp(&other.major)
      .then_with(|| self.minor.cmp(&other.minor))
      .then_with(|| self.patch.cmp(&other.patch))
      .then_with(|| compare_prerelease(self.prerelease(), other.prerelease()))
  }
}

/// One SDK directory, referencing its root by compact inventory index.
#[derive(Clone, Debug)]
pub struct SdkInstallation {
  /// Parsed installed version.
  pub version: SdkVersion,
  /// Index into `SdkInventory::roots`.
  pub root_index: u16,
}

/// A deterministic batch of discovered SDKs and its selected record.
#[derive(Debug)]
pub struct SdkInventory {
  /// SDK roots in resolver search order.
  pub roots: Vec<PathBuf>,
  /// Installations sorted by root order and ascending version.
  pub installations: Vec<SdkInstallation>,
  /// Index of the selected installation.
  pub selected_index: usize,
  /// Nearest `global.json` that influenced selection.
  pub global_json: Option<PathBuf>,
}

/// A deterministic batch of installed SDKs without selection policy.
#[derive(Debug)]
pub struct InstalledSdkInventory {
  /// Host roots in muxer search order.
  pub roots: Vec<PathBuf>,
  /// Complete installations sorted by root order and ascending version.
  pub installations: Vec<SdkInstallation>,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<InstalledSdkInventory>() == 48);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<InstalledSdkInventory>() == align_of::<usize>());

impl InstalledSdkInventory {
  /// Returns the root containing an installation.
  pub fn root(&self, installation: &SdkInstallation) -> &Path {
    &self.roots[usize::from(installation.root_index)]
  }

  /// Constructs the full SDK directory for an installation.
  pub fn installation_path(&self, installation: &SdkInstallation) -> PathBuf {
    self.root(installation).join("sdk").join(installation.version.as_str())
  }
}

/// One shared-framework row backed by the inventory's contiguous text arena.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct RuntimeInstallation {
  family_start: u32,
  version_start: u32,
  family_len: u16,
  version_len: u16,
  root_index: u16,
}

const _: () = assert!(size_of::<RuntimeInstallation>() == 16);
const _: () = assert!(align_of::<RuntimeInstallation>() == 4);

/// Installed shared frameworks in deterministic family/version order.
#[derive(Debug)]
pub struct RuntimeInventory {
  roots: Vec<PathBuf>,
  text: String,
  installations: Vec<RuntimeInstallation>,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<RuntimeInventory>() == 72);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<RuntimeInventory>() == align_of::<usize>());

impl RuntimeInventory {
  /// Returns the compact installation batch.
  pub fn installations(&self) -> &[RuntimeInstallation] {
    &self.installations
  }

  /// Returns an installation's framework family.
  pub fn family(&self, installation: RuntimeInstallation) -> &str {
    text_range(&self.text, installation.family_start, installation.family_len)
  }

  /// Returns an installation's runtime version.
  pub fn version(&self, installation: RuntimeInstallation) -> &str {
    text_range(&self.text, installation.version_start, installation.version_len)
  }

  /// Returns the host root containing an installation.
  pub fn root(&self, installation: RuntimeInstallation) -> &Path {
    &self.roots[usize::from(installation.root_index)]
  }

  /// Constructs the full shared-framework directory.
  pub fn installation_path(&self, installation: RuntimeInstallation) -> PathBuf {
    self
      .root(installation)
      .join("shared")
      .join(self.family(installation))
      .join(self.version(installation))
  }
}

impl SdkInventory {
  /// Returns the selected SDK record.
  pub fn selected(&self) -> &SdkInstallation {
    &self.installations[self.selected_index]
  }

  /// Returns the root containing an installation.
  pub fn root(&self, installation: &SdkInstallation) -> &Path {
    &self.roots[usize::from(installation.root_index)]
  }

  /// Constructs the full SDK directory for an installation.
  pub fn installation_path(&self, installation: &SdkInstallation) -> PathBuf {
    self.root(installation).join("sdk").join(installation.version.as_str())
  }
}

/// Stable SDK discovery failure categories for CLI diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SdkErrorKind {
  /// No usable .NET installation root was found.
  RootNotFound,
  /// A filesystem operation failed.
  Io,
  /// `global.json` was malformed or unsupported.
  GlobalJson,
  /// An SDK version was malformed.
  InvalidVersion,
  /// No installed SDK satisfies the selection policy.
  NoCompatibleSdk,
}

/// An SDK discovery or selection failure.
#[derive(Debug)]
pub struct SdkError {
  kind: SdkErrorKind,
  message: String,
}

impl SdkError {
  fn new(kind: SdkErrorKind, message: String) -> Self {
    Self { kind, message }
  }

  /// Returns the stable error category.
  pub fn kind(&self) -> SdkErrorKind {
    self.kind
  }
}

impl fmt::Display for SdkError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl Error for SdkError {}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum RollForward {
  Patch,
  Feature,
  Minor,
  Major,
  LatestPatch,
  LatestFeature,
  LatestMinor,
  LatestMajor,
  Disable,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct GlobalJson {
  sdk: GlobalSdk,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GlobalSdk {
  version: Option<String>,
  roll_forward: Option<RollForward>,
  allow_prerelease: Option<bool>,
  paths: Option<Vec<String>>,
  error_message: Option<String>,
}

/// Discovers SDK roots from the current environment and selects an SDK.
pub fn discover_sdks(start_directory: &Path) -> Result<SdkInventory, SdkError> {
  let roots = discover_host_roots();
  if roots.is_empty() {
    return Err(SdkError::new(
      SdkErrorKind::RootNotFound,
      "no .NET installation root was found in PATH, DOTNET_ROOT, or platform defaults".into(),
    ));
  }
  discover_sdks_in_roots(start_directory, &roots)
}

/// Lists complete SDK installations without applying `global.json` selection.
pub fn discover_installed_sdks() -> Result<InstalledSdkInventory, SdkError> {
  let roots = discover_host_roots();
  if roots.is_empty() {
    return Err(SdkError::new(
      SdkErrorKind::RootNotFound,
      "no .NET installation root was found in PATH, DOTNET_ROOT, or platform defaults".into(),
    ));
  }
  let installations = discover_installations(&roots)?;
  Ok(InstalledSdkInventory { roots, installations })
}

/// Lists SDK installations for a requested .NET host architecture.
pub fn discover_installed_sdks_for_architecture(architecture: DotnetArchitecture) -> Result<InstalledSdkInventory, SdkError> {
  if DotnetArchitecture::current() == Some(architecture) {
    return discover_installed_sdks();
  }
  let roots = discover_architecture_roots(architecture);
  let installations = discover_installations(&roots)?;
  Ok(InstalledSdkInventory { roots, installations })
}

/// Lists installed shared frameworks from the active host-root batch.
pub fn discover_runtimes() -> Result<RuntimeInventory, SdkError> {
  let roots = discover_host_roots();
  if roots.is_empty() {
    return Err(SdkError::new(
      SdkErrorKind::RootNotFound,
      "no .NET installation root was found in PATH, DOTNET_ROOT, or platform defaults".into(),
    ));
  }
  discover_runtimes_in_owned_roots(roots)
}

/// Lists shared frameworks for a requested .NET host architecture.
pub fn discover_runtimes_for_architecture(architecture: DotnetArchitecture) -> Result<RuntimeInventory, SdkError> {
  if DotnetArchitecture::current() == Some(architecture) {
    return discover_runtimes();
  }
  discover_runtimes_in_owned_roots(discover_architecture_roots(architecture))
}

/// Lists installed shared frameworks from explicit host roots.
pub fn discover_runtimes_in_roots(roots: &[PathBuf]) -> Result<RuntimeInventory, SdkError> {
  discover_runtimes_in_owned_roots(roots.to_vec())
}

fn discover_runtimes_in_owned_roots(roots: Vec<PathBuf>) -> Result<RuntimeInventory, SdkError> {
  if roots.len() > usize::from(u16::MAX) {
    return Err(SdkError::new(SdkErrorKind::GlobalJson, "too many runtime search roots".into()));
  }

  struct WorkFamily {
    family: String,
    versions: Vec<SdkVersion>,
    root_index: u16,
  }

  let mut work = Vec::with_capacity(4);
  let mut discovered_installations = 0usize;
  for (root_index, root) in roots.iter().enumerate() {
    let shared_directory = root.join("shared");
    let families = match fs::read_dir(&shared_directory) {
      Ok(entries) => entries,
      Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
      Err(error) => return Err(io_error("enumerate", &shared_directory, error)),
    };
    for family_entry in families {
      let family_entry = family_entry.map_err(|error| io_error("enumerate", &shared_directory, error))?;
      if !family_entry
        .file_type()
        .map_err(|error| io_error("inspect", &family_entry.path(), error))?
        .is_dir()
      {
        continue;
      }
      let Ok(family) = family_entry.file_name().into_string() else {
        continue;
      };
      let family_directory = family_entry.path();
      let versions = fs::read_dir(&family_directory).map_err(|error| io_error("enumerate", &family_directory, error))?;
      let mut family_versions = Vec::with_capacity(8);
      for version_entry in versions {
        let version_entry = version_entry.map_err(|error| io_error("enumerate", &family_directory, error))?;
        if !version_entry
          .file_type()
          .map_err(|error| io_error("inspect", &version_entry.path(), error))?
          .is_dir()
        {
          continue;
        }
        let Ok(version_text) = version_entry.file_name().into_string() else {
          continue;
        };
        let Ok(version) = SdkVersion::parse_owned(version_text) else {
          continue;
        };
        if discovered_installations == MAX_RUNTIME_INSTALLATIONS {
          return Err(SdkError::new(
            SdkErrorKind::Io,
            format!("runtime inventory exceeds {MAX_RUNTIME_INSTALLATIONS} installations"),
          ));
        }
        discovered_installations += 1;
        family_versions.push(version);
      }
      if !family_versions.is_empty() {
        family_versions.sort_unstable();
        work.push(WorkFamily {
          family,
          versions: family_versions,
          root_index: root_index as u16,
        });
      }
    }
  }
  work.sort_unstable_by(|left, right| left.root_index.cmp(&right.root_index).then_with(|| left.family.cmp(&right.family)));

  let installation_count = work
    .iter()
    .try_fold(0usize, |count, family| count.checked_add(family.versions.len()))
    .ok_or_else(|| SdkError::new(SdkErrorKind::Io, "runtime inventory is too large".into()))?;
  debug_assert_eq!(installation_count, discovered_installations);
  let text_capacity = work
    .iter()
    .map(|family| {
      family
        .versions
        .iter()
        .fold(family.family.len(), |bytes, version| bytes.saturating_add(version.as_str().len()))
    })
    .try_fold(0usize, usize::checked_add)
    .ok_or_else(|| SdkError::new(SdkErrorKind::Io, "runtime inventory text is too large".into()))?;
  if text_capacity > u32::MAX as usize {
    return Err(SdkError::new(SdkErrorKind::Io, "runtime inventory text is too large".into()));
  }
  let mut text = String::with_capacity(text_capacity);
  let mut installations = Vec::with_capacity(installation_count);
  for family in work {
    let family_len = u16::try_from(family.family.len()).map_err(|_| SdkError::new(SdkErrorKind::Io, "runtime family name is too long".into()))?;
    let family_start = text.len() as u32;
    text.push_str(&family.family);
    for version in family.versions {
      let version_len = u16::try_from(version.as_str().len()).map_err(|_| SdkError::new(SdkErrorKind::Io, "runtime version is too long".into()))?;
      let version_start = text.len() as u32;
      text.push_str(version.as_str());
      installations.push(RuntimeInstallation {
        family_start,
        version_start,
        family_len,
        version_len,
        root_index: family.root_index,
      });
    }
  }

  Ok(RuntimeInventory { roots, text, installations })
}

fn text_range(text: &str, start: u32, len: u16) -> &str {
  let start = start as usize;
  &text[start..start + usize::from(len)]
}

/// Discovers and selects SDKs using an explicit host-root batch.
pub fn discover_sdks_in_roots(start_directory: &Path, host_roots: &[PathBuf]) -> Result<SdkInventory, SdkError> {
  let global_json = discover_ancestor_inputs(start_directory, AncestorInputRequest::GLOBAL_JSON)
    .map_err(|error| {
      let kind = match error.kind() {
        AncestorInputErrorKind::UnsupportedFileType | AncestorInputErrorKind::LimitExceeded => SdkErrorKind::GlobalJson,
        AncestorInputErrorKind::NotFound | AncestorInputErrorKind::Io => SdkErrorKind::Io,
      };
      SdkError::new(kind, error.to_string())
    })?
    .into_nearest_path(AncestorInputKind::GlobalJson);
  let config = global_json.as_deref().map(read_global_json).transpose()?.unwrap_or_default();
  let roots = resolve_search_roots(global_json.as_deref(), &config.sdk, host_roots)?;
  let installations = discover_installations(&roots)?;
  let requested = config.sdk.version.as_deref().map(SdkVersion::parse).transpose()?;
  let policy = match (config.sdk.roll_forward, requested.as_ref()) {
    (Some(policy), _) => policy,
    (None, Some(_)) => RollForward::Patch,
    (None, None) => RollForward::LatestMajor,
  };
  if requested.is_none() && !matches!(policy, RollForward::LatestMajor) {
    return Err(SdkError::new(
      SdkErrorKind::GlobalJson,
      "global.json sdk.version is required unless rollForward is latestMajor".into(),
    ));
  }

  let allow_prerelease = config.sdk.allow_prerelease.unwrap_or(true);
  let selected_index = roots
    .iter()
    .enumerate()
    .find_map(|(root_index, _)| select_from_root(&installations, root_index as u16, requested.as_ref(), policy, allow_prerelease));

  let selected_index = selected_index.ok_or_else(|| {
    SdkError::new(
      SdkErrorKind::NoCompatibleSdk,
      config.sdk.error_message.unwrap_or_else(|| {
        let requested = requested.as_ref().map_or("<latest>", SdkVersion::as_str);
        format!("no installed .NET SDK satisfies version {requested:?} with rollForward {policy:?}")
      }),
    )
  })?;

  Ok(SdkInventory {
    roots,
    installations,
    selected_index,
    global_json,
  })
}

fn discover_host_roots() -> Vec<PathBuf> {
  let mut roots = Vec::with_capacity(4);
  let executable_name = format!("dotnet{}", env::consts::EXE_SUFFIX);

  if let Some(path) = env::var_os("PATH") {
    for directory in env::split_paths(&path) {
      let executable = directory.join(&executable_name);
      if executable.is_file() {
        let root = if cfg!(windows) {
          directory
        } else {
          fs::canonicalize(&executable)
            .ok()
            .and_then(|path| path.parent().map(Path::to_owned))
            .unwrap_or(directory)
        };
        push_unique_path(&mut roots, root);
        return roots;
      }
    }
  }

  if let Some(architecture) = dotnet_architecture()
    && let Some(root) = env::var_os(format!("DOTNET_ROOT_{architecture}"))
  {
    push_existing_root(&mut roots, PathBuf::from(root));
    if !roots.is_empty() {
      return roots;
    }
  }
  if let Some(root) = env::var_os("DOTNET_ROOT") {
    push_existing_root(&mut roots, PathBuf::from(root));
    if !roots.is_empty() {
      return roots;
    }
  }

  if cfg!(windows) {
    if let Some(program_files) = env::var_os("ProgramFiles") {
      push_existing_root(&mut roots, PathBuf::from(program_files).join("dotnet"));
    }
  } else {
    for root in ["/usr/share/dotnet", "/usr/lib/dotnet", "/usr/local/share/dotnet"] {
      push_existing_root(&mut roots, PathBuf::from(root));
      if !roots.is_empty() {
        break;
      }
    }
  }

  roots
}

fn discover_architecture_roots(architecture: DotnetArchitecture) -> Vec<PathBuf> {
  let mut roots = Vec::with_capacity(1);
  if let Some(root) = registered_architecture_root(architecture) {
    roots.push(root);
    return roots;
  }
  if let Some(root) = default_architecture_root(architecture)
    && root.is_dir()
  {
    roots.push(root);
  }
  roots
}

#[cfg(windows)]
fn registered_architecture_root(architecture: DotnetArchitecture) -> Option<PathBuf> {
  let key = RegKey::predef(HKEY_LOCAL_MACHINE)
    .open_subkey_with_flags(
      format!(r"SOFTWARE\dotnet\Setup\InstalledVersions\{}", architecture.as_str()),
      KEY_READ | KEY_WOW64_32KEY,
    )
    .ok()?;
  let location: String = key.get_value("InstallLocation").ok()?;
  (!location.is_empty()).then(|| PathBuf::from(location))
}

#[cfg(not(windows))]
fn registered_architecture_root(architecture: DotnetArchitecture) -> Option<PathBuf> {
  let path = PathBuf::from(format!("/etc/dotnet/install_location_{}", architecture.as_str()));
  let file = fs::File::open(path).ok()?;
  let mut line = String::with_capacity(128);
  io::BufReader::new(file).read_line(&mut line).ok()?;
  if line.ends_with('\n') {
    line.pop();
  }
  (!line.is_empty()).then(|| PathBuf::from(line))
}

#[cfg(windows)]
fn default_architecture_root(architecture: DotnetArchitecture) -> Option<PathBuf> {
  let current = DotnetArchitecture::current()?;
  let program_files = match (current, architecture) {
    (DotnetArchitecture::X64, DotnetArchitecture::X86) => env::var_os("ProgramFiles(x86)")?,
    (DotnetArchitecture::Arm64, DotnetArchitecture::X64) => env::var_os("ProgramFiles")?,
    (DotnetArchitecture::X86, DotnetArchitecture::X64) => env::var_os("ProgramW6432")?,
    (DotnetArchitecture::X64, DotnetArchitecture::Arm64) if windows_native_architecture_is_arm64() => env::var_os("ProgramFiles")?,
    _ => return None,
  };
  let mut root = PathBuf::from(program_files);
  root.push("dotnet");
  if current == DotnetArchitecture::Arm64 && architecture == DotnetArchitecture::X64 {
    root.push("x64");
  }
  Some(root)
}

#[cfg(windows)]
fn windows_native_architecture_is_arm64() -> bool {
  ["PROCESSOR_ARCHITEW6432", "PROCESSOR_ARCHITECTURE"]
    .into_iter()
    .filter_map(env::var_os)
    .any(|value| value.eq_ignore_ascii_case("ARM64"))
}

#[cfg(all(not(windows), target_os = "macos"))]
fn default_architecture_root(architecture: DotnetArchitecture) -> Option<PathBuf> {
  (DotnetArchitecture::current() == Some(DotnetArchitecture::Arm64) && architecture == DotnetArchitecture::X64)
    .then(|| PathBuf::from("/usr/local/share/dotnet/x64"))
}

#[cfg(all(not(windows), not(target_os = "macos")))]
const fn default_architecture_root(_architecture: DotnetArchitecture) -> Option<PathBuf> {
  None
}

fn dotnet_architecture() -> Option<&'static str> {
  match env::consts::ARCH {
    "x86_64" => Some("X64"),
    "x86" => Some("X86"),
    "aarch64" => Some("ARM64"),
    "arm" => Some("ARM"),
    _ => None,
  }
}

fn push_existing_root(roots: &mut Vec<PathBuf>, root: PathBuf) {
  let host = root.join(format!("dotnet{}", env::consts::EXE_SUFFIX));
  if host.is_file() || root.join("sdk").is_dir() || root.join("shared").is_dir() {
    push_unique_path(roots, root);
  }
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
  if !paths.iter().any(|existing| paths_equal(existing, &path)) {
    paths.push(path);
  }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
  if cfg!(windows) {
    left
      .as_os_str()
      .to_string_lossy()
      .trim_end_matches(['\\', '/'])
      .eq_ignore_ascii_case(right.as_os_str().to_string_lossy().trim_end_matches(['\\', '/']))
  } else {
    left == right
  }
}

fn read_global_json(path: &Path) -> Result<GlobalJson, SdkError> {
  let bytes = fs::read(path).map_err(|error| io_error("read", path, error))?;
  let text = std::str::from_utf8(&bytes).map_err(|error| SdkError::new(SdkErrorKind::GlobalJson, format!("{} is not UTF-8: {error}", path.display())))?;
  let json = strip_json_comments(text.trim_start_matches('\u{feff}'))
    .map_err(|message| SdkError::new(SdkErrorKind::GlobalJson, format!("invalid {}: {message}", path.display())))?;
  serde_json::from_str(&json).map_err(|error| SdkError::new(SdkErrorKind::GlobalJson, format!("invalid {}: {error}", path.display())))
}

fn resolve_search_roots(global_json: Option<&Path>, sdk: &GlobalSdk, host_roots: &[PathBuf]) -> Result<Vec<PathBuf>, SdkError> {
  let Some(paths) = &sdk.paths else {
    return Ok(host_roots.to_vec());
  };
  let global_directory = global_json
    .and_then(Path::parent)
    .ok_or_else(|| SdkError::new(SdkErrorKind::GlobalJson, "global.json sdk.paths requires a global.json location".into()))?;
  let mut roots = Vec::with_capacity(paths.len());
  for path in paths {
    if path == "$host$" {
      let host = host_roots
        .first()
        .ok_or_else(|| SdkError::new(SdkErrorKind::RootNotFound, "global.json requested $host$ but no host root was found".into()))?;
      push_unique_path(&mut roots, host.clone());
    } else {
      let path = PathBuf::from(path);
      let resolved = if path.is_absolute() { path } else { global_directory.join(path) };
      push_unique_path(&mut roots, resolved);
    }
  }
  Ok(roots)
}

fn discover_installations(roots: &[PathBuf]) -> Result<Vec<SdkInstallation>, SdkError> {
  if roots.len() > usize::from(u16::MAX) {
    return Err(SdkError::new(SdkErrorKind::GlobalJson, "too many SDK search roots".into()));
  }

  let mut installations = Vec::with_capacity(8);
  for (root_index, root) in roots.iter().enumerate() {
    let sdk_directory = root.join("sdk");
    let entries = match fs::read_dir(&sdk_directory) {
      Ok(entries) => entries,
      Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
      Err(error) => return Err(io_error("enumerate", &sdk_directory, error)),
    };
    for entry in entries {
      let entry = entry.map_err(|error| io_error("enumerate", &sdk_directory, error))?;
      if !entry.file_type().map_err(|error| io_error("inspect", &entry.path(), error))?.is_dir() {
        continue;
      }
      let Some(version_text) = entry.file_name().to_str().map(str::to_owned) else {
        continue;
      };
      let Ok(version) = SdkVersion::parse(&version_text) else {
        continue;
      };
      if !entry.path().join("dotnet.dll").is_file() {
        continue;
      }
      if installations.len() == MAX_SDK_INSTALLATIONS {
        return Err(SdkError::new(
          SdkErrorKind::Io,
          format!("SDK inventory exceeds {MAX_SDK_INSTALLATIONS} installations"),
        ));
      }
      installations.push(SdkInstallation {
        version,
        root_index: root_index as u16,
      });
    }
  }
  installations.sort_unstable_by(|left, right| left.root_index.cmp(&right.root_index).then_with(|| left.version.cmp(&right.version)));
  Ok(installations)
}

fn select_from_root(
  installations: &[SdkInstallation],
  root_index: u16,
  requested: Option<&SdkVersion>,
  policy: RollForward,
  allow_prerelease: bool,
) -> Option<usize> {
  let candidates = installations
    .iter()
    .enumerate()
    .filter(|(_, installation)| installation.root_index == root_index)
    .filter(|(_, installation)| allow_prerelease || !installation.version.is_prerelease());

  let Some(requested) = requested else {
    return candidates
      .max_by(|(_, left), (_, right)| left.version.cmp(&right.version))
      .map(|(index, _)| index);
  };

  match policy {
    RollForward::Disable => candidates
      .filter(|(_, installation)| installation.version == *requested)
      .map(|(index, _)| index)
      .next(),
    RollForward::Patch => {
      let exact = candidates
        .clone()
        .find(|(_, installation)| installation.version == *requested)
        .map(|(index, _)| index);
      exact.or_else(|| {
        candidates
          .filter(|(_, installation)| same_feature_band(&installation.version, requested) && installation.version >= *requested)
          .max_by(|(_, left), (_, right)| left.version.cmp(&right.version))
          .map(|(index, _)| index)
      })
    },
    RollForward::LatestPatch => candidates
      .filter(|(_, installation)| same_feature_band(&installation.version, requested) && installation.version >= *requested)
      .max_by(|(_, left), (_, right)| left.version.cmp(&right.version))
      .map(|(index, _)| index),
    RollForward::LatestFeature => candidates
      .filter(|(_, installation)| {
        installation.version.major == requested.major && installation.version.minor == requested.minor && installation.version >= *requested
      })
      .max_by(|(_, left), (_, right)| left.version.cmp(&right.version))
      .map(|(index, _)| index),
    RollForward::LatestMinor => candidates
      .filter(|(_, installation)| installation.version.major == requested.major && installation.version >= *requested)
      .max_by(|(_, left), (_, right)| left.version.cmp(&right.version))
      .map(|(index, _)| index),
    RollForward::LatestMajor => candidates
      .filter(|(_, installation)| installation.version >= *requested)
      .max_by(|(_, left), (_, right)| left.version.cmp(&right.version))
      .map(|(index, _)| index),
    RollForward::Feature => select_nearest_group(
      candidates.filter(|(_, installation)| {
        installation.version.major == requested.major && installation.version.minor == requested.minor && installation.version >= *requested
      }),
      |version| (version.feature_band(), 0, 0),
    ),
    RollForward::Minor => select_nearest_group(
      candidates.filter(|(_, installation)| installation.version.major == requested.major && installation.version >= *requested),
      |version| (version.minor, version.feature_band(), 0),
    ),
    RollForward::Major => select_nearest_group(candidates.filter(|(_, installation)| installation.version >= *requested), |version| {
      (version.major, version.minor, version.feature_band())
    }),
  }
}

fn select_nearest_group<'a>(candidates: impl Iterator<Item = (usize, &'a SdkInstallation)>, group: impl Fn(&SdkVersion) -> (u32, u32, u32)) -> Option<usize> {
  candidates
    .min_by(|(_, left), (_, right)| group(&left.version).cmp(&group(&right.version)).then_with(|| right.version.cmp(&left.version)))
    .map(|(index, _)| index)
}

fn same_feature_band(left: &SdkVersion, right: &SdkVersion) -> bool {
  left.major == right.major && left.minor == right.minor && left.feature_band() == right.feature_band()
}

fn parse_numeric_part(part: Option<&str>, full_version: &str) -> Result<u32, SdkError> {
  let part = part.ok_or_else(|| SdkError::new(SdkErrorKind::InvalidVersion, format!("invalid .NET SDK version {full_version:?}")))?;
  if part.is_empty() || (part.len() > 1 && part.starts_with('0')) || !part.bytes().all(|byte| byte.is_ascii_digit()) {
    return Err(SdkError::new(
      SdkErrorKind::InvalidVersion,
      format!("invalid .NET SDK version {full_version:?}"),
    ));
  }
  part
    .parse()
    .map_err(|_| SdkError::new(SdkErrorKind::InvalidVersion, format!("invalid .NET SDK version {full_version:?}")))
}

fn validate_identifiers(identifiers: Option<&str>, kind: &str, full_version: &str) -> Result<(), SdkError> {
  let Some(identifiers) = identifiers else {
    return Ok(());
  };
  if identifiers.is_empty()
    || identifiers
      .split('.')
      .any(|identifier| identifier.is_empty() || !identifier.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
  {
    return Err(SdkError::new(
      SdkErrorKind::InvalidVersion,
      format!("invalid {kind} in .NET SDK version {full_version:?}"),
    ));
  }
  Ok(())
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

fn strip_json_comments(input: &str) -> Result<String, &'static str> {
  let bytes = input.as_bytes();
  let mut output = Vec::with_capacity(bytes.len());
  let mut index = 0;
  let mut in_string = false;
  let mut escaped = false;

  while index < bytes.len() {
    let byte = bytes[index];
    if in_string {
      output.push(byte);
      if escaped {
        escaped = false;
      } else if byte == b'\\' {
        escaped = true;
      } else if byte == b'"' {
        in_string = false;
      }
      index += 1;
      continue;
    }

    if byte == b'"' {
      in_string = true;
      output.push(byte);
      index += 1;
    } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
      output.extend_from_slice(b"  ");
      index += 2;
      while index < bytes.len() && bytes[index] != b'\n' {
        output.push(b' ');
        index += 1;
      }
    } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
      output.extend_from_slice(b"  ");
      index += 2;
      let mut closed = false;
      while index < bytes.len() {
        if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
          output.extend_from_slice(b"  ");
          index += 2;
          closed = true;
          break;
        }
        output.push(if bytes[index] == b'\n' { b'\n' } else { b' ' });
        index += 1;
      }
      if !closed {
        return Err("unterminated block comment");
      }
    } else {
      output.push(byte);
      index += 1;
    }
  }

  String::from_utf8(output).map_err(|_| "comment stripping produced invalid UTF-8")
}

fn io_error(operation: &str, path: &Path, error: io::Error) -> SdkError {
  SdkError::new(SdkErrorKind::Io, format!("failed to {operation} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
  use std::{
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
    time::{SystemTime, UNIX_EPOCH},
  };

  use super::*;

  static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

  #[test]
  fn dotnet_architecture_names_match_the_host_contract_without_allocation() {
    for (name, architecture) in [
      ("arm", DotnetArchitecture::Arm),
      ("arm64", DotnetArchitecture::Arm64),
      ("armv6", DotnetArchitecture::ArmV6),
      ("loongarch64", DotnetArchitecture::LoongArch64),
      ("ppc64le", DotnetArchitecture::Ppc64Le),
      ("riscv64", DotnetArchitecture::RiscV64),
      ("s390x", DotnetArchitecture::S390X),
      ("x64", DotnetArchitecture::X64),
      ("x86", DotnetArchitecture::X86),
      ("wasm", DotnetArchitecture::Wasm),
    ] {
      assert_eq!(DotnetArchitecture::parse(name), Some(architecture));
      assert_eq!(DotnetArchitecture::parse(&name.to_ascii_uppercase()), Some(architecture));
      assert_eq!(architecture.as_str(), name);
    }
    assert_eq!(DotnetArchitecture::parse("amd64"), None);
    assert_eq!(DotnetArchitecture::parse(""), None);
  }

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let nonce = NEXT_TEMP.fetch_add(1, AtomicOrdering::Relaxed);
      let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let path = env::temp_dir().join(format!("dv-sdk-test-{}-{time}-{nonce}", std::process::id()));
      fs::create_dir_all(&path).unwrap();
      Self(path)
    }

    fn sdk(&self, root: &str, version: &str) -> PathBuf {
      let root = self.0.join(root);
      let installation = root.join("sdk").join(version);
      fs::create_dir_all(&installation).unwrap();
      fs::write(installation.join("dotnet.dll"), []).unwrap();
      root
    }
  }

  impl Drop for TempDirectory {
    fn drop(&mut self) {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }

  #[test]
  fn versions_follow_semver_prerelease_order() {
    let preview_2 = SdkVersion::parse("10.0.100-preview.2").unwrap();
    let preview_10 = SdkVersion::parse("10.0.100-preview.10").unwrap();
    let stable = SdkVersion::parse("10.0.100").unwrap();

    assert!(preview_2 < preview_10);
    assert!(preview_10 < stable);
    assert_eq!(stable.feature_band(), 1);
    assert_eq!(stable.patch_level(), 0);
  }

  #[test]
  fn default_selection_uses_highest_installed_sdk() {
    let temp = TempDirectory::new();
    let root = temp.sdk("dotnet", "9.0.308");
    temp.sdk("dotnet", "10.0.100");

    let inventory = discover_sdks_in_roots(&temp.0, &[root]).unwrap();

    assert_eq!(inventory.selected().version.as_str(), "10.0.100");
  }

  #[test]
  fn incomplete_sdk_directories_are_not_installations() {
    let temp = TempDirectory::new();
    let root = temp.sdk("dotnet", "10.0.100");
    fs::create_dir_all(root.join("sdk/11.0.100-preview.1")).unwrap();

    let inventory = discover_sdks_in_roots(&temp.0, &[root]).unwrap();

    assert_eq!(inventory.installations.len(), 1);
    assert_eq!(inventory.selected().version.as_str(), "10.0.100");
  }

  #[test]
  fn runtime_inventory_is_flat_and_sorted_by_root_family_and_version() {
    let temp = TempDirectory::new();
    let root = temp.0.join("dotnet");
    for path in [
      "shared/Microsoft.NETCore.App/10.0.0",
      "shared/Microsoft.NETCore.App/9.0.11",
      "shared/Microsoft.AspNetCore.App/10.0.0",
      "shared/Microsoft.NETCore.App/not-a-version",
    ] {
      fs::create_dir_all(root.join(path)).unwrap();
    }

    let inventory = discover_runtimes_in_roots(std::slice::from_ref(&root)).unwrap();
    let actual: Vec<_> = inventory
      .installations()
      .iter()
      .map(|installation| {
        (
          inventory.family(*installation),
          inventory.version(*installation),
          inventory.installation_path(*installation),
        )
      })
      .collect();

    assert_eq!(
      actual,
      [
        ("Microsoft.AspNetCore.App", "10.0.0", root.join("shared/Microsoft.AspNetCore.App/10.0.0")),
        ("Microsoft.NETCore.App", "9.0.11", root.join("shared/Microsoft.NETCore.App/9.0.11")),
        ("Microsoft.NETCore.App", "10.0.0", root.join("shared/Microsoft.NETCore.App/10.0.0")),
      ]
    );
  }

  #[test]
  fn global_json_latest_patch_selects_highest_compatible_patch() {
    let temp = TempDirectory::new();
    let root = temp.sdk("dotnet", "9.0.100");
    temp.sdk("dotnet", "9.0.103");
    temp.sdk("dotnet", "9.0.200");
    fs::write(
      temp.0.join("global.json"),
      r#"{
        // The resolver supports the same comments as dotnet.
        "sdk": { "version": "9.0.100", "rollForward": "latestPatch" }
      }"#,
    )
    .unwrap();

    let inventory = discover_sdks_in_roots(&temp.0, &[root]).unwrap();

    assert_eq!(inventory.selected().version.as_str(), "9.0.103");
  }

  #[test]
  fn global_json_paths_uses_first_root_with_a_match() {
    let temp = TempDirectory::new();
    let host = temp.sdk("host", "10.0.100");
    temp.sdk("local", "9.0.101");
    fs::write(
      temp.0.join("global.json"),
      r#"{
        "sdk": {
          "version": "9.0.100",
          "rollForward": "latestPatch",
          "paths": [ "local", "$host$" ]
        }
      }"#,
    )
    .unwrap();

    let inventory = discover_sdks_in_roots(&temp.0, &[host]).unwrap();

    assert_eq!(inventory.selected().version.as_str(), "9.0.101");
    assert!(inventory.root(inventory.selected()).ends_with("local"));
  }

  #[test]
  fn roll_forward_policies_choose_the_documented_version_range() {
    let installations: Vec<_> = ["8.0.100", "8.0.103", "8.0.200", "8.1.100", "9.0.100"]
      .into_iter()
      .map(|version| SdkInstallation {
        version: SdkVersion::parse(version).unwrap(),
        root_index: 0,
      })
      .collect();
    let requested = SdkVersion::parse("8.0.100").unwrap();
    let selected = |policy| select_from_root(&installations, 0, Some(&requested), policy, true).map(|index| installations[index].version.as_str());

    assert_eq!(selected(RollForward::Disable), Some("8.0.100"));
    assert_eq!(selected(RollForward::Patch), Some("8.0.100"));
    assert_eq!(selected(RollForward::Feature), Some("8.0.103"));
    assert_eq!(selected(RollForward::Minor), Some("8.0.103"));
    assert_eq!(selected(RollForward::Major), Some("8.0.103"));
    assert_eq!(selected(RollForward::LatestPatch), Some("8.0.103"));
    assert_eq!(selected(RollForward::LatestFeature), Some("8.0.200"));
    assert_eq!(selected(RollForward::LatestMinor), Some("8.1.100"));
    assert_eq!(selected(RollForward::LatestMajor), Some("9.0.100"));
  }

  #[test]
  fn prerelease_filter_is_applied_before_selection() {
    let installations = [
      SdkInstallation {
        version: SdkVersion::parse("10.0.100").unwrap(),
        root_index: 0,
      },
      SdkInstallation {
        version: SdkVersion::parse("11.0.100-preview.1").unwrap(),
        root_index: 0,
      },
    ];

    let stable = select_from_root(&installations, 0, None, RollForward::LatestMajor, false).unwrap();
    let preview = select_from_root(&installations, 0, None, RollForward::LatestMajor, true).unwrap();

    assert_eq!(installations[stable].version.as_str(), "10.0.100");
    assert_eq!(installations[preview].version.as_str(), "11.0.100-preview.1");
  }
}
