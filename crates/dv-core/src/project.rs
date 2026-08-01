use std::{
  cmp::Ordering,
  error::Error,
  fmt, fs,
  path::{Component, Path, PathBuf},
};

fn compare_ascii_case_insensitive(left: &str, right: &str) -> Ordering {
  left
    .bytes()
    .map(|byte| byte.to_ascii_lowercase())
    .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
}

fn central_package_fingerprint(versions: &[RawCentralPackageVersion], management: bool, transitive_pinning: bool, version_override: bool) -> String {
  use std::fmt::Write as _;

  if !management {
    return String::new();
  }
  let mut hash = Sha256::new();
  hash.update(b"dv-central-packages-v1\0");
  hash.update([u8::from(transitive_pinning), u8::from(version_override)]);
  for package in versions {
    for byte in package.id.bytes() {
      hash.update([byte.to_ascii_lowercase()]);
    }
    hash.update([0]);
    hash.update(package.version.as_deref().unwrap_or_default().trim().as_bytes());
    hash.update([0]);
  }
  let digest = hash.finalize();
  let mut output = String::with_capacity(digest.len() * 2);
  for byte in digest {
    write!(output, "{byte:02x}").expect("writing a String succeeds");
  }
  output
}

use quick_xml::{
  Reader, XmlVersion,
  events::{BytesRef, BytesStart, Event},
};
use sha2::{Digest, Sha256};

use crate::{BENCHMARK_CACHE_LINE_BYTES, FrameworkFamily, TargetFramework};

const SUPPORTED_SDK: &str = "Microsoft.NET.Sdk";
const MAX_XML_DEPTH: usize = 8;
const MAX_REFERENCE_CONDITION_BYTES: usize = 1_024;
const MAX_REFERENCE_CONDITION_OPERATORS: u8 = 32;
const MAX_REFERENCE_CONDITION_DEPTH: u8 = 8;
const MAX_CENTRAL_PACKAGE_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CENTRAL_PACKAGE_ROWS: usize = 100_000;
const MAX_WORKSPACE_CANDIDATES: usize = u16::MAX as usize;
const MAX_WORKSPACE_DIAGNOSTIC_CANDIDATES: usize = 16;
const WORKSPACE_CANDIDATE_CAPACITY: usize = 8;
const WORKSPACE_PATH_CAPACITY: usize = 256;
const NO_REFERENCE_CONDITION: u32 = u32::MAX;
const NO_RUNTIME_IDENTIFIER: u32 = u32::MAX;
const NO_TEXT: TextSpan = TextSpan { start: u32::MAX, len: 0 };

/// A supported build configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectConfiguration {
  /// Developer-oriented outputs.
  Debug,
  /// Optimized outputs.
  Release,
}

impl ProjectConfiguration {
  /// Parses a configuration name using MSBuild-compatible casing.
  pub fn parse(value: &str) -> Option<Self> {
    if value.eq_ignore_ascii_case("Debug") {
      Some(Self::Debug)
    } else if value.eq_ignore_ascii_case("Release") {
      Some(Self::Release)
    } else {
      None
    }
  }

  /// Returns the canonical configuration text.
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Debug => "Debug",
      Self::Release => "Release",
    }
  }
}

/// The managed artifact produced by a project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectOutputType {
  /// An executable assembly.
  Exe,
  /// A library assembly.
  Library,
}

impl ProjectOutputType {
  /// Returns the canonical MSBuild property text.
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Exe => "Exe",
      Self::Library => "Library",
    }
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextSpan {
  start: u32,
  len: u32,
}

const _: () = assert!(size_of::<TextSpan>() == 8);
const _: () = assert!(align_of::<TextSpan>() == 4);

/// A project or solution kind recognized during workspace discovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum WorkspaceCandidateKind {
  /// An MSBuild C# project.
  CSharpProject,
  /// An MSBuild F# project.
  FSharpProject,
  /// An MSBuild Visual Basic project.
  VisualBasicProject,
  /// A text `.sln` solution.
  Solution,
  /// An XML `.slnx` solution.
  XmlSolution,
}

impl WorkspaceCandidateKind {
  fn description(self) -> &'static str {
    match self {
      Self::CSharpProject => "C# project",
      Self::FSharpProject => "F# project",
      Self::VisualBasicProject => "Visual Basic project",
      Self::Solution => "solution",
      Self::XmlSolution => "XML solution",
    }
  }
}

const _: () = assert!(size_of::<WorkspaceCandidateKind>() == 1);
const _: () = assert!(align_of::<WorkspaceCandidateKind>() == 1);

/// One immutable candidate row indexing `WorkspaceInventory` path text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct WorkspaceCandidate {
  path_start: u32,
  path_len: u16,
  kind: WorkspaceCandidateKind,
}

const _: () = assert!(size_of::<WorkspaceCandidate>() == 8);
const _: () = assert!(align_of::<WorkspaceCandidate>() == 4);
const _: () = assert!(BENCHMARK_CACHE_LINE_BYTES / size_of::<WorkspaceCandidate>() == 8);

/// A stable candidate batch for one directory.
#[derive(Debug)]
pub struct WorkspaceInventory {
  root: PathBuf,
  paths: String,
  candidates: Vec<WorkspaceCandidate>,
}

#[cfg(all(target_pointer_width = "64", windows))]
const _: () = assert!(size_of::<WorkspaceInventory>() == 80);
#[cfg(all(target_pointer_width = "64", not(windows)))]
const _: () = assert!(size_of::<WorkspaceInventory>() == 72);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<WorkspaceInventory>() == align_of::<usize>());

impl WorkspaceInventory {
  /// Returns the absolute directory that owns the candidate batch.
  pub fn root(&self) -> &Path {
    &self.root
  }

  /// Returns candidates sorted by their preserved relative path.
  pub fn candidates(&self) -> &[WorkspaceCandidate] {
    &self.candidates
  }

  /// Returns a candidate's recognized file kind.
  pub fn kind(&self, candidate: WorkspaceCandidate) -> WorkspaceCandidateKind {
    candidate.kind
  }

  /// Returns a candidate's preserved relative path.
  pub fn path(&self, candidate: WorkspaceCandidate) -> &str {
    let start = candidate.path_start as usize;
    &self.paths[start..start + usize::from(candidate.path_len)]
  }

  /// Constructs a candidate's full path only at the consumer boundary.
  pub fn full_path(&self, candidate: WorkspaceCandidate) -> PathBuf {
    self.root.join(self.path(candidate))
  }

  /// Returns bytes retained by compact candidate rows and their path arena.
  pub fn working_set_bytes(&self) -> usize {
    self.candidates.len() * size_of::<WorkspaceCandidate>() + self.paths.len()
  }
}

/// NuGet asset families selected by PackageReference metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageAssetFlags(u8);

impl PackageAssetFlags {
  pub const NONE: Self = Self(0);
  pub const RUNTIME: Self = Self(1 << 0);
  pub const COMPILE: Self = Self(1 << 1);
  pub const BUILD: Self = Self(1 << 2);
  pub const NATIVE: Self = Self(1 << 3);
  pub const CONTENT_FILES: Self = Self(1 << 4);
  pub const ANALYZERS: Self = Self(1 << 5);
  pub const BUILD_MULTI_TARGETING: Self = Self(1 << 6);
  pub const BUILD_TRANSITIVE: Self = Self(1 << 7);
  pub const ALL: Self = Self(u8::MAX);
  const DEFAULT_PRIVATE: Self = Self(Self::CONTENT_FILES.0 | Self::ANALYZERS.0 | Self::BUILD.0);
  pub(crate) const NO_CONTENT: Self = Self(Self::ALL.0 & !Self::CONTENT_FILES.0);

  pub const fn contains(self, other: Self) -> bool {
    self.0 & other.0 == other.0
  }

  pub(crate) const fn bits(self) -> u8 {
    self.0
  }

  pub(crate) const fn union(self, other: Self) -> Self {
    Self(self.0 | other.0)
  }

  pub(crate) const fn intersect(self, other: Self) -> Self {
    Self(self.0 & other.0)
  }

  pub(crate) const fn without(self, other: Self) -> Self {
    Self(self.0 & !other.0)
  }
}

/// One package dependency stored as compact text-table spans and inline policy.
///
/// Eight fields occupy 36 bytes at four-byte alignment. One full row fits in
/// an assumed 64-byte benchmark-host cache line; the checked 51-reference
/// solution retains 1,836 bytes in one contiguous allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageReference {
  id: TextSpan,
  version: TextSpan,
  no_warn: TextSpan,
  aliases: TextSpan,
  include_assets: PackageAssetFlags,
  exclude_assets: PackageAssetFlags,
  private_assets: PackageAssetFlags,
  generate_path_property: bool,
}

const _: () = assert!(size_of::<PackageReference>() == 36);
const _: () = assert!(align_of::<PackageReference>() == 4);
const _: () = assert!(BENCHMARK_CACHE_LINE_BYTES / size_of::<PackageReference>() == 1);

/// One explicit shared-framework dependency and its supported version overrides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameworkReference {
  id: TextSpan,
  runtime_version: TextSpan,
  targeting_pack_version: TextSpan,
  target_latest_runtime_patch: Option<bool>,
}

const _: () = assert!(size_of::<FrameworkReference>() == 28);
const _: () = assert!(align_of::<FrameworkReference>() == 4);

/// One selected central package-version row retained for transitive pinning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CentralPackageVersion {
  id: TextSpan,
  version: TextSpan,
}

const _: () = assert!(size_of::<CentralPackageVersion>() == 16);
const _: () = assert!(align_of::<CentralPackageVersion>() == 4);

/// Runtime host roll-forward policy written to the generated runtime config.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeRollForward {
  /// Use only the requested version.
  Disable,
  /// Use the highest patch in the requested major/minor band.
  LatestPatch,
  /// Prefer the requested minor, then the nearest higher minor in the same major.
  Minor,
  /// Prefer the requested major, then the nearest higher major.
  Major,
  /// Use the highest minor in the requested major.
  LatestMinor,
  /// Use the highest installed major, minor, and patch.
  LatestMajor,
}

/// Dependency scope included in NuGet vulnerability auditing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NugetAuditMode {
  /// Audit only direct package references.
  Direct,
  /// Audit direct and transitive package references.
  All,
}

impl NugetAuditMode {
  /// Parses the MSBuild property spelling case-insensitively.
  pub fn parse(value: &str) -> Option<Self> {
    if value.eq_ignore_ascii_case("direct") {
      Some(Self::Direct)
    } else if value.eq_ignore_ascii_case("all") {
      Some(Self::All)
    } else {
      None
    }
  }

  /// Returns the canonical NuGet property spelling.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Direct => "direct",
      Self::All => "all",
    }
  }
}

/// Minimum vulnerability severity reported by NuGet auditing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NugetAuditLevel {
  /// Include low-severity advisories and above.
  Low,
  /// Include moderate-severity advisories and above.
  Moderate,
  /// Include high-severity advisories and above.
  High,
  /// Include only critical advisories.
  Critical,
}

impl NugetAuditLevel {
  /// Parses the MSBuild property spelling case-insensitively.
  pub fn parse(value: &str) -> Option<Self> {
    if value.eq_ignore_ascii_case("low") {
      Some(Self::Low)
    } else if value.eq_ignore_ascii_case("moderate") {
      Some(Self::Moderate)
    } else if value.eq_ignore_ascii_case("high") {
      Some(Self::High)
    } else if value.eq_ignore_ascii_case("critical") {
      Some(Self::Critical)
    } else {
      None
    }
  }

  /// Returns the canonical NuGet property spelling.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Low => "low",
      Self::Moderate => "moderate",
      Self::High => "high",
      Self::Critical => "critical",
    }
  }
}

impl RuntimeRollForward {
  /// Parses the project/runtimeconfig spelling case-insensitively.
  pub fn parse(value: &str) -> Option<Self> {
    if value.eq_ignore_ascii_case("Disable") {
      Some(Self::Disable)
    } else if value.eq_ignore_ascii_case("LatestPatch") {
      Some(Self::LatestPatch)
    } else if value.eq_ignore_ascii_case("Minor") {
      Some(Self::Minor)
    } else if value.eq_ignore_ascii_case("Major") {
      Some(Self::Major)
    } else if value.eq_ignore_ascii_case("LatestMinor") {
      Some(Self::LatestMinor)
    } else if value.eq_ignore_ascii_case("LatestMajor") {
      Some(Self::LatestMajor)
    } else {
      None
    }
  }

  /// Returns the canonical runtimeconfig spelling.
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Disable => "Disable",
      Self::LatestPatch => "LatestPatch",
      Self::Minor => "Minor",
      Self::Major => "Major",
      Self::LatestMinor => "LatestMinor",
      Self::LatestMajor => "LatestMajor",
    }
  }
}

/// One evaluated SDK-style project.
///
/// Variable text is stored once in `text`; source and reference batches contain
/// compact spans into that immutable buffer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSpec {
  text: Box<str>,
  project_path: TextSpan,
  project_directory: TextSpan,
  target_framework_text: TextSpan,
  assembly_name: TextSpan,
  root_namespace: TextSpan,
  runtime_dimensions: Box<[TextSpan]>,
  sources: Box<[TextSpan]>,
  project_references: Box<[TextSpan]>,
  package_references: Box<[PackageReference]>,
  central_package_versions: Box<[CentralPackageVersion]>,
  central_package_fingerprint: TextSpan,
  framework_references: Box<[FrameworkReference]>,
  runtime_framework_version: TextSpan,
  runtime_identifier_index: u32,
  runtime_identifiers_len: u32,
  configuration: ProjectConfiguration,
  output_type: ProjectOutputType,
  nullable: bool,
  implicit_usings: bool,
  deterministic: bool,
  self_contained: bool,
  nuget_audit_enabled: bool,
  restore_package_pruning: bool,
  allow_missing_prune_package_data: bool,
  central_package_management: bool,
  central_transitive_pinning: bool,
  nuget_audit_mode: NugetAuditMode,
  nuget_audit_level: NugetAuditLevel,
  target_latest_runtime_patch: Option<bool>,
  roll_forward: RuntimeRollForward,
  target_framework: TargetFramework,
}

impl ProjectSpec {
  /// Returns the full project-file path.
  pub fn project_path(&self) -> &Path {
    Path::new(self.text(self.project_path))
  }

  /// Returns the full project directory.
  pub fn project_directory(&self) -> &Path {
    Path::new(self.text(self.project_directory))
  }

