use std::{
  cmp::Ordering,
  collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
  env,
  error::Error,
  fmt::{self, Write as _},
  fs,
  io::{self, Read, Write},
  mem::{align_of, size_of},
  path::{Component, Path, PathBuf},
  sync::Arc,
  thread,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use quick_xml::{Reader, XmlVersion, events::Event};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use tokio::{io::AsyncWriteExt, task::JoinSet};
use zip::ZipArchive;

use crate::{FrameworkFamily, NugetAuditLevel, NugetAuditMode, ProjectSpec, TargetFramework, discover_sdks};

const DEFAULT_SOURCE: &str = "https://api.nuget.org/v3/index.json";
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PRUNE_DATA_BYTES: u64 = 1024 * 1024;
const MAX_PRUNE_PACKAGES: usize = 10_000;
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_DOWNLOAD_WORKERS: usize = 24;
const ASYNC_RUNTIME_WORKERS: usize = 2;
const MAX_EXTRACTION_WORKERS: usize = 4;
const MIN_PARALLEL_EXTRACTION_ENTRIES: usize = 8;
const MAX_GRAPH_REVISIONS: u32 = 64;
const PUBLISH_RETRY_DELAYS: [Duration; 3] = [Duration::from_millis(1), Duration::from_millis(4), Duration::from_millis(16)];
const LOCK_SCHEMA_VERSION: u16 = 3;
const SERVICE_CAPABILITY_COUNT: usize = 5;
const PACKAGE_BASE_TYPES: &[&str] = &["PackageBaseAddress/Versioned", "PackageBaseAddress/3.0.0"];
const REGISTRATION_TYPES: &[&str] = &[
  "RegistrationsBaseUrl/Versioned",
  "RegistrationsBaseUrl/3.6.0",
  "RegistrationsBaseUrl/3.4.0",
  "RegistrationsBaseUrl/3.0.0-rc",
  "RegistrationsBaseUrl/3.0.0-beta",
  "RegistrationsBaseUrl",
];
const SEARCH_TYPES: &[&str] = &["SearchQueryService/Versioned", "SearchQueryService/3.4.0", "SearchQueryService/3.0.0-beta"];
const VULNERABILITY_TYPES: &[&str] = &["VulnerabilityInfo/6.7.0"];
const PACKAGE_PUBLISH_TYPES: &[&str] = &["PackagePublish/Versioned", "PackagePublish/2.0.0"];
const NUGET_PROTOCOL_CLIENT_VERSION: &str = "7.0.0";

/// Policy applied when validating NuGet package signatures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignatureValidationMode {
  /// Unsigned packages are allowed and signature problems are warnings.
  Accept,
  /// Every package must satisfy the configured trusted-signers policy.
  Require,
}

impl SignatureValidationMode {
  /// Returns the canonical NuGet configuration spelling.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Accept => "accept",
      Self::Require => "require",
    }
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextSpan {
  start: u32,
  len: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ItemRange {
  start: u32,
  len: u32,
}

const _: () = assert!(size_of::<TextSpan>() == 8);
const _: () = assert!(align_of::<TextSpan>() == 4);
const _: () = assert!(size_of::<ItemRange>() == 8);
const _: () = assert!(align_of::<ItemRange>() == 4);

#[derive(Clone, Debug, Eq, PartialEq)]
struct NugetServiceEndpoints {
  text: Box<str>,
  entries: Box<[TextSpan]>,
  ranges: [ItemRange; SERVICE_CAPABILITY_COUNT],
}

impl NugetServiceEndpoints {
  fn package_base_address(&self) -> Option<&str> {
    self.values(ServiceCapability::PackageBase).next()
  }

  fn values(&self, capability: ServiceCapability) -> impl ExactSizeIterator<Item = &str> {
    self.entries[range(self.ranges[capability as usize])].iter().map(|span| {
      let start = span.start as usize;
      &self.text[start..start + span.len as usize]
    })
  }
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum ServiceCapability {
  PackageBase,
  Registration,
  Search,
  Vulnerability,
  PackagePublish,
}

const _: () = assert!(size_of::<NugetServiceEndpoints>() == 72);
const _: () = assert!(align_of::<NugetServiceEndpoints>() == 8);

/// Stable semantic partition for selected package assets.
///
/// Families are stored as consecutive ranges in this order. The enum is a
/// query key, not a persisted representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageAssetFamily {
  /// Assemblies passed to the compiler as references.
  Compile,
  /// Managed assemblies available to the application at runtime.
  Runtime,
  /// Roslyn analyzers and source generators.
  Analyzer,
  /// Satellite resource assemblies.
  Resource,
  /// Package content files.
  Content,
  /// Inner-build props and targets.
  Build,
  /// Outer-build props and targets.
  BuildMultiTargeting,
  /// Transitive props and targets.
  BuildTransitive,
  /// Legacy native runtime assets.
  Native,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageAssetRanges {
  compile: ItemRange,
  runtime: ItemRange,
  analyzers: ItemRange,
  resources: ItemRange,
  content: ItemRange,
  build: ItemRange,
  build_multi_targeting: ItemRange,
  build_transitive: ItemRange,
  native: ItemRange,
}

const _: () = assert!(size_of::<PackageAssetRanges>() == 72);
const _: () = assert!(align_of::<PackageAssetRanges>() == 4);

/// One compact package graph record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedPackage {
  id: TextSpan,
  version: TextSpan,
  dependencies: ItemRange,
  direct: bool,
}

const _: () = assert!(size_of::<ResolvedPackage>() == 28);
const _: () = assert!(align_of::<ResolvedPackage>() == 4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageAssets {
  hash: TextSpan,
  compile: ItemRange,
  runtime: ItemRange,
  analyzers: ItemRange,
}

const _: () = assert!(size_of::<PackageAssets>() == 32);
const _: () = assert!(align_of::<PackageAssets>() == 4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageExtendedAssets {
  resources: ItemRange,
  content_files: ItemRange,
  build: ItemRange,
  build_multi_targeting: ItemRange,
  build_transitive: ItemRange,
  native: ItemRange,
  runtime_targets: ItemRange,
}

const _: () = assert!(size_of::<PackageExtendedAssets>() == 56);
const _: () = assert!(align_of::<PackageExtendedAssets>() == 4);

/// The role assigned to an RID-specific runtime target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTargetKind {
  /// A managed runtime assembly.
  Runtime,
  /// A native library or executable.
  Native,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RuntimeTargetAsset {
  path: TextSpan,
  runtime_identifier: TextSpan,
  kind: RuntimeTargetKind,
}

const _: () = assert!(size_of::<RuntimeTargetAsset>() == 20);
const _: () = assert!(align_of::<RuntimeTargetAsset>() == 4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AssetFlags(u8);

impl AssetFlags {
  const NONE: Self = Self(0);
  const RUNTIME: Self = Self(1 << 0);
  const COMPILE: Self = Self(1 << 1);
  const BUILD: Self = Self(1 << 2);
  const NATIVE: Self = Self(1 << 3);
  const CONTENT_FILES: Self = Self(1 << 4);
  const ANALYZERS: Self = Self(1 << 5);
  const BUILD_TRANSITIVE: Self = Self(1 << 6);
  const ALL: Self = Self((1 << 7) - 1);
  const NO_CONTENT: Self = Self(Self::ALL.0 & !Self::CONTENT_FILES.0);

  const fn contains(self, other: Self) -> bool {
    self.0 & other.0 == other.0
  }

  const fn union(self, other: Self) -> Self {
    Self(self.0 | other.0)
  }

  const fn intersect(self, other: Self) -> Self {
    Self(self.0 & other.0)
  }

  const fn without(self, other: Self) -> Self {
    Self(self.0 & !other.0)
  }
}

/// Options controlling exact package resolution.
#[derive(Clone, Debug, Default)]
pub struct PackageResolveOptions {
  /// Explicit global-packages directory, overriding environment and config.
  pub packages_directory: Option<PathBuf>,
  /// Explicit NuGet configuration file. When present, no implicit hierarchy
  /// is discovered.
  pub config_file: Option<PathBuf>,
  /// Ordered command-line package sources, replacing configured sources.
  /// Variable-sized external URI text must be owned across project/config
  /// evaluation; the empty default allocates no backing buffer.
  pub sources: Vec<String>,
  /// Reject every operation that would require an HTTP request.
  pub offline: bool,
  /// Write or refresh `dv.lock.json` after successful resolution.
  pub write_lock: bool,
}

/// A NuGet v3 capability selected from a source service index.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum PackageServiceKind {
  /// Per-package metadata and version registration documents.
  Registration,
  /// Flat-container version lists, manifests, and package archives.
  PackageContent,
  /// Keyword package search.
  Search,
  /// Bulk package vulnerability information.
  Vulnerability,
  /// Package push and delete operations.
  PackagePublish,
}

impl PackageServiceKind {
  /// Returns the stable event and human-output spelling.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Registration => "registration",
      Self::PackageContent => "package_content",
      Self::Search => "search",
      Self::Vulnerability => "vulnerability",
      Self::PackagePublish => "package_publish",
    }
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageSourceRecord {
  name: TextSpan,
  location: TextSpan,
  endpoints: ItemRange,
  protocol: NugetProtocol,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageServiceEndpointRecord {
  location: TextSpan,
  kind: PackageServiceKind,
}

const _: () = assert!(size_of::<PackageSourceRecord>() == 28);
const _: () = assert!(align_of::<PackageSourceRecord>() == 4);
const _: () = assert!(size_of::<PackageServiceEndpointRecord>() == 12);
const _: () = assert!(align_of::<PackageServiceEndpointRecord>() == 4);

/// Immutable effective package-source and service-capability batches.
///
/// Source rows retain configuration order. Each row owns one consecutive
/// endpoint range, ordered by [`PackageServiceKind`] and then service-index
/// resource order. All variable text is stored once in `text`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageSourceInventory {
  text: Box<str>,
  sources: Box<[PackageSourceRecord]>,
  endpoints: Box<[PackageServiceEndpointRecord]>,
  network_requests: u32,
  downloaded_bytes: u64,
}

const _: () = assert!(size_of::<PackageSourceInventory>() == 64);
const _: () = assert!(align_of::<PackageSourceInventory>() == 8);

impl PackageSourceInventory {
  /// Returns source indices in merged configuration order.
  pub fn sources(&self) -> std::ops::Range<usize> {
    0..self.sources.len()
  }

  /// Returns the configured source name.
  pub fn source_name(&self, source: usize) -> &str {
    self.get(self.sources[source].name)
  }

  /// Returns the configured source URL or local path.
  pub fn source_location(&self, source: usize) -> &str {
    self.get(self.sources[source].location)
  }

  /// Returns `local`, `v2`, or `v3`.
  pub fn source_protocol(&self, source: usize) -> &'static str {
    self.sources[source].protocol.as_str()
  }

  /// Returns selected endpoint indices for one source.
  pub fn source_endpoints(&self, source: usize) -> std::ops::Range<usize> {
    range(self.sources[source].endpoints)
  }

  /// Returns an endpoint capability.
  pub fn endpoint_kind(&self, endpoint: usize) -> PackageServiceKind {
    self.endpoints[endpoint].kind
  }

  /// Returns an absolute selected endpoint URL.
  pub fn endpoint_location(&self, endpoint: usize) -> &str {
    self.get(self.endpoints[endpoint].location)
  }

  /// Returns service-index HTTP requests performed by discovery.
  pub fn network_requests(&self) -> u32 {
    self.network_requests
  }

  /// Returns service-index response bytes read by discovery.
  pub fn downloaded_bytes(&self) -> u64 {
    self.downloaded_bytes
  }

  fn get(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }
}

/// One immutable resolved package graph and its selected assets.
///
/// Variable text is owned once. Graph records, dependency indices, and asset
/// spans are contiguous and traversed linearly by lock writing and compiler
/// planning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageResolution {
  text: Box<str>,
  cache_root: TextSpan,
  http_cache_root: TextSpan,
  temp_root: TextSpan,
  lock_path: TextSpan,
  target_framework: TextSpan,
  source: TextSpan,
  prune_fingerprint: TextSpan,
  source_protocol: NugetProtocol,
  signature_validation: SignatureValidationMode,
  audit_enabled: bool,
  audit_mode: NugetAuditMode,
  audit_level: NugetAuditLevel,
  proxy_configured: bool,
  packages: Box<[ResolvedPackage]>,
  package_roots: Box<[TextSpan]>,
  fallback_roots: Box<[TextSpan]>,
  package_assets: Box<[PackageAssets]>,
  package_extended_assets: Box<[PackageExtendedAssets]>,
  dependencies: Box<[u32]>,
  assets: Box<[TextSpan]>,
  asset_ranges: PackageAssetRanges,
  runtime_targets: Box<[RuntimeTargetAsset]>,
  cache_hits: u32,
  downloaded_packages: u32,
  network_requests: u32,
  downloaded_bytes: u64,
}

const _: () = assert!(size_of::<PackageResolution>() == 304);
const _: () = assert!(align_of::<PackageResolution>() == align_of::<usize>());

impl PackageResolution {
  /// Returns the global-packages directory used by this graph.
  pub fn cache_root(&self) -> &Path {
    Path::new(self.get(self.cache_root))
  }

  /// Returns the selected NuGet HTTP metadata-cache directory.
  pub fn http_cache_root(&self) -> &Path {
    Path::new(self.get(self.http_cache_root))
  }

  /// Returns the selected NuGet scratch directory.
  pub fn temp_root(&self) -> &Path {
    Path::new(self.get(self.temp_root))
  }

  /// Iterates read-only fallback package roots in lookup order.
  pub fn fallback_roots(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.fallback_roots.iter().map(|span| Path::new(self.get(*span)))
  }

  /// Returns the configured package-signature validation policy.
  pub fn signature_validation(&self) -> SignatureValidationMode {
    self.signature_validation
  }

  /// Returns whether vulnerability auditing is enabled for the project.
  pub fn audit_enabled(&self) -> bool {
    self.audit_enabled
  }

  /// Returns the configured vulnerability-audit dependency scope.
  pub fn audit_mode(&self) -> NugetAuditMode {
    self.audit_mode
  }

  /// Returns the configured minimum vulnerability severity.
  pub fn audit_level(&self) -> NugetAuditLevel {
    self.audit_level
  }

  /// Returns whether an explicit or environment proxy is configured.
  ///
  /// Proxy addresses and credentials are deliberately not retained in the
  /// immutable result or emitted through reporters.
  pub fn proxy_configured(&self) -> bool {
    self.proxy_configured
  }

  /// Returns the deterministic lock-file path.
  pub fn lock_path(&self) -> &Path {
    Path::new(self.get(self.lock_path))
  }

  /// Returns the target framework used for dependency and asset selection.
  pub fn target_framework(&self) -> &str {
    self.get(self.target_framework)
  }

  /// Returns the selected package source.
  pub fn source(&self) -> &str {
    self.get(self.source)
  }

  /// Returns the selected NuGet protocol generation.
  pub fn source_protocol(&self) -> &'static str {
    self.source_protocol.as_str()
  }

  /// Returns package records sorted by case-insensitive identity.
  pub fn packages(&self) -> &[ResolvedPackage] {
    &self.packages
  }

  /// Returns a package identity.
  pub fn package_id(&self, package: ResolvedPackage) -> &str {
    self.get(package.id)
  }

  /// Returns a normalized package version.
  pub fn package_version(&self, package: ResolvedPackage) -> &str {
    self.get(package.version)
  }

  /// Returns the computed package SHA-512, verified against v2 metadata when available.
  pub fn package_hash(&self, index: usize) -> &str {
    self.get(self.package_assets[index].hash)
  }

  /// Returns whether a package was directly referenced by the project.
  pub fn package_is_direct(&self, package: ResolvedPackage) -> bool {
    package.direct
  }

  /// Iterates dependency package indices.
  pub fn package_dependencies(&self, package: ResolvedPackage) -> impl ExactSizeIterator<Item = u32> + '_ {
    let range = range(package.dependencies);
    self.dependencies[range].iter().copied()
  }

  /// Iterates selected compile assemblies across the graph.
  pub fn compile_assets(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.assets(PackageAssetFamily::Compile)
  }

  /// Iterates selected runtime assemblies across the graph.
  pub fn runtime_assets(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.assets(PackageAssetFamily::Runtime)
  }

  /// Iterates package analyzers across the graph.
  pub fn analyzers(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.assets(PackageAssetFamily::Analyzer)
  }

  /// Iterates selected satellite resource assemblies across the graph.
  pub fn resource_assets(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.assets(PackageAssetFamily::Resource)
  }

  /// Iterates selected package content files across the graph.
  pub fn content_files(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.assets(PackageAssetFamily::Content)
  }

  /// Iterates selected inner-build MSBuild imports across the graph.
  pub fn build_assets(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.assets(PackageAssetFamily::Build)
  }

  /// Iterates selected outer-build MSBuild imports across the graph.
  pub fn build_multi_targeting_assets(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.assets(PackageAssetFamily::BuildMultiTargeting)
  }

  /// Iterates selected transitive MSBuild imports across the graph.
  pub fn build_transitive_assets(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.assets(PackageAssetFamily::BuildTransitive)
  }

  /// Iterates selected legacy native assets across the graph.
  pub fn native_assets(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.assets(PackageAssetFamily::Native)
  }

  /// Iterates one selected asset family as a contiguous read-only range.
  pub fn assets(&self, family: PackageAssetFamily) -> impl ExactSizeIterator<Item = &Path> {
    let range = range(match family {
      PackageAssetFamily::Compile => self.asset_ranges.compile,
      PackageAssetFamily::Runtime => self.asset_ranges.runtime,
      PackageAssetFamily::Analyzer => self.asset_ranges.analyzers,
      PackageAssetFamily::Resource => self.asset_ranges.resources,
      PackageAssetFamily::Content => self.asset_ranges.content,
      PackageAssetFamily::Build => self.asset_ranges.build,
      PackageAssetFamily::BuildMultiTargeting => self.asset_ranges.build_multi_targeting,
      PackageAssetFamily::BuildTransitive => self.asset_ranges.build_transitive,
      PackageAssetFamily::Native => self.asset_ranges.native,
    });
    self.assets[range].iter().map(|span| Path::new(self.get(*span)))
  }

  /// Iterates RID-specific runtime targets across the graph.
  pub fn runtime_targets(&self) -> impl ExactSizeIterator<Item = (&Path, &str, RuntimeTargetKind)> {
    self
      .runtime_targets
      .iter()
      .map(|asset| (Path::new(self.get(asset.path)), self.get(asset.runtime_identifier), asset.kind))
  }

  /// Returns how many packages were reused from the cache.
  pub fn cache_hits(&self) -> u32 {
    self.cache_hits
  }

  /// Returns how many packages were downloaded and published.
  pub fn downloaded_packages(&self) -> u32 {
    self.downloaded_packages
  }

  /// Returns HTTP request count, including service discovery.
  pub fn network_requests(&self) -> u32 {
    self.network_requests
  }

  /// Returns package payload bytes downloaded.
  pub fn downloaded_bytes(&self) -> u64 {
    self.downloaded_bytes
  }

  fn package_compile_assets(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_assets[index].compile);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  fn package_runtime_assets(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_assets[index].runtime);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  fn package_analyzers(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_assets[index].analyzers);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  fn package_resource_assets(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_extended_assets[index].resources);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  fn package_content_files(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_extended_assets[index].content_files);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  fn package_build_assets(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_extended_assets[index].build);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  fn package_build_multi_targeting_assets(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_extended_assets[index].build_multi_targeting);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  fn package_build_transitive_assets(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_extended_assets[index].build_transitive);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  fn package_native_assets(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_extended_assets[index].native);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  fn package_runtime_targets(&self, index: usize) -> impl ExactSizeIterator<Item = RuntimeTargetAsset> + '_ {
    let range = range(self.package_extended_assets[index].runtime_targets);
    self.runtime_targets[range].iter().copied()
  }

  fn package_root_at(&self, index: usize) -> &Path {
    Path::new(self.get(self.package_roots[index]))
  }

  fn get(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }

  pub(crate) fn matches_project(&self, project: &ProjectSpec) -> bool {
    let direct_count = self.packages.iter().filter(|package| package.direct).count();
    self.target_framework() == project.target_framework()
      && self.lock_path() == project.project_directory().join("dv.lock.json")
      && direct_count == project.package_references().len()
      && project.package_references().iter().all(|reference| {
        let Ok(range) = VersionRange::parse(project.package_version(*reference)) else {
          return false;
        };
        self.packages.iter().copied().any(|package| {
          package.direct
            && self.package_id(package).eq_ignore_ascii_case(project.package_id(*reference))
            && PackageVersion::parse(self.package_version(package)).is_ok_and(|version| range.contains(&version))
        })
      })
  }
}

/// Stable categories for package configuration, resolution, and cache errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageErrorKind {
  /// NuGet configuration could not be read or is outside the supported subset.
  Configuration,
  /// A package identity or version is malformed or conflicts with the graph.
  Resolution,
  /// No selected asset group is compatible with the evaluated target.
  Incompatible,
  /// Offline mode encountered a package cache miss.
  OfflineMiss,
  /// An HTTP source or response failed.
  Network,
  /// A downloaded or cached package failed integrity validation.
  Integrity,
  /// A package archive is malformed or violates extraction limits.
  Archive,
  /// Cache or lock-file I/O failed.
  Io,
  /// A retained path is not valid Unicode.
  NonUnicodePath,
  /// Compact plan data exceeded its supported range.
  TextOverflow,
}

/// A package failure with stable path or source context.
#[derive(Debug)]
pub struct PackageError {
  kind: PackageErrorKind,
  context: String,
  message: String,
}

impl PackageError {
  /// Returns the stable failure category.
  pub fn kind(&self) -> PackageErrorKind {
    self.kind
  }

  /// Returns the path, source, or package associated with the failure.
  pub fn context(&self) -> &str {
    &self.context
  }

  fn new(kind: PackageErrorKind, context: impl Into<String>, message: impl Into<String>) -> Self {
    Self {
      kind,
      context: context.into(),
      message: message.into(),
    }
  }
}

impl fmt::Display for PackageError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl Error for PackageError {}

#[derive(Clone)]
struct PackageRequest {
  id: String,
  lower_id: String,
  version: String,
  direct: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PackageRequirement {
  id: String,
  lower_id: String,
  range: VersionRange,
  direct: bool,
  include_assets: AssetFlags,
  suppress_parent: AssetFlags,
}

/// Parsed NuGet version used only while converging cold dependency metadata.
/// The normalized text is retained for URLs and output; fixed numeric fields
/// keep the dominant precedence comparison branch allocation-free.
#[derive(Clone, Debug)]
struct PackageVersion {
  normalized: String,
  numbers: [u32; 4],
  prerelease_start: Option<u32>,
}

impl PackageVersion {
  fn parse(value: &str) -> Result<Self, PackageError> {
    if value.is_empty() || value.len() > 256 {
      return Err(PackageError::new(
        PackageErrorKind::Resolution,
        value,
        format!("package version {value:?} must contain between 1 and 256 bytes"),
      ));
    }
    let (precedence, metadata) = value
      .split_once('+')
      .map_or((value, None), |(precedence, metadata)| (precedence, Some(metadata)));
    if metadata.is_some_and(|value| {
      value.is_empty()
        || value
          .split('.')
          .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    }) {
      return Err(PackageError::new(
        PackageErrorKind::Resolution,
        value,
        format!("package version {value:?} has invalid build metadata"),
      ));
    }
    let (numbers_text, prerelease) = precedence
      .split_once('-')
      .map_or((precedence, None), |(numbers, prerelease)| (numbers, Some(prerelease)));
    if prerelease.is_some_and(|value| {
      value.is_empty()
        || value
          .split('.')
          .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    }) {
      return Err(PackageError::new(
        PackageErrorKind::Resolution,
        value,
        format!("package version {value:?} has an invalid prerelease"),
      ));
    }
    let mut numbers = [0u32; 4];
    let mut part_count = 0;
    for (index, part) in numbers_text.split('.').enumerate() {
      if index >= numbers.len() {
        return Err(PackageError::new(
          PackageErrorKind::Resolution,
          value,
          format!("package version {value:?} must contain one to four numeric parts"),
        ));
      }
      numbers[index] = part.parse().map_err(|_| {
        PackageError::new(
          PackageErrorKind::Resolution,
          value,
          format!("package version {value:?} contains a non-numeric version part"),
        )
      })?;
      part_count += 1;
    }
    if part_count == 0 {
      return Err(PackageError::new(
        PackageErrorKind::Resolution,
        value,
        format!("package version {value:?} must contain one to four numeric parts"),
      ));
    }
    let mut normalized = format!("{}.{}.{}", numbers[0], numbers[1], numbers[2]);
    if numbers[3] != 0 {
      write!(normalized, ".{}", numbers[3]).expect("writing a String succeeds");
    }
    let prerelease_start = prerelease.map(|value| {
      normalized.push('-');
      let start = normalized.len();
      normalized.push_str(&value.to_ascii_lowercase());
      start as u32
    });
    Ok(Self {
      normalized,
      numbers,
      prerelease_start,
    })
  }

  fn prerelease(&self) -> Option<&str> {
    self.prerelease_start.map(|start| &self.normalized[start as usize..])
  }
}

impl Ord for PackageVersion {
  fn cmp(&self, other: &Self) -> Ordering {
    self
      .numbers
      .cmp(&other.numbers)
      .then_with(|| compare_prerelease(self.prerelease(), other.prerelease()))
  }
}

impl PartialEq for PackageVersion {
  fn eq(&self, other: &Self) -> bool {
    self.cmp(other) == Ordering::Equal
  }
}

impl Eq for PackageVersion {}

impl PartialOrd for PackageVersion {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    Some(self.cmp(other))
  }
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
          (Some(left), Some(right)) => {
            let ordering = compare_prerelease_part(left, right);
            if ordering != Ordering::Equal {
              return ordering;
            }
          },
          (Some(_), None) => return Ordering::Greater,
          (None, Some(_)) => return Ordering::Less,
          (None, None) => return Ordering::Equal,
        }
      }
    },
  }
}

fn compare_prerelease_part(left: &str, right: &str) -> Ordering {
  match (left.bytes().all(|byte| byte.is_ascii_digit()), right.bytes().all(|byte| byte.is_ascii_digit())) {
    (true, true) => {
      let left = left.trim_start_matches('0');
      let right = right.trim_start_matches('0');
      left.len().cmp(&right.len()).then_with(|| left.cmp(right))
    },
    (true, false) => Ordering::Less,
    (false, true) => Ordering::Greater,
    (false, false) => left.cmp(right),
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VersionBound {
  version: PackageVersion,
  inclusive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VersionRange {
  lower: Option<VersionBound>,
  upper: Option<VersionBound>,
}

impl VersionRange {
  #[cfg(test)]
  fn exact(version: PackageVersion) -> Self {
    Self {
      lower: Some(VersionBound {
        version: version.clone(),
        inclusive: true,
      }),
      upper: Some(VersionBound { version, inclusive: true }),
    }
  }

  fn parse(value: &str) -> Result<Self, PackageError> {
    let value = value.trim();
    if value.is_empty() || value.contains('*') {
      return Err(unsupported_version_range(value));
    }
    if !value.starts_with(['[', '(']) {
      return Ok(Self {
        lower: Some(VersionBound {
          version: PackageVersion::parse(value)?,
          inclusive: true,
        }),
        upper: None,
      });
    }
    let lower_inclusive = value.starts_with('[');
    let upper_inclusive = value.ends_with(']');
    if !value.ends_with([']', ')']) || value.len() < 3 {
      return Err(unsupported_version_range(value));
    }
    let body = &value[1..value.len() - 1];
    if !body.contains(',') {
      if !lower_inclusive || !upper_inclusive {
        return Err(unsupported_version_range(value));
      }
      let version = PackageVersion::parse(body.trim())?;
      return Ok(Self {
        lower: Some(VersionBound {
          version: version.clone(),
          inclusive: true,
        }),
        upper: Some(VersionBound { version, inclusive: true }),
      });
    }
    let (lower, upper) = body.split_once(',').expect("a checked range contains a comma");
    let lower = (!lower.trim().is_empty())
      .then(|| {
        Ok(VersionBound {
          version: PackageVersion::parse(lower.trim())?,
          inclusive: lower_inclusive,
        })
      })
      .transpose()?;
    let upper = (!upper.trim().is_empty())
      .then(|| {
        Ok(VersionBound {
          version: PackageVersion::parse(upper.trim())?,
          inclusive: upper_inclusive,
        })
      })
      .transpose()?;
    if lower.is_none() && upper.is_none() {
      return Err(unsupported_version_range(value));
    }
    let range = Self { lower, upper };
    if let (Some(lower), Some(upper)) = (&range.lower, &range.upper)
      && (lower.version > upper.version || (lower.version == upper.version && (!lower.inclusive || !upper.inclusive)))
    {
      return Err(PackageError::new(
        PackageErrorKind::Resolution,
        value,
        format!("dependency range {value:?} contains no versions"),
      ));
    }
    Ok(range)
  }

  fn contains(&self, version: &PackageVersion) -> bool {
    self
      .lower
      .as_ref()
      .is_none_or(|lower| version > &lower.version || (lower.inclusive && version == &lower.version))
      && self
        .upper
        .as_ref()
        .is_none_or(|upper| version < &upper.version || (upper.inclusive && version == &upper.version))
  }

  fn allows_prerelease(&self) -> bool {
    self.lower.as_ref().is_some_and(|bound| bound.version.prerelease().is_some())
      || self.upper.as_ref().is_some_and(|bound| bound.version.prerelease().is_some())
  }
}

fn unsupported_version_range(value: &str) -> PackageError {
  PackageError::new(
    PackageErrorKind::Resolution,
    value,
    format!("dependency range {value:?} is outside the supported NuGet interval syntax"),
  )
}

struct WorkPackage {
  request: PackageRequest,
  root: PathBuf,
  hash: String,
  dependencies: Vec<PackageRequest>,
  compile_assets: Vec<PathBuf>,
  runtime_assets: Vec<PathBuf>,
  analyzers: Vec<PathBuf>,
  resource_assets: Vec<PathBuf>,
  content_files: Vec<PathBuf>,
  build_assets: Vec<PathBuf>,
  build_multi_targeting_assets: Vec<PathBuf>,
  build_transitive_assets: Vec<PathBuf>,
  native_assets: Vec<PathBuf>,
  runtime_targets: Vec<WorkRuntimeTarget>,
  cache_hit: bool,
  origin: Option<PackageSource>,
}

struct WorkRuntimeTarget {
  path: PathBuf,
  runtime_identifier: String,
  kind: RuntimeTargetKind,
}

struct ResolutionContext<'a> {
  cache_root: &'a Path,
  http_cache_root: &'a Path,
  temp_root: &'a Path,
  fallback_roots: &'a [PathBuf],
  lock_path: &'a Path,
  target_framework: &'a str,
  source: &'a str,
  prune_fingerprint: &'a str,
  source_protocol: NugetProtocol,
  signature_validation: SignatureValidationMode,
  audit_enabled: bool,
  audit_mode: NugetAuditMode,
  audit_level: NugetAuditLevel,
  proxy_configured: bool,
}

#[derive(Default)]
struct PackagePruning {
  text: Box<str>,
  packages: Vec<PrunedPackage>,
  fingerprint: String,
}

#[derive(Clone, Copy)]
struct PrunedPackage {
  lower_id: TextSpan,
  upper_numbers: [u32; 4],
  upper_prerelease: TextSpan,
}

const _: () = assert!(size_of::<PrunedPackage>() == 32);
const _: () = assert!(align_of::<PrunedPackage>() == 4);

struct ParsedPrunedPackage {
  lower_id: String,
  upper: PackageVersion,
}

impl PackagePruning {
  fn contains(&self, lower_id: &str, version: &PackageVersion) -> bool {
    self
      .packages
      .binary_search_by(|package| self.get(package.lower_id).cmp(lower_id))
      .is_ok_and(|index| {
        let package = self.packages[index];
        version.numbers.cmp(&package.upper_numbers).then_with(|| {
          compare_prerelease(
            version.prerelease(),
            (package.upper_prerelease.len != 0).then(|| self.get(package.upper_prerelease)),
          )
        }) != Ordering::Greater
      })
  }

  fn get(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }
}

struct CachedPackage {
  root: PathBuf,
  hash: String,
  dependencies: Option<Vec<PackageRequirement>>,
  cache_hit: bool,
  requests: u32,
  bytes: u64,
  origin: Option<PackageSource>,
}

struct ResolvedGraph {
  packages: BTreeMap<String, WorkPackage>,
  network_requests: u32,
  downloaded_bytes: u64,
}

/// Cold graph state is identity-ordered and owned by the resolver task. Parent
/// identities key constraints so replacing a package version can retract its
/// previous edges without retaining an object graph.
struct ConstraintNode {
  id: String,
  direct: Option<VersionRange>,
  constraints: BTreeMap<String, VersionRange>,
  selected: Option<PackageVersion>,
  metadata_version: Option<PackageVersion>,
  dependencies: Vec<PackageRequirement>,
  available_versions: Option<Vec<PackageVersion>>,
  pruned: bool,
  generation: u32,
}

enum NodeSelection {
  Version(PackageVersion),
  Enumerate,
}

enum MetadataTaskResult {
  Requirements {
    dependencies: Vec<PackageRequirement>,
    requests: u32,
    bytes: u64,
    package: Option<CachedPackage>,
  },
  Versions {
    versions: Vec<PackageVersion>,
    requests: u32,
    bytes: u64,
  },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum NugetProtocol {
  Local,
  V2,
  V3,
}

impl NugetProtocol {
  fn parse_http(value: Option<&str>, source: &str, context: &Path) -> Result<Self, PackageError> {
    match value {
      Some("2") => Ok(Self::V2),
      Some("3") => Ok(Self::V3),
      Some(other) => Err(config_error(
        context,
        format!("package source {source:?} has unsupported protocolVersion {other:?}; expected 2 or 3"),
      )),
      None if source.trim_end_matches('/').ends_with("/v3/index.json") => Ok(Self::V3),
      None => Ok(Self::V2),
    }
  }

  const fn as_str(self) -> &'static str {
    match self {
      Self::Local => "local",
      Self::V2 => "v2",
      Self::V3 => "v3",
    }
  }
}

#[derive(Clone)]
struct PackageSource {
  url: String,
  protocol: NugetProtocol,
}

impl PackageSource {
  fn parse(value: String, protocol: Option<&str>, context: &Path, relative_to: &Path) -> Result<Self, PackageError> {
    if value.trim().is_empty() {
      return Err(config_error(context, "package source cannot be empty"));
    }
    if value.starts_with("https://") {
      let parsed = reqwest::Url::parse(&value).map_err(|error| config_error(context, format!("invalid HTTPS package source {value:?}: {error}")))?;
      if !parsed.has_host() {
        return Err(config_error(context, format!("HTTPS package source {value:?} must include a host")));
      }
      if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(config_error(
          context,
          "package-source URLs must not embed credentials; credential support is tracked by NUGET-008",
        ));
      }
      return Ok(Self {
        protocol: NugetProtocol::parse_http(protocol, &value, context)?,
        url: value,
      });
    }
    if value.starts_with("http://") {
      return Err(config_error(
        context,
        format!("insecure HTTP package source {value:?} requires the explicit policy tracked by NUGET-012"),
      ));
    }
    if let Some(version) = protocol
      && version != "2"
      && version != "3"
    {
      return Err(config_error(
        context,
        format!("local package source {value:?} has unsupported protocolVersion {version:?}; expected 2 or 3"),
      ));
    }
    let path = if value.starts_with("file://") {
      reqwest::Url::parse(&value)
        .map_err(|error| config_error(context, format!("invalid local package-source URI {value:?}: {error}")))?
        .to_file_path()
        .map_err(|()| config_error(context, format!("local package-source URI {value:?} does not identify a filesystem path")))?
    } else {
      if value.contains("://") {
        return Err(config_error(
          context,
          format!("package source {value:?} must be HTTPS, file://, or a local folder path"),
        ));
      }
      let path = PathBuf::from(&value);
      if path.is_absolute() { path } else { relative_to.join(path) }
    };
    let value = path.to_str().ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::NonUnicodePath,
        path.display().to_string(),
        "local package-source path is not valid Unicode",
      )
    })?;
    Ok(Self {
      url: value.to_owned(),
      protocol: NugetProtocol::Local,
    })
  }
}