  /// Returns the selected SDK declaration.
  pub fn sdk(&self) -> &'static str {
    SUPPORTED_SDK
  }

  /// Returns the selected target framework.
  pub fn target_framework(&self) -> &str {
    self.text(self.target_framework_text)
  }

  /// Returns parsed target-framework data for downstream selection.
  pub fn target(&self) -> TargetFramework {
    self.target_framework
  }

  /// Returns the selected runtime identifier, when an inner target is selected.
  pub fn runtime_identifier(&self) -> Option<&str> {
    (self.runtime_identifier_index != NO_RUNTIME_IDENTIFIER).then(|| self.text(self.runtime_dimensions[self.runtime_identifier_index as usize]))
  }

  /// Iterates the ordered `RuntimeIdentifiers` expansion values.
  pub fn runtime_identifiers(&self) -> impl ExactSizeIterator<Item = &str> {
    self.runtime_dimensions[..self.runtime_identifiers_len as usize]
      .iter()
      .map(|span| self.text(*span))
  }

  /// Iterates every unique runtime target dimension without repeating the project.
  pub fn runtime_dimensions(&self) -> impl ExactSizeIterator<Item = &str> {
    self.runtime_dimensions.iter().map(|span| self.text(*span))
  }

  /// Returns the selected build configuration.
  pub fn configuration(&self) -> ProjectConfiguration {
    self.configuration
  }

  /// Returns the managed output type.
  pub fn output_type(&self) -> ProjectOutputType {
    self.output_type
  }

  /// Returns the output assembly name.
  pub fn assembly_name(&self) -> &str {
    self.text(self.assembly_name)
  }

  /// Returns the generated root namespace.
  pub fn root_namespace(&self) -> &str {
    self.text(self.root_namespace)
  }

  /// Returns whether nullable annotations and warnings are enabled.
  pub fn nullable_enabled(&self) -> bool {
    self.nullable
  }

  /// Returns whether SDK implicit global usings are enabled.
  pub fn implicit_usings_enabled(&self) -> bool {
    self.implicit_usings
  }

  /// Returns whether deterministic compiler output is required.
  pub fn deterministic(&self) -> bool {
    self.deterministic
  }

  /// Iterates source paths relative to the project directory.
  pub fn sources(&self) -> impl ExactSizeIterator<Item = &str> {
    self.sources.iter().map(|span| self.text(*span))
  }

  /// Iterates normalized project-reference paths relative to the project.
  pub fn project_references(&self) -> impl ExactSizeIterator<Item = &str> {
    self.project_references.iter().map(|span| self.text(*span))
  }

  /// Returns the compact package-reference batch.
  pub fn package_references(&self) -> &[PackageReference] {
    &self.package_references
  }

  /// Returns a package identifier.
  pub fn package_id(&self, package: PackageReference) -> &str {
    self.text(package.id)
  }

  /// Returns a literal package version or range.
  pub fn package_version(&self, package: PackageReference) -> &str {
    self.text(package.version)
  }

  /// Returns the asset families explicitly or implicitly included.
  pub fn package_include_assets(&self, package: PackageReference) -> PackageAssetFlags {
    package.include_assets
  }

  /// Returns the asset families explicitly excluded.
  pub fn package_exclude_assets(&self, package: PackageReference) -> PackageAssetFlags {
    package.exclude_assets
  }

  /// Returns the effective asset families consumed by this project.
  pub fn package_effective_assets(&self, package: PackageReference) -> PackageAssetFlags {
    package.include_assets.without(package.exclude_assets)
  }

  /// Returns the asset families kept private from a consuming project.
  pub fn package_private_assets(&self, package: PackageReference) -> PackageAssetFlags {
    package.private_assets
  }

  /// Returns package-scoped warning suppressions.
  pub fn package_no_warn(&self, package: PackageReference) -> Option<&str> {
    (package.no_warn != NO_TEXT).then(|| self.text(package.no_warn))
  }

  /// Returns compiler reference aliases for this package.
  pub fn package_aliases(&self, package: PackageReference) -> Option<&str> {
    (package.aliases != NO_TEXT).then(|| self.text(package.aliases))
  }

  /// Returns whether the package root must be exposed as a generated property.
  pub fn package_generate_path_property(&self, package: PackageReference) -> bool {
    package.generate_path_property
  }

  /// Returns whether package versions are supplied by central management.
  pub fn central_package_management_enabled(&self) -> bool {
    self.central_package_management
  }

  /// Returns whether selected central versions pin matching transitive nodes.
  pub fn central_package_transitive_pinning_enabled(&self) -> bool {
    self.central_transitive_pinning
  }

  /// Returns the selected identity-ordered central version batch.
  pub fn central_package_versions(&self) -> &[CentralPackageVersion] {
    &self.central_package_versions
  }

  /// Returns a central package identity.
  pub fn central_package_id(&self, package: CentralPackageVersion) -> &str {
    self.text(package.id)
  }

  /// Returns a central package version or range.
  pub fn central_package_version(&self, package: CentralPackageVersion) -> &str {
    self.text(package.version)
  }

  pub(crate) fn central_package_fingerprint(&self) -> &str {
    self.text(self.central_package_fingerprint)
  }

  /// Returns the compact batch of explicit framework references.
  pub fn framework_references(&self) -> &[FrameworkReference] {
    &self.framework_references
  }

  /// Returns a framework-reference identity.
  pub fn framework_reference_id(&self, reference: FrameworkReference) -> &str {
    self.text(reference.id)
  }

  /// Returns a per-reference runtime version override.
  pub fn framework_runtime_version(&self, reference: FrameworkReference) -> Option<&str> {
    self.optional_text(reference.runtime_version)
  }

  /// Returns a per-reference targeting-pack version override.
  pub fn framework_targeting_pack_version(&self, reference: FrameworkReference) -> Option<&str> {
    self.optional_text(reference.targeting_pack_version)
  }

  /// Returns a per-reference latest-patch override.
  pub fn framework_target_latest_runtime_patch(&self, reference: FrameworkReference) -> Option<bool> {
    reference.target_latest_runtime_patch
  }

  /// Returns the project-wide runtime framework version override.
  pub fn runtime_framework_version(&self) -> Option<&str> {
    self.optional_text(self.runtime_framework_version)
  }

  /// Returns whether deployment includes the selected runtime.
  pub fn self_contained(&self) -> bool {
    self.self_contained
  }

  /// Returns whether package vulnerability auditing is enabled.
  pub fn nuget_audit_enabled(&self) -> bool {
    self.nuget_audit_enabled
  }

  /// Returns whether framework-provided packages are pruned during restore.
  pub fn restore_package_pruning_enabled(&self) -> bool {
    self.restore_package_pruning
  }

  /// Returns whether missing SDK pruning data is tolerated.
  pub fn allow_missing_prune_package_data(&self) -> bool {
    self.allow_missing_prune_package_data
  }

  /// Returns whether auditing covers direct or all dependencies.
  pub fn nuget_audit_mode(&self) -> NugetAuditMode {
    self.nuget_audit_mode
  }

  /// Returns the minimum vulnerability severity reported by auditing.
  pub fn nuget_audit_level(&self) -> NugetAuditLevel {
    self.nuget_audit_level
  }

  /// Returns the explicit project-wide latest-patch preference.
  pub fn target_latest_runtime_patch(&self) -> Option<bool> {
    self.target_latest_runtime_patch
  }

  /// Returns the effective runtime-host roll-forward policy.
  pub fn roll_forward(&self) -> RuntimeRollForward {
    self.roll_forward
  }

  fn text(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }

  fn optional_text(&self, span: TextSpan) -> Option<&str> {
    (span != NO_TEXT).then(|| self.text(span))
  }
}

/// Stable category for project discovery and evaluation failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectErrorKind {
  /// No project exists at the requested location.
  NotFound,
  /// Implicit project selection found more than one candidate.
  Ambiguous,
  /// A filesystem operation failed.
  Io,
  /// Project XML is malformed.
  InvalidXml,
  /// The project uses behavior outside the initial compatibility contract.
  Unsupported,
  /// A supported property has an invalid value.
  InvalidProperty,
  /// A required path cannot be represented by the initial UTF-8 path table.
  NonUnicodePath,
}

/// A project failure with stable category and path context.
#[derive(Debug)]
pub struct ProjectError {
  kind: ProjectErrorKind,
  path: PathBuf,
  message: String,
}

impl ProjectError {
  /// Returns the stable error category.
  pub fn kind(&self) -> ProjectErrorKind {
    self.kind
  }

  /// Returns the project or directory associated with the failure.
  pub fn path(&self) -> &Path {
    &self.path
  }

  fn new(kind: ProjectErrorKind, path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
    Self {
      kind,
      path: path.into(),
      message: message.into(),
    }
  }
}

impl fmt::Display for ProjectError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl Error for ProjectError {}

#[derive(Clone, Copy)]
enum Property {
  TargetFramework,
  TargetFrameworks,
  RuntimeIdentifier,
  RuntimeIdentifiers,
  OutputType,
  Nullable,
  ImplicitUsings,
  AssemblyName,
  RootNamespace,
  Deterministic,
  RuntimeFrameworkVersion,
  TargetLatestRuntimePatch,
  RollForward,
  SelfContained,
  NugetAudit,
  NugetAuditMode,
  NugetAuditLevel,
  RestoreEnablePackagePruning,
  AllowMissingPrunePackageData,
  ManagePackageVersionsCentrally,
  CentralPackageTransitivePinningEnabled,
  CentralPackageVersionOverrideEnabled,
}

#[derive(Clone, Copy)]
enum FrameworkMetadata {
  RuntimeFrameworkVersion,
  TargetingPackVersion,
  TargetLatestRuntimePatch,
}

#[derive(Clone, Copy)]
enum PackageMetadata {
  Version,
  VersionOverride,
  IncludeAssets,
  ExcludeAssets,
  PrivateAssets,
  NoWarn,
  Aliases,
  GeneratePathProperty,
}

#[derive(Clone, Copy)]
enum Element {
  Document,
  Project,
  PropertyGroup,
  ItemGroup(u32),
  Property(Property),
  ProjectReference,
  PackageReference(usize),
  PackageMetadata(usize, PackageMetadata),
  FrameworkReference(usize),
  FrameworkMetadata(usize, FrameworkMetadata),
}

#[derive(Clone, Copy)]
enum CentralProperty {
  ManagePackageVersionsCentrally,
  CentralPackageTransitivePinningEnabled,
  CentralPackageVersionOverrideEnabled,
}

#[derive(Clone, Copy)]
enum CentralElement {
  Document,
  Project,
  PropertyGroup,
  ItemGroup(u32),
  Property(CentralProperty),
  PackageVersion(usize),
  PackageVersionValue(usize),
  GlobalPackageReference(usize),
  GlobalPackageMetadata(usize, PackageMetadata),
}

#[derive(Default)]
struct RawProject {
  target_framework: Option<String>,
  runtime_identifier: Option<String>,
  runtime_identifiers: Option<String>,
  output_type: Option<String>,
  nullable: Option<String>,
  implicit_usings: Option<String>,
  assembly_name: Option<String>,
  root_namespace: Option<String>,
  deterministic: Option<String>,
  runtime_framework_version: Option<String>,
  target_latest_runtime_patch: Option<String>,
  roll_forward: Option<String>,
  self_contained: Option<String>,
  nuget_audit: Option<String>,
  nuget_audit_mode: Option<String>,
  nuget_audit_level: Option<String>,
  restore_enable_package_pruning: Option<String>,
  allow_missing_prune_package_data: Option<String>,
  manage_package_versions_centrally: Option<String>,
  central_package_transitive_pinning_enabled: Option<String>,
  central_package_version_override_enabled: Option<String>,
  conditions: Vec<String>,
  project_references: Vec<RawProjectReference>,
  package_references: Vec<RawPackageReference>,
  framework_references: Vec<RawFrameworkReference>,
}

#[derive(Clone, Copy)]
struct RawReferenceConditions {
  group: u32,
  item: u32,
}

impl Default for RawReferenceConditions {
  fn default() -> Self {
    Self {
      group: NO_REFERENCE_CONDITION,
      item: NO_REFERENCE_CONDITION,
    }
  }
}

const _: () = assert!(size_of::<RawReferenceConditions>() == 8);
const _: () = assert!(align_of::<RawReferenceConditions>() == 4);

struct RawProjectReference {
  path: String,
  conditions: RawReferenceConditions,
}

struct RawPackageReference {
  id: String,
  version: Option<String>,
  version_override: Option<String>,
  include_assets: Option<String>,
  exclude_assets: Option<String>,
  private_assets: Option<String>,
  no_warn: Option<String>,
  aliases: Option<String>,
  generate_path_property: Option<String>,
  conditions: RawReferenceConditions,
  central_global: bool,
}

#[derive(Default)]
struct RawCentralPackages {
  path: Option<PathBuf>,
  manage_package_versions_centrally: Option<String>,
  central_package_transitive_pinning_enabled: Option<String>,
  central_package_version_override_enabled: Option<String>,
  conditions: Vec<String>,
  versions: Vec<RawCentralPackageVersion>,
  globals: Vec<RawPackageReference>,
}

struct RawCentralPackageVersion {
  id: String,
  version: Option<String>,
  conditions: RawReferenceConditions,
}

struct RawFrameworkReference {
  id: String,
  runtime_version: Option<String>,
  targeting_pack_version: Option<String>,
  target_latest_runtime_patch: Option<String>,
  conditions: RawReferenceConditions,
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

  fn push(&mut self, value: &str, path: &Path) -> Result<TextSpan, ProjectError> {
    let start = u32::try_from(self.text.len()).map_err(|_| ProjectError::new(ProjectErrorKind::Unsupported, path, "project text table exceeds 4 GiB"))?;
    let len = u32::try_from(value.len()).map_err(|_| ProjectError::new(ProjectErrorKind::Unsupported, path, "one project value exceeds 4 GiB"))?;
    self.text.push_str(value);
    Ok(TextSpan { start, len })
  }
}

/// Finds exactly one `.csproj` in a directory and evaluates it.
pub fn evaluate_project(start_directory: &Path, configuration: ProjectConfiguration) -> Result<ProjectSpec, ProjectError> {
  let project_path = discover_project(start_directory)?;
  evaluate_project_path(&project_path, configuration)
}

/// Enumerates one directory into a stable batch of recognized project and solution candidates.
pub fn discover_workspace(directory: &Path) -> Result<WorkspaceInventory, ProjectError> {
  let metadata = match fs::metadata(directory) {
    Ok(metadata) => metadata,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
      return Err(ProjectError::new(
        ProjectErrorKind::NotFound,
        directory,
        format!("workspace root {} does not exist", directory.display()),
      ));
    },
    Err(error) => return Err(io_error("inspect", directory, error)),
  };
  if !metadata.is_dir() {
    return Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      directory,
      format!("workspace root {} is not a directory", directory.display()),
    ));
  }
  let root = absolute_path(directory)?;
  let entries = fs::read_dir(&root).map_err(|error| io_error("enumerate", &root, error))?;
  let mut paths = String::new();
  let mut candidates = Vec::new();
  for entry in entries {
    let entry = entry.map_err(|error| io_error("enumerate", &root, error))?;
    if !entry.file_type().map_err(|error| io_error("inspect", &entry.path(), error))?.is_file() {
      continue;
    }
    let file_name = entry.file_name();
    let Some(kind) = workspace_candidate_kind(Path::new(&file_name)) else {
      continue;
    };
    if candidates.is_empty() {
      candidates.reserve_exact(WORKSPACE_CANDIDATE_CAPACITY);
      paths.reserve_exact(WORKSPACE_PATH_CAPACITY);
    }
    if candidates.len() == MAX_WORKSPACE_CANDIDATES {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        &root,
        format!("workspace candidate count exceeds {MAX_WORKSPACE_CANDIDATES}"),
      ));
    }
    let file_name = file_name.to_str().ok_or_else(|| {
      let path = root.join(&file_name);
      ProjectError::new(
        ProjectErrorKind::NonUnicodePath,
        &path,
        format!("workspace candidate {} is not valid Unicode", path.display()),
      )
    })?;
    let path_len = u16::try_from(file_name.len()).map_err(|_| {
      let path = root.join(file_name);
      ProjectError::new(
        ProjectErrorKind::Unsupported,
        &path,
        format!("workspace candidate name exceeds {} UTF-8 bytes", u16::MAX),
      )
    })?;
    let next_path_len = paths
      .len()
      .checked_add(file_name.len())
      .filter(|length| *length <= u32::MAX as usize)
      .ok_or_else(|| ProjectError::new(ProjectErrorKind::Unsupported, &root, "workspace candidate path arena exceeds 4 GiB"))?;
    let path_start = paths.len() as u32;
    paths.push_str(file_name);
    debug_assert_eq!(paths.len(), next_path_len);
    candidates.push(WorkspaceCandidate { path_start, path_len, kind });
  }
  candidates.sort_unstable_by(|left, right| {
    let left_start = left.path_start as usize;
    let right_start = right.path_start as usize;
    paths[left_start..left_start + usize::from(left.path_len)]
      .cmp(&paths[right_start..right_start + usize::from(right.path_len)])
      .then_with(|| (left.kind as u8).cmp(&(right.kind as u8)))
  });
  Ok(WorkspaceInventory { root, paths, candidates })
}

/// Evaluates one explicit SDK-style `.csproj`.
pub fn evaluate_project_path(project_path: &Path, configuration: ProjectConfiguration) -> Result<ProjectSpec, ProjectError> {
  if !is_csproj(project_path) {
    return Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      project_path,
      "the initial evaluator accepts only C# .csproj files",
    ));
  }
  if !project_path.is_file() {
    return Err(ProjectError::new(
      ProjectErrorKind::NotFound,
      project_path,
      format!("project {} does not exist", project_path.display()),
    ));
  }

  let project_path = absolute_path(project_path)?;
  let project_directory = project_path
    .parent()
    .ok_or_else(|| ProjectError::new(ProjectErrorKind::NotFound, &project_path, "project path has no parent directory"))?
    .to_owned();
  let bytes = fs::read(&project_path).map_err(|error| io_error("read", &project_path, error))?;
  let raw = parse_project(&project_path, &bytes)?;
  let central = discover_central_packages(&project_directory)?;
  materialize_project(project_path, &project_directory, configuration, raw, central)
}

/// Evaluates one project-reference closure in deterministic root-first order.
///
/// Each project path is evaluated once. Reference order is preserved while
/// duplicate paths are removed through a sorted command-local path index.
pub fn evaluate_project_closure(root: ProjectSpec) -> Result<Vec<ProjectSpec>, ProjectError> {
  let configuration = root.configuration();
  let mut seen = vec![fs::canonicalize(root.project_path()).map_err(|error| io_error("canonicalize", root.project_path(), error))?];
  let mut projects = vec![root];
  let mut cursor = 0usize;
  while cursor < projects.len() {
    let references = projects[cursor]
      .project_references()
      .map(|reference| projects[cursor].project_directory().join(reference))
      .collect::<Vec<_>>();
    for reference in references {
      let reference = absolute_path(&reference)?;
      let key = fs::canonicalize(&reference).map_err(|error| io_error("canonicalize", &reference, error))?;
      match seen.binary_search(&key) {
        Ok(_) => continue,
        Err(index) => seen.insert(index, key),
      }
      projects.push(evaluate_project_path(&reference, configuration)?);
    }
    cursor += 1;
  }
  Ok(projects)
}

fn discover_central_packages(project_directory: &Path) -> Result<RawCentralPackages, ProjectError> {
  let mut directory = Some(project_directory);
  while let Some(current) = directory {
    let path = current.join("Directory.Packages.props");
    match fs::metadata(&path) {
      Ok(metadata) => {
        if !metadata.is_file() {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            &path,
            "Directory.Packages.props must be a regular file",
          ));
        }
        if metadata.len() > MAX_CENTRAL_PACKAGE_FILE_BYTES {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            &path,
            format!("Directory.Packages.props exceeds the {MAX_CENTRAL_PACKAGE_FILE_BYTES}-byte input limit"),
          ));
        }
        let bytes = fs::read(&path).map_err(|error| io_error("read", &path, error))?;
        let mut central = parse_central_packages(&path, &bytes)?;
        central.path = Some(path);
        return Ok(central);
      },
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
      Err(error) => return Err(io_error("read", &path, error)),
    }
    directory = current.parent();
  }
  Ok(RawCentralPackages::default())
}

fn parse_central_packages(path: &Path, bytes: &[u8]) -> Result<RawCentralPackages, ProjectError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  reader.config_mut().expand_empty_elements = true;
  let mut raw = RawCentralPackages::default();
  let mut stack = [CentralElement::Document; MAX_XML_DEPTH];
  let mut depth = 1usize;
  let mut text = String::new();
  let mut root_seen = false;
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => {
        let parent = stack[depth - 1];
        let next = start_central_element(path, &reader, &element, parent, &mut raw, &mut root_seen)?;
        if depth == MAX_XML_DEPTH {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            "Directory.Packages.props XML nesting is too deep",
          ));
        }
        if matches!(
          next,
          CentralElement::Property(_) | CentralElement::PackageVersionValue(_) | CentralElement::GlobalPackageMetadata(_, _)
        ) {
          text.clear();
        }
        stack[depth] = next;
        depth += 1;
      },
      Ok(Event::End(_)) => {
        if depth == 1 {
          return Err(ProjectError::new(
            ProjectErrorKind::InvalidXml,
            path,
            "Directory.Packages.props contains an unexpected closing element",
          ));
        }
        depth -= 1;
        finish_central_element(path, stack[depth], &mut raw, &text)?;
      },
      Ok(Event::Text(value)) => {
        if !matches!(
          stack[depth - 1],
          CentralElement::Property(_) | CentralElement::PackageVersionValue(_) | CentralElement::GlobalPackageMetadata(_, _)
        ) {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            "Directory.Packages.props contains text outside a supported value",
          ));
        }
        text.push_str(
          &value
            .xml10_content()
            .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("central package text is invalid: {error}")))?,
        );
      },
      Ok(Event::GeneralRef(value)) => {
        if !matches!(
          stack[depth - 1],
          CentralElement::Property(_) | CentralElement::PackageVersionValue(_) | CentralElement::GlobalPackageMetadata(_, _)
        ) {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            "Directory.Packages.props contains an entity outside a supported value",
          ));
        }
        append_reference(path, &value, &mut text)?;
      },
      Ok(Event::Comment(_) | Event::Decl(_)) => {},
      Ok(Event::CData(_) | Event::PI(_) | Event::DocType(_)) => {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          path,
          "Directory.Packages.props contains unsupported XML constructs",
        ));
      },
      Ok(Event::Empty(_)) => unreachable!("empty elements are expanded by the reader"),
      Ok(Event::Eof) => break,
      Err(error) => {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidXml,
          path,
          format!("invalid Directory.Packages.props XML at byte {}: {error}", reader.error_position()),
        ));
      },
    }
  }
  if depth != 1 || !root_seen {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidXml,
      path,
      "Directory.Packages.props does not contain one complete Project element",
    ));
  }
  Ok(raw)
}

fn discover_project(directory: &Path) -> Result<PathBuf, ProjectError> {
  use std::fmt::Write as _;

  let inventory = discover_workspace(directory)?;
  match inventory.candidates().len() {
    0 => Err(ProjectError::new(
      ProjectErrorKind::NotFound,
      directory,
      format!("no project or solution was found in {}", directory.display()),
    )),
    1 => {
      let candidate = inventory.candidates()[0];
      if inventory.kind(candidate) == WorkspaceCandidateKind::CSharpProject {
        Ok(inventory.full_path(candidate))
      } else {
        Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          inventory.full_path(candidate),
          format!(
            "the initial evaluator cannot load the discovered {} {}",
            inventory.kind(candidate).description(),
            inventory.path(candidate)
          ),
        ))
      }
    },
    count => Err(ProjectError::new(ProjectErrorKind::Ambiguous, directory, {
      let mut message = format!("{count} project or solution candidates were found in {}: ", directory.display());
      for (index, candidate) in inventory.candidates().iter().copied().take(MAX_WORKSPACE_DIAGNOSTIC_CANDIDATES).enumerate() {
        if index != 0 {
          message.push_str(", ");
        }
        write!(message, "{} ({})", inventory.path(candidate), inventory.kind(candidate).description()).expect("writing a String succeeds");
      }
      if count > MAX_WORKSPACE_DIAGNOSTIC_CANDIDATES {
        write!(message, ", and {} more", count - MAX_WORKSPACE_DIAGNOSTIC_CANDIDATES).expect("writing a String succeeds");
      }
      message.push_str("; pass one project or solution path explicitly");
      message
    })),
  }
}

fn parse_project(path: &Path, bytes: &[u8]) -> Result<RawProject, ProjectError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  reader.config_mut().expand_empty_elements = true;

  let mut raw = RawProject::default();
  let mut stack = [Element::Document; MAX_XML_DEPTH];
  let mut depth = 1;
  let mut text = String::new();
  let mut root_seen = false;

  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => {
        let parent = stack[depth - 1];
        let next = start_element(path, &reader, &element, parent, &mut raw, &mut root_seen)?;
        if depth == MAX_XML_DEPTH {
          return Err(ProjectError::new(ProjectErrorKind::Unsupported, path, "project XML nesting is too deep"));
        }
        if matches!(next, Element::Property(_) | Element::PackageMetadata(_, _) | Element::FrameworkMetadata(_, _)) {
          text.clear();
        }
        stack[depth] = next;
        depth += 1;
      },
      Ok(Event::End(_)) => {
        if depth == 1 {
          return Err(ProjectError::new(
            ProjectErrorKind::InvalidXml,
            path,
            "project XML contains an unexpected closing element",
          ));
        }
        depth -= 1;
        finish_element(path, stack[depth], &mut raw, &text)?;
      },
      Ok(Event::Text(value)) => {
        let current = stack[depth - 1];
        if matches!(
          current,
          Element::Property(_) | Element::PackageMetadata(_, _) | Element::FrameworkMetadata(_, _)
        ) {
          text.push_str(
            &value
              .xml10_content()
              .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("project text is invalid: {error}")))?,
          );
        } else {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            "project contains text outside a supported property",
          ));
        }
      },
      Ok(Event::GeneralRef(value)) => {
        let current = stack[depth - 1];
        if !matches!(
          current,
          Element::Property(_) | Element::PackageMetadata(_, _) | Element::FrameworkMetadata(_, _)
        ) {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            "project contains an entity outside a supported property",
          ));
        }
        append_reference(path, &value, &mut text)?;
      },
      Ok(Event::Comment(_) | Event::Decl(_)) => {},
      Ok(Event::CData(_) | Event::PI(_) | Event::DocType(_)) => {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          path,
          "CDATA, processing instructions, and document types are not supported",
        ));
      },
      Ok(Event::Empty(_)) => unreachable!("empty elements are expanded by the reader"),
      Ok(Event::Eof) => break,
      Err(error) => {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidXml,
          path,
          format!("invalid project XML at byte {}: {error}", reader.error_position()),
        ));
      },
    }
  }

  if depth != 1 || !root_seen {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidXml,
      path,
      "project XML does not contain one complete Project element",
    ));
  }
  Ok(raw)
}

fn start_central_element(
  path: &Path,
  reader: &Reader<&[u8]>,
  element: &BytesStart<'_>,
  parent: CentralElement,
  raw: &mut RawCentralPackages,
  root_seen: &mut bool,
) -> Result<CentralElement, ProjectError> {
  let qualified_name = element.name();
  let name = element_name(path, qualified_name.as_ref())?;
  match parent {
    CentralElement::Document if name == "Project" => {
      if *root_seen {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidXml,
          path,
          "Directory.Packages.props contains multiple root elements",
        ));
      }
      *root_seen = true;
      validate_attributes(path, reader, element, &[])?;
      Ok(CentralElement::Project)
    },
    CentralElement::Project if name == "PropertyGroup" => {
      validate_attributes(path, reader, element, &["Label"])?;
      Ok(CentralElement::PropertyGroup)
    },
    CentralElement::Project if name == "ItemGroup" => {
      validate_attributes(path, reader, element, &["Label", "Condition"])?;
      let condition = store_condition_value(path, reader, element, &mut raw.conditions)?;
      Ok(CentralElement::ItemGroup(condition))
    },
    CentralElement::PropertyGroup => {
      validate_attributes(path, reader, element, &[])?;
      let property = match name {
        "ManagePackageVersionsCentrally" => CentralProperty::ManagePackageVersionsCentrally,
        "CentralPackageTransitivePinningEnabled" => CentralProperty::CentralPackageTransitivePinningEnabled,
        "CentralPackageVersionOverrideEnabled" => CentralProperty::CentralPackageVersionOverrideEnabled,
        _ => {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            format!("central package property {name} is outside the RES-006 compatibility contract"),
          ));
        },
      };
      Ok(CentralElement::Property(property))
    },
    CentralElement::ItemGroup(group) if name == "PackageVersion" => {
      if raw.versions.len() == MAX_CENTRAL_PACKAGE_ROWS {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          path,
          format!("Directory.Packages.props contains more than {MAX_CENTRAL_PACKAGE_ROWS} PackageVersion rows"),
        ));
      }
      let include = required_attribute(path, reader, element, "Include", &["Version", "Condition"])?;
      let conditions = RawReferenceConditions {
        group,
        item: store_condition_value(path, reader, element, &mut raw.conditions)?,
      };
      let index = raw.versions.len();
      raw.versions.push(RawCentralPackageVersion {
        id: include,
        version: optional_attribute(path, reader, element, "Version")?,
        conditions,
      });
      Ok(CentralElement::PackageVersion(index))
    },
    CentralElement::PackageVersion(index) if name == "Version" => {
      validate_attributes(path, reader, element, &[])?;
      Ok(CentralElement::PackageVersionValue(index))
    },
    CentralElement::ItemGroup(group) if name == "GlobalPackageReference" => {
      if raw.globals.len() == MAX_CENTRAL_PACKAGE_ROWS {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          path,
          format!("Directory.Packages.props contains more than {MAX_CENTRAL_PACKAGE_ROWS} GlobalPackageReference rows"),
        ));
      }
      const METADATA: &[&str] = &["Version", "Condition"];
      let include = required_attribute(path, reader, element, "Include", METADATA)?;
      let conditions = RawReferenceConditions {
        group,
        item: store_condition_value(path, reader, element, &mut raw.conditions)?,
      };
      let index = raw.globals.len();
      raw.globals.push(RawPackageReference {
        id: include,
        version: optional_attribute(path, reader, element, "Version")?,
        version_override: None,
        include_assets: None,
        exclude_assets: None,
        private_assets: None,
        no_warn: None,
        aliases: None,
        generate_path_property: None,
        conditions,
        central_global: true,
      });
      Ok(CentralElement::GlobalPackageReference(index))
    },
    CentralElement::GlobalPackageReference(index) => {
      validate_attributes(path, reader, element, &[])?;
      let metadata = match name {
        "Version" => PackageMetadata::Version,
        _ => {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            format!("global-package metadata element {name} is not supported here"),
          ));
        },
      };
      Ok(CentralElement::GlobalPackageMetadata(index, metadata))
    },
    CentralElement::PackageVersion(_) => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("PackageVersion metadata element {name} is not supported here"),
    )),
    CentralElement::Property(_) | CentralElement::PackageVersionValue(_) | CentralElement::GlobalPackageMetadata(_, _) => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("nested element {name} is not supported in a central package value"),
    )),
    _ => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("element {name} is outside the RES-006 Directory.Packages.props contract"),
    )),
  }
}

fn finish_central_element(path: &Path, element: CentralElement, raw: &mut RawCentralPackages, text: &str) -> Result<(), ProjectError> {
  match element {
    CentralElement::Property(property) => {
      let slot = match property {
        CentralProperty::ManagePackageVersionsCentrally => &mut raw.manage_package_versions_centrally,
        CentralProperty::CentralPackageTransitivePinningEnabled => &mut raw.central_package_transitive_pinning_enabled,
        CentralProperty::CentralPackageVersionOverrideEnabled => &mut raw.central_package_version_override_enabled,
      };
      *slot = Some(text.to_owned());
    },
    CentralElement::PackageVersionValue(index) => {
      let package = &mut raw.versions[index];
      if package.version.is_some() {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          path,
          format!("central package {:?} declares Version more than once", package.id),
        ));
      }
      package.version = Some(text.to_owned());
    },
    CentralElement::GlobalPackageMetadata(index, metadata) => {
      let package = &mut raw.globals[index];
      let (slot, name) = match metadata {
        PackageMetadata::Version => (&mut package.version, "Version"),
        PackageMetadata::VersionOverride => unreachable!("global packages do not accept VersionOverride"),
        PackageMetadata::IncludeAssets => (&mut package.include_assets, "IncludeAssets"),
        PackageMetadata::ExcludeAssets => (&mut package.exclude_assets, "ExcludeAssets"),
        PackageMetadata::PrivateAssets => (&mut package.private_assets, "PrivateAssets"),
        PackageMetadata::NoWarn => (&mut package.no_warn, "NoWarn"),
        PackageMetadata::Aliases => (&mut package.aliases, "Aliases"),
        PackageMetadata::GeneratePathProperty => (&mut package.generate_path_property, "GeneratePathProperty"),
      };
      if slot.is_some() {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          path,
          format!("global package {:?} declares {name} more than once", package.id),
        ));
      }
      *slot = Some(text.to_owned());
    },
    _ => {},
  }
  Ok(())
}