#[derive(Clone)]
enum ServiceEndpoint {
  Local {
    source: String,
    root: PathBuf,
    layout: LocalFeedLayout,
    source_index: u32,
  },
  V2 {
    source: String,
    base: String,
    source_index: u32,
  },
  V3 {
    source: String,
    services: Arc<NugetServiceEndpoints>,
    source_index: u32,
  },
}

/// Local layout is detected once per source. Flat archive paths are retained
/// contiguously so graph workers never repeat directory enumeration.
#[derive(Clone)]
enum LocalFeedLayout {
  Unknown,
  Flat(Arc<[PathBuf]>),
  Hierarchical,
}

impl ServiceEndpoint {
  const fn source_index(&self) -> u32 {
    match self {
      Self::Local { source_index, .. } | Self::V2 { source_index, .. } | Self::V3 { source_index, .. } => *source_index,
    }
  }

  fn source(&self) -> &str {
    match self {
      Self::Local { source, .. } | Self::V2 { source, .. } | Self::V3 { source, .. } => source,
    }
  }

  const fn protocol(&self) -> NugetProtocol {
    match self {
      Self::Local { .. } => NugetProtocol::Local,
      Self::V2 { .. } => NugetProtocol::V2,
      Self::V3 { .. } => NugetProtocol::V3,
    }
  }
}

struct NugetConfiguration {
  cache_root: PathBuf,
  http_cache_root: PathBuf,
  temp_root: PathBuf,
  fallback_roots: Arc<[PathBuf]>,
  sources: Vec<(String, PackageSource)>,
  // Audit resolution consumes this batch without reopening configuration
  // files once vulnerability endpoints are wired into restore.
  #[allow(dead_code)]
  audit_sources: Vec<(String, PackageSource)>,
  source_mapping: Option<Arc<PackageSourceMapping>>,
  signature_validation: SignatureValidationMode,
  proxy: Option<ProxySettings>,
}

#[derive(Default)]
struct NugetConfigMerge {
  sources: Vec<(String, PackageSource)>,
  disabled: Vec<String>,
  audit_sources: Vec<(String, PackageSource)>,
  source_mapping: MergedSourceMapping,
  global_packages: Option<PathBuf>,
  fallback_folders: Vec<FallbackFolder>,
  config_priority: u32,
  signature_validation: Option<SignatureValidationMode>,
  proxy_url: Option<String>,
  proxy_user: Option<String>,
  proxy_password: Option<String>,
  no_proxy: Option<String>,
}

struct FallbackFolder {
  name: String,
  path: PathBuf,
  config_priority: u32,
}

#[derive(Clone)]
struct ProxySettings {
  url: String,
  no_proxy: Option<String>,
}

#[derive(Default)]
struct MergedSourceMapping {
  sources: Vec<MergedSourceMappingEntry>,
  patterns: Vec<String>,
}

struct MergedSourceMappingEntry {
  source: String,
  patterns: ItemRange,
}

#[derive(Default)]
struct PackageSourceMapping {
  text: Box<str>,
  sources: Box<[SourceMappingEntry]>,
  patterns: Box<[SourcePattern]>,
}

#[derive(Clone, Copy)]
struct SourceMappingEntry {
  source_index: u32,
  patterns: ItemRange,
}

#[derive(Clone, Copy)]
struct SourcePattern {
  text: TextSpan,
  prefix: bool,
}

const _: () = assert!(size_of::<SourceMappingEntry>() == 12);
const _: () = assert!(align_of::<SourceMappingEntry>() == 4);
const _: () = assert!(size_of::<SourcePattern>() == 12);
const _: () = assert!(align_of::<SourcePattern>() == 4);

struct PendingSourceMapping {
  source: String,
  pattern_start: usize,
}

#[derive(Serialize, Deserialize)]
struct LockFile {
  schema_version: u16,
  target_framework: String,
  source: String,
  source_protocol: NugetProtocol,
  #[serde(default)]
  prune_fingerprint: String,
  direct: Vec<LockDirect>,
  packages: Vec<LockPackage>,
}

#[derive(Serialize, Deserialize, Eq, PartialEq)]
struct LockDirect {
  id: String,
  version: String,
}

#[derive(Serialize, Deserialize)]
struct LockPackage {
  id: String,
  version: String,
  sha512: String,
  direct: bool,
  dependencies: Vec<LockDirect>,
  compile_assets: Vec<String>,
  runtime_assets: Vec<String>,
  analyzers: Vec<String>,
  #[serde(default)]
  resource_assets: Vec<String>,
  #[serde(default)]
  content_files: Vec<String>,
  #[serde(default)]
  build_assets: Vec<String>,
  #[serde(default)]
  build_multi_targeting_assets: Vec<String>,
  #[serde(default)]
  build_transitive_assets: Vec<String>,
  #[serde(default)]
  native_assets: Vec<String>,
  #[serde(default)]
  runtime_targets: Vec<LockRuntimeTarget>,
}

#[derive(Serialize, Deserialize)]
struct LockRuntimeTarget {
  path: String,
  runtime_identifier: String,
  kind: RuntimeTargetKind,
}

/// Resolves exact package graphs for an evaluated project batch.
///
/// A batch of one is the current CLI case. Empty or package-free projects do
/// not read configuration, inspect caches, or access the network.
pub fn resolve_package_inputs(projects: &[&ProjectSpec], options: &PackageResolveOptions) -> Result<Vec<PackageResolution>, PackageError> {
  let mut resolutions = Vec::with_capacity(projects.len());
  for project in projects {
    if project.package_references().is_empty() {
      resolutions.push(empty_resolution(project)?);
    } else {
      resolutions.push(resolve_project(project, options)?);
    }
  }
  Ok(resolutions)
}

/// Discovers effective package sources and their supported v3 capabilities.
///
/// The transform is batch-first. V3 service indexes are fetched concurrently
/// through a bounded task set, then compacted in project/configuration order.
/// Local and v2 sources require no network work and have empty endpoint ranges.
pub fn inspect_package_sources(projects: &[&ProjectSpec], options: &PackageResolveOptions) -> Result<Vec<PackageSourceInventory>, PackageError> {
  if projects.is_empty() {
    return Ok(Vec::new());
  }
  let runtime = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(ASYNC_RUNTIME_WORKERS)
    .enable_all()
    .build()
    .map_err(|error| {
      PackageError::new(
        PackageErrorKind::Io,
        "package-source scheduler",
        format!("failed to create async runtime: {error}"),
      )
    })?;
  let mut inventories = Vec::with_capacity(projects.len());
  for project in projects {
    let config = discover_configuration(
      project.project_directory(),
      options.packages_directory.as_deref(),
      options.config_file.as_deref(),
      &options.sources,
    )?;
    let client = http_client(config.proxy.as_ref())?;
    inventories.push(runtime.block_on(inspect_source_batch(&client, &config.sources, !options.offline))?);
  }
  Ok(inventories)
}

async fn inspect_source_batch(
  client: &reqwest::Client,
  sources: &[(String, PackageSource)],
  allow_network: bool,
) -> Result<PackageSourceInventory, PackageError> {
  let mut discovered = std::iter::repeat_with(|| None)
    .take(sources.len())
    .collect::<Vec<Option<(NugetServiceEndpoints, u64)>>>();
  if allow_network {
    let jobs = sources
      .iter()
      .enumerate()
      .filter(|(_, (_, source))| source.protocol == NugetProtocol::V3)
      .map(|(index, (_, source))| (index, source.url.clone()))
      .collect::<Vec<_>>();
    let mut tasks = JoinSet::new();
    let mut next = 0usize;
    while next < jobs.len() || !tasks.is_empty() {
      while next < jobs.len() && tasks.len() < MAX_DOWNLOAD_WORKERS {
        let (index, source) = jobs[next].clone();
        let client = client.clone();
        tasks.spawn(async move { (index, fetch_v3_service_index(&client, &source).await) });
        next += 1;
      }
      let (index, result) = tasks
        .join_next()
        .await
        .ok_or_else(|| PackageError::new(PackageErrorKind::Io, "package-source scheduler", "service-index task set ended early"))?
        .map_err(|error| PackageError::new(PackageErrorKind::Io, "package-source scheduler", format!("service-index task failed: {error}")))?;
      discovered[index] = Some(result?);
    }
  }

  let text_capacity = sources
    .iter()
    .map(|(name, source)| name.len() + source.url.len())
    .sum::<usize>()
    .saturating_add(discovered.iter().flatten().map(|(services, _)| services.text.len()).sum::<usize>());
  let mut text = TextTable::with_capacity(text_capacity);
  let mut source_rows = Vec::with_capacity(sources.len());
  let mut endpoint_rows = Vec::new();
  let mut network_requests = 0u32;
  let mut downloaded_bytes = 0u64;
  for (index, (name, source)) in sources.iter().enumerate() {
    let start = u32_len(endpoint_rows.len(), "package-source endpoint range")?;
    if let Some((services, bytes)) = discovered[index].take() {
      network_requests = network_requests
        .checked_add(1)
        .ok_or_else(|| network_error(&source.url, "package-source request count overflow"))?;
      downloaded_bytes = downloaded_bytes
        .checked_add(bytes)
        .ok_or_else(|| network_error(&source.url, "package-source response byte count overflow"))?;
      for (kind, capability) in [
        (PackageServiceKind::Registration, ServiceCapability::Registration),
        (PackageServiceKind::PackageContent, ServiceCapability::PackageBase),
        (PackageServiceKind::Search, ServiceCapability::Search),
        (PackageServiceKind::Vulnerability, ServiceCapability::Vulnerability),
        (PackageServiceKind::PackagePublish, ServiceCapability::PackagePublish),
      ] {
        for endpoint in services.values(capability) {
          endpoint_rows.push(PackageServiceEndpointRecord {
            location: text.push(endpoint)?,
            kind,
          });
        }
      }
    }
    source_rows.push(PackageSourceRecord {
      name: text.push(name)?,
      location: text.push(&source.url)?,
      endpoints: ItemRange {
        start,
        len: u32_len(endpoint_rows.len() - start as usize, "package-source endpoint range")?,
      },
      protocol: source.protocol,
    });
  }
  Ok(PackageSourceInventory {
    text: text.text.into_boxed_str(),
    sources: source_rows.into_boxed_slice(),
    endpoints: endpoint_rows.into_boxed_slice(),
    network_requests,
    downloaded_bytes,
  })
}

fn resolve_project(project: &ProjectSpec, options: &PackageResolveOptions) -> Result<PackageResolution, PackageError> {
  let config = discover_configuration(
    project.project_directory(),
    options.packages_directory.as_deref(),
    options.config_file.as_deref(),
    &options.sources,
  )?;
  validate_signature_policy(config.signature_validation)?;
  validate_audit_policy(project)?;
  let lock_path = project.project_directory().join("dv.lock.json");
  let direct = direct_requests(project)?;
  let target = project.target();
  let target_text = project.target_framework();
  let pruning = discover_package_pruning(project.project_directory(), target)?;
  if let Some(resolution) = read_warm_lock(&lock_path, &config, &direct, project, &pruning.fingerprint)? {
    return Ok(resolution);
  }

  let client = http_client(config.proxy.as_ref())?;
  let runtime = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(ASYNC_RUNTIME_WORKERS)
    .enable_all()
    .build()
    .map_err(|error| PackageError::new(PackageErrorKind::Io, "package scheduler", format!("failed to create async runtime: {error}")))?;
  let graph = runtime.block_on(resolve_streaming_graph(&client, &direct, &config, options, target, target_text, &pruning))?;
  let resolved = graph.packages;

  validate_acyclic(&resolved)?;
  let origin = resolved.values().find_map(|package| package.origin.as_ref());
  let (source, source_protocol) = origin.map_or_else(
    || {
      config.sources.first().map_or_else(
        || (DEFAULT_SOURCE.to_owned(), NugetProtocol::V3),
        |(_, source)| (source.url.clone(), source.protocol),
      )
    },
    |source| (source.url.clone(), source.protocol),
  );
  let resolution = materialize_resolution(
    ResolutionContext {
      cache_root: &config.cache_root,
      http_cache_root: &config.http_cache_root,
      temp_root: &config.temp_root,
      fallback_roots: &config.fallback_roots,
      lock_path: &lock_path,
      target_framework: target_text,
      source: &source,
      prune_fingerprint: &pruning.fingerprint,
      source_protocol,
      signature_validation: config.signature_validation,
      audit_enabled: project.nuget_audit_enabled(),
      audit_mode: project.nuget_audit_mode(),
      audit_level: project.nuget_audit_level(),
      proxy_configured: config.proxy.is_some(),
    },
    &resolved,
    graph.network_requests,
    graph.downloaded_bytes,
  )?;
  if options.write_lock {
    write_lock(&resolution)?;
  }
  Ok(resolution)
}

fn validate_signature_policy(mode: SignatureValidationMode) -> Result<(), PackageError> {
  if mode == SignatureValidationMode::Require {
    return Err(PackageError::new(
      PackageErrorKind::Configuration,
      "signatureValidationMode",
      "signatureValidationMode=require needs package signature verification, which remains tracked by RES-015",
    ));
  }
  Ok(())
}

fn validate_audit_policy(project: &ProjectSpec) -> Result<(), PackageError> {
  if project.nuget_audit_enabled() {
    return Err(PackageError::new(
      PackageErrorKind::Configuration,
      "NuGetAudit",
      "NuGetAudit=true needs vulnerability advisory evaluation, which remains tracked by RES-024; set NuGetAudit=false to opt out explicitly",
    ));
  }
  Ok(())
}

fn direct_requests(project: &ProjectSpec) -> Result<Vec<PackageRequirement>, PackageError> {
  let mut direct = Vec::with_capacity(project.package_references().len());
  let mut seen = BTreeMap::<String, (VersionRange, String)>::new();
  for package in project.package_references() {
    let id = project.package_id(*package);
    let lower_id = normalize_id(id)?;
    let version_text = project.package_version(*package).trim();
    let range = VersionRange::parse(version_text)?;
    if let Some((existing_range, existing_text)) = seen.insert(lower_id.clone(), (range.clone(), version_text.to_owned()))
      && existing_range != range
    {
      return Err(PackageError::new(
        PackageErrorKind::Resolution,
        id,
        format!("package {id} is directly referenced with conflicting versions {existing_text} and {version_text}"),
      ));
    }
    direct.push(PackageRequirement {
      id: id.into(),
      lower_id,
      range,
      direct: true,
      include_assets: AssetFlags::ALL,
      suppress_parent: AssetFlags::NONE,
    });
  }
  direct.sort_unstable_by(|left, right| left.lower_id.cmp(&right.lower_id));
  direct.dedup_by(|left, right| left.lower_id == right.lower_id);
  Ok(direct)
}

fn discover_package_pruning(project_directory: &Path, target: TargetFramework) -> Result<PackagePruning, PackageError> {
  if target.family() != FrameworkFamily::Net || target.major() < 10 {
    return Ok(PackagePruning::default());
  }

  let inventory = discover_sdks(project_directory).map_err(|error| {
    PackageError::new(
      PackageErrorKind::Configuration,
      project_directory.display().to_string(),
      format!("failed to select the SDK needed for package pruning: {error}"),
    )
  })?;
  let selected = inventory.selected();
  let framework_version = target.framework_version();
  let sdk_data = inventory
    .installation_path(selected)
    .join("PrunePackageData")
    .join(&framework_version)
    .join("Microsoft.NETCore.App")
    .join("PackageOverrides.txt");
  if sdk_data.is_file() {
    return read_package_pruning(&sdk_data);
  }

  let pack_root = inventory.root(selected).join("packs").join("Microsoft.NETCore.App.Ref");
  let pack = select_targeting_pack(&pack_root, target)?;
  let pack_data = pack.join("data").join("PackageOverrides.txt");
  if !pack_data.is_file() {
    return Err(PackageError::new(
      PackageErrorKind::Configuration,
      pack_data.display().to_string(),
      format!("selected SDK {} has no package-pruning data for net{}", selected.version, framework_version),
    ));
  }
  read_package_pruning(&pack_data)
}

fn select_targeting_pack(root: &Path, target: TargetFramework) -> Result<PathBuf, PackageError> {
  let entries = fs::read_dir(root).map_err(|error| package_io("enumerate targeting packs", root, error))?;
  let mut selected = None::<(PackageVersion, PathBuf)>;
  for entry in entries {
    let entry = entry.map_err(|error| package_io("enumerate targeting packs", root, error))?;
    if !entry
      .file_type()
      .map_err(|error| package_io("inspect targeting pack", &entry.path(), error))?
      .is_dir()
    {
      continue;
    }
    let Some(value) = entry.file_name().to_str().map(str::to_owned) else {
      continue;
    };
    let Ok(version) = PackageVersion::parse(&value) else {
      continue;
    };
    if version.prerelease().is_some() || version.numbers[0] != u32::from(target.major()) || version.numbers[1] != u32::from(target.minor()) {
      continue;
    }
    if selected.as_ref().is_none_or(|(current, _)| version > *current) {
      selected = Some((version, entry.path()));
    }
  }
  selected.map(|(_, path)| path).ok_or_else(|| {
    PackageError::new(
      PackageErrorKind::Configuration,
      root.display().to_string(),
      format!("selected SDK has no Microsoft.NETCore.App reference pack for net{}", target.framework_version()),
    )
  })
}

fn read_package_pruning(path: &Path) -> Result<PackagePruning, PackageError> {
  let bytes = fs::read(path).map_err(|error| package_io("read package-pruning data", path, error))?;
  if bytes.len() as u64 > MAX_PRUNE_DATA_BYTES {
    return Err(PackageError::new(
      PackageErrorKind::Configuration,
      path.display().to_string(),
      format!("package-pruning data exceeds {MAX_PRUNE_DATA_BYTES} bytes"),
    ));
  }
  let text = std::str::from_utf8(&bytes).map_err(|error| {
    PackageError::new(
      PackageErrorKind::Configuration,
      path.display().to_string(),
      format!("package-pruning data is not UTF-8: {error}"),
    )
  })?;
  let mut packages = Vec::with_capacity(text.lines().size_hint().0);
  for (index, line) in text.lines().enumerate() {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
      continue;
    }
    if packages.len() >= MAX_PRUNE_PACKAGES {
      return Err(PackageError::new(
        PackageErrorKind::Configuration,
        path.display().to_string(),
        format!("package-pruning data exceeds {MAX_PRUNE_PACKAGES} entries"),
      ));
    }
    let Some((id, version)) = line.split_once('|') else {
      return Err(invalid_prune_line(path, index + 1));
    };
    if id.is_empty() || version.is_empty() || version.contains('|') {
      return Err(invalid_prune_line(path, index + 1));
    }
    let mut upper = PackageVersion::parse(version).map_err(|error| {
      PackageError::new(
        PackageErrorKind::Configuration,
        path.display().to_string(),
        format!("invalid package-pruning version on line {}: {error}", index + 1),
      )
    })?;
    if upper.prerelease().is_none() {
      upper.numbers = [upper.numbers[0], upper.numbers[1], 32_767, 0];
      upper.normalized = format!("{}.{}.32767", upper.numbers[0], upper.numbers[1]);
    }
    packages.push(ParsedPrunedPackage {
      lower_id: normalize_id(id).map_err(|error| {
        PackageError::new(
          PackageErrorKind::Configuration,
          path.display().to_string(),
          format!("invalid package-pruning identity on line {}: {error}", index + 1),
        )
      })?,
      upper,
    });
  }
  compact_package_pruning(packages)
}

fn compact_package_pruning(mut packages: Vec<ParsedPrunedPackage>) -> Result<PackagePruning, PackageError> {
  packages.sort_unstable_by(|left, right| left.lower_id.cmp(&right.lower_id).then_with(|| left.upper.cmp(&right.upper)));
  let mut merged = Vec::<ParsedPrunedPackage>::with_capacity(packages.len());
  for package in packages {
    if let Some(previous) = merged.last_mut()
      && previous.lower_id == package.lower_id
    {
      if package.upper > previous.upper {
        previous.upper = package.upper;
      }
      continue;
    }
    merged.push(package);
  }
  let text_capacity = merged
    .iter()
    .map(|package| package.lower_id.len() + package.upper.prerelease().map_or(0, str::len))
    .sum();
  let mut text = TextTable::with_capacity(text_capacity);
  let mut compact = Vec::with_capacity(merged.len());
  let mut hasher = Sha512::new();
  for package in merged {
    hasher.update(package.lower_id.as_bytes());
    hasher.update([0]);
    hasher.update(package.upper.normalized.as_bytes());
    hasher.update([b'\n']);
    let lower_id = text.push(&package.lower_id)?;
    let upper_prerelease = text.push(package.upper.prerelease().unwrap_or(""))?;
    compact.push(PrunedPackage {
      lower_id,
      upper_numbers: package.upper.numbers,
      upper_prerelease,
    });
  }
  Ok(PackagePruning {
    text: text.text.into_boxed_str(),
    packages: compact,
    fingerprint: BASE64.encode(hasher.finalize()),
  })
}

fn invalid_prune_line(path: &Path, line: usize) -> PackageError {
  PackageError::new(
    PackageErrorKind::Configuration,
    path.display().to_string(),
    format!("invalid package-pruning record on line {line}; expected Package.Id|version"),
  )
}

fn discover_configuration(
  project_directory: &Path,
  explicit_cache: Option<&Path>,
  explicit_config: Option<&Path>,
  explicit_sources: &[String],
) -> Result<NugetConfiguration, PackageError> {
  let config_paths = discover_config_paths(project_directory, explicit_config, &NugetConfigRoots::from_environment())?;

  let mut merged = NugetConfigMerge::default();
  if config_paths.is_empty() {
    merged.sources.push((
      "nuget.org".to_owned(),
      PackageSource {
        url: DEFAULT_SOURCE.to_owned(),
        protocol: NugetProtocol::V3,
      },
    ));
  }
  for path in config_paths {
    merge_config(&path, &mut merged)?;
  }
  let proxy = effective_proxy(&merged)?;
  let signature_validation = merged.signature_validation.unwrap_or(SignatureValidationMode::Accept);
  for key in merged.disabled {
    merged.sources.retain(|(name, _)| !name.eq_ignore_ascii_case(&key));
  }
  let sources = command_line_sources(explicit_sources, merged.sources, project_directory)?;
  let source_mapping = if merged.source_mapping.sources.is_empty() {
    None
  } else {
    Some(Arc::new(PackageSourceMapping::compile(merged.source_mapping, &sources)?))
  };
  if sources.is_empty() {
    return Err(PackageError::new(
      PackageErrorKind::Configuration,
      project_directory.display().to_string(),
      "NuGet configuration contains no enabled package source",
    ));
  }
  let cache_root = explicit_cache
    .map(Path::to_owned)
    .or(environment_path("NUGET_PACKAGES")?)
    .or(merged.global_packages)
    .or_else(default_global_packages)
    .ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::Configuration,
        project_directory.display().to_string(),
        "could not determine the global package cache; set NUGET_PACKAGES",
      )
    })?;
  let fallback_roots = match environment_path_list("NUGET_FALLBACK_PACKAGES")? {
    Some(paths) => paths,
    None => ordered_fallback_paths(merged.fallback_folders),
  };
  let http_cache_root = environment_path("NUGET_HTTP_CACHE_PATH")?
    .or_else(default_http_cache)
    .ok_or_else(|| config_error(project_directory, "could not determine the NuGet HTTP cache; set NUGET_HTTP_CACHE_PATH"))?;
  let temp_root = nonempty_env("NUGET_SCRATCH").unwrap_or_else(default_nuget_temp);
  Ok(NugetConfiguration {
    cache_root,
    http_cache_root,
    temp_root,
    fallback_roots: fallback_roots.into(),
    sources,
    audit_sources: merged.audit_sources,
    source_mapping,
    signature_validation,
    proxy,
  })
}

fn command_line_sources(
  overrides: &[String],
  configured: Vec<(String, PackageSource)>,
  project_directory: &Path,
) -> Result<Vec<(String, PackageSource)>, PackageError> {
  if overrides.is_empty() {
    return Ok(configured);
  }
  let mut sources = Vec::with_capacity(overrides.len());
  for value in overrides {
    if value.is_empty() {
      return Err(config_error(Path::new("--source"), "command-line package source cannot be empty"));
    }
    let parsed = PackageSource::parse(value.clone(), None, Path::new("--source"), project_directory)?;
    if sources.iter().any(|(_, source): &(String, PackageSource)| source.url == parsed.url) {
      continue;
    }
    if let Some((name, source)) = configured.iter().rev().find(|(_, source)| source.url == parsed.url) {
      sources.push((name.clone(), source.clone()));
    } else {
      sources.push((value.clone(), parsed));
    }
  }
  Ok(sources)
}

struct NugetConfigRoots {
  machine_config_directory: Option<PathBuf>,
  user_settings_directory: Option<PathBuf>,
}

impl NugetConfigRoots {
  fn from_environment() -> Self {
    let machine_base = if cfg!(windows) {
      nonempty_env("PROGRAMFILES(X86)").or_else(|| nonempty_env("PROGRAMFILES"))
    } else {
      nonempty_env("NUGET_COMMON_APPLICATION_DATA").or_else(|| {
        Some(if cfg!(target_os = "macos") {
          PathBuf::from("/Library/Application Support")
        } else {
          PathBuf::from("/etc/opt")
        })
      })
    };
    let user_settings_directory = if cfg!(windows) {
      nonempty_env("APPDATA").map(|path| path.join("NuGet"))
    } else {
      nonempty_env("HOME").map(|path| path.join(".nuget/NuGet"))
    };
    Self {
      machine_config_directory: machine_base.map(|path| path.join("NuGet/Config")),
      user_settings_directory,
    }
  }
}

fn nonempty_env(name: &str) -> Option<PathBuf> {
  env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn environment_path(name: &str) -> Result<Option<PathBuf>, PackageError> {
  let Some(path) = nonempty_env(name) else {
    return Ok(None);
  };
  if !path.is_absolute() {
    return Err(PackageError::new(
      PackageErrorKind::Configuration,
      name,
      format!("NuGet environment variable {name} must contain an absolute path"),
    ));
  }
  Ok(Some(path))
}

fn environment_path_list(name: &str) -> Result<Option<Vec<PathBuf>>, PackageError> {
  let Some(value) = env::var_os(name).filter(|value| !value.is_empty()) else {
    return Ok(None);
  };
  let value = value.to_str().ok_or_else(|| {
    PackageError::new(
      PackageErrorKind::Configuration,
      name,
      format!("NuGet environment variable {name} is not valid Unicode"),
    )
  })?;
  let mut paths = Vec::with_capacity(value.bytes().filter(|byte| *byte == b';').count() + 1);
  for path in value.split(';').filter(|path| !path.is_empty()) {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
      return Err(PackageError::new(
        PackageErrorKind::Configuration,
        name,
        format!("NuGet environment variable {name} must contain only absolute paths"),
      ));
    }
    paths.push(path);
  }
  Ok(Some(paths))
}

fn ordered_fallback_paths(mut folders: Vec<FallbackFolder>) -> Vec<PathBuf> {
  folders.sort_by(|left, right| right.config_priority.cmp(&left.config_priority));
  folders.into_iter().map(|folder| folder.path).collect()
}

fn default_http_cache() -> Option<PathBuf> {
  if cfg!(windows) {
    nonempty_env("LOCALAPPDATA").map(|path| path.join("NuGet/v3-cache"))
  } else {
    nonempty_env("XDG_DATA_HOME")
      .or_else(|| nonempty_env("HOME").map(|path| path.join(".local/share")))
      .map(|path| path.join("NuGet/http-cache"))
  }
}

fn default_nuget_temp() -> PathBuf {
  let root = env::temp_dir();
  if cfg!(target_os = "linux") {
    let user = env::var_os("USER").unwrap_or_default();
    let mut name = std::ffi::OsString::from("NuGetScratch");
    name.push(user);
    root.join(name)
  } else {
    root.join("NuGetScratch")
  }
}

fn effective_proxy(merged: &NugetConfigMerge) -> Result<Option<ProxySettings>, PackageError> {
  let configured = merged.proxy_url.as_ref().filter(|url| !url.is_empty());
  if let Some(url) = configured {
    if cfg!(windows)
      && (merged.proxy_user.as_ref().is_some_and(|value| !value.is_empty()) || merged.proxy_password.as_ref().is_some_and(|value| !value.is_empty()))
    {
      return Err(PackageError::new(
        PackageErrorKind::Configuration,
        "http_proxy",
        "separate NuGet proxy credentials require encrypted credential support; use a credential-free proxy or an http_proxy URL until NUGET-011",
      ));
    }
    return Ok(Some(ProxySettings {
      url: url.clone(),
      no_proxy: merged.no_proxy.clone().filter(|value| !value.is_empty()),
    }));
  }
  let Some(url) = env::var("http_proxy").ok().filter(|value| !value.is_empty()) else {
    return Ok(None);
  };
  Ok(Some(ProxySettings {
    url,
    no_proxy: env::var("no_proxy").ok().filter(|value| !value.is_empty()),
  }))
}

fn discover_config_paths(project_directory: &Path, explicit_config: Option<&Path>, roots: &NugetConfigRoots) -> Result<Vec<PathBuf>, PackageError> {
  if let Some(path) = explicit_config {
    if !path.is_file() {
      return Err(config_error(path, "explicit NuGet configuration file does not exist or is not a file"));
    }
    return Ok(vec![path.to_owned()]);
  }

  let ancestor_count = project_directory.ancestors().count();
  let mut paths = Vec::with_capacity(ancestor_count + 4);
  if let Some(directory) = roots.machine_config_directory.as_deref() {
    append_config_fragments(directory, &mut paths, false)?;
  }
  if let Some(directory) = roots.user_settings_directory.as_deref() {
    append_config_fragments(&directory.join("config"), &mut paths, true)?;
    let user = directory.join("NuGet.Config");
    if user.is_file() {
      paths.push(user);
    }
  }

  let mut ancestors: Vec<&Path> = project_directory.ancestors().collect();
  ancestors.reverse();
  for directory in ancestors {
    if let Some(path) = config_path_in(directory) {
      paths.push(path);
    }
  }
  Ok(paths)
}

fn append_config_fragments(directory: &Path, paths: &mut Vec<PathBuf>, exclude_default: bool) -> Result<(), PackageError> {
  let entries = match fs::read_dir(directory) {
    Ok(entries) => entries,
    Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied) => return Ok(()),
    Err(error) => return Err(package_io("enumerate NuGet configuration directory", directory, error)),
  };
  let start = paths.len();
  for entry in entries {
    let entry = entry.map_err(|error| package_io("enumerate NuGet configuration directory", directory, error))?;
    let file_type = entry
      .file_type()
      .map_err(|error| package_io("inspect NuGet configuration entry", &entry.path(), error))?;
    let path = entry.path();
    if (file_type.is_file() || (file_type.is_symlink() && path.is_file())) && is_config_fragment(&path) && !(exclude_default && is_default_config_name(&path)) {
      paths.push(path);
    }
  }
  paths[start..].sort_unstable_by(|left, right| right.file_name().cmp(&left.file_name()));
  Ok(())
}

fn is_default_config_name(path: &Path) -> bool {
  path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
    if cfg!(windows) {
      name.eq_ignore_ascii_case("NuGet.Config")
    } else {
      name == "NuGet.Config"
    }
  })
}

fn is_config_fragment(path: &Path) -> bool {
  let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
    return false;
  };
  if cfg!(windows) {
    extension.eq_ignore_ascii_case("config")
  } else {
    matches!(extension, "Config" | "config")
  }
}

fn config_path_in(directory: &Path) -> Option<PathBuf> {
  const CASE_SENSITIVE_NAMES: [&str; 3] = ["nuget.config", "NuGet.config", "NuGet.Config"];
  let names = if cfg!(windows) {
    &CASE_SENSITIVE_NAMES[2..]
  } else {
    &CASE_SENSITIVE_NAMES[..]
  };
  names.iter().map(|name| directory.join(name)).find(|path| path.is_file())
}

fn default_global_packages() -> Option<PathBuf> {
  env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
    .map(PathBuf::from)
    .map(|path| path.join(".nuget").join("packages"))
}

pub(crate) fn global_packages_directory(project_directory: &Path, explicit_cache: Option<&Path>) -> Result<PathBuf, PackageError> {
  if let Some(cache) = explicit_cache {
    return Ok(cache.to_owned());
  }
  if let Some(cache) = environment_path("NUGET_PACKAGES")? {
    return Ok(cache);
  }
  discover_configuration(project_directory, None, None, &[]).map(|configuration| configuration.cache_root)
}

fn merge_config(path: &Path, merged: &mut NugetConfigMerge) -> Result<(), PackageError> {
  let bytes = fs::read(path).map_err(|error| package_io("read NuGet configuration", path, error))?;
  let config_priority = merged.config_priority;
  let mut reader = Reader::from_reader(bytes.as_slice());
  reader.config_mut().trim_text(true);
  let mut section = ConfigSection::Other;
  let mut pending_mapping = None::<PendingSourceMapping>;
  let mut mapping_sources_in_file = Vec::<String>::new();
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => match local_name(element.name().as_ref()) {
        b"packageSources" => section = ConfigSection::Sources,
        b"disabledPackageSources" => section = ConfigSection::Disabled,
        b"auditSources" => section = ConfigSection::AuditSources,
        b"packageSourceMapping" => section = ConfigSection::SourceMapping,
        b"fallbackPackageFolders" => section = ConfigSection::FallbackFolders,
        b"config" => section = ConfigSection::Config,
        b"packageSource" if matches!(section, ConfigSection::SourceMapping) => {
          begin_source_mapping(
            &reader,
            &element,
            path,
            &merged.source_mapping,
            &mut pending_mapping,
            &mut mapping_sources_in_file,
          )?;
        },
        b"package" if matches!(section, ConfigSection::SourceMapping) => {
          append_source_pattern(&reader, &element, path, &mut merged.source_mapping, pending_mapping.as_ref())?;
        },
        _ => {},
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"clear" => match section {
        ConfigSection::Sources => merged.sources.clear(),
        ConfigSection::Disabled => merged.disabled.clear(),
        ConfigSection::AuditSources => merged.audit_sources.clear(),
        ConfigSection::SourceMapping => merged.source_mapping.clear(),
        ConfigSection::FallbackFolders => merged.fallback_folders.clear(),
        ConfigSection::Config => merged.clear_config(),
        ConfigSection::Other => {},
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"add" => {
        let key = config_attribute(&reader, &element, b"key", path)?.ok_or_else(|| config_error(path, "NuGet add element requires key"))?;
        let value = config_attribute(&reader, &element, b"value", path)?.ok_or_else(|| config_error(path, "NuGet add element requires value"))?;
        let value = expand_config_value(value, path)?;
        match section {
          ConfigSection::Sources | ConfigSection::AuditSources => {
            let protocol = config_attribute(&reader, &element, b"protocolVersion", path)?;
            let source = PackageSource::parse(value, protocol.as_deref(), path, path.parent().unwrap_or(Path::new(".")))?;
            let sources = if matches!(section, ConfigSection::Sources) {
              &mut merged.sources
            } else {
              &mut merged.audit_sources
            };
            add_or_replace_source(sources, key, source);
          },
          ConfigSection::Disabled => {
            if !value.eq_ignore_ascii_case("true") && !value.eq_ignore_ascii_case("false") {
              return Err(config_error(path, "disabled package-source values must be true or false"));
            }
            merged.disabled.retain(|name| !name.eq_ignore_ascii_case(&key));
            merged.disabled.push(key);
          },
          ConfigSection::FallbackFolders => {
            let candidate = resolve_config_path(path, &value);
            add_or_replace_path(&mut merged.fallback_folders, key, candidate, config_priority);
          },
          ConfigSection::Config if key.eq_ignore_ascii_case("globalPackagesFolder") => {
            merged.global_packages = Some(resolve_config_path(path, &value));
          },
          ConfigSection::Config if key.eq_ignore_ascii_case("signatureValidationMode") => {
            merged.signature_validation = Some(if value.eq_ignore_ascii_case("require") {
              SignatureValidationMode::Require
            } else {
              SignatureValidationMode::Accept
            });
          },
          ConfigSection::Config if key.eq_ignore_ascii_case("http_proxy") => {
            merged.proxy_url = Some(value);
          },
          ConfigSection::Config if key.eq_ignore_ascii_case("http_proxy.user") => {
            merged.proxy_user = Some(value);
          },
          ConfigSection::Config if key.eq_ignore_ascii_case("http_proxy.password") => {
            merged.proxy_password = Some(value);
          },
          ConfigSection::Config if key.eq_ignore_ascii_case("no_proxy") => {
            merged.no_proxy = Some(value);
          },
          ConfigSection::Other | ConfigSection::SourceMapping | ConfigSection::Config => {},
        }
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"packageSource" && matches!(section, ConfigSection::SourceMapping) => {
        return Err(config_error(path, "NuGet package-source mapping requires at least one package pattern"));
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"package" && matches!(section, ConfigSection::SourceMapping) => {
        append_source_pattern(&reader, &element, path, &mut merged.source_mapping, pending_mapping.as_ref())?;
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"remove" => {
        let key = config_attribute(&reader, &element, b"key", path)?.ok_or_else(|| config_error(path, "NuGet remove element requires key"))?;
        match section {
          ConfigSection::Sources => merged.sources.retain(|(name, _)| !name.eq_ignore_ascii_case(&key)),
          ConfigSection::Disabled => merged.disabled.retain(|name| !name.eq_ignore_ascii_case(&key)),
          ConfigSection::AuditSources => merged.audit_sources.retain(|(name, _)| !name.eq_ignore_ascii_case(&key)),
          ConfigSection::SourceMapping => merged.source_mapping.remove(&key),
          ConfigSection::FallbackFolders => merged.fallback_folders.retain(|folder| !folder.name.eq_ignore_ascii_case(&key)),
          ConfigSection::Config => merged.remove_config(&key),
          ConfigSection::Other => {},
        }
      },
      Ok(Event::End(element)) => match local_name(element.name().as_ref()) {
        b"packageSource" if matches!(section, ConfigSection::SourceMapping) => {
          let pending = pending_mapping
            .take()
            .ok_or_else(|| config_error(path, "NuGet package-source mapping ended without a source"))?;
          merged.source_mapping.finish_source(pending, path)?;
        },
        b"packageSources" | b"disabledPackageSources" | b"auditSources" | b"packageSourceMapping" | b"fallbackPackageFolders" | b"config" => {
          if pending_mapping.is_some() {
            return Err(config_error(path, "NuGet package-source mapping did not close its source"));
          }
          section = ConfigSection::Other;
        },
        _ => {},
      },
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(config_error(path, format!("invalid NuGet configuration XML: {error}"))),
    }
  }
  merged.config_priority = config_priority
    .checked_add(1)
    .ok_or_else(|| config_error(path, "NuGet configuration file count exceeds u32"))?;
  Ok(())
}

fn add_or_replace_source(sources: &mut Vec<(String, PackageSource)>, key: String, source: PackageSource) {
  if let Some((name, existing)) = sources.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(&key)) {
    *name = key;
    *existing = source;
  } else {
    sources.push((key, source));
  }
}

fn add_or_replace_path(paths: &mut Vec<FallbackFolder>, key: String, path: PathBuf, config_priority: u32) {
  if let Some(existing) = paths.iter_mut().find(|folder| folder.name.eq_ignore_ascii_case(&key)) {
    existing.name = key;
    existing.path = path;
    existing.config_priority = config_priority;
  } else {
    paths.push(FallbackFolder {
      name: key,
      path,
      config_priority,
    });
  }
}

fn resolve_config_path(config_path: &Path, value: &str) -> PathBuf {
  let candidate = PathBuf::from(value);
  if candidate.is_absolute() {
    candidate
  } else {
    config_path.parent().unwrap_or(Path::new(".")).join(candidate)
  }
}

impl NugetConfigMerge {
  fn clear_config(&mut self) {
    self.global_packages = None;
    self.signature_validation = None;
    self.proxy_url = None;
    self.proxy_user = None;
    self.proxy_password = None;
    self.no_proxy = None;
  }

  fn remove_config(&mut self, key: &str) {
    if key.eq_ignore_ascii_case("globalPackagesFolder") {
      self.global_packages = None;
    } else if key.eq_ignore_ascii_case("signatureValidationMode") {
      self.signature_validation = None;
    } else if key.eq_ignore_ascii_case("http_proxy") {
      self.proxy_url = None;
    } else if key.eq_ignore_ascii_case("http_proxy.user") {
      self.proxy_user = None;
    } else if key.eq_ignore_ascii_case("http_proxy.password") {
      self.proxy_password = None;
    } else if key.eq_ignore_ascii_case("no_proxy") {
      self.no_proxy = None;
    }
  }
}

fn begin_source_mapping(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  path: &Path,
  mapping: &MergedSourceMapping,
  pending: &mut Option<PendingSourceMapping>,
  seen_in_file: &mut Vec<String>,
) -> Result<(), PackageError> {
  if pending.is_some() {
    return Err(config_error(path, "NuGet package-source mappings cannot be nested"));
  }
  let source = config_attribute(reader, element, b"key", path)?.ok_or_else(|| config_error(path, "NuGet package-source mapping requires a key"))?;
  if source.trim().is_empty() {
    return Err(config_error(path, "NuGet package-source mapping key cannot be empty"));
  }
  if seen_in_file.iter().any(|seen| seen.eq_ignore_ascii_case(&source)) {
    return Err(config_error(path, format!("NuGet package-source mapping contains duplicate source {source:?}")));
  }
  seen_in_file.push(source.clone());
  *pending = Some(PendingSourceMapping {
    source,
    pattern_start: mapping.patterns.len(),
  });
  Ok(())
}

fn append_source_pattern(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  path: &Path,
  mapping: &mut MergedSourceMapping,
  pending: Option<&PendingSourceMapping>,
) -> Result<(), PackageError> {
  if pending.is_none() {
    return Err(config_error(path, "NuGet package pattern must be inside a packageSource mapping"));
  }
  let pattern = config_attribute(reader, element, b"pattern", path)?.ok_or_else(|| config_error(path, "NuGet package mapping requires a pattern"))?;
  if pattern.trim().is_empty() || pattern.encode_utf16().count() > 100 {
    return Err(config_error(path, "NuGet package mapping pattern must contain 1 to 100 UTF-16 code units"));
  }
  mapping.patterns.push(pattern);
  Ok(())
}

impl MergedSourceMapping {
  fn clear(&mut self) {
    self.sources.clear();
    self.patterns.clear();
  }

  fn remove(&mut self, key: &str) {
    self.sources.retain(|source| !source.source.eq_ignore_ascii_case(key));
  }

  fn finish_source(&mut self, pending: PendingSourceMapping, path: &Path) -> Result<(), PackageError> {
    let pattern_count = self.patterns.len().saturating_sub(pending.pattern_start);
    if pattern_count == 0 {
      return Err(config_error(
        path,
        format!("NuGet package-source mapping {:?} requires at least one package pattern", pending.source),
      ));
    }
    let patterns = ItemRange {
      start: u32_len(pending.pattern_start, "package-source mapping pattern index")?,
      len: u32_len(pattern_count, "package-source mapping pattern count")?,
    };
    if let Some(existing) = self.sources.iter_mut().find(|source| source.source.eq_ignore_ascii_case(&pending.source)) {
      existing.source = pending.source;
      existing.patterns = patterns;
    } else {
      self.sources.push(MergedSourceMappingEntry {
        source: pending.source,
        patterns,
      });
    }
    Ok(())
  }

  #[cfg(test)]
  fn patterns_for(&self, key: &str) -> Option<&[String]> {
    let source = self.sources.iter().find(|source| source.source.eq_ignore_ascii_case(key))?;
    let start = source.patterns.start as usize;
    Some(&self.patterns[start..start + source.patterns.len as usize])
  }
}

impl PackageSourceMapping {
  fn compile(merged: MergedSourceMapping, configured_sources: &[(String, PackageSource)]) -> Result<Self, PackageError> {
    let pattern_capacity = merged.sources.iter().map(|source| source.patterns.len as usize).sum();
    let text_capacity = merged
      .sources
      .iter()
      .flat_map(|source| &merged.patterns[range(source.patterns)])
      .map(|pattern| pattern.trim().find('*').unwrap_or(pattern.trim().len()))
      .sum();
    let mut text = TextTable::with_capacity(text_capacity);
    let mut sources = Vec::with_capacity(merged.sources.len());
    let mut patterns = Vec::with_capacity(pattern_capacity);
    for source in merged.sources {
      let source_index = match configured_sources.iter().position(|(name, _)| name.eq_ignore_ascii_case(&source.source)) {
        Some(index) => {
          let index = u32_len(index, "NuGet package-source index")?;
          if index == u32::MAX {
            return Err(PackageError::new(
              PackageErrorKind::TextOverflow,
              "NuGet package sources",
              "NuGet package-source count reserves u32::MAX for an unavailable mapping",
            ));
          }
          index
        },
        None => u32::MAX,
      };
      let start = patterns.len();
      for pattern in &merged.patterns[range(source.patterns)] {
        let pattern = pattern.trim();
        let (pattern, prefix) = pattern.find('*').map_or((pattern, false), |star| (&pattern[..star], true));
        patterns.push(SourcePattern {
          text: text.push(pattern)?,
          prefix,
        });
      }
      sources.push(SourceMappingEntry {
        source_index,
        patterns: ItemRange {
          start: u32_len(start, "NuGet package-source pattern index")?,
          len: u32_len(patterns.len() - start, "NuGet package-source pattern count")?,
        },
      });
    }
    Ok(Self {
      text: text.text.into_boxed_str(),
      sources: sources.into_boxed_slice(),
      patterns: patterns.into_boxed_slice(),
    })
  }

  /// Mapping is parsed once, then queried for every graph identity. The query
  /// scans only compact records and the shared text buffer; it never allocates.
  fn allows(&self, source_index: u32, package_id: &str) -> bool {
    let mut best = 0usize;
    let mut source_best = 0usize;
    for source in &self.sources {
      let is_source = source.source_index == source_index;
      for pattern in &self.patterns[range(source.patterns)] {
        let Some(rank) = self.pattern_rank(*pattern, package_id) else {
          continue;
        };
        best = best.max(rank);
        if is_source {
          source_best = source_best.max(rank);
        }
      }
    }
    best != 0 && source_best == best
  }

  fn pattern_rank(&self, pattern: SourcePattern, package_id: &str) -> Option<usize> {
    let start = pattern.text.start as usize;
    let text = &self.text[start..start + pattern.text.len as usize];
    if pattern.prefix {
      package_id
        .get(..text.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(text))
        .map(|_| text.len() + 1)
    } else if text.eq_ignore_ascii_case(package_id) {
      Some(usize::MAX)
    } else {
      None
    }
  }
}

#[derive(Clone, Copy)]
enum ConfigSection {
  Other,
  Sources,
  Disabled,
  AuditSources,
  SourceMapping,
  FallbackFolders,
  Config,
}

fn config_attribute(reader: &Reader<&[u8]>, element: &quick_xml::events::BytesStart<'_>, name: &[u8], path: &Path) -> Result<Option<String>, PackageError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| config_error(path, format!("invalid NuGet attribute: {error}")))?;
    if local_name(attribute.key.as_ref()) == name {
      return attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
        .map(|value| Some(value.into_owned()))
        .map_err(|error| config_error(path, format!("invalid NuGet attribute value: {error}")));
    }
  }
  Ok(None)
}

fn expand_config_value(value: String, path: &Path) -> Result<String, PackageError> {
  expand_config_value_with(value, path, |name| env::var_os(name))
}

fn expand_config_value_with(value: String, path: &Path, mut lookup: impl FnMut(&str) -> Option<std::ffi::OsString>) -> Result<String, PackageError> {
  let bytes = value.as_bytes();
  let Some(mut marker) = bytes.iter().position(|byte| *byte == b'%') else {
    return Ok(value);
  };
  let mut copied = 0usize;
  let mut expanded = None::<String>;
  while marker + 1 < bytes.len() {
    let Some(end_offset) = bytes[marker + 1..].iter().position(|byte| *byte == b'%') else {
      break;
    };
    let end = marker + 1 + end_offset;
    if end > marker + 1 {
      let name = &value[marker + 1..end];
      if let Some(replacement) = lookup(name) {
        let replacement = replacement
          .to_str()
          .ok_or_else(|| config_error(path, format!("NuGet environment variable {name:?} is not valid Unicode")))?;
        let output = expanded.get_or_insert_with(|| String::with_capacity(value.len().saturating_add(replacement.len())));
        output.push_str(&value[copied..marker]);
        output.push_str(replacement);
        copied = end + 1;
      }
    }
    marker = match bytes[end + 1..].iter().position(|byte| *byte == b'%') {
      Some(next) => end + 1 + next,
      None => break,
    };
  }
  if let Some(mut output) = expanded {
    output.push_str(&value[copied..]);
    Ok(output)
  } else {
    Ok(value)
  }
}

fn config_error(path: &Path, message: impl Into<String>) -> PackageError {
  PackageError::new(PackageErrorKind::Configuration, path.display().to_string(), message)
}

fn http_client(proxy: Option<&ProxySettings>) -> Result<reqwest::Client, PackageError> {
  let mut builder = reqwest::Client::builder().https_only(true).timeout(Duration::from_secs(60));
  if let Some(settings) = proxy {
    let mut configured =
      reqwest::Proxy::all(&settings.url).map_err(|error| config_error(Path::new("http_proxy"), format!("invalid NuGet proxy address: {error}")))?;
    configured = configured.no_proxy(settings.no_proxy.as_deref().and_then(reqwest::NoProxy::from_string));
    builder = builder.no_proxy().proxy(configured);
  }
  builder
    .build()
    .map_err(|error| network_error("HTTP client", format!("failed to create HTTP client: {error}")))
}

async fn discover_service_endpoints(
  client: &reqwest::Client,
  sources: &[(String, PackageSource)],
  allow_network: bool,
) -> Result<(Vec<ServiceEndpoint>, u32), PackageError> {
  let mut local = Vec::with_capacity(sources.len());
  let mut remote = Vec::with_capacity(sources.len());
  let mut requests = 0;
  for (index, (_, source)) in sources.iter().enumerate() {
    let source_index = u32_len(index, "NuGet package-source index")?;
    match source.protocol {
      NugetProtocol::Local => {
        let root = PathBuf::from(&source.url);
        local.push(ServiceEndpoint::Local {
          source: source.url.clone(),
          layout: detect_local_feed_layout(&root)?,
          root,
          source_index,
        });
      },
      NugetProtocol::V2 if allow_network => remote.push(ServiceEndpoint::V2 {
        source: source.url.clone(),
        base: with_trailing_slash(source.url.clone()),
        source_index,
      }),
      NugetProtocol::V3 if allow_network => {
        let (services, _) = fetch_v3_service_index(client, &source.url).await?;
        requests += 1;
        let services = Arc::new(services);
        if services.package_base_address().is_none() {
          return Err(network_error(&source.url, "NuGet v3 source has no compatible PackageBaseAddress resource"));
        }
        remote.push(ServiceEndpoint::V3 {
          source: source.url.clone(),
          services,
          source_index,
        });
      },
      NugetProtocol::V2 | NugetProtocol::V3 => {},
    }
  }
  local.extend(remote);
  Ok((local, requests))
}

fn detect_local_feed_layout(root: &Path) -> Result<LocalFeedLayout, PackageError> {
  let entries = match fs::read_dir(root) {
    Ok(entries) => entries,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(LocalFeedLayout::Unknown),
    Err(error) => return Err(package_io("enumerate local package source", root, error)),
  };
  let mut flat = Vec::new();
  let mut identity_roots = Vec::new();
  for entry in entries {
    let entry = entry.map_err(|error| package_io("enumerate local package source", root, error))?;
    let file_type = entry
      .file_type()
      .map_err(|error| package_io("inspect local package source entry", &entry.path(), error))?;
    if file_type.is_file() && has_nupkg_extension(&entry.path()) {
      flat.push(entry.path());
    } else if file_type.is_dir() {
      identity_roots.push(entry.path());
    }
  }

  let mut hierarchical = false;
  for identity_root in identity_roots {
    let entries = fs::read_dir(&identity_root).map_err(|error| package_io("enumerate local package-source identity", &identity_root, error))?;
    for entry in entries {
      let entry = entry.map_err(|error| package_io("enumerate local package-source identity", &identity_root, error))?;
      let file_type = entry
        .file_type()
        .map_err(|error| package_io("inspect local package-source identity entry", &entry.path(), error))?;
      if file_type.is_file() && has_nupkg_extension(&entry.path()) {
        flat.push(entry.path());
      } else if file_type.is_dir() && hierarchical_source_entry_exists(&identity_root, &entry.path()) {
        hierarchical = true;
      }
    }
  }
  if !flat.is_empty() {
    flat.sort_unstable();
    return Ok(LocalFeedLayout::Flat(flat.into()));
  }
  Ok(if hierarchical {
    LocalFeedLayout::Hierarchical
  } else {
    LocalFeedLayout::Unknown
  })
}

fn has_nupkg_extension(path: &Path) -> bool {
  path
    .extension()
    .and_then(|extension| extension.to_str())
    .is_some_and(|extension| extension.eq_ignore_ascii_case("nupkg"))
}

fn hierarchical_source_entry_exists(identity_root: &Path, version_root: &Path) -> bool {
  let Some(id) = identity_root.file_name().and_then(|value| value.to_str()) else {
    return false;
  };
  let Some(version) = version_root.file_name().and_then(|value| value.to_str()) else {
    return false;
  };
  let Ok(version) = PackageVersion::parse(version) else {
    return false;
  };
  let lower_id = id.to_ascii_lowercase();
  let stem = format!("{lower_id}.{}", version.normalized);
  version_root.join(format!("{stem}.nupkg")).is_file()
    && version_root.join(format!("{lower_id}.nuspec")).is_file()
    && version_root.join(format!("{stem}.nupkg.sha512")).is_file()
}

fn service_types(capability: ServiceCapability) -> &'static [&'static str] {
  match capability {
    ServiceCapability::PackageBase => PACKAGE_BASE_TYPES,
    ServiceCapability::Registration => REGISTRATION_TYPES,
    ServiceCapability::Search => SEARCH_TYPES,
    ServiceCapability::Vulnerability => VULNERABILITY_TYPES,
    ServiceCapability::PackagePublish => PACKAGE_PUBLISH_TYPES,
  }
}

async fn fetch_v3_service_index(client: &reqwest::Client, source: &str) -> Result<(NugetServiceEndpoints, u64), PackageError> {
  let bytes = get_bytes(client, source, MAX_JSON_BYTES, "NuGet service index").await?;
  let document: serde_json::Value =
    serde_json::from_slice(&bytes).map_err(|error| network_error(source, format!("invalid NuGet service-index JSON: {error}")))?;
  let endpoints = parse_v3_service_index(source, &document)?;
  Ok((endpoints, bytes.len() as u64))
}

fn parse_v3_service_index(source: &str, document: &serde_json::Value) -> Result<NugetServiceEndpoints, PackageError> {
  let schema = document
    .get("version")
    .and_then(serde_json::Value::as_str)
    .ok_or_else(|| network_error(source, "NuGet service index has no schema version"))?;
  let schema_version =
    PackageVersion::parse(schema).map_err(|error| network_error(source, format!("invalid NuGet service-index schema version {schema:?}: {error}")))?;
  if schema_version.numbers[0] != 3 {
    return Err(network_error(
      source,
      format!("unsupported NuGet service-index schema version {schema:?}; expected major version 3"),
    ));
  }
  let resources = document
    .get("resources")
    .and_then(serde_json::Value::as_array)
    .ok_or_else(|| network_error(source, "NuGet service index has no resources array"))?;
  if resources.len() > MAX_ARCHIVE_ENTRIES {
    return Err(network_error(source, "NuGet service index exceeds the resource count limit"));
  }
  // This is a protocol-compatibility level, not a selected .NET SDK version.
  // It is isolated here so adding a newly implemented NuGet contract changes
  // one boundary instead of scattering version checks through consumers.
  let supported_client = PackageVersion::parse(NUGET_PROTOCOL_CLIENT_VERSION).expect("the supported NuGet protocol version is valid");
  let mut text = TextTable::with_capacity(resources.len().saturating_mul(64));
  let mut endpoints = Vec::new();
  let mut ranges = [ItemRange { start: 0, len: 0 }; SERVICE_CAPABILITY_COUNT];
  for capability in [
    ServiceCapability::PackageBase,
    ServiceCapability::Registration,
    ServiceCapability::Search,
    ServiceCapability::Vulnerability,
    ServiceCapability::PackagePublish,
  ] {
    let start = u32_len(endpoints.len(), "NuGet service endpoint range")?;
    append_selected_service_endpoints(resources, service_types(capability), &supported_client, &mut text, &mut endpoints)?;
    ranges[capability as usize] = ItemRange {
      start,
      len: u32_len(endpoints.len() - start as usize, "NuGet service endpoint range")?,
    };
  }
  Ok(NugetServiceEndpoints {
    text: text.text.into_boxed_str(),
    entries: endpoints.into_boxed_slice(),
    ranges,
  })
}

fn append_selected_service_endpoints(
  resources: &[serde_json::Value],
  ordered_types: &[&str],
  supported_client: &PackageVersion,
  text: &mut TextTable,
  endpoints: &mut Vec<TextSpan>,
) -> Result<(), PackageError> {
  for resource_type in ordered_types {
    let best = resources
      .iter()
      .filter(|resource| resource_type_matches(resource.get("@type"), resource_type))
      .filter(|resource| valid_service_location(resource).is_some())
      .filter_map(|resource| best_compatible_client_version(resource.get("clientVersion"), supported_client))
      .max();
    let Some(best) = best else {
      continue;
    };
    for resource in resources.iter().filter(|resource| resource_type_matches(resource.get("@type"), resource_type)) {
      if !resource_supports_client_version(resource.get("clientVersion"), &best) {
        continue;
      }
      let Some(location) = valid_service_location(resource) else {
        continue;
      };
      let url = reqwest::Url::parse(location).expect("selected NuGet service location was validated");
      if !url.username().is_empty() || url.password().is_some() {
        return Err(network_error(
          location,
          format!("NuGet service resource {resource_type} must not embed credentials in its URL"),
        ));
      }
      if url.scheme() != "https" {
        return Err(network_error(
          location,
          format!("NuGet service resource {resource_type} uses insecure HTTP; explicit opt-in remains tracked by NUGET-012"),
        ));
      }
      endpoints.push(text.push(location)?);
    }
    return Ok(());
  }
  Ok(())
}

fn valid_service_location(resource: &serde_json::Value) -> Option<&str> {
  let location = resource.get("@id").and_then(serde_json::Value::as_str)?;
  let url = reqwest::Url::parse(location).ok()?;
  (url.has_host() && matches!(url.scheme(), "http" | "https") && url.username().is_empty() && url.password().is_none()).then_some(location)
}

fn best_compatible_client_version(value: Option<&serde_json::Value>, supported: &PackageVersion) -> Option<PackageVersion> {
  match value {
    None => Some(zero_package_version()),
    Some(serde_json::Value::String(value)) => PackageVersion::parse(value).ok().filter(|version| version <= supported),
    Some(serde_json::Value::Array(values)) => values
      .iter()
      .filter_map(serde_json::Value::as_str)
      .filter_map(|value| PackageVersion::parse(value).ok())
      .filter(|version| version <= supported)
      .max(),
    Some(_) => None,
  }
}

fn resource_supports_client_version(value: Option<&serde_json::Value>, selected: &PackageVersion) -> bool {
  match value {
    None => selected == &zero_package_version(),
    Some(serde_json::Value::String(value)) => PackageVersion::parse(value).is_ok_and(|version| &version == selected),
    Some(serde_json::Value::Array(values)) => values
      .iter()
      .filter_map(serde_json::Value::as_str)
      .any(|value| PackageVersion::parse(value).is_ok_and(|version| &version == selected)),
    Some(_) => false,
  }
}

fn zero_package_version() -> PackageVersion {
  PackageVersion {
    normalized: "0.0.0".to_owned(),
    numbers: [0; 4],
    prerelease_start: None,
  }
}

fn resource_type_matches(value: Option<&serde_json::Value>, expected: &str) -> bool {
  match value {
    Some(serde_json::Value::String(value)) => value == expected,
    Some(serde_json::Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(expected)),
    _ => false,
  }
}

fn with_trailing_slash(mut value: String) -> String {
  if !value.ends_with('/') {
    value.push('/');
  }
  value
}