fn start_element(
  path: &Path,
  reader: &Reader<&[u8]>,
  element: &BytesStart<'_>,
  parent: Element,
  raw: &mut RawProject,
  root_seen: &mut bool,
) -> Result<Element, ProjectError> {
  let qualified_name = element.name();
  let name = element_name(path, qualified_name.as_ref())?;
  match parent {
    Element::Document if name == "Project" => {
      if *root_seen {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidXml,
          path,
          "project XML contains multiple root elements",
        ));
      }
      *root_seen = true;
      let sdk = required_attribute(path, reader, element, "Sdk", &[])?;
      if sdk != SUPPORTED_SDK {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          path,
          format!("project SDK {sdk:?} is unsupported; use {SUPPORTED_SDK}"),
        ));
      }
      Ok(Element::Project)
    },
    Element::Project if name == "PropertyGroup" => {
      validate_attributes(path, reader, element, &["Label"])?;
      Ok(Element::PropertyGroup)
    },
    Element::Project if name == "ItemGroup" => {
      validate_attributes(path, reader, element, &["Label", "Condition"])?;
      let condition = store_condition(path, reader, element, raw)?;
      Ok(Element::ItemGroup(condition))
    },
    Element::Project if matches!(name, "Import" | "Target" | "UsingTask" | "Sdk") => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("{name} is outside the initial project compatibility contract"),
    )),
    Element::PropertyGroup => {
      validate_attributes(path, reader, element, &[])?;
      Ok(Element::Property(property(path, name)?))
    },
    Element::ItemGroup(group) if name == "ProjectReference" => {
      let include = required_attribute(path, reader, element, "Include", &["Condition"])?;
      let conditions = RawReferenceConditions {
        group,
        item: store_condition(path, reader, element, raw)?,
      };
      raw.project_references.push(RawProjectReference { path: include, conditions });
      Ok(Element::ProjectReference)
    },
    Element::ItemGroup(group) if name == "PackageReference" => {
      const METADATA: &[&str] = &[
        "Version",
        "VersionOverride",
        "IncludeAssets",
        "ExcludeAssets",
        "PrivateAssets",
        "NoWarn",
        "Aliases",
        "GeneratePathProperty",
        "Condition",
      ];
      let include = required_attribute(path, reader, element, "Include", METADATA)?;
      let version = optional_attribute(path, reader, element, "Version")?;
      let version_override = optional_attribute(path, reader, element, "VersionOverride")?;
      let conditions = RawReferenceConditions {
        group,
        item: store_condition(path, reader, element, raw)?,
      };
      let index = raw.package_references.len();
      raw.package_references.push(RawPackageReference {
        id: include,
        version,
        version_override,
        include_assets: optional_attribute(path, reader, element, "IncludeAssets")?,
        exclude_assets: optional_attribute(path, reader, element, "ExcludeAssets")?,
        private_assets: optional_attribute(path, reader, element, "PrivateAssets")?,
        no_warn: optional_attribute(path, reader, element, "NoWarn")?,
        aliases: optional_attribute(path, reader, element, "Aliases")?,
        generate_path_property: optional_attribute(path, reader, element, "GeneratePathProperty")?,
        conditions,
        central_global: false,
      });
      Ok(Element::PackageReference(index))
    },
    Element::ItemGroup(group) if name == "FrameworkReference" => {
      let include = required_attribute(
        path,
        reader,
        element,
        "Include",
        &["RuntimeFrameworkVersion", "TargetingPackVersion", "TargetLatestRuntimePatch", "Condition"],
      )?;
      let conditions = RawReferenceConditions {
        group,
        item: store_condition(path, reader, element, raw)?,
      };
      let index = raw.framework_references.len();
      raw.framework_references.push(RawFrameworkReference {
        id: include,
        runtime_version: optional_attribute(path, reader, element, "RuntimeFrameworkVersion")?,
        targeting_pack_version: optional_attribute(path, reader, element, "TargetingPackVersion")?,
        target_latest_runtime_patch: optional_attribute(path, reader, element, "TargetLatestRuntimePatch")?,
        conditions,
      });
      Ok(Element::FrameworkReference(index))
    },
    Element::PackageReference(index) => {
      validate_attributes(path, reader, element, &[])?;
      let metadata = match name {
        "Version" => PackageMetadata::Version,
        "VersionOverride" => PackageMetadata::VersionOverride,
        "IncludeAssets" => PackageMetadata::IncludeAssets,
        "ExcludeAssets" => PackageMetadata::ExcludeAssets,
        "PrivateAssets" => PackageMetadata::PrivateAssets,
        "NoWarn" => PackageMetadata::NoWarn,
        "Aliases" => PackageMetadata::Aliases,
        "GeneratePathProperty" => PackageMetadata::GeneratePathProperty,
        _ => {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            format!("package-reference metadata element {name} is not supported here"),
          ));
        },
      };
      Ok(Element::PackageMetadata(index, metadata))
    },
    Element::FrameworkReference(index) => {
      validate_attributes(path, reader, element, &[])?;
      let metadata = match name {
        "RuntimeFrameworkVersion" => FrameworkMetadata::RuntimeFrameworkVersion,
        "TargetingPackVersion" => FrameworkMetadata::TargetingPackVersion,
        "TargetLatestRuntimePatch" => FrameworkMetadata::TargetLatestRuntimePatch,
        _ => {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            format!("framework-reference metadata element {name} is not supported here"),
          ));
        },
      };
      Ok(Element::FrameworkMetadata(index, metadata))
    },
    Element::ProjectReference => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("metadata element {name} is not supported here"),
    )),
    Element::Property(_) | Element::PackageMetadata(_, _) | Element::FrameworkMetadata(_, _) => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("nested element {name} is not supported in a property value"),
    )),
    _ => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("element {name} is outside the initial project compatibility contract"),
    )),
  }
}

fn finish_element(path: &Path, element: Element, raw: &mut RawProject, text: &str) -> Result<(), ProjectError> {
  match element {
    Element::Property(property) => match property {
      Property::TargetFramework => raw.target_framework = Some(text.to_owned()),
      Property::TargetFrameworks => {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          path,
          "multi-target projects are unsupported; use TargetFramework",
        ));
      },
      Property::RuntimeIdentifier => raw.runtime_identifier = Some(text.to_owned()),
      Property::RuntimeIdentifiers => raw.runtime_identifiers = Some(text.to_owned()),
      Property::OutputType => raw.output_type = Some(text.to_owned()),
      Property::Nullable => raw.nullable = Some(text.to_owned()),
      Property::ImplicitUsings => raw.implicit_usings = Some(text.to_owned()),
      Property::AssemblyName => raw.assembly_name = Some(text.to_owned()),
      Property::RootNamespace => raw.root_namespace = Some(text.to_owned()),
      Property::Deterministic => raw.deterministic = Some(text.to_owned()),
      Property::RuntimeFrameworkVersion => raw.runtime_framework_version = Some(text.to_owned()),
      Property::TargetLatestRuntimePatch => raw.target_latest_runtime_patch = Some(text.to_owned()),
      Property::RollForward => raw.roll_forward = Some(text.to_owned()),
      Property::SelfContained => raw.self_contained = Some(text.to_owned()),
      Property::NugetAudit => raw.nuget_audit = Some(text.to_owned()),
      Property::NugetAuditMode => raw.nuget_audit_mode = Some(text.to_owned()),
      Property::NugetAuditLevel => raw.nuget_audit_level = Some(text.to_owned()),
      Property::RestoreEnablePackagePruning => raw.restore_enable_package_pruning = Some(text.to_owned()),
      Property::AllowMissingPrunePackageData => raw.allow_missing_prune_package_data = Some(text.to_owned()),
      Property::ManagePackageVersionsCentrally => raw.manage_package_versions_centrally = Some(text.to_owned()),
      Property::CentralPackageTransitivePinningEnabled => raw.central_package_transitive_pinning_enabled = Some(text.to_owned()),
      Property::CentralPackageVersionOverrideEnabled => raw.central_package_version_override_enabled = Some(text.to_owned()),
    },
    Element::PackageMetadata(index, metadata) => {
      let package = &mut raw.package_references[index];
      let (slot, name) = match metadata {
        PackageMetadata::Version => (&mut package.version, "Version"),
        PackageMetadata::VersionOverride => (&mut package.version_override, "VersionOverride"),
        PackageMetadata::IncludeAssets => (&mut package.include_assets, "IncludeAssets"),
        PackageMetadata::ExcludeAssets => (&mut package.exclude_assets, "ExcludeAssets"),
        PackageMetadata::PrivateAssets => (&mut package.private_assets, "PrivateAssets"),
        PackageMetadata::NoWarn => (&mut package.no_warn, "NoWarn"),
        PackageMetadata::Aliases => (&mut package.aliases, "Aliases"),
        PackageMetadata::GeneratePathProperty => (&mut package.generate_path_property, "GeneratePathProperty"),
      };
      if slot.is_some() {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          path,
          format!("package {:?} declares {name} more than once", package.id),
        ));
      }
      *slot = Some(text.to_owned());
    },
    Element::FrameworkMetadata(index, metadata) => {
      let reference = &mut raw.framework_references[index];
      let (slot, name) = match metadata {
        FrameworkMetadata::RuntimeFrameworkVersion => (&mut reference.runtime_version, "RuntimeFrameworkVersion"),
        FrameworkMetadata::TargetingPackVersion => (&mut reference.targeting_pack_version, "TargetingPackVersion"),
        FrameworkMetadata::TargetLatestRuntimePatch => (&mut reference.target_latest_runtime_patch, "TargetLatestRuntimePatch"),
      };
      if slot.is_some() {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          path,
          format!("framework reference {:?} declares {name} more than once", reference.id),
        ));
      }
      *slot = Some(text.to_owned());
    },
    _ => {},
  }
  Ok(())
}

fn materialize_project(
  project_path: PathBuf,
  project_directory: &Path,
  configuration: ProjectConfiguration,
  raw: RawProject,
  central: RawCentralPackages,
) -> Result<ProjectSpec, ProjectError> {
  let target_framework = required_property(&project_path, "TargetFramework", raw.target_framework)?;
  let parsed_target =
    TargetFramework::parse(&target_framework).map_err(|error| ProjectError::new(ProjectErrorKind::InvalidProperty, &project_path, error.to_string()))?;

  let output_type = match raw.output_type.as_deref().unwrap_or("Library") {
    "Exe" => ProjectOutputType::Exe,
    "Library" => ProjectOutputType::Library,
    value => {
      return Err(ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("OutputType {value:?} is unsupported; use Exe or Library"),
      ));
    },
  };
  let nullable = parse_toggle(&project_path, "Nullable", raw.nullable.as_deref(), false, true)?;
  let implicit_usings = parse_toggle(&project_path, "ImplicitUsings", raw.implicit_usings.as_deref(), false, false)?;
  let deterministic = parse_bool(&project_path, "Deterministic", raw.deterministic.as_deref(), true)?;
  let self_contained = parse_bool(&project_path, "SelfContained", raw.self_contained.as_deref(), false)?;
  let nuget_audit_enabled = parse_bool(&project_path, "NuGetAudit", raw.nuget_audit.as_deref(), true)?;
  let restore_package_pruning = parse_bool(
    &project_path,
    "RestoreEnablePackagePruning",
    raw.restore_enable_package_pruning.as_deref(),
    parsed_target.family() == FrameworkFamily::Net && parsed_target.major() >= 10,
  )?;
  let allow_missing_prune_package_data = parse_bool(
    &project_path,
    "AllowMissingPrunePackageData",
    raw.allow_missing_prune_package_data.as_deref(),
    false,
  )?;
  let central_path = central.path.as_deref().unwrap_or(&project_path);
  let central_package_management = central.path.is_some()
    && parse_bool(
      central_path,
      "ManagePackageVersionsCentrally",
      raw
        .manage_package_versions_centrally
        .as_deref()
        .or(central.manage_package_versions_centrally.as_deref()),
      false,
    )?;
  let central_transitive_pinning = central_package_management
    && parse_bool(
      central_path,
      "CentralPackageTransitivePinningEnabled",
      raw
        .central_package_transitive_pinning_enabled
        .as_deref()
        .or(central.central_package_transitive_pinning_enabled.as_deref()),
      false,
    )?;
  let central_version_override_enabled = parse_bool(
    central_path,
    "CentralPackageVersionOverrideEnabled",
    raw
      .central_package_version_override_enabled
      .as_deref()
      .or(central.central_package_version_override_enabled.as_deref()),
    true,
  )?;
  let nuget_audit_mode = match raw.nuget_audit_mode.as_deref() {
    Some(value) => NugetAuditMode::parse(value).ok_or_else(|| {
      ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("NuGetAuditMode value {value:?} must be direct or all"),
      )
    })?,
    None if parsed_target.major() >= 10 => NugetAuditMode::All,
    None => NugetAuditMode::Direct,
  };
  let nuget_audit_level = match raw.nuget_audit_level.as_deref() {
    Some(value) => NugetAuditLevel::parse(value).ok_or_else(|| {
      ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("NuGetAuditLevel value {value:?} must be low, moderate, high, or critical"),
      )
    })?,
    None => NugetAuditLevel::Low,
  };
  let target_latest_runtime_patch = raw
    .target_latest_runtime_patch
    .as_deref()
    .map(|value| parse_bool(&project_path, "TargetLatestRuntimePatch", Some(value), false))
    .transpose()?;
  let roll_forward = match raw.roll_forward.as_deref() {
    Some(value) => RuntimeRollForward::parse(value).ok_or_else(|| {
      ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("RollForward value {value:?} is unsupported"),
      )
    })?,
    None => RuntimeRollForward::Minor,
  };
  validate_optional_version(&project_path, "RuntimeFrameworkVersion", raw.runtime_framework_version.as_deref())?;
  let selected_runtime = parse_runtime_identifier(&project_path, raw.runtime_identifier.as_deref())?;
  let mut runtime_dimensions = parse_runtime_identifiers(&project_path, raw.runtime_identifiers.as_deref())?;
  let runtime_identifiers_len = u32::try_from(runtime_dimensions.len()).map_err(|_| {
    ProjectError::new(
      ProjectErrorKind::Unsupported,
      &project_path,
      "RuntimeIdentifiers contains more than 4 billion target dimensions",
    )
  })?;
  let runtime_identifier_index = if let Some(selected) = selected_runtime {
    if let Some(index) = runtime_dimensions.iter().position(|candidate| *candidate == selected) {
      index as u32
    } else {
      if runtime_dimensions.len() == NO_RUNTIME_IDENTIFIER as usize {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          &project_path,
          "runtime target dimensions exhaust the compact selected-index space",
        ));
      }
      let index = u32::try_from(runtime_dimensions.len()).map_err(|_| {
        ProjectError::new(
          ProjectErrorKind::Unsupported,
          &project_path,
          "runtime target dimensions exceed the compact 32-bit index space",
        )
      })?;
      runtime_dimensions.push(selected);
      index
    }
  } else {
    NO_RUNTIME_IDENTIFIER
  };
  let default_name = project_path
    .file_stem()
    .and_then(|value| value.to_str())
    .ok_or_else(|| ProjectError::new(ProjectErrorKind::NonUnicodePath, &project_path, "project file name is not valid Unicode"))?;
  let assembly_name = raw.assembly_name.as_deref().unwrap_or(default_name);
  if assembly_name.is_empty() || assembly_name.contains("$(") {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      &project_path,
      "AssemblyName must be a non-empty literal",
    ));
  }
  let root_namespace = raw.root_namespace.as_deref().unwrap_or(assembly_name);
  if root_namespace.is_empty() || root_namespace.contains("$(") {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      &project_path,
      "RootNamespace must be a non-empty literal",
    ));
  }

  let condition_context = ReferenceConditionContext {
    target_framework: &target_framework,
    runtime_identifier: selected_runtime,
    configuration: configuration.as_str(),
  };
  let mut project_references = Vec::with_capacity(raw.project_references.len());
  for reference in raw.project_references {
    if reference_conditions_match(&project_path, &raw.conditions, reference.conditions, &condition_context)? {
      project_references.push(normalize_project_reference(&project_path, &reference.path)?);
    }
  }
  let mut selected_central_versions = Vec::with_capacity(central.versions.len() + central.globals.len());
  let mut selected_global_references = Vec::with_capacity(central.globals.len());
  if central_package_management {
    for package in central.versions {
      if !reference_conditions_match(central_path, &central.conditions, package.conditions, &condition_context)? {
        continue;
      }
      let version = package.version.as_deref().ok_or_else(|| {
        ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          central_path,
          format!("central package {:?} requires a Version", package.id),
        )
      })?;
      if package.id.is_empty() || package.id.contains("$(") || !is_literal_package_version(version) {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          central_path,
          format!("central package {:?} requires a literal identity and version", package.id),
        ));
      }
      selected_central_versions.push(package);
    }
    for reference in central.globals {
      if !reference_conditions_match(central_path, &central.conditions, reference.conditions, &condition_context)? {
        continue;
      }
      let version = reference.version.as_deref().ok_or_else(|| {
        ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          central_path,
          format!("global package {:?} requires a Version", reference.id),
        )
      })?;
      if reference.id.is_empty() || reference.id.contains("$(") || !is_literal_package_version(version) {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          central_path,
          format!("global package {:?} requires a literal identity and version", reference.id),
        ));
      }
      selected_central_versions.push(RawCentralPackageVersion {
        id: reference.id.clone(),
        version: reference.version.clone(),
        conditions: RawReferenceConditions::default(),
      });
      selected_global_references.push(reference);
    }
    selected_central_versions.sort_unstable_by(|left, right| compare_ascii_case_insensitive(&left.id, &right.id));
    for duplicate in selected_central_versions.windows(2) {
      if duplicate[0].id.eq_ignore_ascii_case(&duplicate[1].id) {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          central_path,
          format!("central package {:?} has more than one selected PackageVersion", duplicate[1].id),
        ));
      }
    }
  }
  let central_package_fingerprint = central_package_fingerprint(
    &selected_central_versions,
    central_package_management,
    central_transitive_pinning,
    central_version_override_enabled,
  );
  let mut selected_package_references = Vec::with_capacity(raw.package_references.len() + selected_global_references.len());
  for mut reference in raw.package_references {
    if reference_conditions_match(&project_path, &raw.conditions, reference.conditions, &condition_context)? {
      if let Some(version_override) = reference.version_override.take() {
        if central_package_management && !central_version_override_enabled {
          return Err(ProjectError::new(
            ProjectErrorKind::InvalidProperty,
            &project_path,
            format!("package {:?} cannot use VersionOverride because central overrides are disabled", reference.id),
          ));
        }
        reference.version = Some(version_override);
      } else if central_package_management {
        if reference.version.is_some() {
          return Err(ProjectError::new(
            ProjectErrorKind::InvalidProperty,
            &project_path,
            format!(
              "centrally managed package {:?} must use PackageVersion or VersionOverride instead of Version",
              reference.id
            ),
          ));
        }
        reference.version = selected_central_versions
          .binary_search_by(|candidate| compare_ascii_case_insensitive(&candidate.id, &reference.id))
          .ok()
          .and_then(|index| selected_central_versions[index].version.clone());
        if reference.version.is_none() {
          return Err(ProjectError::new(
            ProjectErrorKind::InvalidProperty,
            &project_path,
            format!("centrally managed package {:?} has no selected PackageVersion", reference.id),
          ));
        }
      }
      selected_package_references.push(reference);
    }
  }
  selected_package_references.extend(selected_global_references);
  let mut selected_framework_references = Vec::with_capacity(raw.framework_references.len());
  for reference in raw.framework_references {
    if reference_conditions_match(&project_path, &raw.conditions, reference.conditions, &condition_context)? {
      selected_framework_references.push(reference);
    }
  }

  let sources = collect_sources(project_directory, &project_path)?;
  let project_path_text = unicode_path(&project_path, &project_path)?;
  let project_directory_text = unicode_path(project_directory, &project_path)?;
  let estimated_text = project_path_text.len()
    + project_directory_text.len()
    + target_framework.len()
    + assembly_name.len()
    + root_namespace.len()
    + runtime_dimensions.iter().map(|value| value.len()).sum::<usize>()
    + sources.iter().map(String::len).sum::<usize>()
    + project_references.iter().map(String::len).sum::<usize>()
    + central_package_fingerprint.len()
    + selected_central_versions
      .iter()
      .map(|package| package.id.len() + package.version.as_ref().map_or(0, String::len))
      .sum::<usize>()
    + selected_package_references
      .iter()
      .map(|package| {
        package.id.len()
          + package.version.as_ref().map_or(0, String::len)
          + package.no_warn.as_ref().map_or(0, String::len)
          + package.aliases.as_ref().map_or(0, String::len)
      })
      .sum::<usize>()
    + raw.runtime_framework_version.as_ref().map_or(0, String::len)
    + selected_framework_references
      .iter()
      .map(|reference| {
        reference.id.len() + reference.runtime_version.as_ref().map_or(0, String::len) + reference.targeting_pack_version.as_ref().map_or(0, String::len)
      })
      .sum::<usize>();
  let mut table = TextTable::with_capacity(estimated_text);
  let project_path_span = table.push(project_path_text, &project_path)?;
  let project_directory_span = table.push(project_directory_text, &project_path)?;
  let target_framework_span = table.push(&target_framework, &project_path)?;
  let assembly_name_span = table.push(assembly_name, &project_path)?;
  let root_namespace_span = table.push(root_namespace, &project_path)?;
  let runtime_dimension_spans = runtime_dimensions
    .iter()
    .map(|value| table.push(value, &project_path))
    .collect::<Result<Box<_>, _>>()?;
  let source_spans = sources.iter().map(|source| table.push(source, &project_path)).collect::<Result<Box<_>, _>>()?;
  let reference_spans = project_references
    .iter()
    .map(|reference| table.push(reference, &project_path))
    .collect::<Result<Box<_>, _>>()?;
  let central_package_fingerprint_span = table.push(&central_package_fingerprint, &project_path)?;
  let central_package_versions = selected_central_versions
    .iter()
    .map(|package| {
      Ok(CentralPackageVersion {
        id: table.push(&package.id, &project_path)?,
        version: table.push(package.version.as_deref().expect("selected central versions were validated"), &project_path)?,
      })
    })
    .collect::<Result<Box<_>, ProjectError>>()?;
  let mut package_references = Vec::with_capacity(selected_package_references.len());
  for package in selected_package_references {
    let version = package.version.ok_or_else(|| {
      ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("package {:?} requires a Version", package.id),
      )
    })?;
    if !is_literal_package_version(&version) {
      return Err(ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("package {:?} version {version:?} is not a literal version or range", package.id),
      ));
    }
    let default_include_assets = if package.central_global {
      PackageAssetFlags::RUNTIME
        .union(PackageAssetFlags::BUILD)
        .union(PackageAssetFlags::BUILD_MULTI_TARGETING)
        .union(PackageAssetFlags::NATIVE)
        .union(PackageAssetFlags::CONTENT_FILES)
        .union(PackageAssetFlags::ANALYZERS)
    } else {
      PackageAssetFlags::ALL
    };
    let include_assets = parse_package_assets(&project_path, "IncludeAssets", package.include_assets.as_deref(), default_include_assets)?;
    let exclude_assets = parse_package_assets(&project_path, "ExcludeAssets", package.exclude_assets.as_deref(), PackageAssetFlags::NONE)?;
    let private_assets = parse_package_assets(
      &project_path,
      "PrivateAssets",
      package.private_assets.as_deref(),
      if package.central_global {
        PackageAssetFlags::ALL
      } else {
        PackageAssetFlags::DEFAULT_PRIVATE
      },
    )?;
    let no_warn = optional_literal_metadata(&project_path, "NoWarn", package.no_warn.as_deref())?
      .map(|value| table.push(value, &project_path))
      .transpose()?
      .unwrap_or(NO_TEXT);
    let aliases = optional_literal_metadata(&project_path, "Aliases", package.aliases.as_deref())?
      .map(|value| table.push(value, &project_path))
      .transpose()?
      .unwrap_or(NO_TEXT);
    let generate_path_property = parse_bool(
      &project_path,
      "GeneratePathProperty",
      package.generate_path_property.as_deref().filter(|value| !value.trim().is_empty()),
      false,
    )?;
    package_references.push(PackageReference {
      id: table.push(&package.id, &project_path)?,
      version: table.push(&version, &project_path)?,
      no_warn,
      aliases,
      include_assets,
      exclude_assets,
      private_assets,
      generate_path_property,
    });
  }
  let runtime_framework_version_span = match raw.runtime_framework_version.as_deref() {
    Some(version) => table.push(version, &project_path)?,
    None => NO_TEXT,
  };
  let mut framework_references = Vec::with_capacity(selected_framework_references.len());
  for reference in selected_framework_references {
    if reference.id.is_empty() || reference.id.contains("$(") {
      return Err(ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        "FrameworkReference Include must be a non-empty literal",
      ));
    }
    if framework_references.iter().any(|existing: &FrameworkReference| {
      let start = existing.id.start as usize;
      let existing_id = &table.text[start..start + existing.id.len as usize];
      existing_id.eq_ignore_ascii_case(&reference.id)
    }) {
      return Err(ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("framework reference {:?} is declared more than once", reference.id),
      ));
    }
    validate_optional_version(&project_path, "RuntimeFrameworkVersion", reference.runtime_version.as_deref())?;
    validate_optional_version(&project_path, "TargetingPackVersion", reference.targeting_pack_version.as_deref())?;
    let target_latest_runtime_patch = reference
      .target_latest_runtime_patch
      .as_deref()
      .map(|value| parse_bool(&project_path, "TargetLatestRuntimePatch", Some(value), false))
      .transpose()?;
    framework_references.push(FrameworkReference {
      id: table.push(&reference.id, &project_path)?,
      runtime_version: match reference.runtime_version.as_deref() {
        Some(version) => table.push(version, &project_path)?,
        None => NO_TEXT,
      },
      targeting_pack_version: match reference.targeting_pack_version.as_deref() {
        Some(version) => table.push(version, &project_path)?,
        None => NO_TEXT,
      },
      target_latest_runtime_patch,
    });
  }

  Ok(ProjectSpec {
    text: table.text.into_boxed_str(),
    project_path: project_path_span,
    project_directory: project_directory_span,
    target_framework_text: target_framework_span,
    assembly_name: assembly_name_span,
    root_namespace: root_namespace_span,
    runtime_dimensions: runtime_dimension_spans,
    sources: source_spans,
    project_references: reference_spans,
    package_references: package_references.into_boxed_slice(),
    central_package_versions,
    central_package_fingerprint: central_package_fingerprint_span,
    framework_references: framework_references.into_boxed_slice(),
    runtime_framework_version: runtime_framework_version_span,
    runtime_identifier_index,
    runtime_identifiers_len,
    configuration,
    output_type,
    nullable,
    implicit_usings,
    deterministic,
    self_contained,
    nuget_audit_enabled,
    restore_package_pruning,
    allow_missing_prune_package_data,
    central_package_management,
    central_transitive_pinning,
    nuget_audit_mode,
    nuget_audit_level,
    target_latest_runtime_patch,
    roll_forward,
    target_framework: parsed_target,
  })
}