async fn resolve_streaming_graph(
  client: &reqwest::Client,
  direct: &[PackageRequirement],
  config: &NugetConfiguration,
  options: &PackageResolveOptions,
  target: TargetFramework,
  target_text: &str,
  pruning: &PackagePruning,
) -> Result<ResolvedGraph, PackageError> {
  let mut nodes = BTreeMap::<String, ConstraintNode>::new();
  let mut dirty = BTreeSet::new();
  for request in direct {
    nodes.insert(
      request.lower_id.clone(),
      ConstraintNode {
        id: request.id.clone(),
        direct: Some(request.range.clone()),
        constraints: BTreeMap::new(),
        selected: None,
        metadata_version: None,
        dependencies: Vec::new(),
        available_versions: None,
        pruned: false,
        generation: 0,
      },
    );
    dirty.insert(request.lower_id.clone());
  }
  let mut ready = BTreeSet::new();
  stabilize_constraint_nodes(&mut nodes, &mut dirty, &mut ready, pruning)?;
  let mut endpoints: Option<Arc<[ServiceEndpoint]>> = None;
  let mut network_requests = 0;
  let mut downloaded_bytes = 0;
  let mut metadata_packages = BTreeMap::<(String, String), CachedPackage>::new();
  let mut tasks = JoinSet::new();
  let mut in_flight = BTreeSet::new();

  while !ready.is_empty() || !tasks.is_empty() {
    while tasks.len() < MAX_DOWNLOAD_WORKERS {
      let Some(lower_id) = ready
        .iter()
        .find(|id| !in_flight.contains(*id) && nodes.get(*id).is_some_and(|node| !node.pruned))
        .cloned()
      else {
        break;
      };
      ready.remove(&lower_id);
      let Some(node) = nodes.get(&lower_id) else {
        continue;
      };
      let request = node.selected.as_ref().map(|version| PackageRequest {
        id: node.id.clone(),
        lower_id: lower_id.clone(),
        version: version.normalized.clone(),
        direct: node.direct.is_some(),
      });
      let cache_miss = request
        .as_ref()
        .is_none_or(|request| find_package_root(&config.cache_root, &config.fallback_roots, request).is_none());
      if cache_miss && endpoints.is_none() {
        if !config.cache_root.is_dir() {
          fs::create_dir_all(&config.cache_root).map_err(|error| package_io("create package cache", &config.cache_root, error))?;
        }
        let (discovered, requests) = discover_service_endpoints(client, &config.sources, !options.offline).await?;
        network_requests += requests;
        endpoints = Some(discovered.into());
      }

      let task_client = client.clone();
      let task_cache_root = config.cache_root.clone();
      let task_fallback_roots = Arc::clone(&config.fallback_roots);
      let task_temp_root = config.temp_root.clone();
      let task_endpoints = endpoints.clone().unwrap_or_else(|| Arc::from([]));
      let task_source_mapping = config.source_mapping.clone();
      let generation = node.generation;
      let task_version = request.as_ref().map(|request| request.version.clone());
      let task_target = target;
      in_flight.insert(lower_id.clone());
      tasks.spawn(async move {
        let storage = PackageStorage {
          cache_root: &task_cache_root,
          fallback_roots: &task_fallback_roots,
          temp_root: &task_temp_root,
        };
        let result = load_node_metadata(
          &task_client,
          request.as_ref(),
          &lower_id,
          storage,
          &task_endpoints,
          task_source_mapping.as_deref(),
          task_target,
        )
        .await;
        (lower_id, generation, task_version, result)
      });
    }

    let (lower_id, generation, task_version, result) = tasks.join_next().await.ok_or_else(package_worker_stopped)?.map_err(|error| {
      PackageError::new(
        PackageErrorKind::Io,
        "package scheduler",
        format!("package metadata task stopped before the graph completed: {error}"),
      )
    })?;
    in_flight.remove(&lower_id);
    let stale = nodes.get(&lower_id).is_none_or(|node| node.generation != generation);
    let result = match result {
      Ok(result) => result,
      Err(_) if stale => {
        if nodes.get(&lower_id).is_some_and(|node| !node.pruned) {
          ready.insert(lower_id);
        }
        continue;
      },
      Err(error) => return Err(error),
    };
    match result {
      MetadataTaskResult::Versions { versions, requests, bytes } => {
        network_requests += requests;
        downloaded_bytes += bytes;
        if let Some(node) = nodes.get_mut(&lower_id) {
          node.available_versions = Some(versions);
          dirty.insert(lower_id);
        }
      },
      MetadataTaskResult::Requirements {
        dependencies,
        requests,
        bytes,
        package,
      } => {
        network_requests += requests;
        downloaded_bytes += bytes;
        if let Some(package) = package
          && let Some(version) = task_version
        {
          metadata_packages.insert((lower_id.clone(), version), package);
        }
        if stale {
          if nodes.get(&lower_id).is_some_and(|node| !node.pruned) {
            ready.insert(lower_id.clone());
          }
        } else {
          install_node_dependencies(&lower_id, generation, dependencies, &mut nodes, &mut dirty)?;
        }
      },
    }
    stabilize_constraint_nodes(&mut nodes, &mut dirty, &mut ready, pruning)?;
  }

  let mut exact = BTreeMap::new();
  for (lower_id, node) in &nodes {
    if node.pruned {
      continue;
    }
    let version = node
      .selected
      .as_ref()
      .ok_or_else(|| resolution_error(&node.id, "package graph did not select a version"))?;
    exact.insert(
      lower_id.clone(),
      PackageRequest {
        id: node.id.clone(),
        lower_id: lower_id.clone(),
        version: version.normalized.clone(),
        direct: node.direct.is_some(),
      },
    );
  }

  let mut acquisition = BTreeMap::new();
  let mut acquired = BTreeMap::<String, (PackageRequest, CachedPackage)>::new();
  for (lower_id, request) in exact {
    match metadata_packages.remove(&(lower_id.clone(), request.version.clone())) {
      Some(cached) => {
        acquired.insert(lower_id, (request, cached));
      },
      _ => {
        acquisition.insert(lower_id, request);
      },
    }
  }
  let mut acquisition_tasks = JoinSet::new();
  while !acquisition.is_empty() || !acquisition_tasks.is_empty() {
    while acquisition_tasks.len() < MAX_DOWNLOAD_WORKERS
      && let Some((_, request)) = acquisition.pop_first()
    {
      let task_client = client.clone();
      let task_cache_root = config.cache_root.clone();
      let task_fallback_roots = Arc::clone(&config.fallback_roots);
      let task_temp_root = config.temp_root.clone();
      let task_endpoints = endpoints.clone().unwrap_or_else(|| Arc::from([]));
      let task_source_mapping = config.source_mapping.clone();
      let parallel_extract = acquisition_tasks.is_empty() && acquisition.is_empty();
      acquisition_tasks.spawn(async move {
        let storage = PackageStorage {
          cache_root: &task_cache_root,
          fallback_roots: &task_fallback_roots,
          temp_root: &task_temp_root,
        };
        let result = ensure_package(
          &task_client,
          &request,
          storage,
          &task_endpoints,
          task_source_mapping.as_deref(),
          target,
          parallel_extract,
        )
        .await;
        (request, result)
      });
    }
    let (request, cached) = acquisition_tasks
      .join_next()
      .await
      .ok_or_else(package_worker_stopped)?
      .map_err(package_blocking_task_error)?;
    let cached = cached?;
    network_requests += cached.requests;
    downloaded_bytes += cached.bytes;
    acquired.insert(request.lower_id.clone(), (request, cached));
  }

  let asset_flags = flatten_asset_flags(&nodes);
  let mut resolved = BTreeMap::<String, WorkPackage>::new();
  for (lower_id, (request, cached)) in acquired {
    let dependencies = concrete_dependencies(&nodes, &request.lower_id)?;
    let flags = asset_flags.get(&lower_id).copied().unwrap_or(AssetFlags::NONE);
    let parsed = parse_cached_package(request.clone(), cached, target, target_text, dependencies, flags)?;
    resolved.insert(lower_id, parsed);
  }

  Ok(ResolvedGraph {
    packages: resolved,
    network_requests,
    downloaded_bytes,
  })
}

fn flatten_asset_flags(nodes: &BTreeMap<String, ConstraintNode>) -> BTreeMap<String, AssetFlags> {
  let mut result = BTreeMap::<String, AssetFlags>::new();
  let mut queue = VecDeque::<(&str, AssetFlags)>::new();
  for (id, node) in nodes {
    if node.direct.is_some() && !node.pruned {
      queue.push_back((id, AssetFlags::ALL));
    }
  }
  while let Some((id, incoming)) = queue.pop_front() {
    let current = result.get(id).copied().unwrap_or(AssetFlags::NONE);
    if current.contains(incoming) {
      continue;
    }
    result.insert(id.to_owned(), current.union(incoming));
    let Some(node) = nodes.get(id) else {
      continue;
    };
    for dependency in &node.dependencies {
      if dependency.suppress_parent == AssetFlags::ALL || nodes.get(&dependency.lower_id).is_some_and(|node| node.pruned) {
        continue;
      }
      queue.push_back((
        &dependency.lower_id,
        incoming.intersect(dependency.include_assets).without(dependency.suppress_parent),
      ));
    }
  }
  result
}

fn select_node_version(node: &ConstraintNode) -> Result<NodeSelection, PackageError> {
  fn consider_lower<'a>(candidate: &mut Option<&'a PackageVersion>, range: &'a VersionRange) {
    if let Some(lower) = &range.lower
      && lower.inclusive
      && candidate.is_none_or(|candidate| lower.version > *candidate)
    {
      *candidate = Some(&lower.version);
    }
  }

  let allows_prerelease = node.direct.as_ref().map_or_else(
    || node.constraints.values().any(VersionRange::allows_prerelease),
    VersionRange::allows_prerelease,
  );
  let accepts = |version: &PackageVersion| {
    (version.prerelease().is_none() || allows_prerelease)
      && node.direct.as_ref().map_or_else(
        || node.constraints.values().all(|range| range.contains(version)),
        |direct| direct.contains(version),
      )
  };
  if let Some(versions) = &node.available_versions {
    return versions
      .iter()
      .find(|version| accepts(version))
      .cloned()
      .map(NodeSelection::Version)
      .ok_or_else(|| resolution_error(&node.id, "no available package version satisfies the dependency constraints"));
  }
  let mut candidate = None::<&PackageVersion>;
  if let Some(direct) = &node.direct {
    consider_lower(&mut candidate, direct);
  } else {
    for range in node.constraints.values() {
      consider_lower(&mut candidate, range);
    }
  }
  match candidate {
    Some(candidate) if accepts(candidate) => Ok(NodeSelection::Version(candidate.clone())),
    Some(_) if node.direct.is_none() => Err(resolution_error(&node.id, "dependency version ranges have no common version")),
    _ => Ok(NodeSelection::Enumerate),
  }
}

fn stabilize_constraint_nodes(
  nodes: &mut BTreeMap<String, ConstraintNode>,
  dirty: &mut BTreeSet<String>,
  ready: &mut BTreeSet<String>,
  pruning: &PackagePruning,
) -> Result<(), PackageError> {
  while let Some(lower_id) = dirty.pop_first() {
    let Some(node) = nodes.get(&lower_id) else {
      continue;
    };
    if node.direct.is_none() && node.constraints.is_empty() {
      let removed = nodes.remove(&lower_id).expect("a checked node exists");
      ready.remove(&lower_id);
      for dependency in removed.dependencies {
        if let Some(child) = nodes.get_mut(&dependency.lower_id) {
          child.constraints.remove(&lower_id);
          dirty.insert(dependency.lower_id);
        }
      }
      continue;
    }
    let selection = select_node_version(node)?;
    let next = match selection {
      NodeSelection::Version(version) => Some(version),
      NodeSelection::Enumerate => None,
    };
    let pruned = next
      .as_ref()
      .is_some_and(|version| node.direct.is_none() && pruning.contains(&lower_id, version));
    if next.is_some() && nodes.get(&lower_id).is_some_and(|node| node.selected == next && node.pruned == pruned) {
      continue;
    }
    let node = nodes.get_mut(&lower_id).expect("a checked node exists");
    let previous_dependencies = std::mem::take(&mut node.dependencies);
    node.selected = next;
    node.pruned = pruned;
    node.metadata_version = None;
    node.generation = node
      .generation
      .checked_add(1)
      .filter(|generation| *generation <= MAX_GRAPH_REVISIONS)
      .ok_or_else(|| resolution_error(&node.id, "package dependency graph did not converge"))?;
    if pruned {
      ready.remove(&lower_id);
    } else {
      ready.insert(lower_id.clone());
    }
    for dependency in previous_dependencies {
      if let Some(child) = nodes.get_mut(&dependency.lower_id) {
        child.constraints.remove(&lower_id);
        dirty.insert(dependency.lower_id);
      }
    }
  }
  Ok(())
}

fn install_node_dependencies(
  lower_id: &str,
  generation: u32,
  dependencies: Vec<PackageRequirement>,
  nodes: &mut BTreeMap<String, ConstraintNode>,
  dirty: &mut BTreeSet<String>,
) -> Result<(), PackageError> {
  let mut unique = BTreeMap::<String, PackageRequirement>::new();
  for dependency in dependencies {
    if let Some(existing) = unique.insert(dependency.lower_id.clone(), dependency.clone())
      && existing.range != dependency.range
    {
      return Err(resolution_error(
        &dependency.id,
        "a package dependency group contains duplicate identities with different ranges",
      ));
    }
  }
  let dependencies: Vec<_> = unique.into_values().collect();
  {
    let node = nodes
      .get_mut(lower_id)
      .ok_or_else(|| resolution_error(lower_id, "package graph node disappeared"))?;
    if node.generation != generation {
      return Ok(());
    }
    node.metadata_version = node.selected.clone();
    node.dependencies = dependencies.clone();
  }
  for dependency in dependencies {
    let child = nodes.entry(dependency.lower_id.clone()).or_insert_with(|| ConstraintNode {
      id: dependency.id.clone(),
      direct: None,
      constraints: BTreeMap::new(),
      selected: None,
      metadata_version: None,
      dependencies: Vec::new(),
      available_versions: None,
      pruned: false,
      generation: 0,
    });
    child.constraints.insert(lower_id.to_owned(), dependency.range);
    dirty.insert(dependency.lower_id);
  }
  Ok(())
}

fn concrete_dependencies(nodes: &BTreeMap<String, ConstraintNode>, lower_id: &str) -> Result<Vec<PackageRequest>, PackageError> {
  nodes[lower_id]
    .dependencies
    .iter()
    .filter_map(|dependency| {
      let node = match nodes.get(&dependency.lower_id) {
        Some(node) => node,
        None => return Some(Err(resolution_error(&dependency.id, "dependency graph has no selected package"))),
      };
      if node.pruned {
        return None;
      }
      let selected = match node.selected.as_ref() {
        Some(selected) => selected,
        None => return Some(Err(resolution_error(&dependency.id, "dependency graph has no selected package"))),
      };
      Some(Ok(PackageRequest {
        id: dependency.id.clone(),
        lower_id: dependency.lower_id.clone(),
        version: selected.normalized.clone(),
        direct: false,
      }))
    })
    .collect()
}

fn resolution_error(context: impl Into<String>, message: impl Into<String>) -> PackageError {
  PackageError::new(PackageErrorKind::Resolution, context, message)
}

#[derive(Clone, Copy)]
struct PackageStorage<'a> {
  cache_root: &'a Path,
  fallback_roots: &'a [PathBuf],
  temp_root: &'a Path,
}

const _: () = assert!(size_of::<PackageStorage<'_>>() == 6 * size_of::<usize>());
const _: () = assert!(align_of::<PackageStorage<'_>>() == align_of::<usize>());

const _: () = assert!(size_of::<PackageStorage<'static>>() == 48);
const _: () = assert!(align_of::<PackageStorage<'static>>() == align_of::<usize>());

async fn load_node_metadata(
  client: &reqwest::Client,
  request: Option<&PackageRequest>,
  lower_id: &str,
  storage: PackageStorage<'_>,
  endpoints: &[ServiceEndpoint],
  source_mapping: Option<&PackageSourceMapping>,
  target: TargetFramework,
) -> Result<MetadataTaskResult, PackageError> {
  if request.is_none() && endpoints.is_empty() {
    let cached_versions = enumerate_cached_versions(storage.cache_root, storage.fallback_roots, lower_id)?;
    if !cached_versions.is_empty() {
      return Ok(MetadataTaskResult::Versions {
        versions: cached_versions,
        requests: 0,
        bytes: 0,
      });
    }
  }
  if let Some(request) = request {
    if let Some(root) = find_package_root(storage.cache_root, storage.fallback_roots, request) {
      let request = request.clone();
      let dependencies = tokio::task::spawn_blocking(move || read_cached_requirements(&root, &request, target))
        .await
        .map_err(package_blocking_task_error)??;
      return Ok(MetadataTaskResult::Requirements {
        dependencies,
        requests: 0,
        bytes: 0,
        package: None,
      });
    }

    if endpoints.is_empty() {
      let cached_versions = enumerate_cached_versions(storage.cache_root, storage.fallback_roots, lower_id)?;
      if !cached_versions.is_empty() {
        return Ok(MetadataTaskResult::Versions {
          versions: cached_versions,
          requests: 0,
          bytes: 0,
        });
      }
    }

    match ensure_package(client, request, storage, endpoints, source_mapping, target, false).await {
      Ok(cached) => {
        let dependencies = match &cached.dependencies {
          Some(dependencies) => dependencies.clone(),
          None => read_cached_requirements(&cached.root, request, target)?,
        };
        return Ok(MetadataTaskResult::Requirements {
          dependencies,
          requests: cached.requests,
          bytes: cached.bytes,
          package: Some(cached),
        });
      },
      Err(error) if error.kind() == PackageErrorKind::Network => {},
      Err(error) => return Err(error),
    }
  }

  if endpoints.is_empty() {
    return Err(PackageError::new(
      PackageErrorKind::OfflineMiss,
      lower_id,
      format!("package {lower_id} has no compatible version in the global package cache"),
    ));
  }

  // NuGet considers the global packages folder alongside enabled sources for
  // floating/ranged requests. Local sources must not hide already cached
  // versions merely because they remain available in offline mode.
  let mut versions = enumerate_cached_versions(storage.cache_root, storage.fallback_roots, lower_id)?;
  let mut requests = 0u32;
  let mut bytes = 0u64;
  for endpoint in endpoints {
    if source_mapping.is_some_and(|mapping| !mapping.allows(endpoint.source_index(), lower_id)) {
      continue;
    }
    match endpoint {
      ServiceEndpoint::Local { .. } => versions.extend(enumerate_local_versions(endpoint, lower_id)?),
      ServiceEndpoint::V3 { services, .. } => {
        let package_base = services.package_base_address().expect("v3 endpoint discovery requires package content");
        let separator = if package_base.ends_with('/') { "" } else { "/" };
        let url = format!("{package_base}{separator}{lower_id}/index.json");
        let Some(body) = get_optional_bytes(client, &url, MAX_JSON_BYTES, "NuGet package version index").await? else {
          requests += 1;
          continue;
        };
        requests += 1;
        bytes += body.len() as u64;
        let document: V3VersionIndex =
          serde_json::from_slice(&body).map_err(|error| network_error(&url, format!("invalid NuGet package version index: {error}")))?;
        if document.versions.len() > MAX_ARCHIVE_ENTRIES {
          return Err(network_error(&url, "NuGet package version index exceeds the version count limit"));
        }
        for version in document.versions {
          versions.push(PackageVersion::parse(&version)?);
        }
      },
      ServiceEndpoint::V2 { base, .. } => {
        let batch = enumerate_v2_versions(client, base, lower_id).await?;
        versions.extend(batch.versions);
        requests = requests
          .checked_add(batch.requests)
          .ok_or_else(|| network_error(base, "NuGet v2 request count overflow"))?;
        bytes = bytes
          .checked_add(batch.bytes)
          .ok_or_else(|| network_error(base, "NuGet v2 response byte count overflow"))?;
      },
    }
  }
  versions.sort_unstable();
  versions.dedup();
  if versions.is_empty() {
    return Err(PackageError::new(
      PackageErrorKind::Network,
      lower_id,
      format!("no enabled source could enumerate package {lower_id}"),
    ));
  }
  Ok(MetadataTaskResult::Versions { versions, requests, bytes })
}

#[derive(Deserialize)]
struct V3VersionIndex {
  versions: Vec<String>,
}

struct VersionBatch {
  versions: Vec<PackageVersion>,
  requests: u32,
  bytes: u64,
}

async fn enumerate_v2_versions(client: &reqwest::Client, base: &str, lower_id: &str) -> Result<VersionBatch, PackageError> {
  let mut url = format!("{base}FindPackagesById()?id='{lower_id}'&semVerLevel=2.0.0");
  let mut visited = HashSet::new();
  let mut versions = Vec::new();
  let mut requests = 0u32;
  let mut bytes = 0u64;
  loop {
    if !visited.insert(url.clone()) {
      return Err(network_error(&url, "NuGet v2 version enumeration contains a continuation cycle"));
    }
    if visited.len() > MAX_ARCHIVE_ENTRIES {
      return Err(network_error(&url, "NuGet v2 version enumeration exceeds the page count limit"));
    }
    let Some(body) = get_optional_bytes(client, &url, MAX_JSON_BYTES, "NuGet v2 version page").await? else {
      requests = requests.checked_add(1).ok_or_else(|| network_error(&url, "NuGet v2 request count overflow"))?;
      break;
    };
    requests = requests.checked_add(1).ok_or_else(|| network_error(&url, "NuGet v2 request count overflow"))?;
    bytes = bytes
      .checked_add(body.len() as u64)
      .ok_or_else(|| network_error(&url, "NuGet v2 response byte count overflow"))?;
    let page = parse_v2_version_page(&url, &body)?;
    if versions.len().checked_add(page.versions.len()).is_none_or(|count| count > MAX_ARCHIVE_ENTRIES) {
      return Err(network_error(&url, "NuGet v2 version enumeration exceeds the version count limit"));
    }
    versions.extend(page.versions);
    let Some(next) = page.next else {
      break;
    };
    let current = reqwest::Url::parse(&url).map_err(|error| network_error(&url, format!("invalid NuGet v2 page URL: {error}")))?;
    let continuation = current
      .join(&next)
      .map_err(|error| network_error(&url, format!("invalid NuGet v2 continuation URL {next:?}: {error}")))?;
    if continuation.scheme() != "https" {
      return Err(network_error(continuation.as_str(), "NuGet v2 continuation URL must use HTTPS"));
    }
    url = continuation.into();
  }
  Ok(VersionBatch { versions, requests, bytes })
}

struct V2VersionPage {
  versions: Vec<PackageVersion>,
  next: Option<String>,
}

fn parse_v2_version_page(url: &str, bytes: &[u8]) -> Result<V2VersionPage, PackageError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  let mut reading_version = false;
  let mut versions = Vec::new();
  let mut next = None;
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => {
        reading_version = local_name(element.name().as_ref()) == b"Version";
        if local_name(element.name().as_ref()) == b"link" && network_attribute(&reader, &element, b"rel", url)?.as_deref() == Some("next") {
          let href = network_attribute(&reader, &element, b"href", url)?.ok_or_else(|| network_error(url, "NuGet v2 continuation link has no href"))?;
          if next.replace(href).is_some() {
            return Err(network_error(url, "NuGet v2 version page has multiple continuation links"));
          }
        }
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"link" => {
        if network_attribute(&reader, &element, b"rel", url)?.as_deref() == Some("next") {
          let href = network_attribute(&reader, &element, b"href", url)?.ok_or_else(|| network_error(url, "NuGet v2 continuation link has no href"))?;
          if next.replace(href).is_some() {
            return Err(network_error(url, "NuGet v2 version page has multiple continuation links"));
          }
        }
      },
      Ok(Event::Text(text)) if reading_version => {
        let value = text
          .xml_content(XmlVersion::Implicit1_0)
          .map_err(|error| network_error(url, format!("invalid NuGet v2 version text: {error}")))?;
        versions.push(PackageVersion::parse(&value).map_err(|error| network_error(url, format!("invalid NuGet v2 package version {value:?}: {error}")))?);
      },
      Ok(Event::End(element)) => {
        if local_name(element.name().as_ref()) == b"Version" {
          reading_version = false;
        }
      },
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(network_error(url, format!("invalid NuGet v2 version XML: {error}"))),
    }
  }
  Ok(V2VersionPage { versions, next })
}

fn network_attribute(reader: &Reader<&[u8]>, element: &quick_xml::events::BytesStart<'_>, name: &[u8], url: &str) -> Result<Option<String>, PackageError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| network_error(url, format!("invalid NuGet v2 attribute: {error}")))?;
    if local_name(attribute.key.as_ref()) == name {
      return attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
        .map(|value| Some(value.into_owned()))
        .map_err(|error| network_error(url, format!("invalid NuGet v2 attribute value: {error}")));
    }
  }
  Ok(None)
}

fn read_cached_requirements(root: &Path, request: &PackageRequest, target: TargetFramework) -> Result<Vec<PackageRequirement>, PackageError> {
  let nuspec_path = find_nuspec(root)?;
  let nuspec = fs::read(&nuspec_path).map_err(|error| package_io("read package manifest", &nuspec_path, error))?;
  parse_nuspec_requirements(&nuspec_path, &nuspec, request, target)
}

fn enumerate_cached_versions(cache_root: &Path, fallback_roots: &[PathBuf], lower_id: &str) -> Result<Vec<PackageVersion>, PackageError> {
  let mut versions = Vec::new();
  append_cached_versions(cache_root, lower_id, &mut versions)?;
  for root in fallback_roots {
    append_cached_versions(root, lower_id, &mut versions)?;
  }
  versions.sort_unstable();
  versions.dedup();
  Ok(versions)
}

fn append_cached_versions(cache_root: &Path, lower_id: &str, versions: &mut Vec<PackageVersion>) -> Result<(), PackageError> {
  let identity_root = cache_root.join(lower_id);
  let entries = match fs::read_dir(&identity_root) {
    Ok(entries) => entries,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
    Err(error) => return Err(package_io("enumerate cached package versions", &identity_root, error)),
  };
  for entry in entries {
    let entry = entry.map_err(|error| package_io("enumerate cached package versions", &identity_root, error))?;
    if entry
      .file_type()
      .map_err(|error| package_io("inspect cached package version", &entry.path(), error))?
      .is_dir()
      && let Some(version) = entry.file_name().to_str()
      && let Ok(version) = PackageVersion::parse(version)
    {
      versions.push(version);
    }
  }
  Ok(())
}

fn local_package_path(endpoint: &ServiceEndpoint, request: &PackageRequest) -> Result<Option<PathBuf>, PackageError> {
  let ServiceEndpoint::Local { root, layout, .. } = endpoint else {
    return Ok(None);
  };
  match layout {
    LocalFeedLayout::Unknown => Ok(None),
    LocalFeedLayout::Hierarchical => {
      let version_root = root.join(&request.lower_id).join(&request.version);
      let archive = version_root.join(format!("{}.{}.nupkg", request.lower_id, request.version));
      let manifest = version_root.join(format!("{}.nuspec", request.lower_id));
      let hash = version_root.join(format!("{}.{}.nupkg.sha512", request.lower_id, request.version));
      Ok((archive.is_file() && manifest.is_file() && hash.is_file()).then_some(archive))
    },
    LocalFeedLayout::Flat(archives) => {
      for archive in archives.iter().filter(|path| possible_flat_archive(path, &request.lower_id)) {
        let (id, version) = read_local_archive_identity(archive)?;
        if id.eq_ignore_ascii_case(&request.id) && version.normalized == request.version {
          return Ok(Some(archive.clone()));
        }
      }
      Ok(None)
    },
  }
}

fn local_expected_hash(endpoint: &ServiceEndpoint, request: &PackageRequest) -> Result<Option<String>, PackageError> {
  let ServiceEndpoint::Local {
    root,
    layout: LocalFeedLayout::Hierarchical,
    ..
  } = endpoint
  else {
    return Ok(None);
  };
  let path = root
    .join(&request.lower_id)
    .join(&request.version)
    .join(format!("{}.{}.nupkg.sha512", request.lower_id, request.version));
  let value = fs::read_to_string(&path).map_err(|error| package_io("read local package hash", &path, error))?;
  let value = value.trim();
  let decoded = BASE64.decode(value).map_err(|error| {
    PackageError::new(
      PackageErrorKind::Integrity,
      path.display().to_string(),
      format!("invalid local package SHA-512: {error}"),
    )
  })?;
  if decoded.len() != 64 {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      path.display().to_string(),
      "local package SHA-512 must decode to 64 bytes",
    ));
  }
  Ok(Some(value.to_owned()))
}

fn enumerate_local_versions(endpoint: &ServiceEndpoint, lower_id: &str) -> Result<Vec<PackageVersion>, PackageError> {
  let ServiceEndpoint::Local { root, layout, .. } = endpoint else {
    return Ok(Vec::new());
  };
  let mut versions = Vec::new();
  match layout {
    LocalFeedLayout::Unknown => {},
    LocalFeedLayout::Hierarchical => {
      let identity_root = root.join(lower_id);
      let entries = match fs::read_dir(&identity_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(versions),
        Err(error) => return Err(package_io("enumerate hierarchical package versions", &identity_root, error)),
      };
      for entry in entries {
        let entry = entry.map_err(|error| package_io("enumerate hierarchical package versions", &identity_root, error))?;
        if entry
          .file_type()
          .map_err(|error| package_io("inspect hierarchical package version", &entry.path(), error))?
          .is_dir()
          && hierarchical_source_entry_exists(&identity_root, &entry.path())
          && let Some(version) = entry.file_name().to_str()
          && let Ok(version) = PackageVersion::parse(version)
        {
          versions.push(version);
        }
      }
    },
    LocalFeedLayout::Flat(archives) => {
      for archive in archives.iter().filter(|path| possible_flat_archive(path, lower_id)) {
        let (id, version) = read_local_archive_identity(archive)?;
        if id.eq_ignore_ascii_case(lower_id) {
          versions.push(version);
        }
      }
    },
  }
  Ok(versions)
}

fn possible_flat_archive(path: &Path, lower_id: &str) -> bool {
  let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
    return false;
  };
  name.len() > lower_id.len() + ".nupkg".len()
    && name.get(..lower_id.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(lower_id))
    && name.as_bytes().get(lower_id.len()) == Some(&b'.')
}

fn read_local_archive_identity(path: &Path) -> Result<(String, PackageVersion), PackageError> {
  let file = fs::File::open(path).map_err(|error| package_io("open local package archive", path, error))?;
  let mut archive = ZipArchive::new(file).map_err(|error| archive_error(path, format!("invalid local package archive: {error}")))?;
  let mut manifest = None;
  for index in 0..archive.len() {
    let mut entry = archive
      .by_index(index)
      .map_err(|error| archive_error(path, format!("failed to inspect local package entry {index}: {error}")))?;
    let entry_path = Path::new(entry.name());
    if entry_path.components().count() != 1
      || !entry_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("nuspec"))
    {
      continue;
    }
    if manifest.is_some() {
      return Err(archive_error(path, "local package contains multiple root nuspec files"));
    }
    if entry.size() > MAX_JSON_BYTES {
      return Err(archive_error(path, "local package nuspec exceeds the metadata size limit"));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
      .read_to_end(&mut bytes)
      .map_err(|error| archive_error(path, format!("failed to read local package nuspec: {error}")))?;
    manifest = Some(bytes);
  }
  let manifest = manifest.ok_or_else(|| archive_error(path, "local package contains no root nuspec"))?;
  parse_local_nuspec_identity(path, &manifest)
}

fn parse_local_nuspec_identity(path: &Path, bytes: &[u8]) -> Result<(String, PackageVersion), PackageError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  let mut current = NuspecText::None;
  let mut id = None;
  let mut version = None;
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => {
        current = match local_name(element.name().as_ref()) {
          b"id" if id.is_none() => NuspecText::Id,
          b"version" if version.is_none() => NuspecText::Version,
          _ => NuspecText::None,
        };
      },
      Ok(Event::Text(text)) => {
        let value = text
          .xml_content(XmlVersion::Implicit1_0)
          .map_err(|error| package_manifest_error(path, format!("invalid local nuspec text: {error}")))?
          .into_owned();
        match current {
          NuspecText::Id => id = Some(value),
          NuspecText::Version => version = Some(value),
          NuspecText::None => {},
        }
      },
      Ok(Event::End(_)) => current = NuspecText::None,
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(package_manifest_error(path, format!("invalid local nuspec XML: {error}"))),
    }
  }
  let id = id.ok_or_else(|| package_manifest_error(path, "local nuspec has no package id"))?;
  let version = version.ok_or_else(|| package_manifest_error(path, "local nuspec has no package version"))?;
  Ok((id, PackageVersion::parse(&version)?))
}

async fn get_optional_bytes(client: &reqwest::Client, url: &str, limit: u64, kind: &str) -> Result<Option<Vec<u8>>, PackageError> {
  let mut response = client
    .get(url)
    .send()
    .await
    .map_err(|error| network_error(url, format!("HTTP request failed: {error}")))?;
  if response.status() == reqwest::StatusCode::NOT_FOUND {
    return Ok(None);
  }
  response
    .error_for_status_ref()
    .map_err(|error| network_error(url, format!("HTTP request failed: {error}")))?;
  if response.content_length().is_some_and(|length| length > limit) {
    return Err(network_error(url, format!("{kind} response exceeds the {limit} byte limit")));
  }
  let mut bytes = Vec::with_capacity(response.content_length().unwrap_or(0).min(limit) as usize);
  while let Some(chunk) = response
    .chunk()
    .await
    .map_err(|error| network_error(url, format!("read {kind} response: {error}")))?
  {
    if bytes.len().checked_add(chunk.len()).is_none_or(|length| length as u64 > limit) {
      return Err(network_error(url, format!("{kind} response exceeds the {limit} byte limit")));
    }
    bytes.extend_from_slice(&chunk);
  }
  Ok(Some(bytes))
}

fn package_worker_stopped() -> PackageError {
  PackageError::new(
    PackageErrorKind::Io,
    "package scheduler",
    "package fetch worker stopped before the graph completed",
  )
}

async fn ensure_package(
  client: &reqwest::Client,
  request: &PackageRequest,
  storage: PackageStorage<'_>,
  endpoints: &[ServiceEndpoint],
  source_mapping: Option<&PackageSourceMapping>,
  target: TargetFramework,
  parallel_extract: bool,
) -> Result<CachedPackage, PackageError> {
  if let Some(root) = find_package_root(storage.cache_root, storage.fallback_roots, request) {
    let request = request.clone();
    return tokio::task::spawn_blocking(move || validate_cached_package(&root, &request, true, 0, 0))
      .await
      .map_err(package_blocking_task_error)?;
  }
  let mut last_error = None;
  for endpoint in endpoints {
    if source_mapping.is_some_and(|mapping| !mapping.allows(endpoint.source_index(), &request.lower_id)) {
      continue;
    }
    match download_and_publish(client, request, storage.cache_root, storage.temp_root, endpoint, target, parallel_extract).await {
      Ok(package) => return Ok(package),
      Err(error) if error.kind() == PackageErrorKind::Network => last_error = Some(error),
      Err(error) => return Err(error),
    }
  }
  Err(last_error.unwrap_or_else(|| {
    PackageError::new(
      PackageErrorKind::Network,
      format!("{} {}", request.id, request.version),
      format!("no enabled source could provide package {} {}", request.id, request.version),
    )
  }))
}