fn collect_sources(project_directory: &Path, project_path: &Path) -> Result<Vec<String>, ProjectError> {
  let mut directories = vec![project_directory.to_owned()];
  let mut sources = Vec::new();
  while let Some(directory) = directories.pop() {
    let entries = fs::read_dir(&directory).map_err(|error| io_error("enumerate", &directory, error))?;
    for entry in entries {
      let entry = entry.map_err(|error| io_error("enumerate", &directory, error))?;
      let path = entry.path();
      let file_type = entry.file_type().map_err(|error| io_error("inspect", &path, error))?;
      if file_type.is_dir() {
        if !is_output_directory(&entry.file_name()) {
          directories.push(path);
        }
      } else if file_type.is_file() && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("cs")) {
        let relative = path.strip_prefix(project_directory).expect("entries discovered below the project directory");
        sources.push(portable_path(relative, project_path)?);
      }
    }
  }
  sources.sort_unstable();
  Ok(sources)
}

fn is_output_directory(name: &std::ffi::OsStr) -> bool {
  name.eq_ignore_ascii_case("bin") || name.eq_ignore_ascii_case("obj")
}

fn parse_toggle(path: &Path, property: &str, value: Option<&str>, default: bool, only_enable: bool) -> Result<bool, ProjectError> {
  let Some(value) = value else {
    return Ok(default);
  };
  if value.eq_ignore_ascii_case("enable") {
    Ok(true)
  } else if !only_enable && value.eq_ignore_ascii_case("disable") {
    Ok(false)
  } else {
    Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("{property} value {value:?} is unsupported"),
    ))
  }
}

fn parse_bool(path: &Path, property: &str, value: Option<&str>, default: bool) -> Result<bool, ProjectError> {
  let Some(value) = value else {
    return Ok(default);
  };
  if value.eq_ignore_ascii_case("true") {
    Ok(true)
  } else if value.eq_ignore_ascii_case("false") {
    Ok(false)
  } else {
    Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("{property} value {value:?} must be true or false"),
    ))
  }
}

fn validate_optional_version(path: &Path, property: &str, value: Option<&str>) -> Result<(), ProjectError> {
  if value.is_some_and(|value| value.is_empty() || value.contains("$(")) {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("{property} must be a non-empty literal version"),
    ));
  }
  Ok(())
}

fn parse_runtime_identifier<'a>(path: &Path, value: Option<&'a str>) -> Result<Option<&'a str>, ProjectError> {
  let Some(value) = value.filter(|value| !value.is_empty()) else {
    return Ok(None);
  };
  if value.contains("$(") || value.contains(';') {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("RuntimeIdentifier {value:?} must be one literal runtime identifier"),
    ));
  }
  Ok(Some(value))
}

fn parse_runtime_identifiers<'a>(path: &Path, value: Option<&'a str>) -> Result<Vec<&'a str>, ProjectError> {
  let Some(value) = value else {
    return Ok(Vec::new());
  };
  if value.contains("$(") {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      "RuntimeIdentifiers must be a literal semicolon-delimited list",
    ));
  }

  let mut identifiers = Vec::with_capacity(value.bytes().filter(|byte| *byte == b';').count() + 1);
  for identifier in value.split(';').map(str::trim).filter(|identifier| !identifier.is_empty()) {
    // RID batches are tiny. A linear uniqueness check avoids a second allocation
    // and keeps the hot downstream representation contiguous.
    if !identifiers.contains(&identifier) {
      identifiers.push(identifier);
    }
  }
  Ok(identifiers)
}

// Reference conditions are a transient linear transform: borrowed condition
// text plus three stable dimensions becomes one selection bit. The common
// exact-property path allocates no evaluation storage; compound property
// interpolation alone needs a temporary variable-sized string.
struct ReferenceConditionContext<'a> {
  target_framework: &'a str,
  runtime_identifier: Option<&'a str>,
  configuration: &'a str,
}

struct ReferenceConditionParser<'a, 'context> {
  input: &'a str,
  position: usize,
  comparisons: u8,
  depth: u8,
  context: &'context ReferenceConditionContext<'context>,
  path: &'context Path,
}

enum ReferenceConditionValue<'input, 'context> {
  Input(&'input str),
  Context(&'context str),
  Expanded(String),
}

impl ReferenceConditionValue<'_, '_> {
  fn as_str(&self) -> &str {
    match self {
      Self::Input(value) => value,
      Self::Context(value) => value,
      Self::Expanded(value) => value,
    }
  }
}

impl<'a, 'context> ReferenceConditionParser<'a, 'context> {
  fn evaluate(path: &'context Path, input: &'a str, context: &'context ReferenceConditionContext<'context>) -> Result<bool, ProjectError> {
    if input.len() > MAX_REFERENCE_CONDITION_BYTES {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        format!("reference Condition exceeds the {MAX_REFERENCE_CONDITION_BYTES}-byte evaluation limit"),
      ));
    }
    if !input.is_ascii() {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        "reference Condition must use ASCII syntax and dimension values",
      ));
    }
    let mut parser = Self {
      input,
      position: 0,
      comparisons: 0,
      depth: 0,
      context,
      path,
    };
    let value = parser.parse_or()?;
    parser.skip_whitespace();
    if parser.position != parser.input.len() {
      return Err(parser.unsupported("contains syntax outside equality, inequality, And, Or, !, and parentheses"));
    }
    Ok(value)
  }

  fn parse_or(&mut self) -> Result<bool, ProjectError> {
    let mut value = self.parse_and()?;
    while self.consume_keyword("Or") {
      let right = self.parse_and()?;
      value |= right;
    }
    Ok(value)
  }

  fn parse_and(&mut self) -> Result<bool, ProjectError> {
    let mut value = self.parse_unary()?;
    while self.consume_keyword("And") {
      let right = self.parse_unary()?;
      value &= right;
    }
    Ok(value)
  }

  fn parse_unary(&mut self) -> Result<bool, ProjectError> {
    self.skip_whitespace();
    if self.consume_byte(b'!') {
      self.enter_nested()?;
      let value = self.parse_unary()?;
      self.depth -= 1;
      return Ok(!value);
    }
    if self.consume_byte(b'(') {
      self.enter_nested()?;
      let value = self.parse_or()?;
      self.depth -= 1;
      self.skip_whitespace();
      if !self.consume_byte(b')') {
        return Err(self.invalid("has an unclosed parenthesized expression"));
      }
      return Ok(value);
    }
    self.parse_comparison()
  }

  fn parse_comparison(&mut self) -> Result<bool, ProjectError> {
    if self.comparisons == MAX_REFERENCE_CONDITION_OPERATORS {
      return Err(self.unsupported(&format!("contains more than {MAX_REFERENCE_CONDITION_OPERATORS} comparisons")));
    }
    self.comparisons += 1;
    let left = self.parse_operand()?;
    self.skip_whitespace();
    let equal = if self.consume_text("==") {
      true
    } else if self.consume_text("!=") {
      false
    } else if left.as_str().eq_ignore_ascii_case("true") {
      return Ok(true);
    } else if left.as_str().eq_ignore_ascii_case("false") {
      return Ok(false);
    } else {
      return Err(self.unsupported("must compare dimension values with == or !="));
    };
    let right = self.parse_operand()?;
    let matches = left.as_str().eq_ignore_ascii_case(right.as_str());
    Ok(if equal { matches } else { !matches })
  }

  fn parse_operand(&mut self) -> Result<ReferenceConditionValue<'a, 'context>, ProjectError> {
    self.skip_whitespace();
    let Some(first) = self.input.as_bytes().get(self.position).copied() else {
      return Err(self.invalid("ends before a comparison value"));
    };
    let value = if matches!(first, b'\'' | b'"') {
      self.position += 1;
      let start = self.position;
      let Some(offset) = self.input[start..].bytes().position(|byte| byte == first) else {
        return Err(self.invalid("contains an unterminated quoted value"));
      };
      self.position = start + offset + 1;
      &self.input[start..start + offset]
    } else {
      let start = self.position;
      while self.position < self.input.len() {
        if self.input[self.position..].starts_with("$(") {
          let Some(close) = self.input[self.position + 2..].find(')') else {
            return Err(self.invalid("contains an unterminated property reference"));
          };
          self.position += close + 3;
          continue;
        }
        let byte = self.input.as_bytes()[self.position];
        if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b'!' | b'=') {
          break;
        }
        self.position += 1;
      }
      if self.position == start {
        return Err(self.invalid("is missing a comparison value"));
      }
      &self.input[start..self.position]
    };
    self.expand_operand(value)
  }

  fn expand_operand(&self, value: &'a str) -> Result<ReferenceConditionValue<'a, 'context>, ProjectError> {
    let Some(first) = value.find("$(") else {
      return Ok(ReferenceConditionValue::Input(value));
    };
    if first == 0
      && let Some(close) = value[2..].find(')')
      && close + 3 == value.len()
    {
      return self.property_value(&value[2..close + 2]).map(ReferenceConditionValue::Context);
    }
    let mut expanded = String::with_capacity(value.len() + 16);
    let mut remaining = value;
    let mut offset = first;
    loop {
      expanded.push_str(&remaining[..offset]);
      let property_start = offset + 2;
      let Some(close) = remaining[property_start..].find(')') else {
        return Err(self.invalid("contains an unterminated property reference"));
      };
      let property_end = property_start + close;
      let property = &remaining[property_start..property_end];
      expanded.push_str(self.property_value(property)?);
      remaining = &remaining[property_end + 1..];
      let Some(next) = remaining.find("$(") else {
        expanded.push_str(remaining);
        break;
      };
      offset = next;
    }
    Ok(ReferenceConditionValue::Expanded(expanded))
  }

  fn enter_nested(&mut self) -> Result<(), ProjectError> {
    if self.depth == MAX_REFERENCE_CONDITION_DEPTH {
      return Err(self.unsupported(&format!("exceeds the {MAX_REFERENCE_CONDITION_DEPTH}-level nesting limit")));
    }
    self.depth += 1;
    Ok(())
  }

  fn property_value(&self, property: &str) -> Result<&'context str, ProjectError> {
    if property.eq_ignore_ascii_case("TargetFramework") {
      Ok(self.context.target_framework)
    } else if property.eq_ignore_ascii_case("RuntimeIdentifier") {
      Ok(self.context.runtime_identifier.unwrap_or(""))
    } else if property.eq_ignore_ascii_case("Configuration") {
      Ok(self.context.configuration)
    } else {
      Err(self.unsupported(&format!("references unsupported property $({property})")))
    }
  }

  fn consume_keyword(&mut self, keyword: &str) -> bool {
    let original = self.position;
    self.skip_whitespace();
    let end = self.position + keyword.len();
    let matches = self.input.get(self.position..end).is_some_and(|value| value.eq_ignore_ascii_case(keyword))
      && self.input.as_bytes().get(end).is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_');
    if matches {
      self.position = end;
    } else {
      self.position = original;
    }
    matches
  }

  fn consume_text(&mut self, value: &str) -> bool {
    if self.input[self.position..].starts_with(value) {
      self.position += value.len();
      true
    } else {
      false
    }
  }

  fn consume_byte(&mut self, value: u8) -> bool {
    if self.input.as_bytes().get(self.position) == Some(&value) {
      self.position += 1;
      true
    } else {
      false
    }
  }

  fn skip_whitespace(&mut self) {
    while self.input.as_bytes().get(self.position).is_some_and(u8::is_ascii_whitespace) {
      self.position += 1;
    }
  }

  fn invalid(&self, message: &str) -> ProjectError {
    ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      self.path,
      format!("reference Condition {:?} {message}", self.input),
    )
  }

  fn unsupported(&self, message: &str) -> ProjectError {
    ProjectError::new(
      ProjectErrorKind::Unsupported,
      self.path,
      format!("reference Condition {:?} {message}", self.input),
    )
  }
}