fn package_root(cache_root: &Path, request: &PackageRequest) -> PathBuf {
  cache_root.join(&request.lower_id).join(&request.version)
}

fn find_package_root(cache_root: &Path, fallback_roots: &[PathBuf], request: &PackageRequest) -> Option<PathBuf> {
  let mut candidate = package_root(cache_root, request);
  if candidate.is_dir() {
    return Some(candidate);
  }
  for fallback in fallback_roots {
    candidate.clear();
    candidate.push(fallback);
    candidate.push(&request.lower_id);
    candidate.push(&request.version);
    if candidate.is_dir() {
      return Some(candidate);
    }
  }
  None
}

struct PackageMetadata {
  content_url: String,
  expected_hash: Option<String>,
  expected_size: Option<u64>,
  requests: u32,
}

/// Owned handoff from async network/file streaming to bounded blocking archive work.
struct DownloadedPackage {
  request: PackageRequest,
  cache_root: PathBuf,
  endpoint: ServiceEndpoint,
  temp_root: PathBuf,
  nupkg_name: String,
  nupkg_path: PathBuf,
  hash: String,
  bytes: u64,
  requests: u32,
  target: TargetFramework,
  parallel_extract: bool,
}

async fn download_and_publish(
  client: &reqwest::Client,
  request: &PackageRequest,
  cache_root: &Path,
  temp_root: &Path,
  endpoint: &ServiceEndpoint,
  target: TargetFramework,
  parallel_extract: bool,
) -> Result<CachedPackage, PackageError> {
  if matches!(endpoint, ServiceEndpoint::Local { .. }) {
    let archive = local_package_path(endpoint, request)?.ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::Network,
        endpoint.source(),
        format!("local package source does not contain {} {}", request.id, request.version),
      )
    })?;
    let request = request.clone();
    let cache_root = cache_root.to_owned();
    let endpoint = endpoint.clone();
    return tokio::task::spawn_blocking(move || install_local_package(&archive, request, cache_root, endpoint, target, parallel_extract))
      .await
      .map_err(package_blocking_task_error)?;
  }
  let metadata = match endpoint {
    ServiceEndpoint::Local { .. } => unreachable!("local package acquisition returned above"),
    ServiceEndpoint::V2 { base, .. } => v2_package_metadata(client, request, base).await?,
    ServiceEndpoint::V3 { services, .. } => v3_package_metadata(
      request,
      services.package_base_address().expect("v3 endpoint discovery requires package content"),
    ),
  };
  if let Some(size) = metadata.expected_size
    && size > MAX_PACKAGE_BYTES
  {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      &metadata.content_url,
      format!("package size {size} exceeds the {MAX_PACKAGE_BYTES} byte limit"),
    ));
  }

  let scratch_root = unique_temp_root(temp_root, request);
  tokio::fs::create_dir_all(&scratch_root)
    .await
    .map_err(|error| package_io("create package scratch directory", &scratch_root, error))?;
  let scratch_guard = TempGuard(Some(scratch_root.clone()));
  let nupkg_name = format!("{}.{}.nupkg", request.lower_id, request.version);
  let scratch_nupkg = scratch_root.join(&nupkg_name);
  let (hash, bytes) = download_package(client, &metadata.content_url, &scratch_nupkg).await?;
  if let Some(expected) = metadata.expected_size
    && bytes != expected
  {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      &metadata.content_url,
      format!("downloaded package size {bytes} does not match source metadata size {expected}"),
    ));
  }
  if let Some(expected) = &metadata.expected_hash
    && hash != *expected
  {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      &metadata.content_url,
      "downloaded package SHA-512 does not match source metadata",
    ));
  }
  // Publication must remain an atomic rename on the global-cache volume.
  // Link from NuGet scratch when possible and copy only across volumes.
  let staging_root = unique_temp_root(cache_root, request);
  tokio::fs::create_dir_all(&staging_root)
    .await
    .map_err(|error| package_io("create package staging directory", &staging_root, error))?;
  let staging_guard = TempGuard(Some(staging_root.clone()));
  let nupkg_path = staging_root.join(&nupkg_name);
  if tokio::fs::hard_link(&scratch_nupkg, &nupkg_path).await.is_err() {
    tokio::fs::copy(&scratch_nupkg, &nupkg_path)
      .await
      .map_err(|error| package_io("copy package archive into cache staging", &nupkg_path, error))?;
  }
  let downloaded = DownloadedPackage {
    request: request.clone(),
    cache_root: cache_root.to_owned(),
    endpoint: endpoint.clone(),
    temp_root: staging_root,
    nupkg_name,
    nupkg_path,
    hash,
    bytes,
    requests: metadata.requests + 1,
    target,
    parallel_extract,
  };
  tokio::task::spawn_blocking(move || finish_download_and_publish(downloaded, staging_guard, scratch_guard))
    .await
    .map_err(package_blocking_task_error)?
}

fn install_local_package(
  archive: &Path,
  request: PackageRequest,
  cache_root: PathBuf,
  endpoint: ServiceEndpoint,
  target: TargetFramework,
  parallel_extract: bool,
) -> Result<CachedPackage, PackageError> {
  let expected_hash = local_expected_hash(&endpoint, &request)?;
  let size = fs::metadata(archive)
    .map_err(|error| package_io("inspect local package archive", archive, error))?
    .len();
  if size > MAX_PACKAGE_BYTES {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      archive.display().to_string(),
      format!("local package archive size {size} exceeds the {MAX_PACKAGE_BYTES} byte limit"),
    ));
  }
  let staging_root = unique_temp_root(&cache_root, &request);
  fs::create_dir_all(&staging_root).map_err(|error| package_io("create package staging directory", &staging_root, error))?;
  let staging_guard = TempGuard(Some(staging_root.clone()));
  let nupkg_name = format!("{}.{}.nupkg", request.lower_id, request.version);
  let nupkg_path = staging_root.join(&nupkg_name);
  if fs::hard_link(archive, &nupkg_path).is_err() {
    fs::copy(archive, &nupkg_path).map_err(|error| package_io("copy local package archive into cache staging", &nupkg_path, error))?;
  }
  let (hash, bytes) = hash_local_package(&nupkg_path)?;
  if bytes != size {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      archive.display().to_string(),
      format!("local package archive changed size while being read: expected {size} bytes, read {bytes}"),
    ));
  }
  if expected_hash.as_deref().is_some_and(|expected| expected != hash) {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      archive.display().to_string(),
      "local package SHA-512 does not match source metadata",
    ));
  }
  let downloaded = DownloadedPackage {
    request,
    cache_root,
    endpoint,
    temp_root: staging_root,
    nupkg_name,
    nupkg_path,
    hash,
    bytes,
    requests: 0,
    target,
    parallel_extract,
  };
  finish_download_and_publish(downloaded, staging_guard, TempGuard(None))
}

fn hash_local_package(path: &Path) -> Result<(String, u64), PackageError> {
  let mut file = fs::File::open(path).map_err(|error| package_io("open local package archive", path, error))?;
  let mut hasher = Sha512::new();
  let mut buffer = [0u8; 64 * 1024];
  let mut total = 0u64;
  loop {
    let read = file.read(&mut buffer).map_err(|error| package_io("read local package archive", path, error))?;
    if read == 0 {
      break;
    }
    total = total.checked_add(read as u64).filter(|total| *total <= MAX_PACKAGE_BYTES).ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::Integrity,
        path.display().to_string(),
        "local package archive exceeds the size limit",
      )
    })?;
    hasher.update(&buffer[..read]);
  }
  Ok((BASE64.encode(hasher.finalize()), total))
}

fn finish_download_and_publish(downloaded: DownloadedPackage, mut staging_guard: TempGuard, _scratch_guard: TempGuard) -> Result<CachedPackage, PackageError> {
  validate_and_extract_archive(&downloaded.nupkg_path, &downloaded.temp_root, downloaded.parallel_extract)?;
  normalize_nuspec_name(&downloaded.temp_root, &downloaded.request)?;
  let nuspec_path = downloaded.temp_root.join(format!("{}.nuspec", downloaded.request.lower_id));
  let nuspec = fs::read(&nuspec_path).map_err(|error| package_io("read package manifest", &nuspec_path, error))?;
  let dependencies = parse_nuspec_requirements(&nuspec_path, &nuspec, &downloaded.request, downloaded.target)?;
  fs::write(
    downloaded.temp_root.join(format!("{}.sha512", downloaded.nupkg_name)),
    downloaded.hash.as_bytes(),
  )
  .map_err(|error| package_io("write package hash", &downloaded.temp_root, error))?;
  let package_metadata = serde_json::json!({
    "schemaVersion": 1,
    "sha512": &downloaded.hash,
    "source": downloaded.endpoint.source(),
    "protocol": downloaded.endpoint.protocol().as_str(),
  });
  fs::write(
    downloaded.temp_root.join(".dv.metadata.json"),
    serde_json::to_vec_pretty(&package_metadata).expect("serializing package metadata succeeds"),
  )
  .map_err(|error| package_io("write package metadata", &downloaded.temp_root, error))?;

  let final_root = package_root(&downloaded.cache_root, &downloaded.request);
  fs::create_dir_all(final_root.parent().expect("package version has an identity parent"))
    .map_err(|error| package_io("create package identity directory", &final_root, error))?;
  let published = publish_package_directory(&downloaded.temp_root, &final_root)?;
  if published {
    staging_guard.0 = None;
  }
  let mut cached = if published {
    CachedPackage {
      root: final_root,
      hash: downloaded.hash,
      dependencies: Some(dependencies),
      cache_hit: false,
      requests: downloaded.requests,
      bytes: downloaded.bytes,
      origin: None,
    }
  } else {
    validate_cached_package(&final_root, &downloaded.request, false, downloaded.requests, downloaded.bytes)?
  };
  cached.origin = Some(PackageSource {
    url: downloaded.endpoint.source().to_owned(),
    protocol: downloaded.endpoint.protocol(),
  });
  Ok(cached)
}

fn publish_package_directory(staged: &Path, destination: &Path) -> Result<bool, PackageError> {
  for delay in PUBLISH_RETRY_DELAYS.into_iter().map(Some).chain([None]) {
    match fs::rename(staged, destination) {
      Ok(()) => return Ok(true),
      Err(_) if destination.exists() => return Ok(false),
      Err(error) if error.kind() == io::ErrorKind::PermissionDenied && delay.is_some() => {
        thread::sleep(delay.expect("permission retry has a delay"));
      },
      Err(error) => return Err(package_io("publish package atomically", destination, error)),
    }
  }
  unreachable!("the final publish attempt returns")
}

fn v3_package_metadata(request: &PackageRequest, package_base: &str) -> PackageMetadata {
  let separator = if package_base.ends_with('/') { "" } else { "/" };
  PackageMetadata {
    content_url: format!(
      "{package_base}{separator}{}/{}/{}.{}.nupkg",
      request.lower_id, request.version, request.lower_id, request.version
    ),
    expected_hash: None,
    expected_size: None,
    requests: 0,
  }
}

async fn v2_package_metadata(client: &reqwest::Client, request: &PackageRequest, base: &str) -> Result<PackageMetadata, PackageError> {
  let metadata_url = format!("{base}Packages(Id='{}',Version='{}')", request.id, request.version);
  let bytes = get_bytes(client, &metadata_url, MAX_JSON_BYTES, "NuGet v2 metadata").await?;
  parse_v2_package_metadata(request, &metadata_url, &bytes)
}

fn parse_v2_package_metadata(request: &PackageRequest, metadata_url: &str, bytes: &[u8]) -> Result<PackageMetadata, PackageError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  let mut current = V2MetadataText::None;
  let mut id = None;
  let mut version = None;
  let mut hash = None;
  let mut algorithm = None;
  let mut size = None;
  let mut content_url = None;
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => {
        current = match local_name(element.name().as_ref()) {
          b"Id" => V2MetadataText::Id,
          b"Version" => V2MetadataText::Version,
          b"PackageHash" => V2MetadataText::Hash,
          b"PackageHashAlgorithm" => V2MetadataText::Algorithm,
          b"PackageSize" => V2MetadataText::Size,
          _ => V2MetadataText::None,
        };
        if local_name(element.name().as_ref()) == b"content" {
          content_url = config_attribute(&reader, &element, b"src", Path::new(metadata_url))?;
        }
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"content" => {
        content_url = config_attribute(&reader, &element, b"src", Path::new(metadata_url))?;
      },
      Ok(Event::Text(text)) => {
        let value = text
          .xml_content(XmlVersion::Implicit1_0)
          .map_err(|error| network_error(metadata_url, format!("invalid NuGet v2 metadata text: {error}")))?
          .into_owned();
        match current {
          V2MetadataText::Id => id = Some(value),
          V2MetadataText::Version => version = Some(value),
          V2MetadataText::Hash => hash = Some(value),
          V2MetadataText::Algorithm => algorithm = Some(value),
          V2MetadataText::Size => size = value.parse::<u64>().ok(),
          V2MetadataText::None => {},
        }
      },
      Ok(Event::End(_)) => current = V2MetadataText::None,
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(network_error(metadata_url, format!("invalid NuGet v2 metadata XML: {error}"))),
    }
  }
  let found_id = id.ok_or_else(|| network_error(metadata_url, "NuGet v2 metadata has no package Id"))?;
  let found_version = version.ok_or_else(|| network_error(metadata_url, "NuGet v2 metadata has no package Version"))?;
  if !found_id.eq_ignore_ascii_case(&request.id) || normalize_version(&found_version)? != request.version {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      metadata_url,
      format!(
        "NuGet v2 metadata identity {found_id} {found_version} does not match requested {} {}",
        request.id, request.version
      ),
    ));
  }
  let algorithm = algorithm.ok_or_else(|| network_error(metadata_url, "NuGet v2 metadata has no package hash algorithm"))?;
  if !algorithm.eq_ignore_ascii_case("SHA512") {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      metadata_url,
      format!("unsupported package hash algorithm {algorithm:?}"),
    ));
  }
  let content_url = content_url.ok_or_else(|| network_error(metadata_url, "NuGet v2 metadata has no package content URL"))?;
  if !content_url.starts_with("https://") {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      &content_url,
      "NuGet v2 package content URL must use HTTPS",
    ));
  }
  Ok(PackageMetadata {
    content_url,
    expected_hash: Some(hash.ok_or_else(|| network_error(metadata_url, "NuGet v2 metadata has no package hash"))?),
    expected_size: Some(size.ok_or_else(|| network_error(metadata_url, "NuGet v2 metadata has no valid package size"))?),
    requests: 1,
  })
}

#[derive(Clone, Copy)]
enum V2MetadataText {
  None,
  Id,
  Version,
  Hash,
  Algorithm,
  Size,
}

async fn download_package(client: &reqwest::Client, url: &str, destination: &Path) -> Result<(String, u64), PackageError> {
  let mut response = client
    .get(url)
    .send()
    .await
    .map_err(|error| network_error(url, format!("package download failed: {error}")))?;
  response
    .error_for_status_ref()
    .map_err(|error| network_error(url, format!("package download failed: {error}")))?;
  let content_length = response.content_length();
  if content_length.is_some_and(|length| length > MAX_PACKAGE_BYTES) {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      url,
      format!("package Content-Length exceeds the {MAX_PACKAGE_BYTES} byte limit"),
    ));
  }
  let mut output = tokio::fs::File::create(destination)
    .await
    .map_err(|error| package_io("create package archive", destination, error))?;
  let mut hasher = Sha512::new();
  let mut total = 0u64;
  while let Some(chunk) = response
    .chunk()
    .await
    .map_err(|error| network_error(url, format!("read package response: {error}")))?
  {
    let read = chunk.len();
    total = total
      .checked_add(read as u64)
      .filter(|total| *total <= MAX_PACKAGE_BYTES)
      .ok_or_else(|| PackageError::new(PackageErrorKind::Integrity, url, "package response exceeds the download limit"))?;
    hasher.update(&chunk);
    output
      .write_all(&chunk)
      .await
      .map_err(|error| package_io("write package archive", destination, error))?;
  }
  output.flush().await.map_err(|error| package_io("flush package archive", destination, error))?;
  if let Some(expected) = content_length
    && total != expected
  {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      url,
      format!("package response body has {total} bytes but Content-Length declared {expected}"),
    ));
  }
  Ok((BASE64.encode(hasher.finalize()), total))
}

async fn get_bytes(client: &reqwest::Client, url: &str, limit: u64, kind: &str) -> Result<Vec<u8>, PackageError> {
  let mut response = client
    .get(url)
    .send()
    .await
    .map_err(|error| network_error(url, format!("HTTP request failed: {error}")))?;
  response
    .error_for_status_ref()
    .map_err(|error| network_error(url, format!("HTTP request failed: {error}")))?;
  if response.content_length().is_some_and(|length| length > limit) {
    return Err(network_error(url, format!("{kind} response exceeds the {limit} byte limit")));
  }
  let capacity = response.content_length().unwrap_or(0).min(limit) as usize;
  let mut bytes = Vec::with_capacity(capacity);
  while let Some(chunk) = response
    .chunk()
    .await
    .map_err(|error| network_error(url, format!("read {kind} response: {error}")))?
  {
    let next = bytes
      .len()
      .checked_add(chunk.len())
      .filter(|length| *length as u64 <= limit)
      .ok_or_else(|| network_error(url, format!("{kind} response exceeds the {limit} byte limit")))?;
    bytes.extend_from_slice(&chunk);
    debug_assert_eq!(bytes.len(), next);
  }
  Ok(bytes)
}

fn package_blocking_task_error(error: tokio::task::JoinError) -> PackageError {
  PackageError::new(
    PackageErrorKind::Io,
    "package scheduler",
    format!("blocking package task stopped before completion: {error}"),
  )
}

fn network_error(context: impl Into<String>, message: impl Into<String>) -> PackageError {
  PackageError::new(PackageErrorKind::Network, context, message)
}

struct ArchiveEntryPlan {
  index: usize,
  path: PathBuf,
  is_directory: bool,
}

fn validate_and_extract_archive(nupkg_path: &Path, destination: &Path, parallel: bool) -> Result<(), PackageError> {
  let file = fs::File::open(nupkg_path).map_err(|error| package_io("open package archive", nupkg_path, error))?;
  let mut archive = ZipArchive::new(file).map_err(|error| archive_error(nupkg_path, format!("invalid ZIP archive: {error}")))?;
  if archive.len() > MAX_ARCHIVE_ENTRIES {
    return Err(archive_error(
      nupkg_path,
      format!("archive contains {} entries; limit is {MAX_ARCHIVE_ENTRIES}", archive.len()),
    ));
  }
  let mut names = HashSet::with_capacity(archive.len());
  let mut plans = Vec::with_capacity(archive.len());
  let mut total = 0u64;
  for index in 0..archive.len() {
    let entry = archive
      .by_index(index)
      .map_err(|error| archive_error(nupkg_path, format!("failed to inspect ZIP entry {index}: {error}")))?;
    let enclosed = entry
      .enclosed_name()
      .ok_or_else(|| archive_error(nupkg_path, format!("archive entry {:?} escapes the package root", entry.name())))?;
    if enclosed
      .components()
      .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir | Component::ParentDir))
    {
      return Err(archive_error(nupkg_path, format!("archive entry {:?} escapes the package root", entry.name())));
    }
    if entry.size() > MAX_ENTRY_BYTES {
      return Err(archive_error(
        nupkg_path,
        format!("archive entry {:?} exceeds the entry-size limit", entry.name()),
      ));
    }
    total = total
      .checked_add(entry.size())
      .filter(|total| *total <= MAX_EXPANDED_BYTES)
      .ok_or_else(|| archive_error(nupkg_path, "archive exceeds the total expansion limit"))?;
    let folded = entry.name().replace('\\', "/").to_ascii_lowercase();
    if !names.insert(folded) {
      return Err(archive_error(nupkg_path, format!("archive contains duplicate path {:?}", entry.name())));
    }
    if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
      return Err(archive_error(nupkg_path, format!("archive contains symbolic link {:?}", entry.name())));
    }
    plans.push(ArchiveEntryPlan {
      index,
      path: enclosed.to_owned(),
      is_directory: entry.is_dir(),
    });
  }

  for plan in &plans {
    let target = destination.join(&plan.path);
    if plan.is_directory {
      fs::create_dir_all(&target).map_err(|error| package_io("create package directory", &target, error))?;
    } else if let Some(parent) = target.parent() {
      fs::create_dir_all(parent).map_err(|error| package_io("create package directory", parent, error))?;
    }
  }

  let file_count = plans.iter().filter(|plan| !plan.is_directory).count();
  if !parallel || file_count < MIN_PARALLEL_EXTRACTION_ENTRIES {
    return extract_archive_range(&mut archive, &plans, nupkg_path, destination);
  }

  let worker_count = file_count.min(MAX_EXTRACTION_WORKERS);
  let mut archives = Vec::with_capacity(worker_count);
  for _ in 0..worker_count {
    let file = fs::File::open(nupkg_path).map_err(|error| package_io("open package archive", nupkg_path, error))?;
    archives.push(ZipArchive::new(file).map_err(|error| archive_error(nupkg_path, format!("invalid ZIP archive: {error}")))?);
  }
  thread::scope(|scope| {
    let plans = plans.as_slice();
    let mut workers = Vec::with_capacity(worker_count);
    for (worker, mut archive) in archives.into_iter().enumerate() {
      let start = plans.len() * worker / worker_count;
      let end = plans.len() * (worker + 1) / worker_count;
      workers.push(scope.spawn(move || extract_archive_range(&mut archive, &plans[start..end], nupkg_path, destination)));
    }
    for worker in workers {
      worker.join().map_err(|_| archive_error(nupkg_path, "package extraction worker panicked"))??;
    }
    Ok(())
  })
}

fn extract_archive_range(archive: &mut ZipArchive<fs::File>, plans: &[ArchiveEntryPlan], archive_path: &Path, destination: &Path) -> Result<(), PackageError> {
  for plan in plans {
    if plan.is_directory {
      continue;
    }
    let mut entry = archive
      .by_index(plan.index)
      .map_err(|error| archive_error(archive_path, format!("failed to read ZIP entry {}: {error}", plan.index)))?;
    let target = destination.join(&plan.path);
    let mut output = fs::File::create(&target).map_err(|error| package_io("extract package file", &target, error))?;
    io::copy(&mut entry, &mut output).map_err(|error| package_io("extract package file", &target, error))?;
  }
  Ok(())
}

fn archive_error(path: &Path, message: impl Into<String>) -> PackageError {
  PackageError::new(PackageErrorKind::Archive, path.display().to_string(), message)
}

fn normalize_nuspec_name(root: &Path, request: &PackageRequest) -> Result<(), PackageError> {
  let expected = root.join(format!("{}.nuspec", request.lower_id));
  if expected.is_file() {
    return Ok(());
  }
  let nuspecs: Vec<PathBuf> = fs::read_dir(root)
    .map_err(|error| package_io("enumerate package root", root, error))?
    .filter_map(Result::ok)
    .map(|entry| entry.path())
    .filter(|path| path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("nuspec")))
    .collect();
  if nuspecs.len() != 1 {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      root.display().to_string(),
      format!("package {} {} must contain exactly one root nuspec", request.id, request.version),
    ));
  }
  fs::rename(&nuspecs[0], &expected).map_err(|error| package_io("normalize package nuspec", &expected, error))
}

fn validate_cached_package(root: &Path, request: &PackageRequest, cache_hit: bool, requests: u32, bytes: u64) -> Result<CachedPackage, PackageError> {
  if !root.is_dir() {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      root.display().to_string(),
      "package cache entry is not a directory",
    ));
  }
  let marker_valid = root.join(".nupkg.metadata").is_file() || root.join(".dv.metadata.json").is_file();
  let nupkg = root.join(format!("{}.{}.nupkg", request.lower_id, request.version));
  let hash_path = root.join(format!("{}.{}.nupkg.sha512", request.lower_id, request.version));
  let nuspec = find_nuspec(root)?;
  if !marker_valid || !nupkg.is_file() || !hash_path.is_file() || !nuspec.is_file() {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      root.display().to_string(),
      format!("package cache entry for {} {} is incomplete", request.id, request.version),
    ));
  }
  let hash = fs::read_to_string(&hash_path)
    .map_err(|error| package_io("read package hash", &hash_path, error))?
    .trim()
    .to_owned();
  let decoded = BASE64.decode(&hash).map_err(|error| {
    PackageError::new(
      PackageErrorKind::Integrity,
      hash_path.display().to_string(),
      format!("invalid package SHA-512: {error}"),
    )
  })?;
  if decoded.len() != 64 {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      hash_path.display().to_string(),
      "package SHA-512 must decode to 64 bytes",
    ));
  }
  Ok(CachedPackage {
    root: root.to_owned(),
    hash,
    dependencies: None,
    cache_hit,
    requests,
    bytes,
    origin: None,
  })
}

fn find_nuspec(root: &Path) -> Result<PathBuf, PackageError> {
  let mut found = None;
  for entry in fs::read_dir(root).map_err(|error| package_io("enumerate package root", root, error))? {
    let entry = entry.map_err(|error| package_io("enumerate package root", root, error))?;
    let path = entry.path();
    if entry.file_type().map_err(|error| package_io("inspect package root", &path, error))?.is_file()
      && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("nuspec"))
    {
      if found.is_some() {
        return Err(PackageError::new(
          PackageErrorKind::Integrity,
          root.display().to_string(),
          "package contains multiple root nuspec files",
        ));
      }
      found = Some(path);
    }
  }
  found.ok_or_else(|| PackageError::new(PackageErrorKind::Integrity, root.display().to_string(), "package contains no root nuspec"))
}

#[cfg(test)]
fn validate_staged_nuspec_identity(root: &Path, request: &PackageRequest) -> Result<(), PackageError> {
  let path = find_nuspec(root)?;
  let bytes = fs::read(&path).map_err(|error| package_io("read package manifest", &path, error))?;
  let mut reader = Reader::from_reader(bytes.as_slice());
  reader.config_mut().trim_text(true);
  let mut current = NuspecText::None;
  let mut id = None;
  let mut version = None;
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => {
        current = match local_name(element.name().as_ref()) {
          b"id" if id.is_none() => NuspecText::Id,
          b"version" if version.is_none() => NuspecText::Version,
          _ => NuspecText::None,
        };
      },
      Ok(Event::Text(text)) => {
        let value = text
          .xml_content(XmlVersion::Implicit1_0)
          .map_err(|error| package_manifest_error(&path, format!("invalid nuspec text: {error}")))?
          .into_owned();
        match current {
          NuspecText::Id => id = Some(value),
          NuspecText::Version => version = Some(value),
          NuspecText::None => {},
        }
      },
      Ok(Event::End(_)) => current = NuspecText::None,
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(package_manifest_error(&path, format!("invalid nuspec XML: {error}"))),
    }
  }
  let found_id = id.ok_or_else(|| package_manifest_error(&path, "nuspec has no package id"))?;
  let found_version = version.ok_or_else(|| package_manifest_error(&path, "nuspec has no package version"))?;
  if !found_id.eq_ignore_ascii_case(&request.id) || normalize_version(&found_version)? != request.version {
    return Err(package_manifest_error(
      &path,
      format!(
        "nuspec identity {found_id} {found_version} does not match requested {} {}",
        request.id, request.version
      ),
    ));
  }
  Ok(())
}

fn parse_cached_package(
  request: PackageRequest,
  cached: CachedPackage,
  target: TargetFramework,
  _target_text: &str,
  dependencies: Vec<PackageRequest>,
  flags: AssetFlags,
) -> Result<WorkPackage, PackageError> {
  let compile_assets = select_if(flags.contains(AssetFlags::COMPILE), || select_compile_assets(&cached.root, target))?;
  let runtime_assets = select_if(flags.contains(AssetFlags::RUNTIME), || select_runtime_assets(&cached.root, target))?;
  // Package analyzers are resolved graph-wide by ResolvePackageAssets rather
  // than serialized as a target-library family in project.assets.json.
  let analyzers = collect_analyzers(&cached.root)?;
  let resource_assets = select_if(flags.contains(AssetFlags::RUNTIME), || select_resource_assets(&cached.root, target))?;
  let content_files = select_content_files(&cached.root, target, flags.contains(AssetFlags::CONTENT_FILES))?;
  let native_assets = select_if(flags.contains(AssetFlags::NATIVE), || select_legacy_native_assets(&cached.root))?;
  let runtime_targets = select_runtime_targets(&cached.root, target, flags)?;
  let selected_build = select_build_assets(&cached.root, "build", &request.id, target, true)?;
  let selected_build_transitive = select_build_assets(&cached.root, "buildTransitive", &request.id, target, true)?;
  let build_transitive_assets = if flags.contains(AssetFlags::BUILD) || flags.contains(AssetFlags::BUILD_TRANSITIVE) {
    selected_build_transitive
  } else {
    Vec::new()
  };
  let mut build_assets = if flags.contains(AssetFlags::BUILD) {
    selected_build
  } else if build_transitive_assets.is_empty() {
    excluded_asset_marker(&selected_build)
  } else {
    Vec::new()
  };
  build_assets.retain(|asset| {
    !build_transitive_assets.iter().any(|transitive| {
      transitive
        .file_name()
        .is_some_and(|name| asset.file_name().is_some_and(|asset_name| asset_name.eq_ignore_ascii_case(name)))
    })
  });
  let selected_build_multi_targeting = select_build_assets(&cached.root, "buildMultiTargeting", &request.id, target, false)?;
  let build_multi_targeting_assets = if flags.contains(AssetFlags::BUILD) {
    selected_build_multi_targeting
  } else {
    excluded_asset_marker(&selected_build_multi_targeting)
  };
  Ok(WorkPackage {
    request,
    root: cached.root,
    hash: cached.hash,
    dependencies,
    compile_assets,
    runtime_assets,
    analyzers,
    resource_assets,
    content_files,
    build_assets,
    build_multi_targeting_assets,
    build_transitive_assets,
    native_assets,
    runtime_targets,
    cache_hit: cached.cache_hit,
    origin: cached.origin,
  })
}

fn select_if<T>(enabled: bool, select: impl FnOnce() -> Result<Vec<T>, PackageError>) -> Result<Vec<T>, PackageError> {
  if enabled { select() } else { Ok(Vec::new()) }
}

fn excluded_asset_marker(assets: &[PathBuf]) -> Vec<PathBuf> {
  assets
    .iter()
    .min_by(|left, right| left.components().count().cmp(&right.components().count()).then_with(|| left.cmp(right)))
    .and_then(|asset| asset.parent().map(|directory| directory.join("_._")))
    .into_iter()
    .collect()
}

struct DependencyGroup {
  framework: Option<String>,
  dependencies: Vec<RawDependency>,
}

struct RawDependency {
  id: String,
  version: String,
  include_assets: AssetFlags,
  suppress_parent: AssetFlags,
}

#[cfg(test)]
fn parse_nuspec(path: &Path, bytes: &[u8], request: &PackageRequest, target: TargetFramework) -> Result<Vec<PackageRequest>, PackageError> {
  parse_nuspec_requirements(path, bytes, request, target)?
    .into_iter()
    .map(|requirement| {
      Ok(PackageRequest {
        id: requirement.id,
        lower_id: requirement.lower_id,
        version: minimum_version_from_range(&requirement.range)?.normalized,
        direct: requirement.direct,
      })
    })
    .collect()
}

fn parse_nuspec_requirements(path: &Path, bytes: &[u8], request: &PackageRequest, target: TargetFramework) -> Result<Vec<PackageRequirement>, PackageError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  let mut current_text = NuspecText::None;
  let mut id = None;
  let mut version = None;
  let mut groups = Vec::<DependencyGroup>::new();
  let mut ungrouped = Vec::new();
  let mut in_dependencies = false;
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => match local_name(element.name().as_ref()) {
        b"id" if id.is_none() => current_text = NuspecText::Id,
        b"version" if version.is_none() => current_text = NuspecText::Version,
        b"dependencies" => in_dependencies = true,
        b"group" if in_dependencies => {
          groups.push(DependencyGroup {
            framework: nuspec_attribute(&reader, &element, b"targetFramework", path)?,
            dependencies: Vec::new(),
          });
        },
        _ => {},
      },
      Ok(Event::Empty(element)) if in_dependencies && local_name(element.name().as_ref()) == b"group" => {
        groups.push(DependencyGroup {
          framework: nuspec_attribute(&reader, &element, b"targetFramework", path)?,
          dependencies: Vec::new(),
        });
      },
      Ok(Event::Empty(element)) if in_dependencies && local_name(element.name().as_ref()) == b"dependency" => {
        let dependency_id = nuspec_attribute(&reader, &element, b"id", path)?.ok_or_else(|| package_manifest_error(path, "dependency requires id"))?;
        let dependency_version =
          nuspec_attribute(&reader, &element, b"version", path)?.ok_or_else(|| package_manifest_error(path, "dependency requires version"))?;
        let include_assets = parse_asset_flags(nuspec_attribute(&reader, &element, b"include", path)?.as_deref(), AssetFlags::NO_CONTENT, path)?;
        let exclude_assets = parse_asset_flags(nuspec_attribute(&reader, &element, b"exclude", path)?.as_deref(), AssetFlags::NONE, path)?;
        let dependency = RawDependency {
          id: dependency_id,
          version: dependency_version,
          include_assets: include_assets.without(exclude_assets),
          suppress_parent: AssetFlags::NONE,
        };
        if let Some(group) = groups.last_mut() {
          group.dependencies.push(dependency);
        } else {
          ungrouped.push(dependency);
        }
      },
      Ok(Event::Text(text)) => {
        let value = text
          .xml_content(XmlVersion::Implicit1_0)
          .map_err(|error| package_manifest_error(path, format!("invalid nuspec text: {error}")))?
          .into_owned();
        match current_text {
          NuspecText::Id => id = Some(value),
          NuspecText::Version => version = Some(value),
          NuspecText::None => {},
        }
      },
      Ok(Event::End(element)) => match local_name(element.name().as_ref()) {
        b"id" | b"version" => current_text = NuspecText::None,
        b"dependencies" => in_dependencies = false,
        _ => {},
      },
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(package_manifest_error(path, format!("invalid nuspec XML: {error}"))),
    }
  }
  let found_id = id.ok_or_else(|| package_manifest_error(path, "nuspec has no package id"))?;
  let found_version = version.ok_or_else(|| package_manifest_error(path, "nuspec has no package version"))?;
  if !found_id.eq_ignore_ascii_case(&request.id) || normalize_version(&found_version)? != request.version {
    return Err(package_manifest_error(
      path,
      format!(
        "nuspec identity {found_id} {found_version} does not match requested {} {}",
        request.id, request.version
      ),
    ));
  }
  let selected = groups
    .iter()
    .filter_map(|group| {
      group
        .framework
        .as_deref()
        .map_or(Some(0), |framework| framework_score(Some(framework), target))
        .map(|score| (score, group))
    })
    .max_by_key(|(score, _)| *score)
    .map(|(_, group)| &group.dependencies);
  let selected = if groups.is_empty() {
    &ungrouped
  } else {
    selected.ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::Incompatible,
        format!("{} {}", request.id, request.version),
        format!(
          "package {} {} has no dependency group compatible with the evaluated target",
          request.id, request.version
        ),
      )
    })?
  };
  selected
    .iter()
    .map(|dependency| {
      Ok(PackageRequirement {
        id: dependency.id.clone(),
        lower_id: normalize_id(&dependency.id)?,
        range: VersionRange::parse(&dependency.version)?,
        direct: false,
        include_assets: dependency.include_assets,
        suppress_parent: dependency.suppress_parent,
      })
    })
    .collect()
}

fn parse_asset_flags(value: Option<&str>, default: AssetFlags, path: &Path) -> Result<AssetFlags, PackageError> {
  let Some(value) = value else {
    return Ok(default);
  };
  let mut flags = AssetFlags::NONE;
  for token in value.split([',', ';']).map(str::trim).filter(|token| !token.is_empty()) {
    let flag = if token.eq_ignore_ascii_case("all") {
      AssetFlags::ALL
    } else if token.eq_ignore_ascii_case("none") {
      AssetFlags::NONE
    } else if token.eq_ignore_ascii_case("runtime") {
      AssetFlags::RUNTIME
    } else if token.eq_ignore_ascii_case("compile") {
      AssetFlags::COMPILE
    } else if token.eq_ignore_ascii_case("build") {
      AssetFlags::BUILD
    } else if token.eq_ignore_ascii_case("native") {
      AssetFlags::NATIVE
    } else if token.eq_ignore_ascii_case("contentFiles") {
      AssetFlags::CONTENT_FILES
    } else if token.eq_ignore_ascii_case("analyzers") {
      AssetFlags::ANALYZERS
    } else if token.eq_ignore_ascii_case("buildTransitive") {
      AssetFlags::BUILD_TRANSITIVE.union(AssetFlags::BUILD)
    } else {
      return Err(package_manifest_error(path, format!("unknown dependency asset category {token:?}")));
    };
    flags = flags.union(flag);
  }
  Ok(flags)
}

#[derive(Clone, Copy)]
enum NuspecText {
  None,
  Id,
  Version,
}

fn nuspec_attribute(reader: &Reader<&[u8]>, element: &quick_xml::events::BytesStart<'_>, name: &[u8], path: &Path) -> Result<Option<String>, PackageError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| package_manifest_error(path, format!("invalid nuspec attribute: {error}")))?;
    if local_name(attribute.key.as_ref()) == name {
      return attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
        .map(|value| Some(value.into_owned()))
        .map_err(|error| package_manifest_error(path, format!("invalid nuspec attribute value: {error}")));
    }
  }
  Ok(None)
}

fn package_manifest_error(path: &Path, message: impl Into<String>) -> PackageError {
  PackageError::new(PackageErrorKind::Integrity, path.display().to_string(), message)
}

fn select_compile_assets(root: &Path, target: TargetFramework) -> Result<Vec<PathBuf>, PackageError> {
  if let Some(directory) = select_framework_directory(&root.join("ref"), target)? {
    return dlls_in(&directory);
  }
  select_framework_directory(&root.join("lib"), target)?.map_or_else(|| Ok(Vec::new()), |directory| dlls_in(&directory))
}

fn select_runtime_assets(root: &Path, target: TargetFramework) -> Result<Vec<PathBuf>, PackageError> {
  select_framework_directory(&root.join("lib"), target)?.map_or_else(|| Ok(Vec::new()), |directory| dlls_in(&directory))
}

fn select_resource_assets(root: &Path, target: TargetFramework) -> Result<Vec<PathBuf>, PackageError> {
  let Some(directory) = select_framework_directory(&root.join("lib"), target)? else {
    return Ok(Vec::new());
  };
  let mut resources = Vec::new();
  for entry in fs::read_dir(&directory).map_err(|error| package_io("enumerate package resources", &directory, error))? {
    let entry = entry.map_err(|error| package_io("enumerate package resources", &directory, error))?;
    let path = entry.path();
    if entry
      .file_type()
      .map_err(|error| package_io("inspect package resources", &path, error))?
      .is_dir()
    {
      collect_files(&path, &mut resources, |path| {
        path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
      })?;
    }
  }
  resources.sort_unstable();
  Ok(resources)
}

fn select_content_files(root: &Path, target: TargetFramework, included: bool) -> Result<Vec<PathBuf>, PackageError> {
  let content_root = root.join("contentFiles");
  if !content_root.is_dir() {
    return Ok(Vec::new());
  }
  if !included {
    return Ok(vec![content_root.join("any/any/_._")]);
  }
  let mut selected = Vec::new();
  for language in ["any", "cs"] {
    let language_root = content_root.join(language);
    if !language_root.is_dir() {
      continue;
    }
    let directory = select_framework_directory(&language_root, target)?.or_else(|| {
      let any = language_root.join("any");
      any.is_dir().then_some(any)
    });
    if let Some(directory) = directory {
      collect_files(&directory, &mut selected, |_| true)?;
    }
  }
  selected.sort_unstable();
  selected.dedup();
  Ok(selected)
}

fn select_legacy_native_assets(root: &Path) -> Result<Vec<PathBuf>, PackageError> {
  let directory = root.join("native");
  let mut assets = Vec::new();
  if directory.is_dir() {
    collect_files(&directory, &mut assets, |_| true)?;
    assets.sort_unstable();
  }
  Ok(assets)
}

fn select_build_assets(root: &Path, category: &str, package_id: &str, target: TargetFramework, use_framework: bool) -> Result<Vec<PathBuf>, PackageError> {
  let category_root = root.join(category);
  if !category_root.is_dir() {
    return Ok(Vec::new());
  }
  let directory = if use_framework {
    select_framework_directory(&category_root, target)?.unwrap_or_else(|| category_root.clone())
  } else {
    category_root
  };
  let props = format!("{package_id}.props");
  let targets = format!("{package_id}.targets");
  let mut entries = fs::read_dir(&directory)
    .map_err(|error| package_io("enumerate package build assets", &directory, error))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|error| package_io("enumerate package build assets", &directory, error))?;
  entries.sort_unstable_by_key(|entry| entry.file_name().to_ascii_lowercase());
  let mut selected = Vec::with_capacity(2);
  let mut placeholder = None;
  for entry in entries {
    let path = entry.path();
    if !entry
      .file_type()
      .map_err(|error| package_io("inspect package build asset", &path, error))?
      .is_file()
    {
      continue;
    }
    let name = entry.file_name();
    let name = name.to_string_lossy();
    if name.eq_ignore_ascii_case(&props) || name.eq_ignore_ascii_case(&targets) {
      selected.push(path);
    } else if name == "_._" {
      placeholder = Some(path);
    }
  }
  if selected.is_empty()
    && let Some(placeholder) = placeholder
  {
    selected.push(placeholder);
  }
  selected.sort_unstable();
  Ok(selected)
}

fn select_runtime_targets(root: &Path, target: TargetFramework, flags: AssetFlags) -> Result<Vec<WorkRuntimeTarget>, PackageError> {
  let runtimes = root.join("runtimes");
  if !runtimes.is_dir() || !flags.contains(AssetFlags::RUNTIME) && !flags.contains(AssetFlags::NATIVE) {
    return Ok(Vec::new());
  }
  let mut selected = Vec::new();
  for entry in fs::read_dir(&runtimes).map_err(|error| package_io("enumerate package runtimes", &runtimes, error))? {
    let entry = entry.map_err(|error| package_io("enumerate package runtimes", &runtimes, error))?;
    let rid_root = entry.path();
    if !entry
      .file_type()
      .map_err(|error| package_io("inspect package runtime", &rid_root, error))?
      .is_dir()
    {
      continue;
    }
    let Some(runtime_identifier) = entry.file_name().to_str().map(str::to_owned) else {
      continue;
    };
    if flags.contains(AssetFlags::RUNTIME)
      && let Some(directory) = select_framework_directory(&rid_root.join("lib"), target)?
    {
      for path in dlls_in(&directory)? {
        selected.push(WorkRuntimeTarget {
          path,
          runtime_identifier: runtime_identifier.clone(),
          kind: RuntimeTargetKind::Runtime,
        });
      }
    }
    if flags.contains(AssetFlags::NATIVE) {
      let native_assets = select_framework_directory(&rid_root.join("nativeassets"), target)?;
      let native = native_assets.or_else(|| {
        let directory = rid_root.join("native");
        directory.is_dir().then_some(directory)
      });
      if let Some(directory) = native {
        let mut paths = Vec::new();
        collect_files(&directory, &mut paths, |_| true)?;
        for path in paths {
          selected.push(WorkRuntimeTarget {
            path,
            runtime_identifier: runtime_identifier.clone(),
            kind: RuntimeTargetKind::Native,
          });
        }
      }
    }
  }
  selected.sort_unstable_by(|left, right| {
    left
      .runtime_identifier
      .cmp(&right.runtime_identifier)
      .then_with(|| left.path.cmp(&right.path))
      .then_with(|| (left.kind as u8).cmp(&(right.kind as u8)))
  });
  Ok(selected)
}

fn select_framework_directory(category: &Path, target: TargetFramework) -> Result<Option<PathBuf>, PackageError> {
  if !category.is_dir() {
    return Ok(None);
  }
  let mut best: Option<(u32, String, PathBuf)> = None;
  for entry in fs::read_dir(category).map_err(|error| package_io("enumerate package assets", category, error))? {
    let entry = entry.map_err(|error| package_io("enumerate package assets", category, error))?;
    if !entry
      .file_type()
      .map_err(|error| package_io("inspect package assets", &entry.path(), error))?
      .is_dir()
    {
      continue;
    }
    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
      continue;
    };
    let Some(score) = framework_score(Some(&name), target) else {
      continue;
    };
    if best.as_ref().is_none_or(|current| (score, &name) > (current.0, &current.1)) {
      best = Some((score, name, entry.path()));
    }
  }
  Ok(best.map(|(_, _, path)| path))
}

fn framework_score(framework: Option<&str>, target: TargetFramework) -> Option<u32> {
  let canonical = framework?.trim().trim_start_matches('.');
  let candidate = TargetFramework::parse(canonical).ok()?;
  let version = u32::from(candidate.major()) * 100 + u32::from(candidate.minor());
  match candidate.family() {
    FrameworkFamily::Net
      if target.family() == FrameworkFamily::Net && candidate.major() >= 5 && (candidate.major(), candidate.minor()) <= (target.major(), target.minor()) =>
    {
      Some(30_000 + version)
    },
    FrameworkFamily::NetCoreApp if target.family() == FrameworkFamily::Net && (candidate.major(), candidate.minor()) <= (3, 1) => Some(20_000 + version),
    FrameworkFamily::NetStandard if target.family() == FrameworkFamily::Net && (candidate.major(), candidate.minor()) <= (2, 1) => Some(10_000 + version),
    _ => None,
  }
}

fn dlls_in(directory: &Path) -> Result<Vec<PathBuf>, PackageError> {
  let mut assets = Vec::new();
  for entry in fs::read_dir(directory).map_err(|error| package_io("enumerate package assets", directory, error))? {
    let entry = entry.map_err(|error| package_io("enumerate package assets", directory, error))?;
    let path = entry.path();
    if entry.file_type().map_err(|error| package_io("inspect package asset", &path, error))?.is_file()
      && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
    {
      assets.push(path);
    }
  }
  assets.sort_unstable();
  Ok(assets)
}

fn collect_files(directory: &Path, target: &mut Vec<PathBuf>, include: impl Copy + Fn(&Path) -> bool) -> Result<(), PackageError> {
  let mut directories = vec![directory.to_owned()];
  while let Some(directory) = directories.pop() {
    for entry in fs::read_dir(&directory).map_err(|error| package_io("enumerate package assets", &directory, error))? {
      let entry = entry.map_err(|error| package_io("enumerate package assets", &directory, error))?;
      let path = entry.path();
      let file_type = entry.file_type().map_err(|error| package_io("inspect package asset", &path, error))?;
      if file_type.is_dir() {
        directories.push(path);
      } else if file_type.is_file() && include(&path) {
        target.push(path);
      }
    }
  }
  Ok(())
}

fn collect_analyzers(root: &Path) -> Result<Vec<PathBuf>, PackageError> {
  let analyzer_root = root.join("analyzers");
  if !analyzer_root.is_dir() {
    return Ok(Vec::new());
  }
  let mut analyzers = Vec::new();
  collect_files(&analyzer_root, &mut analyzers, |path| {
    let normalized = path.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
      && !normalized.ends_with(".resources.dll")
      && !normalized.contains("/dotnet/vb/")
  })?;
  analyzers.sort_unstable();
  Ok(analyzers)
}

#[cfg(test)]
fn minimum_version_from_range(range: &VersionRange) -> Result<PackageVersion, PackageError> {
  match &range.lower {
    Some(lower) if lower.inclusive => Ok(lower.version.clone()),
    _ => Err(PackageError::new(
      PackageErrorKind::Resolution,
      "dependency range",
      "dependency range requires package source version enumeration",
    )),
  }
}

fn normalize_id(value: &str) -> Result<String, PackageError> {
  if value.is_empty() || value.len() > 100 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')) {
    return Err(PackageError::new(
      PackageErrorKind::Resolution,
      value,
      format!("package identity {value:?} is outside the supported NuGet identifier form"),
    ));
  }
  Ok(value.to_ascii_lowercase())
}

fn normalize_version(value: &str) -> Result<String, PackageError> {
  Ok(PackageVersion::parse(value)?.normalized)
}

fn validate_acyclic(packages: &BTreeMap<String, WorkPackage>) -> Result<(), PackageError> {
  fn visit(id: &str, packages: &BTreeMap<String, WorkPackage>, visiting: &mut BTreeSet<String>, visited: &mut BTreeSet<String>) -> Result<(), PackageError> {
    if visited.contains(id) {
      return Ok(());
    }
    if !visiting.insert(id.to_owned()) {
      return Err(PackageError::new(
        PackageErrorKind::Resolution,
        id,
        format!("package dependency cycle includes {id}"),
      ));
    }
    if let Some(package) = packages.get(id) {
      for dependency in &package.dependencies {
        visit(&dependency.lower_id, packages, visiting, visited)?;
      }
    }
    visiting.remove(id);
    visited.insert(id.to_owned());
    Ok(())
  }

  let mut visiting = BTreeSet::new();
  let mut visited = BTreeSet::new();
  for id in packages.keys() {
    visit(id, packages, &mut visiting, &mut visited)?;
  }
  Ok(())
}

fn materialize_resolution(
  context: ResolutionContext<'_>,
  work: &BTreeMap<String, WorkPackage>,
  network_requests: u32,
  downloaded_bytes: u64,
) -> Result<PackageResolution, PackageError> {
  let indices: BTreeMap<&str, u32> = work.keys().enumerate().map(|(index, id)| (id.as_str(), index as u32)).collect();
  let estimated = work
    .values()
    .map(|package| {
      package.request.id.len()
        + package.request.version.len()
        + package.hash.len()
        + package
          .compile_assets
          .iter()
          .chain(&package.runtime_assets)
          .chain(&package.analyzers)
          .chain(&package.resource_assets)
          .chain(&package.content_files)
          .chain(&package.build_assets)
          .chain(&package.build_multi_targeting_assets)
          .chain(&package.build_transitive_assets)
          .chain(&package.native_assets)
          .map(|path| path.as_os_str().len())
          .sum::<usize>()
        + package
          .runtime_targets
          .iter()
          .map(|asset| asset.path.as_os_str().len() + asset.runtime_identifier.len())
          .sum::<usize>()
    })
    .sum::<usize>()
    + context.cache_root.as_os_str().len()
    + context.http_cache_root.as_os_str().len()
    + context.temp_root.as_os_str().len()
    + context.fallback_roots.iter().map(|path| path.as_os_str().len()).sum::<usize>()
    + context.lock_path.as_os_str().len()
    + context.target_framework.len()
    + context.source.len()
    + context.prune_fingerprint.len();
  let mut table = TextTable::with_capacity(estimated);
  let cache_root_span = table.push_path(context.cache_root)?;
  let http_cache_root_span = table.push_path(context.http_cache_root)?;
  let temp_root_span = table.push_path(context.temp_root)?;
  let fallback_roots = context.fallback_roots.iter().map(|path| table.push_path(path)).collect::<Result<Box<_>, _>>()?;
  let lock_path_span = table.push_path(context.lock_path)?;
  let target_framework_span = table.push(context.target_framework)?;
  let source_span = table.push(context.source)?;
  let prune_fingerprint_span = table.push(context.prune_fingerprint)?;
  let mut packages = Vec::with_capacity(work.len());
  let mut package_roots = Vec::with_capacity(work.len());
  let mut package_assets = Vec::with_capacity(work.len());
  let mut package_extended_assets = Vec::with_capacity(work.len());
  let mut dependencies = Vec::new();
  let (asset_ranges, asset_count) = plan_asset_ranges(work)?;
  let mut assets = vec![TextSpan { start: 0, len: 0 }; asset_count];
  let mut asset_cursors = [
    asset_ranges.compile.start,
    asset_ranges.runtime.start,
    asset_ranges.analyzers.start,
    asset_ranges.resources.start,
    asset_ranges.content.start,
    asset_ranges.build.start,
    asset_ranges.build_multi_targeting.start,
    asset_ranges.build_transitive.start,
    asset_ranges.native.start,
  ];
  let mut runtime_targets = Vec::new();
  let mut cache_hits = 0u32;

  for package in work.values() {
    let dependency_start = u32_len(dependencies.len(), "package dependency range")?;
    for dependency in &package.dependencies {
      dependencies.push(*indices.get(dependency.lower_id.as_str()).ok_or_else(|| {
        PackageError::new(
          PackageErrorKind::Resolution,
          &dependency.id,
          format!("resolved graph omitted dependency {} {}", dependency.id, dependency.version),
        )
      })?);
    }
    let dependency_len = u32_len(package.dependencies.len(), "package dependency range")?;
    let compile = push_asset_range(&mut table, &mut assets, &mut asset_cursors[0], &package.compile_assets)?;
    let runtime = push_asset_range(&mut table, &mut assets, &mut asset_cursors[1], &package.runtime_assets)?;
    let analyzer_range = push_asset_range(&mut table, &mut assets, &mut asset_cursors[2], &package.analyzers)?;
    let resources = push_asset_range(&mut table, &mut assets, &mut asset_cursors[3], &package.resource_assets)?;
    let content = push_asset_range(&mut table, &mut assets, &mut asset_cursors[4], &package.content_files)?;
    let build = push_asset_range(&mut table, &mut assets, &mut asset_cursors[5], &package.build_assets)?;
    let build_multi_targeting = push_asset_range(&mut table, &mut assets, &mut asset_cursors[6], &package.build_multi_targeting_assets)?;
    let build_transitive = push_asset_range(&mut table, &mut assets, &mut asset_cursors[7], &package.build_transitive_assets)?;
    let native = push_asset_range(&mut table, &mut assets, &mut asset_cursors[8], &package.native_assets)?;
    let runtime_target_start = u32_len(runtime_targets.len(), "package runtime target range")?;
    for asset in &package.runtime_targets {
      runtime_targets.push(RuntimeTargetAsset {
        path: table.push_path(&asset.path)?,
        runtime_identifier: table.push(&asset.runtime_identifier)?,
        kind: asset.kind,
      });
    }
    let runtime_target_range = ItemRange {
      start: runtime_target_start,
      len: u32_len(package.runtime_targets.len(), "package runtime target range")?,
    };
    packages.push(ResolvedPackage {
      id: table.push(&package.request.id)?,
      version: table.push(&package.request.version)?,
      dependencies: ItemRange {
        start: dependency_start,
        len: dependency_len,
      },
      direct: package.request.direct,
    });
    package_roots.push(table.push_path(&package.root)?);
    package_assets.push(PackageAssets {
      hash: table.push(&package.hash)?,
      compile,
      runtime,
      analyzers: analyzer_range,
    });
    package_extended_assets.push(PackageExtendedAssets {
      resources,
      content_files: content,
      build,
      build_multi_targeting,
      build_transitive,
      native,
      runtime_targets: runtime_target_range,
    });
    cache_hits += u32::from(package.cache_hit);
  }

  debug_assert_eq!(asset_cursors[0], asset_ranges.compile.start + asset_ranges.compile.len);
  debug_assert_eq!(asset_cursors[1], asset_ranges.runtime.start + asset_ranges.runtime.len);
  debug_assert_eq!(asset_cursors[2], asset_ranges.analyzers.start + asset_ranges.analyzers.len);
  debug_assert_eq!(asset_cursors[3], asset_ranges.resources.start + asset_ranges.resources.len);
  debug_assert_eq!(asset_cursors[4], asset_ranges.content.start + asset_ranges.content.len);
  debug_assert_eq!(asset_cursors[5], asset_ranges.build.start + asset_ranges.build.len);
  debug_assert_eq!(
    asset_cursors[6],
    asset_ranges.build_multi_targeting.start + asset_ranges.build_multi_targeting.len
  );
  debug_assert_eq!(asset_cursors[7], asset_ranges.build_transitive.start + asset_ranges.build_transitive.len);
  debug_assert_eq!(asset_cursors[8], asset_ranges.native.start + asset_ranges.native.len);

  Ok(PackageResolution {
    text: table.text.into_boxed_str(),
    cache_root: cache_root_span,
    http_cache_root: http_cache_root_span,
    temp_root: temp_root_span,
    lock_path: lock_path_span,
    target_framework: target_framework_span,
    source: source_span,
    prune_fingerprint: prune_fingerprint_span,
    source_protocol: context.source_protocol,
    signature_validation: context.signature_validation,
    audit_enabled: context.audit_enabled,
    audit_mode: context.audit_mode,
    audit_level: context.audit_level,
    proxy_configured: context.proxy_configured,
    packages: packages.into_boxed_slice(),
    package_roots: package_roots.into_boxed_slice(),
    fallback_roots,
    package_assets: package_assets.into_boxed_slice(),
    package_extended_assets: package_extended_assets.into_boxed_slice(),
    dependencies: dependencies.into_boxed_slice(),
    assets: assets.into_boxed_slice(),
    asset_ranges,
    runtime_targets: runtime_targets.into_boxed_slice(),
    cache_hits,
    downloaded_packages: work.len() as u32 - cache_hits,
    network_requests,
    downloaded_bytes,
  })
}

fn plan_asset_ranges(work: &BTreeMap<String, WorkPackage>) -> Result<(PackageAssetRanges, usize), PackageError> {
  let mut counts = [0usize; 9];
  for package in work.values() {
    for (count, additional) in counts.iter_mut().zip([
      package.compile_assets.len(),
      package.runtime_assets.len(),
      package.analyzers.len(),
      package.resource_assets.len(),
      package.content_files.len(),
      package.build_assets.len(),
      package.build_multi_targeting_assets.len(),
      package.build_transitive_assets.len(),
      package.native_assets.len(),
    ]) {
      *count = count
        .checked_add(additional)
        .ok_or_else(|| PackageError::new(PackageErrorKind::TextOverflow, "package assets", "package asset count overflowed usize"))?;
    }
  }
  let mut start = 0usize;
  let mut next = |length: usize| {
    let range = ItemRange {
      start: u32_len(start, "package asset family range")?,
      len: u32_len(length, "package asset family range")?,
    };
    start = start
      .checked_add(length)
      .ok_or_else(|| PackageError::new(PackageErrorKind::TextOverflow, "package assets", "package asset count overflowed usize"))?;
    Ok(range)
  };
  let ranges = PackageAssetRanges {
    compile: next(counts[0])?,
    runtime: next(counts[1])?,
    analyzers: next(counts[2])?,
    resources: next(counts[3])?,
    content: next(counts[4])?,
    build: next(counts[5])?,
    build_multi_targeting: next(counts[6])?,
    build_transitive: next(counts[7])?,
    native: next(counts[8])?,
  };
  Ok((ranges, start))
}

fn push_asset_range(table: &mut TextTable, target: &mut [TextSpan], cursor: &mut u32, paths: &[PathBuf]) -> Result<ItemRange, PackageError> {
  let start = *cursor;
  let len = u32_len(paths.len(), "package asset range")?;
  let end = start
    .checked_add(len)
    .ok_or_else(|| PackageError::new(PackageErrorKind::TextOverflow, "package assets", "package asset range overflowed u32"))?;
  let slots = target
    .get_mut(start as usize..end as usize)
    .ok_or_else(|| PackageError::new(PackageErrorKind::TextOverflow, "package assets", "package asset range exceeds its family"))?;
  for (slot, path) in slots.iter_mut().zip(paths) {
    *slot = table.push_path(path)?;
  }
  *cursor = end;
  Ok(ItemRange { start, len })
}

fn empty_resolution(project: &ProjectSpec) -> Result<PackageResolution, PackageError> {
  let mut table = TextTable::with_capacity(project.project_path().as_os_str().len() + project.target_framework().len() + 32);
  let empty = table.push("")?;
  let lock = table.push_path(&project.project_directory().join("dv.lock.json"))?;
  let target_framework = table.push(project.target_framework())?;
  Ok(PackageResolution {
    text: table.text.into_boxed_str(),
    cache_root: empty,
    http_cache_root: empty,
    temp_root: empty,
    lock_path: lock,
    target_framework,
    source: empty,
    prune_fingerprint: empty,
    source_protocol: NugetProtocol::V3,
    signature_validation: SignatureValidationMode::Accept,
    audit_enabled: project.nuget_audit_enabled(),
    audit_mode: project.nuget_audit_mode(),
    audit_level: project.nuget_audit_level(),
    proxy_configured: false,
    packages: Box::new([]),
    package_roots: Box::new([]),
    fallback_roots: Box::new([]),
    package_assets: Box::new([]),
    package_extended_assets: Box::new([]),
    dependencies: Box::new([]),
    assets: Box::new([]),
    asset_ranges: PackageAssetRanges {
      compile: ItemRange { start: 0, len: 0 },
      runtime: ItemRange { start: 0, len: 0 },
      analyzers: ItemRange { start: 0, len: 0 },
      resources: ItemRange { start: 0, len: 0 },
      content: ItemRange { start: 0, len: 0 },
      build: ItemRange { start: 0, len: 0 },
      build_multi_targeting: ItemRange { start: 0, len: 0 },
      build_transitive: ItemRange { start: 0, len: 0 },
      native: ItemRange { start: 0, len: 0 },
    },
    runtime_targets: Box::new([]),
    cache_hits: 0,
    downloaded_packages: 0,
    network_requests: 0,
    downloaded_bytes: 0,
  })
}

fn read_warm_lock(
  path: &Path,
  config: &NugetConfiguration,
  direct: &[PackageRequirement],
  project: &ProjectSpec,
  prune_fingerprint: &str,
) -> Result<Option<PackageResolution>, PackageError> {
  let target_text = project.target_framework();
  let bytes = match fs::read(path) {
    Ok(bytes) => bytes,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
    Err(error) => return Err(package_io("read dv package lock", path, error)),
  };
  let lock: LockFile = serde_json::from_slice(&bytes).map_err(|error| {
    PackageError::new(
      PackageErrorKind::Integrity,
      path.display().to_string(),
      format!("invalid dv package lock: {error}"),
    )
  })?;
  let direct_matches = lock.direct.len() == direct.len()
    && direct.iter().all(|request| {
      lock
        .direct
        .iter()
        .find(|locked| locked.id.eq_ignore_ascii_case(&request.id))
        .and_then(|locked| PackageVersion::parse(&locked.version).ok())
        .is_some_and(|version| request.range.contains(&version))
    });
  if lock.schema_version != LOCK_SCHEMA_VERSION
    || lock.target_framework != target_text
    || lock.prune_fingerprint != prune_fingerprint
    || !direct_matches
    || !config
      .sources
      .iter()
      .any(|(_, source)| source.url == lock.source && source.protocol == lock.source_protocol)
  {
    return Ok(None);
  }

  let mut work = BTreeMap::new();
  for package in lock.packages {
    let request = PackageRequest {
      lower_id: normalize_id(&package.id)?,
      version: normalize_version(&package.version)?,
      id: package.id,
      direct: package.direct,
    };
    let root = find_package_root(&config.cache_root, &config.fallback_roots, &request).ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::Integrity,
        format!("{} {}", request.id, request.version),
        format!(
          "locked package cache entry for {} {} is absent from global and fallback package folders",
          request.id, request.version
        ),
      )
    })?;
    validate_locked_package(&root, &request, &package.sha512)?;
    let compile_assets = lock_asset_paths(&root, &package.compile_assets)?;
    let runtime_assets = lock_asset_paths(&root, &package.runtime_assets)?;
    let analyzers = lock_asset_paths(&root, &package.analyzers)?;
    let resource_assets = lock_asset_paths(&root, &package.resource_assets)?;
    let content_files = lock_asset_paths(&root, &package.content_files)?;
    let build_assets = lock_asset_paths(&root, &package.build_assets)?;
    let build_multi_targeting_assets = lock_asset_paths(&root, &package.build_multi_targeting_assets)?;
    let build_transitive_assets = lock_asset_paths(&root, &package.build_transitive_assets)?;
    let native_assets = lock_asset_paths(&root, &package.native_assets)?;
    let runtime_targets = package
      .runtime_targets
      .into_iter()
      .map(|asset| {
        if asset.runtime_identifier.is_empty() || asset.runtime_identifier.contains(['/', '\\']) || asset.runtime_identifier.contains("..") {
          return Err(PackageError::new(
            PackageErrorKind::Integrity,
            root.display().to_string(),
            format!("invalid locked runtime identifier {:?}", asset.runtime_identifier),
          ));
        }
        Ok(WorkRuntimeTarget {
          path: lock_asset_path(&root, &asset.path)?,
          runtime_identifier: asset.runtime_identifier,
          kind: asset.kind,
        })
      })
      .collect::<Result<Vec<_>, PackageError>>()?;
    let dependencies = package
      .dependencies
      .into_iter()
      .map(|dependency| {
        Ok(PackageRequest {
          lower_id: normalize_id(&dependency.id)?,
          version: normalize_version(&dependency.version)?,
          id: dependency.id,
          direct: false,
        })
      })
      .collect::<Result<Vec<_>, PackageError>>()?;
    if work
      .insert(
        request.lower_id.clone(),
        WorkPackage {
          request,
          root,
          hash: package.sha512,
          dependencies,
          compile_assets,
          runtime_assets,
          analyzers,
          resource_assets,
          content_files,
          build_assets,
          build_multi_targeting_assets,
          build_transitive_assets,
          native_assets,
          runtime_targets,
          cache_hit: true,
          origin: None,
        },
      )
      .is_some()
    {
      return Err(PackageError::new(
        PackageErrorKind::Integrity,
        path.display().to_string(),
        "dv package lock contains a duplicate package identity",
      ));
    }
  }
  for request in direct {
    if !work
      .get(&request.lower_id)
      .is_some_and(|package| package.request.direct && PackageVersion::parse(&package.request.version).is_ok_and(|version| request.range.contains(&version)))
    {
      return Err(PackageError::new(
        PackageErrorKind::Integrity,
        path.display().to_string(),
        format!("dv package lock omits a compatible direct package {}", request.id),
      ));
    }
  }
  validate_acyclic(&work)?;
  materialize_resolution(
    ResolutionContext {
      cache_root: &config.cache_root,
      http_cache_root: &config.http_cache_root,
      temp_root: &config.temp_root,
      fallback_roots: &config.fallback_roots,
      lock_path: path,
      target_framework: target_text,
      source: &lock.source,
      prune_fingerprint,
      source_protocol: lock.source_protocol,
      signature_validation: config.signature_validation,
      audit_enabled: project.nuget_audit_enabled(),
      audit_mode: project.nuget_audit_mode(),
      audit_level: project.nuget_audit_level(),
      proxy_configured: config.proxy.is_some(),
    },
    &work,
    0,
    0,
  )
  .map(Some)
}