fn reference_conditions_match(
  path: &Path,
  conditions: &[String],
  selected: RawReferenceConditions,
  context: &ReferenceConditionContext<'_>,
) -> Result<bool, ProjectError> {
  if selected.group != NO_REFERENCE_CONDITION && !ReferenceConditionParser::evaluate(path, &conditions[selected.group as usize], context)? {
    return Ok(false);
  }
  if selected.item != NO_REFERENCE_CONDITION && !ReferenceConditionParser::evaluate(path, &conditions[selected.item as usize], context)? {
    return Ok(false);
  }
  Ok(true)
}

fn required_property(path: &Path, name: &str, value: Option<String>) -> Result<String, ProjectError> {
  value
    .filter(|value| !value.is_empty() && !value.contains("$("))
    .ok_or_else(|| ProjectError::new(ProjectErrorKind::InvalidProperty, path, format!("{name} must be a non-empty literal")))
}

fn property(path: &Path, name: &str) -> Result<Property, ProjectError> {
  match name {
    "TargetFramework" => Ok(Property::TargetFramework),
    "TargetFrameworks" => Ok(Property::TargetFrameworks),
    "RuntimeIdentifier" => Ok(Property::RuntimeIdentifier),
    "RuntimeIdentifiers" => Ok(Property::RuntimeIdentifiers),
    "OutputType" => Ok(Property::OutputType),
    "Nullable" => Ok(Property::Nullable),
    "ImplicitUsings" => Ok(Property::ImplicitUsings),
    "AssemblyName" => Ok(Property::AssemblyName),
    "RootNamespace" => Ok(Property::RootNamespace),
    "Deterministic" => Ok(Property::Deterministic),
    "RuntimeFrameworkVersion" => Ok(Property::RuntimeFrameworkVersion),
    "TargetLatestRuntimePatch" => Ok(Property::TargetLatestRuntimePatch),
    "RollForward" => Ok(Property::RollForward),
    "SelfContained" => Ok(Property::SelfContained),
    "NuGetAudit" => Ok(Property::NugetAudit),
    "NuGetAuditMode" => Ok(Property::NugetAuditMode),
    "NuGetAuditLevel" => Ok(Property::NugetAuditLevel),
    "RestoreEnablePackagePruning" => Ok(Property::RestoreEnablePackagePruning),
    "AllowMissingPrunePackageData" => Ok(Property::AllowMissingPrunePackageData),
    "ManagePackageVersionsCentrally" => Ok(Property::ManagePackageVersionsCentrally),
    "CentralPackageTransitivePinningEnabled" => Ok(Property::CentralPackageTransitivePinningEnabled),
    "CentralPackageVersionOverrideEnabled" => Ok(Property::CentralPackageVersionOverrideEnabled),
    _ => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("property {name} is outside the initial project compatibility contract"),
    )),
  }
}

fn validate_attributes(path: &Path, reader: &Reader<&[u8]>, element: &BytesStart<'_>, allowed: &[&str]) -> Result<(), ProjectError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid XML attribute: {error}")))?;
    let name = element_name(path, attribute.key.as_ref())?;
    if name == "Condition" && !allowed.contains(&name) {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        "MSBuild Condition evaluation is not supported yet",
      ));
    }
    if !allowed.contains(&name) {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        format!("attribute {name} is outside the initial project compatibility contract"),
      ));
    }
    attribute
      .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
      .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid {name} attribute: {error}")))?;
  }
  Ok(())
}

fn required_attribute(path: &Path, reader: &Reader<&[u8]>, element: &BytesStart<'_>, required: &str, additional: &[&str]) -> Result<String, ProjectError> {
  let mut value = None;
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid XML attribute: {error}")))?;
    let name = element_name(path, attribute.key.as_ref())?;
    if name == "Condition" && !additional.contains(&name) {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        "MSBuild Condition evaluation is not supported yet",
      ));
    }
    if name != required && !additional.contains(&name) {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        format!("attribute {name} is outside the initial project compatibility contract"),
      ));
    }
    let decoded = attribute
      .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
      .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid {name} attribute: {error}")))?;
    if name == required && value.replace(decoded.into_owned()).is_some() {
      return Err(ProjectError::new(
        ProjectErrorKind::InvalidXml,
        path,
        format!("attribute {required} appears more than once"),
      ));
    }
  }
  value.ok_or_else(|| ProjectError::new(ProjectErrorKind::InvalidProperty, path, format!("element requires a {required} attribute")))
}

fn optional_attribute(path: &Path, reader: &Reader<&[u8]>, element: &BytesStart<'_>, requested: &str) -> Result<Option<String>, ProjectError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid XML attribute: {error}")))?;
    let name = element_name(path, attribute.key.as_ref())?;
    if name == requested {
      return attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
        .map(|value| Some(value.into_owned()))
        .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid {name} attribute: {error}")));
    }
  }
  Ok(None)
}

fn store_condition(path: &Path, reader: &Reader<&[u8]>, element: &BytesStart<'_>, raw: &mut RawProject) -> Result<u32, ProjectError> {
  store_condition_value(path, reader, element, &mut raw.conditions)
}

fn store_condition_value(path: &Path, reader: &Reader<&[u8]>, element: &BytesStart<'_>, conditions: &mut Vec<String>) -> Result<u32, ProjectError> {
  let Some(condition) = optional_attribute(path, reader, element, "Condition")? else {
    return Ok(NO_REFERENCE_CONDITION);
  };
  if condition.trim().is_empty() {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      "reference Condition must not be empty",
    ));
  }
  if condition.len() > MAX_REFERENCE_CONDITION_BYTES {
    return Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("reference Condition exceeds the {MAX_REFERENCE_CONDITION_BYTES}-byte evaluation limit"),
    ));
  }
  let index = u32::try_from(conditions.len())
    .map_err(|_| ProjectError::new(ProjectErrorKind::Unsupported, path, "project contains more than 4 billion reference conditions"))?;
  conditions.push(condition);
  Ok(index)
}

fn append_reference(path: &Path, reference: &BytesRef<'_>, output: &mut String) -> Result<(), ProjectError> {
  if let Some(character) = reference
    .resolve_char_ref()
    .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid character reference: {error}")))?
  {
    output.push(character);
    return Ok(());
  }
  let name = reference
    .decode()
    .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid entity reference: {error}")))?;
  let value = match name.as_ref() {
    "amp" => '&',
    "lt" => '<',
    "gt" => '>',
    "apos" => '\'',
    "quot" => '"',
    _ => {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        format!("custom XML entity &{name}; is unsupported"),
      ));
    },
  };
  output.push(value);
  Ok(())
}

fn normalize_project_reference(path: &Path, value: &str) -> Result<String, ProjectError> {
  if value.is_empty() || value.contains("$(") || value.contains('*') || value.contains('?') {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("ProjectReference Include {value:?} must be one literal path"),
    ));
  }
  let normalized = value.replace('\\', "/");
  if !normalized.to_ascii_lowercase().ends_with(".csproj") {
    return Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("project reference {value:?} is not a C# project"),
    ));
  }
  Ok(normalized)
}

fn parse_package_assets(path: &Path, name: &str, value: Option<&str>, default: PackageAssetFlags) -> Result<PackageAssetFlags, ProjectError> {
  let Some(value) = value else {
    return Ok(default);
  };
  if value.contains("$(") {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("{name} must be a literal asset list"),
    ));
  }
  let mut flags = PackageAssetFlags::NONE;
  let mut token_count = 0usize;
  let mut contains_all_or_none = false;
  for asset in value.split([',', ';']).map(str::trim).filter(|asset| !asset.is_empty()) {
    token_count += 1;
    let selected = if asset.eq_ignore_ascii_case("none") {
      contains_all_or_none = true;
      PackageAssetFlags::NONE
    } else if asset.eq_ignore_ascii_case("all") {
      contains_all_or_none = true;
      PackageAssetFlags::ALL
    } else if asset.eq_ignore_ascii_case("runtime") {
      PackageAssetFlags::RUNTIME
    } else if asset.eq_ignore_ascii_case("compile") {
      PackageAssetFlags::COMPILE
    } else if asset.eq_ignore_ascii_case("build") {
      PackageAssetFlags::BUILD
    } else if asset.eq_ignore_ascii_case("buildmultitargeting") {
      PackageAssetFlags::BUILD_MULTI_TARGETING
    } else if asset.eq_ignore_ascii_case("buildtransitive") {
      PackageAssetFlags::BUILD_TRANSITIVE.union(PackageAssetFlags::BUILD)
    } else if asset.eq_ignore_ascii_case("native") {
      PackageAssetFlags::NATIVE
    } else if asset.eq_ignore_ascii_case("contentfiles") {
      PackageAssetFlags::CONTENT_FILES
    } else if asset.eq_ignore_ascii_case("analyzers") {
      PackageAssetFlags::ANALYZERS
    } else {
      return Err(ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        path,
        format!("{name} contains unsupported asset {asset:?}"),
      ));
    };
    flags = flags.union(selected);
  }
  if token_count == 0 {
    return Ok(default);
  }
  if token_count != 1 && contains_all_or_none {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("{name} values all and none must appear by themselves"),
    ));
  }
  Ok(flags)
}

fn optional_literal_metadata<'a>(path: &Path, name: &str, value: Option<&'a str>) -> Result<Option<&'a str>, ProjectError> {
  match value {
    Some(value) if value.contains("$(") => Err(ProjectError::new(ProjectErrorKind::InvalidProperty, path, format!("{name} must be literal"))),
    Some(value) if value.trim().is_empty() => Ok(None),
    Some(value) => Ok(Some(value.trim())),
    None => Ok(None),
  }
}

fn is_literal_package_version(value: &str) -> bool {
  !value.is_empty() && value.len() <= 256 && !value.contains("$(")
}

fn is_csproj(path: &Path) -> bool {
  path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("csproj"))
}

fn workspace_candidate_kind(path: &Path) -> Option<WorkspaceCandidateKind> {
  let extension = path.extension()?;
  if extension.eq_ignore_ascii_case("csproj") {
    Some(WorkspaceCandidateKind::CSharpProject)
  } else if extension.eq_ignore_ascii_case("fsproj") {
    Some(WorkspaceCandidateKind::FSharpProject)
  } else if extension.eq_ignore_ascii_case("vbproj") {
    Some(WorkspaceCandidateKind::VisualBasicProject)
  } else if extension.eq_ignore_ascii_case("sln") {
    Some(WorkspaceCandidateKind::Solution)
  } else if extension.eq_ignore_ascii_case("slnx") {
    Some(WorkspaceCandidateKind::XmlSolution)
  } else {
    None
  }
}

fn absolute_path(path: &Path) -> Result<PathBuf, ProjectError> {
  let absolute = if path.is_absolute() {
    path.to_owned()
  } else {
    std::path::absolute(path).map_err(|error| io_error("resolve", path, error))?
  };
  Ok(absolute.components().collect())
}

fn unicode_path<'a>(path: &'a Path, project_path: &Path) -> Result<&'a str, ProjectError> {
  path.to_str().ok_or_else(|| {
    ProjectError::new(
      ProjectErrorKind::NonUnicodePath,
      project_path,
      format!("path {} is not valid Unicode", path.display()),
    )
  })
}

fn portable_path(path: &Path, project_path: &Path) -> Result<String, ProjectError> {
  let mut output = String::new();
  for component in path.components() {
    if !output.is_empty() {
      output.push('/');
    }
    match component {
      Component::Normal(value) => output.push_str(value.to_str().ok_or_else(|| {
        ProjectError::new(
          ProjectErrorKind::NonUnicodePath,
          project_path,
          format!("source path {} is not valid Unicode", path.display()),
        )
      })?),
      _ => {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          project_path,
          format!("source path {} is not relative to its project", path.display()),
        ));
      },
    }
  }
  Ok(output)
}

fn element_name<'a>(path: &Path, name: &'a [u8]) -> Result<&'a str, ProjectError> {
  std::str::from_utf8(name).map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("element name is not UTF-8: {error}")))
}