fn lock_asset_paths(root: &Path, values: &[String]) -> Result<Vec<PathBuf>, PackageError> {
  let mut paths = Vec::with_capacity(values.len());
  for value in values {
    paths.push(lock_asset_path(root, value)?);
  }
  Ok(paths)
}

fn lock_asset_path(root: &Path, value: &str) -> Result<PathBuf, PackageError> {
  let relative = Path::new(value);
  if relative.is_absolute()
    || relative
      .components()
      .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
  {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      root.display().to_string(),
      format!("lock asset path {value:?} escapes its package"),
    ));
  }
  // The completion marker and locked archive hash prove the immutable entry.
  // Per-asset stats add thousands of syscalls to a large warm plan; consumers
  // validate concrete files when they open or copy them.
  Ok(root.join(relative))
}

fn validate_locked_package(root: &Path, request: &PackageRequest, expected_hash: &str) -> Result<(), PackageError> {
  // A completion marker is published only after the immutable package root is
  // fully extracted. Check the common dv-owned marker first so the warm path
  // performs one metadata request per package rather than two.
  if !root.join(".dv.metadata.json").is_file() && !root.join(".nupkg.metadata").is_file() {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      root.display().to_string(),
      format!("locked package cache entry for {} {} has no completion marker", request.id, request.version),
    ));
  }
  let hash_path = root.join(format!("{}.{}.nupkg.sha512", request.lower_id, request.version));
  let decoded = BASE64.decode(expected_hash).map_err(|error| {
    PackageError::new(
      PackageErrorKind::Integrity,
      hash_path.display().to_string(),
      format!("invalid locked package SHA-512: {error}"),
    )
  })?;
  if decoded.len() != 64 {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      hash_path.display().to_string(),
      "locked package SHA-512 must decode to 64 bytes",
    ));
  }
  Ok(())
}

fn write_lock(resolution: &PackageResolution) -> Result<(), PackageError> {
  if resolution.packages.is_empty() {
    return Ok(());
  }
  let mut direct = Vec::new();
  let mut packages = Vec::with_capacity(resolution.packages.len());
  for (index, package) in resolution.packages.iter().copied().enumerate() {
    let id = resolution.package_id(package).to_owned();
    let version = resolution.package_version(package).to_owned();
    if package.direct {
      direct.push(LockDirect {
        id: id.clone(),
        version: version.clone(),
      });
    }
    let dependencies = resolution
      .package_dependencies(package)
      .map(|dependency| {
        let dependency = resolution.packages[dependency as usize];
        LockDirect {
          id: resolution.package_id(dependency).to_owned(),
          version: resolution.package_version(dependency).to_owned(),
        }
      })
      .collect();
    let root = resolution.package_root_at(index);
    packages.push(LockPackage {
      id,
      version,
      sha512: resolution.package_hash(index).to_owned(),
      direct: package.direct,
      dependencies,
      compile_assets: relative_assets(root, resolution.package_compile_assets(index))?,
      runtime_assets: relative_assets(root, resolution.package_runtime_assets(index))?,
      analyzers: relative_assets(root, resolution.package_analyzers(index))?,
      resource_assets: relative_assets(root, resolution.package_resource_assets(index))?,
      content_files: relative_assets(root, resolution.package_content_files(index))?,
      build_assets: relative_assets(root, resolution.package_build_assets(index))?,
      build_multi_targeting_assets: relative_assets(root, resolution.package_build_multi_targeting_assets(index))?,
      build_transitive_assets: relative_assets(root, resolution.package_build_transitive_assets(index))?,
      native_assets: relative_assets(root, resolution.package_native_assets(index))?,
      runtime_targets: resolution
        .package_runtime_targets(index)
        .map(|asset| {
          Ok(LockRuntimeTarget {
            path: relative_asset(root, resolution.get(asset.path))?,
            runtime_identifier: resolution.get(asset.runtime_identifier).to_owned(),
            kind: asset.kind,
          })
        })
        .collect::<Result<Vec<_>, PackageError>>()?,
    });
  }
  let lock = LockFile {
    schema_version: LOCK_SCHEMA_VERSION,
    target_framework: resolution.target_framework().into(),
    source: resolution.source().into(),
    source_protocol: resolution.source_protocol,
    prune_fingerprint: resolution.get(resolution.prune_fingerprint).into(),
    direct,
    packages,
  };
  let mut bytes = serde_json::to_vec_pretty(&lock).expect("serializing dv package lock succeeds");
  bytes.push(b'\n');
  let path = resolution.lock_path();
  if fs::read(path).is_ok_and(|existing| existing == bytes) {
    return Ok(());
  }
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|error| package_io("create lock directory", parent, error))?;
  }
  let temp = path.with_extension(format!("lock.{}.tmp", std::process::id()));
  let mut file = fs::File::create(&temp).map_err(|error| package_io("create temporary lock", &temp, error))?;
  file.write_all(&bytes).map_err(|error| package_io("write temporary lock", &temp, error))?;
  file.sync_all().map_err(|error| package_io("flush temporary lock", &temp, error))?;
  if let Err(error) = fs::rename(&temp, path) {
    if path.exists() {
      fs::remove_file(path).map_err(|remove_error| package_io("replace package lock", path, remove_error))?;
      fs::rename(&temp, path).map_err(|rename_error| package_io("replace package lock", path, rename_error))?;
    } else {
      return Err(package_io("publish package lock", path, error));
    }
  }
  Ok(())
}

fn relative_assets<'a>(root: &Path, assets: impl Iterator<Item = &'a str>) -> Result<Vec<String>, PackageError> {
  assets.map(|asset| relative_asset(root, asset)).collect()
}

fn relative_asset(root: &Path, asset: &str) -> Result<String, PackageError> {
  let path = Path::new(asset);
  path
    .strip_prefix(root)
    .map_err(|_| {
      PackageError::new(
        PackageErrorKind::Integrity,
        path.display().to_string(),
        "package asset is outside its cache entry",
      )
    })
    .and_then(portable_path)
}

fn portable_path(path: &Path) -> Result<String, PackageError> {
  let value = path.to_str().ok_or_else(|| {
    PackageError::new(
      PackageErrorKind::NonUnicodePath,
      path.display().to_string(),
      "package asset path is not valid Unicode",
    )
  })?;
  Ok(value.replace('\\', "/"))
}

fn local_name(name: &[u8]) -> &[u8] {
  name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn unique_temp_root(cache_root: &Path, request: &PackageRequest) -> PathBuf {
  let nonce = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
  cache_root.join(format!(".{}.{}.{}.{}.tmp", request.lower_id, request.version, std::process::id(), nonce))
}

struct TempGuard(Option<PathBuf>);

impl Drop for TempGuard {
  fn drop(&mut self) {
    if let Some(path) = self.0.take() {
      let _ = fs::remove_dir_all(path);
    }
  }
}

fn package_io(operation: &str, path: &Path, error: io::Error) -> PackageError {
  PackageError::new(
    PackageErrorKind::Io,
    path.display().to_string(),
    format!("failed to {operation} {}: {error}", path.display()),
  )
}

fn range(value: ItemRange) -> std::ops::Range<usize> {
  let start = value.start as usize;
  start..start + value.len as usize
}

fn u32_len(value: usize, meaning: &str) -> Result<u32, PackageError> {
  u32::try_from(value).map_err(|_| PackageError::new(PackageErrorKind::TextOverflow, meaning, format!("{meaning} exceeds u32")))
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

  fn push(&mut self, value: &str) -> Result<TextSpan, PackageError> {
    let start = u32_len(self.text.len(), "package text table")?;
    let len = u32_len(value.len(), "package text value")?;
    self.text.push_str(value);
    Ok(TextSpan { start, len })
  }

  fn push_path(&mut self, path: &Path) -> Result<TextSpan, PackageError> {
    let value = path.to_str().ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::NonUnicodePath,
        path.display().to_string(),
        "package path is not valid Unicode",
      )
    })?;
    self.push(value)
  }
}

#[cfg(test)]
mod tests {
  use std::{
    env,
    sync::atomic::{AtomicU64, Ordering},
  };

  use crate::{ProjectConfiguration, evaluate_project_path};
  use zip::{ZipWriter, write::SimpleFileOptions};

  use super::*;

  static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
      let path = env::temp_dir().join(format!("dv-package-test-{}-{nonce}", std::process::id()));
      fs::create_dir_all(&path).unwrap();
      Self(path)
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) -> PathBuf {
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

  fn request() -> PackageRequest {
    PackageRequest {
      id: "Sample.Package".into(),
      lower_id: "sample.package".into(),
      version: "1.2.3".into(),
      direct: true,
    }
  }

  fn write_test_package(temp: &TempDirectory, relative: &str, id: &str, version: &str) -> PathBuf {
    let path = temp.0.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = fs::File::create(&path).unwrap();
    let mut archive = ZipWriter::new(file);
    archive
      .start_file(
        format!("{id}.nuspec"),
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
      )
      .unwrap();
    archive
      .write_all(format!(r#"<package><metadata><id>{id}</id><version>{version}</version></metadata></package>"#).as_bytes())
      .unwrap();
    archive
      .start_file(
        format!("lib/net10.0/{id}.dll"),
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
      )
      .unwrap();
    archive.write_all(b"managed assembly placeholder").unwrap();
    archive.finish().unwrap();
    path
  }

  #[test]
  fn package_versions_follow_nuget_semver_precedence_and_normalization() {
    let alpha_two = PackageVersion::parse("1.0-alpha.2+BUILD").unwrap();
    let alpha_ten = PackageVersion::parse("1.0.0-alpha.10").unwrap();
    let stable = PackageVersion::parse("1.0.0.0").unwrap();

    assert_eq!(alpha_two.normalized, "1.0.0-alpha.2");
    assert!(alpha_two < alpha_ten);
    assert!(alpha_ten < stable);
    assert_eq!(stable.normalized, "1.0.0");
  }

  #[test]
  fn typed_ranges_preserve_inclusive_and_exclusive_bounds() {
    let minimum = VersionRange::parse("1.2").unwrap();
    let bounded = VersionRange::parse("(1.2,2.0]").unwrap();
    let one_two = PackageVersion::parse("1.2.0").unwrap();
    let one_three = PackageVersion::parse("1.3.0").unwrap();
    let two = PackageVersion::parse("2.0.0").unwrap();

    assert!(minimum.contains(&one_two));
    assert!(minimum.contains(&two));
    assert!(!bounded.contains(&one_two));
    assert!(bounded.contains(&one_three));
    assert!(bounded.contains(&two));
  }

  #[test]
  fn sdk_pruning_data_uses_the_framework_patch_ceiling() {
    let temp = TempDirectory::new();
    let path = temp.write(
      "PackageOverrides.txt",
      "System.IO.Pipelines|10.0.0\nSystem.Runtime.CompilerServices.Unsafe|7.0.0\n",
    );

    let pruning = read_package_pruning(&path).unwrap();

    assert!(pruning.contains("system.io.pipelines", &PackageVersion::parse("10.0.32767").unwrap()));
    assert!(!pruning.contains("system.io.pipelines", &PackageVersion::parse("10.0.32768").unwrap()));
    assert!(pruning.contains("system.runtime.compilerservices.unsafe", &PackageVersion::parse("7.0.19").unwrap()));
    assert_eq!(pruning.packages.len(), 2);
    assert!(!pruning.fingerprint.is_empty());
  }

  #[test]
  fn legacy_lock_without_a_pruning_fingerprint_is_a_cold_miss() {
    let temp = TempDirectory::new();
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
    );
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let path = temp.write(
      "dv.lock.json",
      r#"{"schema_version":1,"target_framework":"net10.0","source":"https://api.nuget.org/v3/index.json","source_protocol":"v3","direct":[],"packages":[]}"#,
    );
    let config = NugetConfiguration {
      cache_root: temp.0.join("packages"),
      http_cache_root: temp.0.join("http-cache"),
      temp_root: temp.0.join("scratch"),
      fallback_roots: Arc::from([]),
      sources: vec![(
        "nuget.org".into(),
        PackageSource {
          url: DEFAULT_SOURCE.into(),
          protocol: NugetProtocol::V3,
        },
      )],
      audit_sources: Vec::new(),
      source_mapping: None,
      signature_validation: SignatureValidationMode::Accept,
      proxy: None,
    };

    let result = read_warm_lock(&path, &config, &[], &project, "current-table").unwrap();

    assert!(result.is_none());
  }

  #[test]
  fn pruning_retracts_transitive_edges_but_never_removes_direct_packages() {
    let pruning = compact_package_pruning(vec![
      ParsedPrunedPackage {
        lower_id: "direct.package".into(),
        upper: PackageVersion::parse("10.0.32767").unwrap(),
      },
      ParsedPrunedPackage {
        lower_id: "framework.package".into(),
        upper: PackageVersion::parse("10.0.32767").unwrap(),
      },
    ])
    .unwrap();
    let grandchild = PackageRequirement {
      id: "Grandchild.Package".into(),
      lower_id: "grandchild.package".into(),
      range: VersionRange::parse("1.0").unwrap(),
      direct: false,
      include_assets: AssetFlags::ALL,
      suppress_parent: AssetFlags::NONE,
    };
    let mut nodes = BTreeMap::from([
      (
        "framework.package".into(),
        ConstraintNode {
          id: "Framework.Package".into(),
          direct: None,
          constraints: BTreeMap::from([("parent.package".into(), VersionRange::parse("10.0").unwrap())]),
          selected: None,
          metadata_version: None,
          dependencies: vec![grandchild],
          available_versions: None,
          pruned: false,
          generation: 0,
        },
      ),
      (
        "grandchild.package".into(),
        ConstraintNode {
          id: "Grandchild.Package".into(),
          direct: None,
          constraints: BTreeMap::from([("framework.package".into(), VersionRange::parse("1.0").unwrap())]),
          selected: Some(PackageVersion::parse("1.0.0").unwrap()),
          metadata_version: None,
          dependencies: Vec::new(),
          available_versions: None,
          pruned: false,
          generation: 1,
        },
      ),
      (
        "direct.package".into(),
        ConstraintNode {
          id: "Direct.Package".into(),
          direct: Some(VersionRange::parse("10.0").unwrap()),
          constraints: BTreeMap::new(),
          selected: None,
          metadata_version: None,
          dependencies: Vec::new(),
          available_versions: None,
          pruned: false,
          generation: 0,
        },
      ),
    ]);
    let mut dirty = BTreeSet::from(["framework.package".into(), "direct.package".into()]);
    let mut ready = BTreeSet::new();

    stabilize_constraint_nodes(&mut nodes, &mut dirty, &mut ready, &pruning).unwrap();

    assert!(nodes["framework.package"].pruned);
    assert!(!nodes.contains_key("grandchild.package"));
    assert!(!nodes["direct.package"].pruned);
    assert_eq!(ready, BTreeSet::from(["direct.package".into()]));
  }

  #[test]
  fn cousin_constraints_choose_the_lowest_common_version() {
    let node = ConstraintNode {
      id: "Common.Package".into(),
      direct: None,
      constraints: BTreeMap::from([
        ("a".into(), VersionRange::parse("1.0").unwrap()),
        ("b".into(), VersionRange::parse("[2.0,3.0)").unwrap()),
      ]),
      selected: None,
      metadata_version: None,
      dependencies: Vec::new(),
      available_versions: None,
      pruned: false,
      generation: 0,
    };

    let NodeSelection::Version(selected) = select_node_version(&node).unwrap() else {
      panic!("inclusive lower bounds select without enumeration");
    };
    assert_eq!(selected.normalized, "2.0.0");
  }

  #[test]
  fn direct_dependency_wins_over_a_transitive_minimum() {
    let node = ConstraintNode {
      id: "Direct.Package".into(),
      direct: Some(VersionRange::exact(PackageVersion::parse("1.0.0").unwrap())),
      constraints: BTreeMap::from([("parent".into(), VersionRange::parse("2.0").unwrap())]),
      selected: None,
      metadata_version: None,
      dependencies: Vec::new(),
      available_versions: None,
      pruned: false,
      generation: 0,
    };

    let NodeSelection::Version(selected) = select_node_version(&node).unwrap() else {
      panic!("an exact direct dependency selects without enumeration");
    };
    assert_eq!(selected.normalized, "1.0.0");
  }

  #[test]
  fn stable_ranges_do_not_select_prerelease_versions_during_enumeration() {
    let node = ConstraintNode {
      id: "Stable.Package".into(),
      direct: Some(VersionRange::parse("[1.0,2.0)").unwrap()),
      constraints: BTreeMap::new(),
      selected: None,
      metadata_version: None,
      dependencies: Vec::new(),
      available_versions: Some(vec![PackageVersion::parse("1.1.0-beta.1").unwrap(), PackageVersion::parse("1.1.0").unwrap()]),
      pruned: false,
      generation: 0,
    };

    let NodeSelection::Version(selected) = select_node_version(&node).unwrap() else {
      panic!("an enumerated stable version is available");
    };
    assert_eq!(selected.normalized, "1.1.0");
  }

  #[test]
  fn changing_a_selection_retracts_its_stale_dependency_edges() {
    let child_requirement = PackageRequirement {
      id: "Child.Package".into(),
      lower_id: "child.package".into(),
      range: VersionRange::parse("1.0").unwrap(),
      direct: false,
      include_assets: AssetFlags::ALL,
      suppress_parent: AssetFlags::NONE,
    };
    let mut nodes = BTreeMap::from([
      (
        "parent.package".into(),
        ConstraintNode {
          id: "Parent.Package".into(),
          direct: Some(VersionRange::exact(PackageVersion::parse("2.0.0").unwrap())),
          constraints: BTreeMap::new(),
          selected: Some(PackageVersion::parse("1.0.0").unwrap()),
          metadata_version: Some(PackageVersion::parse("1.0.0").unwrap()),
          dependencies: vec![child_requirement],
          available_versions: None,
          pruned: false,
          generation: 1,
        },
      ),
      (
        "child.package".into(),
        ConstraintNode {
          id: "Child.Package".into(),
          direct: None,
          constraints: BTreeMap::from([("parent.package".into(), VersionRange::parse("1.0").unwrap())]),
          selected: Some(PackageVersion::parse("1.0.0").unwrap()),
          metadata_version: None,
          dependencies: Vec::new(),
          available_versions: None,
          pruned: false,
          generation: 1,
        },
      ),
    ]);
    let mut dirty = BTreeSet::from(["parent.package".into()]);
    let mut ready = BTreeSet::new();

    stabilize_constraint_nodes(&mut nodes, &mut dirty, &mut ready, &PackagePruning::default()).unwrap();

    assert_eq!(nodes["parent.package"].selected.as_ref().unwrap().normalized, "2.0.0");
    assert!(!nodes.contains_key("child.package"));
    assert!(ready.contains("parent.package"));
  }

  #[test]
  fn nuget_config_keeps_v2_and_v3_as_typed_sources() {
    let temp = TempDirectory::new();
    let path = temp.write(
      "NuGet.Config",
      r#"<configuration><packageSources><clear />
<add key="legacy" value="https://packages.example.test/api/v2/" protocolVersion="2" />
<add key="modern" value="https://packages.example.test/v3/index.json" protocolVersion="3" />
</packageSources></configuration>"#,
    );
    let mut merged = NugetConfigMerge::default();

    merge_config(&path, &mut merged).unwrap();

    assert_eq!(merged.sources[0].0, "legacy");
    assert_eq!(merged.sources[0].1.protocol, NugetProtocol::V2);
    assert_eq!(merged.sources[1].0, "modern");
    assert_eq!(merged.sources[1].1.protocol, NugetProtocol::V3);
  }

  #[test]
  fn package_source_rejects_embedded_credentials_before_reporting() {
    let error = PackageSource::parse(
      "https://user:secret@packages.example.test/v3/index.json".into(),
      Some("3"),
      Path::new("NuGet.Config"),
      Path::new("."),
    )
    .err()
    .expect("embedded credentials must be rejected");

    assert_eq!(error.kind(), PackageErrorKind::Configuration);
    assert!(error.message.contains("must not embed credentials"));
  }

  #[test]
  fn nuget_audit_sources_and_source_mappings_merge_as_typed_batches() {
    let temp = TempDirectory::new();
    let lower = temp.write(
      "lower.config",
      r#"<configuration>
<auditSources><clear /><add key="security" value="https://audit.example.test/v2" protocolVersion="2" /><add key="stale" value="https://stale.example.test/v3/index.json" /></auditSources>
<packageSourceMapping>
  <packageSource key="selected"><package pattern="Old.*" /></packageSource>
  <packageSource key="internal"><package pattern="Company.*" /></packageSource>
</packageSourceMapping>
</configuration>"#,
    );
    let middle = temp.write(
      "middle.config",
      r#"<configuration>
<auditSources><add key="SECURITY" value="https://audit.example.test/v3/index.json" protocolVersion="3" /><remove key="stale" /></auditSources>
<packageSourceMapping>
  <packageSource key="SELECTED"><package pattern="Newtonsoft.Json" /><package pattern="Newtonsoft.*" /></packageSource>
</packageSourceMapping>
</configuration>"#,
    );
    let higher = temp.write(
      "higher.config",
      r#"<configuration>
<auditSources><clear /><add key="final" value="https://final.example.test/v3/index.json" protocolVersion="3" /></auditSources>
<packageSourceMapping><clear /><packageSource key="selected"><package pattern="Newtonsoft.*" /></packageSource></packageSourceMapping>
</configuration>"#,
    );
    let mut merged = NugetConfigMerge::default();

    merge_config(&lower, &mut merged).unwrap();
    merge_config(&middle, &mut merged).unwrap();

    assert_eq!(merged.audit_sources.len(), 1);
    assert_eq!(merged.audit_sources[0].0, "SECURITY");
    assert_eq!(merged.audit_sources[0].1.protocol, NugetProtocol::V3);
    assert_eq!(merged.source_mapping.patterns_for("selected").unwrap(), ["Newtonsoft.Json", "Newtonsoft.*"]);
    assert_eq!(merged.source_mapping.patterns_for("internal").unwrap(), ["Company.*"]);

    merge_config(&higher, &mut merged).unwrap();

    assert_eq!(merged.audit_sources.len(), 1);
    assert_eq!(merged.audit_sources[0].0, "final");
    assert_eq!(merged.audit_sources[0].1.protocol, NugetProtocol::V3);
    assert_eq!(merged.source_mapping.sources.len(), 1);
    assert_eq!(merged.source_mapping.patterns_for("selected").unwrap(), ["Newtonsoft.*"]);
  }

  #[test]
  fn nuget_source_mapping_rejects_duplicate_or_empty_source_groups() {
    let temp = TempDirectory::new();
    let duplicate = temp.write(
      "duplicate.config",
      r#"<configuration><packageSourceMapping>
<packageSource key="selected"><package pattern="A.*" /></packageSource>
<packageSource key="SELECTED"><package pattern="B.*" /></packageSource>
</packageSourceMapping></configuration>"#,
    );
    let empty = temp.write(
      "empty.config",
      r#"<configuration><packageSourceMapping><packageSource key="selected"></packageSource></packageSourceMapping></configuration>"#,
    );

    let duplicate_error = merge_config(&duplicate, &mut NugetConfigMerge::default()).unwrap_err();
    let empty_error = merge_config(&empty, &mut NugetConfigMerge::default()).unwrap_err();

    assert_eq!(duplicate_error.kind(), PackageErrorKind::Configuration);
    assert!(duplicate_error.to_string().contains("duplicate source"));
    assert_eq!(empty_error.kind(), PackageErrorKind::Configuration);
    assert!(empty_error.to_string().contains("at least one package pattern"));
  }

  #[test]
  fn nuget_source_mapping_uses_the_longest_case_insensitive_pattern() {
    let temp = TempDirectory::new();
    let path = temp.write(
      "NuGet.Config",
      r#"<configuration><packageSourceMapping>
<packageSource key="fallback"><package pattern="*" /></packageSource>
<packageSource key="family"><package pattern="Newtonsoft.*" /></packageSource>
<packageSource key="exact-a"><package pattern="Newtonsoft.Json" /></packageSource>
<packageSource key="exact-b"><package pattern="newtonsoft.json" /></packageSource>
</packageSourceMapping></configuration>"#,
    );
    let mut merged = NugetConfigMerge::default();

    merge_config(&path, &mut merged).unwrap();
    let sources = ["fallback", "family", "exact-a", "exact-b"]
      .into_iter()
      .map(|name| {
        (
          name.to_owned(),
          PackageSource {
            url: DEFAULT_SOURCE.to_owned(),
            protocol: NugetProtocol::V3,
          },
        )
      })
      .collect::<Vec<_>>();
    let mapping = PackageSourceMapping::compile(merged.source_mapping, &sources).unwrap();

    assert!(mapping.allows(2, "newtonsoft.json"));
    assert!(mapping.allows(3, "Newtonsoft.Json"));
    assert!(!mapping.allows(1, "Newtonsoft.Json"));
    assert!(!mapping.allows(0, "Newtonsoft.Json"));
    assert!(mapping.allows(1, "Newtonsoft.Schema"));
    assert!(!mapping.allows(0, "Newtonsoft.Schema"));
    assert!(mapping.allows(0, "Other.Package"));
  }

  #[test]
  fn unavailable_exact_mapping_does_not_fall_back_to_a_broader_source() {
    let temp = TempDirectory::new();
    let path = temp.write(
      "NuGet.Config",
      r#"<configuration><packageSourceMapping>
<packageSource key="fallback"><package pattern="*" /></packageSource>
<packageSource key="disabled"><package pattern="Private.Package" /></packageSource>
</packageSourceMapping></configuration>"#,
    );
    let mut merged = NugetConfigMerge::default();
    merge_config(&path, &mut merged).unwrap();
    let sources = vec![(
      "fallback".to_owned(),
      PackageSource {
        url: DEFAULT_SOURCE.to_owned(),
        protocol: NugetProtocol::V3,
      },
    )];
    let mapping = PackageSourceMapping::compile(merged.source_mapping, &sources).unwrap();

    assert!(!mapping.allows(0, "Private.Package"));
    assert!(mapping.allows(0, "Public.Package"));
  }

  #[test]
  fn nuget_keyed_sections_merge_clear_add_remove_and_disabled_sources() {
    let temp = TempDirectory::new();
    let lower = temp.write(
      "lower.config",
      r#"<configuration>
<config><add key="globalPackagesFolder" value="lower-packages" /></config>
<packageSources><clear /><add key="selected" value="https://old.example.test/v2" /><add key="removed" value="https://removed.example.test/v2" /></packageSources>
<disabledPackageSources><add key="selected" value="true" /><add key="removed" value="true" /></disabledPackageSources>
</configuration>"#,
    );
    let higher = temp.write(
      "higher.config",
      r#"<configuration>
<config><clear /><add key="globalPackagesFolder" value="higher-packages" /></config>
<packageSources><add key="SELECTED" value="https://selected.example.test/v3/index.json" protocolVersion="3" /><remove key="removed" /></packageSources>
<disabledPackageSources><clear /><add key="selected" value="false" /><add key="removed" value="true" /><remove key="SELECTED" /><remove key="REMOVED" /></disabledPackageSources>
</configuration>"#,
    );
    let mut merged = NugetConfigMerge::default();
    merged.sources.push((
      "nuget.org".into(),
      PackageSource {
        url: DEFAULT_SOURCE.into(),
        protocol: NugetProtocol::V3,
      },
    ));

    merge_config(&lower, &mut merged).unwrap();
    merge_config(&higher, &mut merged).unwrap();

    assert_eq!(merged.sources.len(), 1);
    assert_eq!(merged.sources[0].0, "SELECTED");
    assert_eq!(merged.sources[0].1.url, "https://selected.example.test/v3/index.json");
    assert_eq!(merged.sources[0].1.protocol, NugetProtocol::V3);
    assert!(merged.disabled.is_empty());
    assert_eq!(merged.global_packages, Some(temp.0.join("higher-packages")));
  }

  #[test]
  fn nuget_storage_signature_and_proxy_policy_merge_as_typed_values() {
    let temp = TempDirectory::new();
    let lower = temp.write(
      "lower.config",
      r#"<configuration>
<fallbackPackageFolders><clear /><add key="shared" value="lower/shared" /><add key="legacy" value="lower/legacy" /><add key="stale" value="lower/stale" /></fallbackPackageFolders>
<config>
  <add key="globalPackagesFolder" value="lower/packages" />
  <add key="signatureValidationMode" value="require" />
  <add key="http_proxy" value="http://lower.proxy:8080" />
  <add key="http_proxy.user" value="lower-user" />
  <add key="http_proxy.password" value="lower-secret" />
  <add key="no_proxy" value="localhost" />
</config>
</configuration>"#,
    );
    let higher = temp.write(
      "higher.config",
      r#"<configuration>
<fallbackPackageFolders><remove key="stale" /><add key="SHARED" value="higher/shared" /><add key="final" value="higher/final" /></fallbackPackageFolders>
<config>
  <clear />
  <add key="globalPackagesFolder" value="higher/packages" />
  <add key="signatureValidationMode" value="unexpected" />
  <add key="http_proxy" value="http://higher.proxy:9090" />
  <add key="no_proxy" value="example.test,localhost" />
</config>
</configuration>"#,
    );
    let mut merged = NugetConfigMerge::default();

    merge_config(&lower, &mut merged).unwrap();
    assert_eq!(merged.signature_validation, Some(SignatureValidationMode::Require));
    merge_config(&higher, &mut merged).unwrap();

    assert_eq!(merged.global_packages, Some(temp.0.join("higher/packages")));
    assert_eq!(merged.signature_validation, Some(SignatureValidationMode::Accept));
    assert_eq!(merged.fallback_folders.len(), 3);
    assert_eq!(merged.fallback_folders[0].name, "SHARED");
    assert_eq!(merged.fallback_folders[0].path, temp.0.join("higher/shared"));
    assert_eq!(merged.fallback_folders[0].config_priority, 1);
    assert_eq!(merged.fallback_folders[1].name, "legacy");
    assert_eq!(merged.fallback_folders[1].config_priority, 0);
    assert_eq!(merged.fallback_folders[2].name, "final");
    assert_eq!(merged.fallback_folders[2].path, temp.0.join("higher/final"));
    assert_eq!(merged.fallback_folders[2].config_priority, 1);
    assert_eq!(
      ordered_fallback_paths(merged.fallback_folders)
        .iter()
        .map(|path| path.strip_prefix(&temp.0).unwrap())
        .collect::<Vec<_>>(),
      [Path::new("higher/shared"), Path::new("higher/final"), Path::new("lower/legacy")]
    );
    assert_eq!(merged.proxy_url.as_deref(), Some("http://higher.proxy:9090"));
    assert_eq!(merged.proxy_user, None);
    assert_eq!(merged.proxy_password, None);
    assert_eq!(merged.no_proxy.as_deref(), Some("example.test,localhost"));
  }

  #[test]
  fn command_line_sources_replace_config_and_keep_matching_mapping_identity() {
    let temp = TempDirectory::new();
    let config = temp.write(
      "selected.config",
      r#"<configuration>
<packageSources>
  <clear />
  <add key="selected" value="https://packages.example.test/v3/index.json" protocolVersion="3" />
  <add key="ignored" value="https://ignored.example.test/api/v2" protocolVersion="2" />
</packageSources>
<packageSourceMapping>
  <clear />
  <packageSource key="selected"><package pattern="Example.*" /></packageSource>
</packageSourceMapping>
</configuration>"#,
    );
    let overrides = vec![
      "https://packages.example.test/v3/index.json".to_owned(),
      "https://new.example.test/v3/index.json".to_owned(),
      "https://packages.example.test/v3/index.json".to_owned(),
    ];

    let discovered = discover_configuration(&temp.0, Some(&temp.0.join("packages")), Some(&config), &overrides).unwrap();

    assert_eq!(discovered.sources.len(), 2);
    assert_eq!(discovered.sources[0].0, "selected");
    assert_eq!(discovered.sources[0].1.protocol, NugetProtocol::V3);
    assert_eq!(discovered.sources[1].0, "https://new.example.test/v3/index.json");
    assert_eq!(discovered.sources[1].1.protocol, NugetProtocol::V3);
    let mapping = discovered.source_mapping.unwrap();
    assert!(mapping.allows(0, "Example.Package"));
    assert!(!mapping.allows(1, "Example.Package"));
  }

  #[test]
  fn fallback_package_roots_are_searched_after_the_global_cache() {
    let temp = TempDirectory::new();
    let global = temp.0.join("global");
    let first = temp.0.join("first");
    let second = temp.0.join("second");
    fs::create_dir_all(second.join("sample.package/1.2.3")).unwrap();
    fs::create_dir_all(first.join("sample.package/2.0.0")).unwrap();

    let found = find_package_root(&global, &[first.clone(), second.clone()], &request()).unwrap();
    let versions = enumerate_cached_versions(&global, &[first, second.clone()], "sample.package").unwrap();

    assert_eq!(found, second.join("sample.package/1.2.3"));
    assert_eq!(
      versions.iter().map(|version| version.normalized.as_str()).collect::<Vec<_>>(),
      ["1.2.3", "2.0.0"]
    );
  }

  #[test]
  #[cfg(windows)]
  fn encrypted_proxy_credentials_fail_instead_of_becoming_plaintext_basic_auth() {
    let merged = NugetConfigMerge {
      proxy_url: Some("http://proxy.example.test:8080".into()),
      proxy_user: Some("user".into()),
      proxy_password: Some("encrypted-value".into()),
      ..NugetConfigMerge::default()
    };

    let result = effective_proxy(&merged);
    if cfg!(windows) {
      let error = match result {
        Err(error) => error,
        Ok(_) => panic!("Windows encrypted proxy credentials must fail"),
      };
      assert_eq!(error.kind(), PackageErrorKind::Configuration);
      assert!(!error.to_string().contains("encrypted-value"));
    } else {
      let proxy = result.unwrap().unwrap();
      assert_eq!(proxy.url, "http://proxy.example.test:8080");
    }
  }

  #[test]
  fn required_signatures_fail_until_packages_are_actually_verified() {
    let error = validate_signature_policy(SignatureValidationMode::Require).unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::Configuration);
    assert!(error.to_string().contains("RES-015"));
    validate_signature_policy(SignatureValidationMode::Accept).unwrap();
  }

  #[test]
  fn enabled_audit_fails_until_advisories_are_actually_evaluated() {
    let temp = TempDirectory::new();
    let enabled_path = temp.write(
      "Enabled.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
    );
    let disabled_path = temp.write(
      "Disabled.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup></Project>"#,
    );
    let enabled = evaluate_project_path(&enabled_path, ProjectConfiguration::Debug).unwrap();
    let disabled = evaluate_project_path(&disabled_path, ProjectConfiguration::Debug).unwrap();

    let error = validate_audit_policy(&enabled).unwrap_err();
    assert_eq!(error.kind(), PackageErrorKind::Configuration);
    assert!(error.to_string().contains("RES-024"));
    validate_audit_policy(&disabled).unwrap();
  }

  #[test]
  fn nuget_environment_expansion_is_single_pass_and_preserves_unknown_values() {
    let path = Path::new("NuGet.Config");
    let expanded = expand_config_value_with("before/%ROOT%/%UNKNOWN%/%CHAIN%/after".into(), path, |name| match name {
      "ROOT" => Some(std::ffi::OsString::from("packages")),
      "CHAIN" => Some(std::ffi::OsString::from("%ROOT%")),
      _ => None,
    })
    .unwrap();

    assert_eq!(expanded, "before/packages/%UNKNOWN%/%ROOT%/after");
    let unchanged = String::from("100% unchanged");
    let original_allocation = unchanged.as_ptr();
    let unchanged = expand_config_value_with(unchanged, path, |_| None).unwrap();
    assert_eq!(unchanged, "100% unchanged");
    assert_eq!(unchanged.as_ptr(), original_allocation);
    assert_eq!(expand_config_value_with("%%".into(), path, |_| None).unwrap(), "%%");
  }

  #[test]
  fn nuget_config_discovery_orders_machine_user_and_drive_scopes() {
    let temp = TempDirectory::new();
    for relative in [
      "machine/10.config",
      "machine/20.Config",
      "user/config/10.config",
      "user/config/20.Config",
      "user/config/NuGet.Config",
      "user/NuGet.Config",
      "drive/NuGet.Config",
      "drive/repository/NuGet.Config",
      "drive/repository/src/NuGet.Config",
    ] {
      temp.write(relative, "<configuration />");
    }
    let project_directory = temp.0.join("drive/repository/src");
    let roots = NugetConfigRoots {
      machine_config_directory: Some(temp.0.join("machine")),
      user_settings_directory: Some(temp.0.join("user")),
    };

    let paths = discover_config_paths(&project_directory, None, &roots).unwrap();
    let relative = paths.iter().filter_map(|path| path.strip_prefix(&temp.0).ok()).collect::<Vec<_>>();

    assert_eq!(
      relative,
      [
        Path::new("machine/20.Config"),
        Path::new("machine/10.config"),
        Path::new("user/config/20.Config"),
        Path::new("user/config/10.config"),
        Path::new("user/NuGet.Config"),
        Path::new("drive/NuGet.Config"),
        Path::new("drive/repository/NuGet.Config"),
        Path::new("drive/repository/src/NuGet.Config"),
      ]
    );
  }

  #[test]
  fn explicit_nuget_config_is_the_only_discovered_file() {
    let temp = TempDirectory::new();
    temp.write("machine/machine.config", "<configuration />");
    temp.write("user/NuGet.Config", "<configuration />");
    temp.write("repository/NuGet.Config", "<configuration />");
    let explicit = temp.write("selected/custom.xml", "<configuration />");
    let roots = NugetConfigRoots {
      machine_config_directory: Some(temp.0.join("machine")),
      user_settings_directory: Some(temp.0.join("user")),
    };

    let selected = discover_config_paths(&temp.0.join("repository"), Some(&explicit), &roots).unwrap();
    assert_eq!(selected.as_slice(), std::slice::from_ref(&explicit));
    let no_sources = discover_configuration(&temp.0.join("repository"), Some(&temp.0.join("packages")), Some(&explicit), &[])
      .err()
      .unwrap();
    assert_eq!(no_sources.kind(), PackageErrorKind::Configuration);
    assert!(no_sources.to_string().contains("no enabled package source"));
    let missing = temp.0.join("missing.config");
    let error = discover_config_paths(&temp.0.join("repository"), Some(&missing), &roots).unwrap_err();
    assert_eq!(error.kind(), PackageErrorKind::Configuration);
    assert_eq!(error.context(), missing.display().to_string());
  }

  #[test]
  fn parses_exact_v2_atom_metadata_without_console_scraping() {
    let metadata = br#"<entry xmlns="http://www.w3.org/2005/Atom" xmlns:d="http://schemas.microsoft.com/ado/2007/08/dataservices">
<content type="application/zip" src="https://packages.example.test/api/v2/package/Sample.Package/1.2.3" />
<m:properties xmlns:m="http://schemas.microsoft.com/ado/2007/08/dataservices/metadata">
<d:Id>Sample.Package</d:Id><d:Version>1.2.3</d:Version>
<d:PackageHash>AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==</d:PackageHash>
<d:PackageHashAlgorithm>SHA512</d:PackageHashAlgorithm><d:PackageSize>42</d:PackageSize>
</m:properties></entry>"#;

    let parsed = parse_v2_package_metadata(&request(), "https://packages.example.test/api/v2/Packages(...)", metadata).unwrap();

    assert_eq!(parsed.content_url, "https://packages.example.test/api/v2/package/Sample.Package/1.2.3");
    assert_eq!(parsed.expected_size, Some(42));
    assert_eq!(parsed.requests, 1);
  }

  #[test]
  fn parses_v2_version_pages_and_their_continuation() {
    let page = br#"<feed xmlns="http://www.w3.org/2005/Atom" xmlns:d="http://schemas.microsoft.com/ado/2007/08/dataservices">
<entry><content><m:properties xmlns:m="http://schemas.microsoft.com/ado/2007/08/dataservices/metadata"><d:Version>1.2.0</d:Version></m:properties></content></entry>
<entry><content><m:properties xmlns:m="http://schemas.microsoft.com/ado/2007/08/dataservices/metadata"><d:Version>2.0.0-beta.1</d:Version></m:properties></content></entry>
<link rel="next" href="?page=2&amp;semVerLevel=2.0.0" /></feed>"#;

    let parsed = parse_v2_version_page("https://packages.example.test/api/v2/FindPackagesById()", page).unwrap();

    assert_eq!(
      parsed.versions.iter().map(|version| version.normalized.as_str()).collect::<Vec<_>>(),
      ["1.2.0", "2.0.0-beta.1"]
    );
    assert_eq!(parsed.next.as_deref(), Some("?page=2&semVerLevel=2.0.0"));
  }

  #[test]
  fn rejects_ambiguous_v2_version_continuations() {
    let page = br#"<feed><link rel="next" href="?page=2" /><link rel="next" href="?page=3" /></feed>"#;

    let error = parse_v2_version_page("https://packages.example.test/api/v2/FindPackagesById()", page)
      .err()
      .unwrap();

    assert_eq!(error.kind(), PackageErrorKind::Network);
    assert!(error.to_string().contains("multiple continuation links"));
  }

  #[test]
  fn exact_v3_package_uses_only_the_discovered_flat_container() {
    let service_index = serde_json::json!({
      "version": "3.0.0",
      "resources": [{
        "@id": "https://content.example.test/arbitrary/root",
        "@type": ["PackageBaseAddress/3.0.0", "Other/1.0.0"]
      }]
    });
    let services = parse_v3_service_index("https://feed.example.test/custom-index", &service_index).unwrap();
    let package_base = services.package_base_address().unwrap();

    let metadata = v3_package_metadata(&request(), package_base);

    assert_eq!(package_base, "https://content.example.test/arbitrary/root");
    assert_eq!(
      metadata.content_url,
      "https://content.example.test/arbitrary/root/sample.package/1.2.3/sample.package.1.2.3.nupkg"
    );
    assert_eq!(metadata.expected_hash, None);
    assert_eq!(metadata.expected_size, None);
    assert_eq!(metadata.requests, 0);
  }

  #[test]
  fn service_index_selects_official_type_order_and_compatible_client_version() {
    let document = serde_json::json!({
      "version": "3.1.0",
      "resources": [
        { "@id": "https://feed.test/registration-legacy/", "@type": "RegistrationsBaseUrl/3.6.0" },
        { "@id": "https://feed.test/registration-current-a/", "@type": "RegistrationsBaseUrl/Versioned", "clientVersion": ["4.3.0-alpha", "4.3.0"] },
        { "@id": "not an absolute URL", "@type": "RegistrationsBaseUrl/Versioned", "clientVersion": "6.0.0" },
        { "@id": "https://secret@feed.test/registration/", "@type": "RegistrationsBaseUrl/Versioned", "clientVersion": "5.0.0" },
        { "@id": "https://feed.test/registration-future/", "@type": "RegistrationsBaseUrl/Versioned", "clientVersion": "8.0.0" },
        { "@id": "https://feed.test/registration-current-b/", "@type": ["Other", "RegistrationsBaseUrl/Versioned"], "clientVersion": "4.3.0" },
        { "@id": "https://feed.test/content/", "@type": ["PackageBaseAddress/3.0.0", "Other"] },
        { "@id": "https://feed.test/search-a", "@type": "SearchQueryService/3.0.0-beta" },
        { "@id": "https://feed.test/search-b", "@type": "SearchQueryService/3.0.0-beta" },
        { "@id": "https://feed.test/vulnerabilities", "@type": "VulnerabilityInfo/6.7.0" },
        { "@id": "https://feed.test/publish", "@type": "PackagePublish/2.0.0" }
      ]
    });

    let services = parse_v3_service_index("https://feed.test/index.json", &document).unwrap();

    assert_eq!(services.package_base_address(), Some("https://feed.test/content/"));
    assert_eq!(
      services.values(ServiceCapability::Registration).collect::<Vec<_>>(),
      ["https://feed.test/registration-current-a/", "https://feed.test/registration-current-b/"]
    );
    assert_eq!(
      services.values(ServiceCapability::Search).collect::<Vec<_>>(),
      ["https://feed.test/search-a", "https://feed.test/search-b"]
    );
    assert_eq!(
      services.values(ServiceCapability::Vulnerability).collect::<Vec<_>>(),
      ["https://feed.test/vulnerabilities"]
    );
    assert_eq!(
      services.values(ServiceCapability::PackagePublish).collect::<Vec<_>>(),
      ["https://feed.test/publish"]
    );
  }

  #[test]
  fn service_index_rejects_unsupported_schema_and_insecure_selected_resources() {
    let schema = serde_json::json!({ "version": "4.0.0", "resources": [] });
    let error = parse_v3_service_index("https://feed.test/index.json", &schema).err().unwrap();
    assert_eq!(error.kind(), PackageErrorKind::Network);
    assert!(error.to_string().contains("expected major version 3"));

    let insecure = serde_json::json!({
      "version": "3.0.0",
      "resources": [{ "@id": "http://feed.test/content/", "@type": "PackageBaseAddress/3.0.0" }]
    });
    let error = parse_v3_service_index("https://feed.test/index.json", &insecure).err().unwrap();
    assert_eq!(error.kind(), PackageErrorKind::Network);
    assert!(error.to_string().contains("NUGET-012"));
  }

  #[test]
  fn staged_package_identity_must_match_before_publication() {
    let temp = TempDirectory::new();
    temp.write(
      "sample.package.nuspec",
      r#"<package><metadata><id>Different.Package</id><version>1.2.3</version></metadata></package>"#,
    );

    let error = validate_staged_nuspec_identity(&temp.0, &request()).unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::Integrity);
  }

  #[test]
  fn locked_package_validation_uses_the_completion_marker_and_lock_hash() {
    let temp = TempDirectory::new();
    temp.write(".dv.metadata.json", "{}");
    let hash = BASE64.encode([0u8; 64]);

    validate_locked_package(&temp.0, &request(), &hash).unwrap();

    let error = validate_locked_package(&temp.0, &request(), "not-base64").unwrap_err();
    assert_eq!(error.kind(), PackageErrorKind::Integrity);
    fs::remove_file(temp.0.join(".dv.metadata.json")).unwrap();
    let error = validate_locked_package(&temp.0, &request(), &hash).unwrap_err();
    assert_eq!(error.kind(), PackageErrorKind::Integrity);
  }

  #[test]
  fn nuspec_dependency_groups_follow_the_evaluated_target() {
    let manifest = br#"<package><metadata><id>Sample.Package</id><version>1.2.3</version><dependencies>
<group targetFramework="netstandard2.0"><dependency id="Base.Dependency" version="1.0" /></group>
<group targetFramework="net10.0"><dependency id="Current.Dependency" version="[2.0]" /></group>
</dependencies></metadata></package>"#;
    let path = Path::new("sample.package.nuspec");

    let net8 = parse_nuspec(path, manifest, &request(), TargetFramework::parse("net8.0").unwrap()).unwrap();
    let net10 = parse_nuspec(path, manifest, &request(), TargetFramework::parse("net10.0").unwrap()).unwrap();

    assert_eq!(net8[0].id, "Base.Dependency");
    assert_eq!(net8[0].version, "1.0.0");
    assert_eq!(net10[0].id, "Current.Dependency");
    assert_eq!(net10[0].version, "2.0.0");
  }

  #[test]
  fn nuspec_dependency_asset_filters_subtract_excludes_from_no_content_default() {
    let manifest = br#"<package><metadata><id>Sample.Package</id><version>1.2.3</version><dependencies>
<group targetFramework="net10.0"><dependency id="Child.Package" version="1.0" exclude="Build,Analyzers" /></group>
</dependencies></metadata></package>"#;

    let dependencies = parse_nuspec_requirements(
      Path::new("sample.package.nuspec"),
      manifest,
      &request(),
      TargetFramework::parse("net10.0").unwrap(),
    )
    .unwrap();

    assert_eq!(dependencies.len(), 1);
    assert!(dependencies[0].include_assets.contains(AssetFlags::RUNTIME));
    assert!(dependencies[0].include_assets.contains(AssetFlags::COMPILE));
    assert!(dependencies[0].include_assets.contains(AssetFlags::BUILD_TRANSITIVE));
    assert!(!dependencies[0].include_assets.contains(AssetFlags::BUILD));
    assert!(!dependencies[0].include_assets.contains(AssetFlags::ANALYZERS));
    assert!(!dependencies[0].include_assets.contains(AssetFlags::CONTENT_FILES));
    assert_eq!(dependencies[0].suppress_parent, AssetFlags::NONE);
  }

  #[test]
  fn package_asset_selection_materializes_every_portable_family() {
    let temp = TempDirectory::new();
    temp.write("ref/net10.0/Sample.Package.dll", []);
    temp.write("lib/net10.0/Sample.Package.dll", []);
    temp.write("lib/net10.0/de/Sample.Package.resources.dll", []);
    temp.write("contentFiles/any/any/readme.txt", []);
    temp.write("build/net10.0/Sample.Package.props", []);
    temp.write("build/net10.0/Unrelated.props", []);
    temp.write("buildTransitive/net10.0/Sample.Package.targets", []);
    temp.write("buildMultiTargeting/Sample.Package.props", []);
    temp.write("analyzers/dotnet/cs/Generator.dll", []);
    temp.write("analyzers/dotnet/cs/de/Generator.resources.dll", []);
    temp.write("analyzers/dotnet/vb/VisualBasic.dll", []);
    temp.write("runtimes/win/lib/net10.0/Sample.Package.dll", []);
    temp.write("runtimes/linux-x64/native/libsample.so", []);
    let cached = CachedPackage {
      root: temp.0.clone(),
      hash: BASE64.encode([0u8; 64]),
      dependencies: None,
      cache_hit: true,
      requests: 0,
      bytes: 0,
      origin: None,
    };

    let package = parse_cached_package(
      request(),
      cached,
      TargetFramework::parse("net10.0").unwrap(),
      "net10.0",
      Vec::new(),
      AssetFlags::ALL,
    )
    .unwrap();

    assert_eq!(package.compile_assets, [temp.0.join("ref/net10.0/Sample.Package.dll")]);
    assert_eq!(package.runtime_assets, [temp.0.join("lib/net10.0/Sample.Package.dll")]);
    assert_eq!(package.resource_assets, [temp.0.join("lib/net10.0/de/Sample.Package.resources.dll")]);
    assert_eq!(package.content_files, [temp.0.join("contentFiles/any/any/readme.txt")]);
    assert_eq!(package.build_assets, [temp.0.join("build/net10.0/Sample.Package.props")]);
    assert_eq!(package.build_transitive_assets, [temp.0.join("buildTransitive/net10.0/Sample.Package.targets")]);
    assert_eq!(package.build_multi_targeting_assets, [temp.0.join("buildMultiTargeting/Sample.Package.props")]);
    assert_eq!(package.analyzers, [temp.0.join("analyzers/dotnet/cs/Generator.dll")]);
    assert_eq!(package.runtime_targets.len(), 2);
    assert!(
      package
        .runtime_targets
        .iter()
        .any(|asset| asset.kind == RuntimeTargetKind::Runtime && asset.runtime_identifier == "win")
    );
    assert!(
      package
        .runtime_targets
        .iter()
        .any(|asset| asset.kind == RuntimeTargetKind::Native && asset.runtime_identifier == "linux-x64")
    );
  }

  #[test]
  fn dependency_only_meta_package_is_a_valid_graph_node() {
    let temp = TempDirectory::new();
    temp.write(
      "sample.package.nuspec",
      r#"<package><metadata><id>Sample.Package</id><version>1.2.3</version><dependencies>
<group targetFramework="netstandard2.0"><dependency id="Base.Dependency" version="[1.0]" /></group>
</dependencies></metadata></package>"#,
    );
    let cached = CachedPackage {
      root: temp.0.clone(),
      hash: BASE64.encode([0u8; 64]),
      dependencies: None,
      cache_hit: true,
      requests: 0,
      bytes: 0,
      origin: None,
    };

    let package = parse_cached_package(
      request(),
      cached,
      TargetFramework::parse("net10.0").unwrap(),
      "net10.0",
      vec![PackageRequest {
        id: "Base.Dependency".into(),
        lower_id: "base.dependency".into(),
        version: "1.0.0".into(),
        direct: false,
      }],
      AssetFlags::ALL,
    )
    .unwrap();

    assert_eq!(package.dependencies.len(), 1);
    assert!(package.compile_assets.is_empty());
    assert!(package.runtime_assets.is_empty());
  }

  #[test]
  fn streaming_scheduler_enqueues_a_dependency_as_soon_as_its_parent_is_parsed() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Meta.Package" Version="1.0.0" /></ItemGroup></Project>"#,
    );
    for (id, version, nuspec) in [
      (
        "meta.package",
        "1.0.0",
        r#"<package><metadata><id>Meta.Package</id><version>1.0.0</version><dependencies>
<group targetFramework="netstandard2.0"><dependency id="Child.Package" version="[2.0]" /></group>
</dependencies></metadata></package>"#,
      ),
      (
        "child.package",
        "2.0.0",
        r#"<package><metadata><id>Child.Package</id><version>2.0.0</version></metadata></package>"#,
      ),
    ] {
      let root = format!("packages/{id}/{version}");
      temp.write(&format!("{root}/{id}.nuspec"), nuspec);
      temp.write(&format!("{root}/{id}.{version}.nupkg"), []);
      temp.write(&format!("{root}/{id}.{version}.nupkg.sha512"), BASE64.encode([0u8; 64]));
      temp.write(&format!("{root}/.dv.metadata.json"), "{}");
    }
    temp.write("packages/child.package/2.0.0/lib/net6.0/Child.Package.dll", []);
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      config_file: None,
      sources: Vec::new(),
      offline: true,
      write_lock: true,
    };

    let resolution = resolve_package_inputs(&[&project], &options).unwrap().remove(0);
    let identities = resolution
      .packages()
      .iter()
      .copied()
      .map(|package| resolution.package_id(package))
      .collect::<Vec<_>>();

    assert_eq!(identities, ["Child.Package", "Meta.Package"]);
    assert_eq!(resolution.cache_hits(), 2);
    assert_eq!(resolution.network_requests(), 0);
  }

  #[test]
  fn local_flat_and_hierarchical_sources_resolve_ranges_without_http() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Flat.Package" Version="[1.0,2.0]" /><PackageReference Include="Tree.Package" Version="2.0.0" /></ItemGroup></Project>"#,
    );
    // Flat feeds trust the archive nuspec, not a potentially stale filename.
    write_test_package(&temp, "flat/Flat.Package.9.9.9.nupkg", "Flat.Package", "1.2.3");
    let tree_archive = write_test_package(&temp, "hierarchical/tree.package/2.0.0/tree.package.2.0.0.nupkg", "Tree.Package", "2.0.0");
    temp.write(
      "hierarchical/tree.package/2.0.0/tree.package.nuspec",
      r#"<package><metadata><id>Tree.Package</id><version>2.0.0</version></metadata></package>"#,
    );
    let tree_hash = BASE64.encode(Sha512::digest(fs::read(tree_archive).unwrap()));
    temp.write("hierarchical/tree.package/2.0.0/tree.package.2.0.0.nupkg.sha512", tree_hash);
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      config_file: None,
      sources: vec![
        temp.0.join("flat").to_string_lossy().into_owned(),
        temp.0.join("hierarchical").to_string_lossy().into_owned(),
      ],
      offline: true,
      write_lock: true,
    };

    let resolution = resolve_package_inputs(&[&project], &options).unwrap().remove(0);
    let packages = resolution
      .packages()
      .iter()
      .copied()
      .map(|package| (resolution.package_id(package), resolution.package_version(package)))
      .collect::<Vec<_>>();

    assert_eq!(packages, [("Flat.Package", "1.2.3"), ("Tree.Package", "2.0.0")]);
    assert_eq!(resolution.source_protocol(), "local");
    assert_eq!(resolution.network_requests(), 0);
    assert_eq!(resolution.downloaded_packages(), 2);
    assert_eq!(resolution.compile_assets().count(), 2);
  }

  #[test]
  fn hierarchical_local_source_rejects_a_mismatched_hash() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Tree.Package" Version="2.0.0" /></ItemGroup></Project>"#,
    );
    write_test_package(&temp, "feed/tree.package/2.0.0/tree.package.2.0.0.nupkg", "Tree.Package", "2.0.0");
    temp.write(
      "feed/tree.package/2.0.0/tree.package.nuspec",
      r#"<package><metadata><id>Tree.Package</id><version>2.0.0</version></metadata></package>"#,
    );
    temp.write("feed/tree.package/2.0.0/tree.package.2.0.0.nupkg.sha512", BASE64.encode([0u8; 64]));
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      config_file: None,
      sources: vec![temp.0.join("feed").to_string_lossy().into_owned()],
      offline: true,
      write_lock: true,
    };

    let error = resolve_package_inputs(&[&project], &options).unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::Integrity);
    assert!(error.to_string().contains("does not match source metadata"));
  }

  #[test]
  fn direct_project_ranges_select_the_lowest_available_version_offline() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Range.Package" Version="(1.0,3.0)" /></ItemGroup></Project>"#,
    );
    for version in ["1.0.0", "2.0.0"] {
      let root = format!("packages/range.package/{version}");
      temp.write(
        &format!("{root}/range.package.nuspec"),
        format!(r#"<package><metadata><id>Range.Package</id><version>{version}</version></metadata></package>"#),
      );
      temp.write(&format!("{root}/range.package.{version}.nupkg"), []);
      temp.write(&format!("{root}/range.package.{version}.nupkg.sha512"), BASE64.encode([0u8; 64]));
      temp.write(&format!("{root}/.dv.metadata.json"), "{}");
      temp.write(&format!("{root}/lib/net10.0/Range.Package.dll"), []);
    }
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      config_file: None,
      sources: Vec::new(),
      offline: true,
      write_lock: false,
    };

    let resolution = resolve_package_inputs(&[&project], &options).unwrap().remove(0);

    assert_eq!(resolution.packages().len(), 1);
    assert_eq!(resolution.package_version(resolution.packages()[0]), "2.0.0");
    assert_eq!(resolution.network_requests(), 0);
  }

  #[test]
  fn warm_cache_and_lock_select_assets_for_the_evaluated_target_without_http() {
    let temp = TempDirectory::new();
    temp.write(
      "NuGet.Config",
      r#"<configuration><packageSources><clear /><add key="legacy" value="https://packages.example.test/api/v2/" protocolVersion="2" /></packageSources></configuration>"#,
    );
    temp.write("Program.cs", "");
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net8.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Sample.Package" Version="1.2.3" /></ItemGroup></Project>"#,
    );
    let cache = temp.0.join("packages");
    let root = cache.join("sample.package/1.2.3");
    temp.write(
      "packages/sample.package/1.2.3/sample.package.nuspec",
      r#"<package><metadata><id>Sample.Package</id><version>1.2.3</version></metadata></package>"#,
    );
    temp.write("packages/sample.package/1.2.3/sample.package.1.2.3.nupkg", []);
    temp.write("packages/sample.package/1.2.3/sample.package.1.2.3.nupkg.sha512", BASE64.encode([0u8; 64]));
    temp.write("packages/sample.package/1.2.3/.dv.metadata.json", "{}");
    temp.write("packages/sample.package/1.2.3/lib/net6.0/Sample.Package.dll", []);
    temp.write("packages/sample.package/1.2.3/lib/net6.0/de/Sample.Package.resources.dll", []);
    temp.write("packages/sample.package/1.2.3/lib/net10.0/Sample.Package.dll", []);
    temp.write("packages/sample.package/1.2.3/contentFiles/any/any/readme.txt", []);
    temp.write("packages/sample.package/1.2.3/build/net6.0/Sample.Package.props", []);
    temp.write("packages/sample.package/1.2.3/buildMultiTargeting/Sample.Package.props", []);
    temp.write("packages/sample.package/1.2.3/buildTransitive/net6.0/Sample.Package.targets", []);
    temp.write("packages/sample.package/1.2.3/analyzers/dotnet/cs/Sample.Analyzer.dll", []);
    temp.write("packages/sample.package/1.2.3/native/sample.native", []);
    temp.write("packages/sample.package/1.2.3/runtimes/win/lib/net6.0/Sample.Package.dll", []);
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(cache),
      config_file: None,
      sources: Vec::new(),
      offline: true,
      write_lock: true,
    };

    let first = resolve_package_inputs(&[&project], &options).unwrap().remove(0);
    let second = resolve_package_inputs(&[&project], &options).unwrap().remove(0);

    assert_eq!(first.target_framework(), "net8.0");
    assert_eq!(first.source_protocol(), "v2");
    assert_eq!(first.network_requests(), 0);
    assert_eq!(second.network_requests(), 0);
    assert_eq!(second.cache_hits(), 1);
    assert_eq!(second.compile_assets().collect::<Vec<_>>(), [root.join("lib/net6.0/Sample.Package.dll")]);
    assert_eq!(
      second.assets(PackageAssetFamily::Compile).collect::<Vec<_>>(),
      second.compile_assets().collect::<Vec<_>>()
    );
    assert_eq!(second.runtime_assets().collect::<Vec<_>>(), [root.join("lib/net6.0/Sample.Package.dll")]);
    assert_eq!(second.analyzers().collect::<Vec<_>>(), [root.join("analyzers/dotnet/cs/Sample.Analyzer.dll")]);
    assert_eq!(
      second.resource_assets().collect::<Vec<_>>(),
      [root.join("lib/net6.0/de/Sample.Package.resources.dll")]
    );
    assert_eq!(second.content_files().collect::<Vec<_>>(), [root.join("contentFiles/any/any/readme.txt")]);
    assert_eq!(second.build_assets().collect::<Vec<_>>(), [root.join("build/net6.0/Sample.Package.props")]);
    assert_eq!(
      second.build_multi_targeting_assets().collect::<Vec<_>>(),
      [root.join("buildMultiTargeting/Sample.Package.props")]
    );
    assert_eq!(
      second.build_transitive_assets().collect::<Vec<_>>(),
      [root.join("buildTransitive/net6.0/Sample.Package.targets")]
    );
    assert_eq!(second.native_assets().collect::<Vec<_>>(), [root.join("native/sample.native")]);
    let ranges = second.asset_ranges;
    assert_eq!(ranges.compile.start, 0);
    for (left, right) in [
      (ranges.compile, ranges.runtime),
      (ranges.runtime, ranges.analyzers),
      (ranges.analyzers, ranges.resources),
      (ranges.resources, ranges.content),
      (ranges.content, ranges.build),
      (ranges.build, ranges.build_multi_targeting),
      (ranges.build_multi_targeting, ranges.build_transitive),
      (ranges.build_transitive, ranges.native),
    ] {
      assert_eq!(left.start + left.len, right.start);
    }
    assert_eq!(ranges.native.start + ranges.native.len, u32::try_from(second.assets.len()).unwrap());
    let runtime_target = root.join("runtimes/win/lib/net6.0/Sample.Package.dll");
    assert_eq!(
      second.runtime_targets().collect::<Vec<_>>(),
      [(runtime_target.as_path(), "win", RuntimeTargetKind::Runtime)]
    );
  }

  #[test]
  fn archive_paths_cannot_escape_the_staging_directory() {
    let temp = TempDirectory::new();
    let archive_path = temp.0.join("malicious.nupkg");
    let file = fs::File::create(&archive_path).unwrap();
    let mut archive = ZipWriter::new(file);
    archive.start_file("../escape.dll", SimpleFileOptions::default()).unwrap();
    archive.write_all(b"not allowed").unwrap();
    archive.finish().unwrap();

    let error = validate_and_extract_archive(&archive_path, &temp.0.join("staging"), false).unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::Archive);
    assert!(!temp.0.join("escape.dll").exists());
  }

  #[test]
  fn package_publish_reuses_an_existing_atomic_winner() {
    let temp = TempDirectory::new();
    let staged = temp.0.join("staged");
    let destination = temp.0.join("destination");
    fs::create_dir(&staged).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("winner"), []).unwrap();

    assert!(!publish_package_directory(&staged, &destination).unwrap());
    assert!(staged.is_dir());
    assert!(destination.is_dir());
  }

  #[test]
  fn parallel_archive_extraction_preserves_every_entry() {
    let temp = TempDirectory::new();
    let archive_path = temp.0.join("parallel.nupkg");
    let file = fs::File::create(&archive_path).unwrap();
    let mut archive = ZipWriter::new(file);
    for index in 0..MIN_PARALLEL_EXTRACTION_ENTRIES {
      archive
        .start_file(format!("lib/net10.0/asset-{index}.dll"), SimpleFileOptions::default())
        .unwrap();
      archive.write_all(format!("asset {index}").as_bytes()).unwrap();
    }
    archive.finish().unwrap();
    let destination = temp.0.join("staging");

    validate_and_extract_archive(&archive_path, &destination, true).unwrap();

    for index in 0..MIN_PARALLEL_EXTRACTION_ENTRIES {
      assert_eq!(
        fs::read_to_string(destination.join(format!("lib/net10.0/asset-{index}.dll"))).unwrap(),
        format!("asset {index}")
      );
    }
  }
}