fn io_error(operation: &str, path: &Path, error: std::io::Error) -> ProjectError {
  ProjectError::new(ProjectErrorKind::Io, path, format!("failed to {operation} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
  use std::{
    env,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
  };

  use super::*;

  static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
      let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let path = env::temp_dir().join(format!("dv-project-test-{}-{time}-{nonce}", std::process::id()));
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

  fn project_xml(properties: &str, items: &str) -> String {
    format!(
      r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net10.0</TargetFramework>
    {properties}
  </PropertyGroup>
  {items}
</Project>"#
    )
  }

  #[test]
  fn workspace_inventory_packs_every_candidate_kind_in_stable_path_order() {
    let temp = TempDirectory::new();
    temp.write("E.slnx", "<Solution />");
    temp.write("A.csproj", "<Project />");
    temp.write("D.sln", "");
    temp.write("C.VBPROJ", "<Project />");
    temp.write("B.FsPrOj", "<Project />");
    temp.write("notes.txt", "ignored");
    temp.write("nested/Ignored.csproj", "<Project />");

    let inventory = discover_workspace(&temp.0).unwrap();
    let rows = inventory
      .candidates()
      .iter()
      .copied()
      .map(|candidate| (inventory.path(candidate), inventory.kind(candidate)))
      .collect::<Vec<_>>();

    assert_eq!(inventory.root(), temp.0);
    assert_eq!(
      rows,
      [
        ("A.csproj", WorkspaceCandidateKind::CSharpProject),
        ("B.FsPrOj", WorkspaceCandidateKind::FSharpProject),
        ("C.VBPROJ", WorkspaceCandidateKind::VisualBasicProject),
        ("D.sln", WorkspaceCandidateKind::Solution),
        ("E.slnx", WorkspaceCandidateKind::XmlSolution),
      ]
    );
    assert_eq!(inventory.working_set_bytes(), 5 * size_of::<WorkspaceCandidate>() + 35);
  }

  #[cfg(target_os = "linux")]
  #[test]
  fn workspace_inventory_rejects_a_recognized_non_unicode_candidate() {
    use std::os::unix::ffi::OsStringExt;

    let temp = TempDirectory::new();
    let name = std::ffi::OsString::from_vec(vec![b'A', 0xff, b'.', b'c', b's', b'p', b'r', b'o', b'j']);
    fs::write(temp.0.join(name), b"<Project />").unwrap();

    let error = discover_workspace(&temp.0).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::NonUnicodePath);
  }

  #[test]
  fn empty_missing_and_non_directory_workspace_roots_have_explicit_boundaries() {
    let temp = TempDirectory::new();
    let empty = temp.0.join("empty");
    fs::create_dir(&empty).unwrap();
    let inventory = discover_workspace(&empty).unwrap();
    assert!(inventory.candidates().is_empty());
    assert_eq!(inventory.paths.capacity(), 0);
    assert_eq!(inventory.candidates.capacity(), 0);
    assert_eq!(inventory.working_set_bytes(), 0);

    let missing = temp.0.join("missing");
    assert_eq!(discover_workspace(&missing).unwrap_err().kind(), ProjectErrorKind::NotFound);
    let file = temp.write("not-a-directory.csproj", "<Project />");
    assert_eq!(discover_workspace(&file).unwrap_err().kind(), ProjectErrorKind::Unsupported);
  }

  #[test]
  fn implicit_selection_reports_every_candidate_in_stable_order() {
    let temp = TempDirectory::new();
    temp.write("B.sln", "");
    temp.write("A.csproj", "<Project />");

    let error = evaluate_project(&temp.0, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::Ambiguous);
    let message = error.to_string();
    assert!(message.contains("A.csproj (C# project), B.sln (solution)"), "{message}");
    assert!(message.contains("pass one project or solution path explicitly"), "{message}");
  }

  #[test]
  fn implicit_selection_rejects_one_unsupported_candidate_kind() {
    let temp = TempDirectory::new();
    let solution = temp.write("App.slnx", "<Solution />");

    let error = evaluate_project(&temp.0, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::Unsupported);
    assert_eq!(error.path(), solution);
    assert!(error.to_string().contains("XML solution App.slnx"));
  }

  #[test]
  fn discovers_and_evaluates_one_project_into_compact_batches() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "class Program {}");
    temp.write("Features/Other.cs", "class Other {}");
    temp.write("obj/Generated.g.cs", "class Generated {}");
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "<OutputType>Exe</OutputType><Nullable>enable</Nullable><ImplicitUsings>enable</ImplicitUsings>",
        r#"<ItemGroup><PackageReference Include="Example.Package" Version="1.2.3" /></ItemGroup>"#,
      ),
    );

    let result = evaluate_project(&temp.0, ProjectConfiguration::Release).unwrap();

    assert_eq!(result.project_path(), project);
    assert_eq!(result.configuration(), ProjectConfiguration::Release);
    assert_eq!(result.output_type(), ProjectOutputType::Exe);
    assert_eq!(result.assembly_name(), "App");
    assert_eq!(result.root_namespace(), "App");
    assert_eq!(result.runtime_identifier(), None);
    assert_eq!(result.runtime_dimensions().len(), 0);
    assert!(result.nullable_enabled());
    assert!(result.implicit_usings_enabled());
    assert!(result.deterministic());
    assert_eq!(result.sources().collect::<Vec<_>>(), ["Features/Other.cs", "Program.cs"]);
    let package = result.package_references()[0];
    assert_eq!(result.package_id(package), "Example.Package");
    assert_eq!(result.package_version(package), "1.2.3");
  }

  #[test]
  fn retains_a_recognized_legacy_target_for_package_restore() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "Legacy.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net48</TargetFramework></PropertyGroup></Project>"#,
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();

    assert_eq!(result.target_framework(), "net48");
    assert!(!result.target().is_modern_net());
  }

  #[test]
  fn evaluates_package_reference_policy_into_typed_inline_fields() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "",
        r#"<ItemGroup><PackageReference Include="Example.Package" Version="1.2.3"><IncludeAssets>compile;runtime;buildMultitargeting</IncludeAssets><ExcludeAssets>runtime</ExcludeAssets><PrivateAssets>all</PrivateAssets><NoWarn>NU1603;NU1701</NoWarn><Aliases>ExampleAlias</Aliases><GeneratePathProperty>true</GeneratePathProperty></PackageReference></ItemGroup>"#,
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();
    let package = result.package_references()[0];

    assert!(result.package_include_assets(package).contains(PackageAssetFlags::COMPILE));
    assert!(result.package_include_assets(package).contains(PackageAssetFlags::RUNTIME));
    assert!(result.package_include_assets(package).contains(PackageAssetFlags::BUILD_MULTI_TARGETING));
    assert_eq!(result.package_exclude_assets(package), PackageAssetFlags::RUNTIME);
    assert_eq!(
      result.package_effective_assets(package),
      PackageAssetFlags::COMPILE.union(PackageAssetFlags::BUILD_MULTI_TARGETING)
    );
    assert_eq!(result.package_private_assets(package), PackageAssetFlags::ALL);
    assert_eq!(result.package_no_warn(package), Some("NU1603;NU1701"));
    assert_eq!(result.package_aliases(package), Some("ExampleAlias"));
    assert!(result.package_generate_path_property(package));
  }

  #[test]
  fn defaults_package_reference_policy_to_nuget_values() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      &project_xml("", r#"<ItemGroup><PackageReference Include="Example.Package" Version="1.2.3" /></ItemGroup>"#),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();
    let package = result.package_references()[0];

    assert_eq!(result.package_include_assets(package), PackageAssetFlags::ALL);
    assert_eq!(result.package_exclude_assets(package), PackageAssetFlags::NONE);
    assert_eq!(result.package_private_assets(package), PackageAssetFlags::DEFAULT_PRIVATE);
    assert_eq!(result.package_no_warn(package), None);
    assert_eq!(result.package_aliases(package), None);
    assert!(!result.package_generate_path_property(package));
  }

  #[test]
  fn rejects_ambiguous_or_unknown_package_asset_lists() {
    let temp = TempDirectory::new();
    let mixed = temp.write(
      "Mixed.csproj",
      &project_xml(
        "",
        r#"<ItemGroup><PackageReference Include="Example.Package" Version="1.2.3" IncludeAssets="all;compile" /></ItemGroup>"#,
      ),
    );
    let unknown = temp.write(
      "Unknown.csproj",
      &project_xml(
        "",
        r#"<ItemGroup><PackageReference Include="Example.Package" Version="1.2.3" PrivateAssets="telepathy" /></ItemGroup>"#,
      ),
    );

    let mixed = evaluate_project_path(&mixed, ProjectConfiguration::Debug).unwrap_err();
    let unknown = evaluate_project_path(&unknown, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(mixed.kind(), ProjectErrorKind::InvalidProperty);
    assert!(mixed.to_string().contains("must appear by themselves"));
    assert_eq!(unknown.kind(), ProjectErrorKind::InvalidProperty);
    assert!(unknown.to_string().contains("unsupported asset"));
  }

  #[test]
  fn materializes_runtime_targets_as_one_compact_dimension_batch() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "<RuntimeIdentifier>osx-arm64</RuntimeIdentifier><RuntimeIdentifiers>win-x64; linux-x64;win-x64;</RuntimeIdentifiers>",
        "",
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();

    assert_eq!(result.runtime_identifier(), Some("osx-arm64"));
    assert_eq!(result.runtime_identifiers().collect::<Vec<_>>(), ["win-x64", "linux-x64"]);
    assert_eq!(result.runtime_dimensions().collect::<Vec<_>>(), ["win-x64", "linux-x64", "osx-arm64"]);
    assert_eq!(result.text.matches("win-x64").count(), 1);
    assert_eq!(result.text.matches("osx-arm64").count(), 1);
  }

  #[test]
  fn captures_framework_reference_versions_and_runtime_policy() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "<RuntimeFrameworkVersion>10.0.1</RuntimeFrameworkVersion><TargetLatestRuntimePatch>false</TargetLatestRuntimePatch><RollForward>LatestMinor</RollForward>",
        r#"<ItemGroup><FrameworkReference Include="Microsoft.AspNetCore.App" TargetingPackVersion="10.0.2"><RuntimeFrameworkVersion>10.0.3</RuntimeFrameworkVersion><TargetLatestRuntimePatch>true</TargetLatestRuntimePatch></FrameworkReference></ItemGroup>"#,
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();
    let reference = result.framework_references()[0];

    assert_eq!(result.framework_reference_id(reference), "Microsoft.AspNetCore.App");
    assert_eq!(result.framework_runtime_version(reference), Some("10.0.3"));
    assert_eq!(result.framework_targeting_pack_version(reference), Some("10.0.2"));
    assert_eq!(result.framework_target_latest_runtime_patch(reference), Some(true));
    assert_eq!(result.runtime_framework_version(), Some("10.0.1"));
    assert_eq!(result.target_latest_runtime_patch(), Some(false));
    assert_eq!(result.roll_forward(), RuntimeRollForward::LatestMinor);
    assert!(!result.self_contained());
  }

  #[test]
  fn evaluates_typed_nuget_audit_policy_and_net10_defaults() {
    let temp = TempDirectory::new();
    let explicit = temp.write(
      "Explicit.csproj",
      &project_xml(
        "<NuGetAudit>false</NuGetAudit><NuGetAuditMode>direct</NuGetAuditMode><NuGetAuditLevel>critical</NuGetAuditLevel>",
        "",
      ),
    );
    let defaulted = temp.write("Defaulted.csproj", &project_xml("", ""));

    let explicit = evaluate_project_path(&explicit, ProjectConfiguration::Debug).unwrap();
    let defaulted = evaluate_project_path(&defaulted, ProjectConfiguration::Debug).unwrap();

    assert!(!explicit.nuget_audit_enabled());
    assert_eq!(explicit.nuget_audit_mode(), NugetAuditMode::Direct);
    assert_eq!(explicit.nuget_audit_level(), NugetAuditLevel::Critical);
    assert!(defaulted.nuget_audit_enabled());
    assert_eq!(defaulted.nuget_audit_mode(), NugetAuditMode::All);
    assert_eq!(defaulted.nuget_audit_level(), NugetAuditLevel::Low);
  }

  #[test]
  fn evaluates_package_pruning_defaults_and_explicit_legacy_opt_in() {
    let temp = TempDirectory::new();
    let net10 = temp.write("Net10.csproj", &project_xml("", ""));
    let net9 = temp.write(
      "Net9.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net9.0</TargetFramework></PropertyGroup></Project>"#,
    );
    let opted_in = temp.write(
      "Net9OptIn.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net9.0</TargetFramework><RestoreEnablePackagePruning>true</RestoreEnablePackagePruning><AllowMissingPrunePackageData>true</AllowMissingPrunePackageData></PropertyGroup></Project>"#,
    );

    let net10 = evaluate_project_path(&net10, ProjectConfiguration::Debug).unwrap();
    let net9 = evaluate_project_path(&net9, ProjectConfiguration::Debug).unwrap();
    let opted_in = evaluate_project_path(&opted_in, ProjectConfiguration::Debug).unwrap();

    assert!(net10.restore_package_pruning_enabled());
    assert!(!net9.restore_package_pruning_enabled());
    assert!(opted_in.restore_package_pruning_enabled());
    assert!(opted_in.allow_missing_prune_package_data());
  }

  #[test]
  fn selected_runtime_reuses_the_plural_dimension_span() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "<RuntimeIdentifier>win-x64</RuntimeIdentifier><RuntimeIdentifiers>win-x64;linux-x64</RuntimeIdentifiers>",
        "",
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();

    assert_eq!(result.runtime_identifier(), Some("win-x64"));
    assert_eq!(result.runtime_dimensions().collect::<Vec<_>>(), ["win-x64", "linux-x64"]);
    assert_eq!(result.runtime_identifier_index, 0);
    assert_eq!(result.text.matches("win-x64").count(), 1);
  }

  #[test]
  fn rejects_dynamic_runtime_dimensions() {
    let temp = TempDirectory::new();
    let project = temp.write("App.csproj", &project_xml("<RuntimeIdentifiers>win-x64;$(OtherRids)</RuntimeIdentifiers>", ""));

    let error = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::InvalidProperty);
    assert!(error.to_string().contains("literal"));
  }

  #[test]
  fn captures_project_references_and_nested_package_versions() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    temp.write("Lib/Lib.csproj", &project_xml("", ""));
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "",
        r#"<ItemGroup>
          <ProjectReference Include="Lib\Lib.csproj" />
          <PackageReference Include="Example.Package"><Version>2.0.0-preview.1</Version></PackageReference>
        </ItemGroup>"#,
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();

    assert_eq!(result.project_references().collect::<Vec<_>>(), ["Lib/Lib.csproj"]);
    let package = result.package_references()[0];
    assert_eq!(result.package_id(package), "Example.Package");
    assert_eq!(result.package_version(package), "2.0.0-preview.1");
  }

  #[test]
  fn evaluates_a_project_reference_closure_once_in_root_first_order() {
    let temp = TempDirectory::new();
    temp.write("Shared/Shared.csproj", &project_xml("", ""));
    temp.write(
      "Left/Left.csproj",
      &project_xml("", r#"<ItemGroup><ProjectReference Include="../Shared/Shared.csproj" /></ItemGroup>"#),
    );
    temp.write(
      "Right/Right.csproj",
      &project_xml("", r#"<ItemGroup><ProjectReference Include="../Shared/Shared.csproj" /></ItemGroup>"#),
    );
    let root = temp.write(
      "App.csproj",
      &project_xml(
        "",
        r#"<ItemGroup><ProjectReference Include="Left/Left.csproj" /><ProjectReference Include="Right/Right.csproj" /></ItemGroup>"#,
      ),
    );

    let root = evaluate_project_path(&root, ProjectConfiguration::Release).unwrap();
    let projects = evaluate_project_closure(root).unwrap();
    let names = projects
      .iter()
      .map(|project| project.project_path().file_name().unwrap().to_string_lossy().into_owned())
      .collect::<Vec<_>>();

    assert_eq!(names, ["App.csproj", "Left.csproj", "Right.csproj", "Shared.csproj"]);
    assert!(projects.iter().all(|project| project.configuration() == ProjectConfiguration::Release));
  }

  #[test]
  fn filters_reference_batches_by_framework_runtime_and_configuration() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "<RuntimeIdentifier>win-x64</RuntimeIdentifier>",
        r#"<ItemGroup Condition="'$(TargetFramework)' == 'net10.0' And '$(Configuration)' == 'Release'">
          <PackageReference Include="Selected.Group" Version="1.0.0" />
          <PackageReference Include="Excluded.Runtime" Condition="'$(RuntimeIdentifier)' == 'linux-x64'" />
          <ProjectReference Include="Selected\Library.csproj" Condition="'$(TargetFramework)|$(RuntimeIdentifier)' == 'net10.0|win-x64'" />
        </ItemGroup>
        <ItemGroup>
          <FrameworkReference Include="Microsoft.AspNetCore.App" Condition="('$(Configuration)' == 'Debug') Or ('$(RuntimeIdentifier)' == 'win-x64')" />
          <PackageReference Include="Excluded.Configuration" Condition="'$(Configuration)' == 'Debug'" />
        </ItemGroup>"#,
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Release).unwrap();

    assert_eq!(result.project_references().collect::<Vec<_>>(), ["Selected/Library.csproj"]);
    assert_eq!(result.package_references().len(), 1);
    assert_eq!(result.package_id(result.package_references()[0]), "Selected.Group");
    assert_eq!(result.framework_references().len(), 1);
    assert_eq!(result.framework_reference_id(result.framework_references()[0]), "Microsoft.AspNetCore.App");
  }

  #[test]
  fn applies_and_before_or_and_supports_boolean_negation() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "<RuntimeIdentifier>win-x64</RuntimeIdentifier>",
        r#"<ItemGroup>
          <PackageReference Include="Selected.Precedence" Version="1.0.0" Condition="'$(Configuration)' == 'Release' Or '$(Configuration)' == 'Debug' And '$(RuntimeIdentifier)' == 'linux-x64'" />
          <PackageReference Include="Excluded.Precedence" Version="1.0.0" Condition="'$(Configuration)' == 'Debug' Or '$(Configuration)' == 'Release' And '$(RuntimeIdentifier)' == 'linux-x64'" />
          <PackageReference Include="Selected.Negation" Version="1.0.0" Condition="!('$(Configuration)' == 'Debug')" />
          <PackageReference Include="Selected.Debug" Version="1.0.0" Condition="'$(Configuration)' == 'Debug'" />
        </ItemGroup>"#,
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Release).unwrap();
    let ids = result
      .package_references()
      .iter()
      .map(|reference| result.package_id(*reference))
      .collect::<Vec<_>>();

    assert_eq!(ids, ["Selected.Precedence", "Selected.Negation"]);

    let debug = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();
    let debug_ids = debug
      .package_references()
      .iter()
      .map(|reference| debug.package_id(*reference))
      .collect::<Vec<_>>();
    assert_eq!(debug_ids, ["Excluded.Precedence", "Selected.Debug"]);
  }

  #[test]
  fn treats_an_unselected_runtime_identifier_as_empty_text() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "",
        r#"<ItemGroup><PackageReference Include="Portable" Version="1.0.0" Condition="'$(RuntimeIdentifier)' == ''" /></ItemGroup>"#,
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Release).unwrap();

    assert_eq!(result.package_references().len(), 1);
    assert_eq!(result.package_id(result.package_references()[0]), "Portable");
  }

  #[test]
  fn rejects_unbounded_or_unknown_reference_conditions() {
    let temp = TempDirectory::new();
    let too_many = std::iter::repeat_n("'$(Configuration)' == 'Release'", MAX_REFERENCE_CONDITION_OPERATORS as usize + 1)
      .collect::<Vec<_>>()
      .join(" Or ");
    let too_long = "x".repeat(MAX_REFERENCE_CONDITION_BYTES + 1);
    let too_deep = format!(
      "{}true{}",
      "!(".repeat(MAX_REFERENCE_CONDITION_DEPTH as usize + 1),
      ")".repeat(MAX_REFERENCE_CONDITION_DEPTH as usize + 1)
    );
    let cases = [
      ("Unknown.csproj", "'$(Unknown)' == 'value'", ProjectErrorKind::Unsupported),
      ("Unterminated.csproj", "'$(Configuration)", ProjectErrorKind::InvalidProperty),
      ("Empty.csproj", "", ProjectErrorKind::InvalidProperty),
      ("Long.csproj", &too_long, ProjectErrorKind::Unsupported),
      ("Wide.csproj", &too_many, ProjectErrorKind::Unsupported),
      ("Deep.csproj", &too_deep, ProjectErrorKind::Unsupported),
    ];

    for (name, condition, expected) in cases {
      let project = temp.write(
        name,
        &project_xml(
          "",
          &format!(r#"<ItemGroup><PackageReference Include="Example" Version="1.0.0" Condition="{condition}" /></ItemGroup>"#),
        ),
      );

      let error = evaluate_project_path(&project, ProjectConfiguration::Release).unwrap_err();
      assert_eq!(error.kind(), expected, "{name}: {error}");
      assert!(error.to_string().contains("Condition"), "{name}: {error}");
    }
  }

  #[test]
  fn materializes_package_reference_policy_from_attributes_and_children() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "",
        r#"<ItemGroup>
          <PackageReference Include="Attribute.Package" Version="1.2.3" IncludeAssets="compile;runtime;" ExcludeAssets="runtime" PrivateAssets="all" NoWarn="NU1603;NU1701" Aliases="AttributeAlias" GeneratePathProperty="true" />
          <PackageReference Include="Child.Package">
            <Version>2.0.0</Version>
            <IncludeAssets>compile;buildTransitive</IncludeAssets>
            <ExcludeAssets>native</ExcludeAssets>
            <PrivateAssets>contentFiles;analyzers</PrivateAssets>
            <NoWarn> NU1901,NU1902 </NoWarn>
            <Aliases>ChildAlias</Aliases>
            <GeneratePathProperty>true</GeneratePathProperty>
          </PackageReference>
          <PackageReference Include="Default.Package" Version="3.0.0" />
        </ItemGroup>"#,
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();
    let attribute = result.package_references()[0];
    let child = result.package_references()[1];
    let defaulted = result.package_references()[2];

    assert_eq!(
      result.package_include_assets(attribute),
      PackageAssetFlags::COMPILE.union(PackageAssetFlags::RUNTIME)
    );
    assert_eq!(result.package_exclude_assets(attribute), PackageAssetFlags::RUNTIME);
    assert_eq!(result.package_effective_assets(attribute), PackageAssetFlags::COMPILE);
    assert_eq!(result.package_private_assets(attribute), PackageAssetFlags::ALL);
    assert_eq!(result.package_no_warn(attribute), Some("NU1603;NU1701"));
    assert_eq!(result.package_aliases(attribute), Some("AttributeAlias"));
    assert!(result.package_generate_path_property(attribute));

    assert_eq!(
      result.package_include_assets(child),
      PackageAssetFlags::COMPILE
        .union(PackageAssetFlags::BUILD)
        .union(PackageAssetFlags::BUILD_TRANSITIVE)
    );
    assert_eq!(result.package_exclude_assets(child), PackageAssetFlags::NATIVE);
    assert_eq!(
      result.package_private_assets(child),
      PackageAssetFlags::CONTENT_FILES.union(PackageAssetFlags::ANALYZERS)
    );
    assert_eq!(result.package_no_warn(child), Some("NU1901,NU1902"));
    assert_eq!(result.package_aliases(child), Some("ChildAlias"));
    assert!(result.package_generate_path_property(child));

    assert_eq!(result.package_include_assets(defaulted), PackageAssetFlags::ALL);
    assert_eq!(result.package_exclude_assets(defaulted), PackageAssetFlags::NONE);
    assert_eq!(result.package_private_assets(defaulted), PackageAssetFlags::DEFAULT_PRIVATE);
    assert_eq!(result.package_no_warn(defaulted), None);
    assert_eq!(result.package_aliases(defaulted), None);
    assert!(!result.package_generate_path_property(defaulted));
  }

  #[test]
  fn selects_reference_batches_by_target_runtime_and_configuration() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "<RuntimeIdentifier>win-x64</RuntimeIdentifier>",
        r#"<ItemGroup Condition="'$(TargetFramework)|$(RuntimeIdentifier)' == 'NET10.0|WIN-X64' And !('$(Configuration)' != 'Release')">
          <ProjectReference Include="Lib/Release.csproj" Condition="true" />
          <PackageReference Include="Release.Package" Version="1.0.0" Condition="'$(Configuration)' == 'release'" />
          <FrameworkReference Include="Microsoft.AspNetCore.App" Condition="'$(RuntimeIdentifier)' != 'linux-x64'" />
        </ItemGroup>
        <ItemGroup Condition="'$(Configuration)' == 'Debug' Or '$(TargetFramework)' == 'net9.0'">
          <ProjectReference Include="Lib/Debug.csproj" />
          <PackageReference Include="Debug.Package" Version="2.0.0" />
        </ItemGroup>
        <ItemGroup Condition="false">
          <ProjectReference Include="$(InvalidProject)" />
          <PackageReference Include="Invalid.Package" />
          <FrameworkReference Include="$(InvalidFramework)" />
        </ItemGroup>"#,
      ),
    );

    let release = evaluate_project_path(&project, ProjectConfiguration::Release).unwrap();
    assert_eq!(release.project_references().collect::<Vec<_>>(), ["Lib/Release.csproj"]);
    assert_eq!(release.package_references().len(), 1);
    assert_eq!(release.package_id(release.package_references()[0]), "Release.Package");
    assert_eq!(release.framework_references().len(), 1);
    assert_eq!(release.framework_reference_id(release.framework_references()[0]), "Microsoft.AspNetCore.App");

    let debug = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();
    assert_eq!(debug.project_references().collect::<Vec<_>>(), ["Lib/Debug.csproj"]);
    assert_eq!(debug.package_references().len(), 1);
    assert_eq!(debug.package_id(debug.package_references()[0]), "Debug.Package");
    assert!(debug.framework_references().is_empty());
  }

  #[test]
  fn applies_central_versions_overrides_globals_and_transitive_pin_policy() {
    let temp = TempDirectory::new();
    temp.write(
      "Directory.Packages.props",
      r#"<Project>
  <PropertyGroup>
    <ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally>
    <CentralPackageTransitivePinningEnabled>true</CentralPackageTransitivePinningEnabled>
  </PropertyGroup>
  <ItemGroup>
    <PackageVersion Include="Direct.Package" Version="1.2.3" />
    <PackageVersion Include="Pinned.Package" Version="[2.0.0]" />
    <PackageVersion Include="Conditional.Package" Version="3.0.0" Condition="'$(TargetFramework)' == 'net10.0'" />
    <PackageVersion Include="Conditional.Package" Version="9.0.0" Condition="'$(TargetFramework)' == 'net9.0'" />
    <GlobalPackageReference Include="Global.Tool" Version="4.0.0" />
  </ItemGroup>
</Project>"#,
    );
    temp.write("src/Program.cs", "");
    let project = temp.write(
      "src/App.csproj",
      &project_xml(
        "",
        r#"<ItemGroup>
          <PackageReference Include="Direct.Package" />
          <PackageReference Include="Conditional.Package" VersionOverride="3.1.0" />
        </ItemGroup>"#,
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Release).unwrap();
    let packages = result
      .package_references()
      .iter()
      .map(|package| (result.package_id(*package), result.package_version(*package)))
      .collect::<Vec<_>>();
    assert_eq!(
      packages,
      [("Direct.Package", "1.2.3"), ("Conditional.Package", "3.1.0"), ("Global.Tool", "4.0.0")]
    );
    assert!(result.central_package_management_enabled());
    assert!(result.central_package_transitive_pinning_enabled());
    let central = result
      .central_package_versions()
      .iter()
      .map(|package| (result.central_package_id(*package), result.central_package_version(*package)))
      .collect::<Vec<_>>();
    assert_eq!(
      central,
      [
        ("Conditional.Package", "3.0.0"),
        ("Direct.Package", "1.2.3"),
        ("Global.Tool", "4.0.0"),
        ("Pinned.Package", "[2.0.0]")
      ]
    );
    let global = result.package_references()[2];
    assert_eq!(
      result.package_include_assets(global),
      PackageAssetFlags::RUNTIME
        .union(PackageAssetFlags::BUILD)
        .union(PackageAssetFlags::BUILD_MULTI_TARGETING)
        .union(PackageAssetFlags::NATIVE)
        .union(PackageAssetFlags::CONTENT_FILES)
        .union(PackageAssetFlags::ANALYZERS)
    );
    assert_eq!(result.package_private_assets(global), PackageAssetFlags::ALL);
  }

  #[test]
  fn rejects_invalid_central_package_contracts_before_resolution() {
    for (name, central_property, package) in [
      (
        "ProjectVersion",
        "<ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally>",
        r#"<PackageReference Include="Example" Version="1.0.0" />"#,
      ),
      (
        "MissingVersion",
        "<ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally>",
        r#"<PackageReference Include="Missing" />"#,
      ),
      (
        "DisabledOverride",
        "<ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally><CentralPackageVersionOverrideEnabled>false</CentralPackageVersionOverrideEnabled>",
        r#"<PackageReference Include="Example" VersionOverride="2.0.0" />"#,
      ),
    ] {
      let temp = TempDirectory::new();
      temp.write(
        "Directory.Packages.props",
        &format!(
          r#"<Project><PropertyGroup>{central_property}</PropertyGroup><ItemGroup><PackageVersion Include="Example" Version="1.0.0" /></ItemGroup></Project>"#
        ),
      );
      let project = temp.write(&format!("src/{name}.csproj"), &project_xml("", &format!("<ItemGroup>{package}</ItemGroup>")));
      let error = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap_err();
      assert_eq!(error.kind(), ProjectErrorKind::InvalidProperty, "{name}: {error}");
    }
  }

  #[test]
  fn version_override_wins_with_or_without_central_management() {
    let standalone = TempDirectory::new();
    let standalone_project = standalone.write(
      "Standalone.csproj",
      &project_xml(
        "",
        r#"<ItemGroup><PackageReference Include="Example" Version="1.0.0" VersionOverride="2.0.0" /></ItemGroup>"#,
      ),
    );
    let result = evaluate_project_path(&standalone_project, ProjectConfiguration::Debug).unwrap();
    assert_eq!(result.package_version(result.package_references()[0]), "2.0.0");

    let central = TempDirectory::new();
    central.write(
      "Directory.Packages.props",
      r#"<Project><PropertyGroup><ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally></PropertyGroup>
<ItemGroup><PackageVersion Include="Example" Version="1.0.0" /></ItemGroup></Project>"#,
    );
    let central_project = central.write(
      "Central.csproj",
      &project_xml(
        "",
        r#"<ItemGroup><PackageReference Include="Example" Version="1.5.0" VersionOverride="2.0.0" /></ItemGroup>"#,
      ),
    );
    let result = evaluate_project_path(&central_project, ProjectConfiguration::Debug).unwrap();
    assert_eq!(result.package_version(result.package_references()[0]), "2.0.0");
  }

  #[test]
  fn nearest_directory_packages_props_wins_without_merging_its_parent() {
    let temp = TempDirectory::new();
    temp.write(
      "Directory.Packages.props",
      r#"<Project><PropertyGroup><ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally></PropertyGroup>
<ItemGroup><PackageVersion Include="Example" Version="1.0.0" /><PackageVersion Include="Parent.Only" Version="9.0.0" /></ItemGroup></Project>"#,
    );
    temp.write(
      "child/Directory.Packages.props",
      r#"<Project><PropertyGroup><ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally></PropertyGroup>
<ItemGroup><PackageVersion Include="Example" Version="2.0.0" /></ItemGroup></Project>"#,
    );
    let project = temp.write(
      "child/src/App.csproj",
      &project_xml("", r#"<ItemGroup><PackageReference Include="Example" /></ItemGroup>"#),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();
    assert_eq!(result.package_version(result.package_references()[0]), "2.0.0");
    assert_eq!(result.central_package_versions().len(), 1);
    assert_eq!(result.central_package_id(result.central_package_versions()[0]), "Example");
  }

  #[test]
  fn rejects_reference_conditions_outside_the_bounded_dimension_grammar() {
    let temp = TempDirectory::new();
    for (index, condition) in ["'$(Other)' == 'value'", "'$(Configuration)' &lt; 'Release'", "Exists('other.props')"]
      .into_iter()
      .enumerate()
    {
      let project = temp.write(
        &format!("Unsupported{index}.csproj"),
        &project_xml(
          "",
          &format!(r#"<ItemGroup><PackageReference Include="Example" Version="1.0.0" Condition="{condition}" /></ItemGroup>"#),
        ),
      );
      let error = evaluate_project_path(&project, ProjectConfiguration::Release).unwrap_err();
      assert_eq!(error.kind(), ProjectErrorKind::Unsupported, "unexpected error for {condition}");
      assert!(error.to_string().contains("Condition"));
    }
  }

  #[test]
  fn rejects_ambiguous_or_dynamic_package_reference_policy() {
    let temp = TempDirectory::new();
    for (index, item) in [
      r#"<PackageReference Include="Example" Version="1.0.0" IncludeAssets="all;compile" />"#,
      r#"<PackageReference Include="Example" Version="1.0.0" ExcludeAssets="unknown" />"#,
      r#"<PackageReference Include="Example" Version="1.0.0" Aliases="$(Alias)" />"#,
      r#"<PackageReference Include="Example" Version="1.0.0" GeneratePathProperty="sometimes" />"#,
      r#"<PackageReference Include="Example" Version="1.0.0" PrivateAssets="all"><PrivateAssets>none</PrivateAssets></PackageReference>"#,
    ]
    .into_iter()
    .enumerate()
    {
      let project = temp.write(&format!("Invalid{index}.csproj"), &project_xml("", &format!("<ItemGroup>{item}</ItemGroup>")));
      let error = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap_err();
      assert_eq!(error.kind(), ProjectErrorKind::InvalidProperty, "unexpected error for {item}");
    }
  }

  #[test]
  fn implicit_selection_rejects_ambiguity() {
    let temp = TempDirectory::new();
    temp.write("A.csproj", &project_xml("", ""));
    temp.write("B.csproj", &project_xml("", ""));

    let error = evaluate_project(&temp.0, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::Ambiguous);
  }

  #[test]
  fn rejects_conditions_instead_of_guessing() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup Condition="'$(Configuration)' == 'Debug'"><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
    );

    let error = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::Unsupported);
    assert!(error.to_string().contains("Condition"));
  }

  #[test]
  fn rejects_multi_targeting() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFrameworks>net10.0;net9.0</TargetFrameworks></PropertyGroup></Project>"#,
    );

    let error = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::Unsupported);
    assert!(error.to_string().contains("multi-target"));
  }
}
