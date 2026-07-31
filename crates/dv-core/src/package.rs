use std::{
  cmp::Ordering,
  collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
  env,
  error::Error,
  fmt::{self, Write as _},
  fs,
  io::{self, Write},
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

use crate::{FrameworkFamily, ProjectSpec, TargetFramework, discover_sdks};

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
  /// Reject every operation that would require an HTTP request.
  pub offline: bool,
  /// Write or refresh `dv.lock.json` after successful resolution.
  pub write_lock: bool,
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
  lock_path: TextSpan,
  target_framework: TextSpan,
  source: TextSpan,
  prune_fingerprint: TextSpan,
  source_protocol: NugetProtocol,
  packages: Box<[ResolvedPackage]>,
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

const _: () = assert!(size_of::<PackageResolution>() == 248);
const _: () = assert!(align_of::<PackageResolution>() == align_of::<usize>());

impl PackageResolution {
  /// Returns the global-packages directory used by this graph.
  pub fn cache_root(&self) -> &Path {
    Path::new(self.get(self.cache_root))
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
  lock_path: &'a Path,
  target_framework: &'a str,
  source: &'a str,
  prune_fingerprint: &'a str,
  source_protocol: NugetProtocol,
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
  V2,
  V3,
}

impl NugetProtocol {
  fn parse(value: Option<&str>, source: &str, context: &Path) -> Result<Self, PackageError> {
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

#[derive(Clone)]
enum ServiceEndpoint {
  V2 { source: String, base: String },
  V3 { source: String, package_base: String },
}

impl ServiceEndpoint {
  fn source(&self) -> &str {
    match self {
      Self::V2 { source, .. } | Self::V3 { source, .. } => source,
    }
  }

  const fn protocol(&self) -> NugetProtocol {
    match self {
      Self::V2 { .. } => NugetProtocol::V2,
      Self::V3 { .. } => NugetProtocol::V3,
    }
  }
}

struct NugetConfiguration {
  cache_root: PathBuf,
  sources: Vec<PackageSource>,
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

fn resolve_project(project: &ProjectSpec, options: &PackageResolveOptions) -> Result<PackageResolution, PackageError> {
  let config = discover_configuration(
    project.project_directory(),
    options.packages_directory.as_deref(),
    options.config_file.as_deref(),
  )?;
  let lock_path = project.project_directory().join("dv.lock.json");
  let direct = direct_requests(project)?;
  let target = project.target();
  let target_text = project.target_framework();
  let pruning = discover_package_pruning(project.project_directory(), target)?;
  if let Some(resolution) = read_warm_lock(&lock_path, &config, &direct, target_text, &pruning.fingerprint)? {
    return Ok(resolution);
  }

  let client = http_client()?;
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
        |source| (source.url.clone(), source.protocol),
      )
    },
    |source| (source.url.clone(), source.protocol),
  );
  let resolution = materialize_resolution(
    ResolutionContext {
      cache_root: &config.cache_root,
      lock_path: &lock_path,
      target_framework: target_text,
      source: &source,
      prune_fingerprint: &pruning.fingerprint,
      source_protocol,
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

fn discover_configuration(project_directory: &Path, explicit_cache: Option<&Path>, explicit_config: Option<&Path>) -> Result<NugetConfiguration, PackageError> {
  let config_paths = discover_config_paths(project_directory, explicit_config, &NugetConfigRoots::from_environment())?;

  let mut sources = if config_paths.is_empty() {
    vec![(
      "nuget.org".to_owned(),
      PackageSource {
        url: DEFAULT_SOURCE.to_owned(),
        protocol: NugetProtocol::V3,
      },
    )]
  } else {
    Vec::new()
  };
  let mut disabled = BTreeSet::new();
  let mut configured_cache = None;
  for path in config_paths {
    merge_config(&path, &mut sources, &mut disabled, &mut configured_cache)?;
  }
  for key in disabled {
    sources.retain(|(name, _)| !name.eq_ignore_ascii_case(&key));
  }
  let sources: Vec<PackageSource> = sources.into_iter().map(|(_, source)| source).collect();
  if sources.is_empty() {
    return Err(PackageError::new(
      PackageErrorKind::Configuration,
      project_directory.display().to_string(),
      "NuGet configuration contains no enabled package source",
    ));
  }
  let cache_root = explicit_cache
    .map(Path::to_owned)
    .or_else(|| env::var_os("NUGET_PACKAGES").map(PathBuf::from))
    .or(configured_cache)
    .or_else(default_global_packages)
    .ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::Configuration,
        project_directory.display().to_string(),
        "could not determine the global package cache; set NUGET_PACKAGES",
      )
    })?;
  Ok(NugetConfiguration { cache_root, sources })
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
  if let Some(cache) = nonempty_env("NUGET_PACKAGES") {
    return Ok(cache);
  }
  discover_configuration(project_directory, None, None).map(|configuration| configuration.cache_root)
}

fn merge_config(
  path: &Path,
  sources: &mut Vec<(String, PackageSource)>,
  disabled: &mut BTreeSet<String>,
  global_packages: &mut Option<PathBuf>,
) -> Result<(), PackageError> {
  let bytes = fs::read(path).map_err(|error| package_io("read NuGet configuration", path, error))?;
  let mut reader = Reader::from_reader(bytes.as_slice());
  reader.config_mut().trim_text(true);
  let mut section = ConfigSection::Other;
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => {
        section = match local_name(element.name().as_ref()) {
          b"packageSources" => ConfigSection::Sources,
          b"disabledPackageSources" => ConfigSection::Disabled,
          b"config" => ConfigSection::Config,
          _ => section,
        };
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"clear" => match section {
        ConfigSection::Sources => sources.clear(),
        ConfigSection::Disabled => disabled.clear(),
        ConfigSection::Other | ConfigSection::Config => {},
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"add" => {
        let key = config_attribute(&reader, &element, b"key", path)?.ok_or_else(|| config_error(path, "NuGet add element requires key"))?;
        let value = config_attribute(&reader, &element, b"value", path)?.ok_or_else(|| config_error(path, "NuGet add element requires value"))?;
        if value.contains('%') || value.contains("$(") {
          return Err(config_error(path, "environment expansion in NuGet.Config is not supported yet"));
        }
        match section {
          ConfigSection::Sources => {
            let protocol = config_attribute(&reader, &element, b"protocolVersion", path)?;
            let source = PackageSource {
              protocol: NugetProtocol::parse(protocol.as_deref(), &value, path)?,
              url: value,
            };
            if let Some((_, existing)) = sources.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(&key)) {
              *existing = source;
            } else {
              sources.push((key, source));
            }
          },
          ConfigSection::Disabled => {
            if value.eq_ignore_ascii_case("true") {
              disabled.insert(key);
            } else if value.eq_ignore_ascii_case("false") {
              disabled.retain(|name| !name.eq_ignore_ascii_case(&key));
            } else {
              return Err(config_error(path, "disabled package-source values must be true or false"));
            }
          },
          ConfigSection::Config if key.eq_ignore_ascii_case("globalPackagesFolder") => {
            let candidate = PathBuf::from(value);
            *global_packages = Some(if candidate.is_absolute() {
              candidate
            } else {
              path.parent().unwrap_or(Path::new(".")).join(candidate)
            });
          },
          ConfigSection::Other | ConfigSection::Config => {},
        }
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"remove" && matches!(section, ConfigSection::Sources) => {
        let key = config_attribute(&reader, &element, b"key", path)?.ok_or_else(|| config_error(path, "NuGet remove element requires key"))?;
        sources.retain(|(name, _)| !name.eq_ignore_ascii_case(&key));
      },
      Ok(Event::End(element)) if matches!(local_name(element.name().as_ref()), b"packageSources" | b"disabledPackageSources" | b"config") => {
        section = ConfigSection::Other;
      },
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(config_error(path, format!("invalid NuGet configuration XML: {error}"))),
    }
  }
  Ok(())
}

#[derive(Clone, Copy)]
enum ConfigSection {
  Other,
  Sources,
  Disabled,
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

fn config_error(path: &Path, message: impl Into<String>) -> PackageError {
  PackageError::new(PackageErrorKind::Configuration, path.display().to_string(), message)
}

fn http_client() -> Result<reqwest::Client, PackageError> {
  reqwest::Client::builder()
    .https_only(true)
    .timeout(Duration::from_secs(60))
    .build()
    .map_err(|error| network_error("HTTP client", format!("failed to create HTTP client: {error}")))
}

async fn discover_service_endpoints(client: &reqwest::Client, sources: &[PackageSource]) -> Result<(Vec<ServiceEndpoint>, u32), PackageError> {
  let mut endpoints = Vec::with_capacity(sources.len());
  let mut requests = 0;
  for source in sources {
    if !source.url.starts_with("https://") {
      return Err(PackageError::new(
        PackageErrorKind::Configuration,
        &source.url,
        format!("package resolution supports HTTPS NuGet v2 and v3 sources; {:?} is unsupported", source.url),
      ));
    }
    match source.protocol {
      NugetProtocol::V2 => endpoints.push(ServiceEndpoint::V2 {
        source: source.url.clone(),
        base: with_trailing_slash(source.url.clone()),
      }),
      NugetProtocol::V3 => {
        let document: serde_json::Value = get_json(client, &source.url).await?;
        requests += 1;
        endpoints.push(ServiceEndpoint::V3 {
          source: source.url.clone(),
          package_base: package_base_from_service_index(&source.url, &document)?,
        });
      },
    }
  }
  Ok((endpoints, requests))
}

fn package_base_from_service_index(source: &str, document: &serde_json::Value) -> Result<String, PackageError> {
  let resources = document
    .get("resources")
    .and_then(serde_json::Value::as_array)
    .ok_or_else(|| network_error(source, "NuGet service index has no resources array"))?;
  let base = resources
    .iter()
    .find_map(|resource| {
      resource_type_matches(resource.get("@type"), "PackageBaseAddress/3.0.0")
        .then(|| resource.get("@id").and_then(serde_json::Value::as_str))
        .flatten()
    })
    .ok_or_else(|| network_error(source, "NuGet source has no PackageBaseAddress/3.0.0 resource"))?;
  if !base.starts_with("https://") {
    return Err(network_error(base, "NuGet PackageBaseAddress must use HTTPS"));
  }
  Ok(with_trailing_slash(base.to_owned()))
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
      let cache_miss = request.as_ref().is_none_or(|request| !package_root(&config.cache_root, request).exists());
      if cache_miss && !options.offline && endpoints.is_none() {
        if !config.cache_root.is_dir() {
          fs::create_dir_all(&config.cache_root).map_err(|error| package_io("create package cache", &config.cache_root, error))?;
        }
        let (discovered, requests) = discover_service_endpoints(client, &config.sources).await?;
        network_requests += requests;
        endpoints = Some(discovered.into());
      }

      let task_client = client.clone();
      let task_cache_root = config.cache_root.clone();
      let task_endpoints = endpoints.clone().unwrap_or_else(|| Arc::from([]));
      let generation = node.generation;
      let task_version = request.as_ref().map(|request| request.version.clone());
      let task_target = target;
      in_flight.insert(lower_id.clone());
      tasks.spawn(async move {
        let result = load_node_metadata(&task_client, request.as_ref(), &lower_id, &task_cache_root, &task_endpoints, task_target).await;
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
      let task_endpoints = endpoints.clone().unwrap_or_else(|| Arc::from([]));
      let parallel_extract = acquisition_tasks.is_empty() && acquisition.is_empty();
      acquisition_tasks.spawn(async move {
        let result = ensure_package(&task_client, &request, &task_cache_root, &task_endpoints, target, parallel_extract).await;
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

async fn load_node_metadata(
  client: &reqwest::Client,
  request: Option<&PackageRequest>,
  lower_id: &str,
  cache_root: &Path,
  endpoints: &[ServiceEndpoint],
  target: TargetFramework,
) -> Result<MetadataTaskResult, PackageError> {
  if request.is_none() && endpoints.is_empty() {
    let cached_versions = enumerate_cached_versions(cache_root, lower_id)?;
    if !cached_versions.is_empty() {
      return Ok(MetadataTaskResult::Versions {
        versions: cached_versions,
        requests: 0,
        bytes: 0,
      });
    }
  }
  if let Some(request) = request {
    let root = package_root(cache_root, request);
    if root.exists() {
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
      let cached_versions = enumerate_cached_versions(cache_root, lower_id)?;
      if !cached_versions.is_empty() {
        return Ok(MetadataTaskResult::Versions {
          versions: cached_versions,
          requests: 0,
          bytes: 0,
        });
      }
    }

    match ensure_package(client, request, cache_root, endpoints, target, false).await {
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

  let mut versions = Vec::new();
  let mut requests = 0;
  let mut bytes = 0;
  for endpoint in endpoints {
    if let ServiceEndpoint::V3 { package_base, .. } = endpoint {
      let url = format!("{package_base}{lower_id}/index.json");
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

fn read_cached_requirements(root: &Path, request: &PackageRequest, target: TargetFramework) -> Result<Vec<PackageRequirement>, PackageError> {
  let nuspec_path = find_nuspec(root)?;
  let nuspec = fs::read(&nuspec_path).map_err(|error| package_io("read package manifest", &nuspec_path, error))?;
  parse_nuspec_requirements(&nuspec_path, &nuspec, request, target)
}

fn enumerate_cached_versions(cache_root: &Path, lower_id: &str) -> Result<Vec<PackageVersion>, PackageError> {
  let identity_root = cache_root.join(lower_id);
  let entries = match fs::read_dir(&identity_root) {
    Ok(entries) => entries,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(error) => return Err(package_io("enumerate cached package versions", &identity_root, error)),
  };
  let mut versions = Vec::new();
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
  versions.sort_unstable();
  versions.dedup();
  Ok(versions)
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
  cache_root: &Path,
  endpoints: &[ServiceEndpoint],
  target: TargetFramework,
  parallel_extract: bool,
) -> Result<CachedPackage, PackageError> {
  let root = package_root(cache_root, request);
  if root.exists() {
    let request = request.clone();
    return tokio::task::spawn_blocking(move || validate_cached_package(&root, &request, true, 0, 0))
      .await
      .map_err(package_blocking_task_error)?;
  }
  let mut last_error = None;
  for endpoint in endpoints {
    match download_and_publish(client, request, cache_root, endpoint, target, parallel_extract).await {
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
  endpoint: &ServiceEndpoint,
  target: TargetFramework,
  parallel_extract: bool,
) -> Result<CachedPackage, PackageError> {
  let metadata = match endpoint {
    ServiceEndpoint::V2 { base, .. } => v2_package_metadata(client, request, base).await?,
    ServiceEndpoint::V3 { package_base, .. } => v3_package_metadata(request, package_base),
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

  let temp_root = unique_temp_root(cache_root, request);
  tokio::fs::create_dir(&temp_root)
    .await
    .map_err(|error| package_io("create package staging directory", &temp_root, error))?;
  let guard = TempGuard(Some(temp_root.clone()));
  let nupkg_name = format!("{}.{}.nupkg", request.lower_id, request.version);
  let nupkg_path = temp_root.join(&nupkg_name);
  let (hash, bytes) = download_package(client, &metadata.content_url, &nupkg_path).await?;
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
  let downloaded = DownloadedPackage {
    request: request.clone(),
    cache_root: cache_root.to_owned(),
    endpoint: endpoint.clone(),
    temp_root,
    nupkg_name,
    nupkg_path,
    hash,
    bytes,
    requests: metadata.requests + 1,
    target,
    parallel_extract,
  };
  tokio::task::spawn_blocking(move || finish_download_and_publish(downloaded, guard))
    .await
    .map_err(package_blocking_task_error)?
}

fn finish_download_and_publish(downloaded: DownloadedPackage, mut guard: TempGuard) -> Result<CachedPackage, PackageError> {
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
    guard.0 = None;
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
  PackageMetadata {
    content_url: format!(
      "{package_base}{}/{}/{}.{}.nupkg",
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

async fn get_json<T: for<'de> Deserialize<'de>>(client: &reqwest::Client, url: &str) -> Result<T, PackageError> {
  let bytes = get_bytes(client, url, MAX_JSON_BYTES, "JSON").await?;
  serde_json::from_slice(&bytes).map_err(|error| network_error(url, format!("invalid JSON response: {error}")))
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
    + context.lock_path.as_os_str().len()
    + context.target_framework.len()
    + context.source.len()
    + context.prune_fingerprint.len();
  let mut table = TextTable::with_capacity(estimated);
  let cache_root_span = table.push_path(context.cache_root)?;
  let lock_path_span = table.push_path(context.lock_path)?;
  let target_framework_span = table.push(context.target_framework)?;
  let source_span = table.push(context.source)?;
  let prune_fingerprint_span = table.push(context.prune_fingerprint)?;
  let mut packages = Vec::with_capacity(work.len());
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
    lock_path: lock_path_span,
    target_framework: target_framework_span,
    source: source_span,
    prune_fingerprint: prune_fingerprint_span,
    source_protocol: context.source_protocol,
    packages: packages.into_boxed_slice(),
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
    lock_path: lock,
    target_framework,
    source: empty,
    prune_fingerprint: empty,
    source_protocol: NugetProtocol::V3,
    packages: Box::new([]),
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
  target_text: &str,
  prune_fingerprint: &str,
) -> Result<Option<PackageResolution>, PackageError> {
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
      .any(|source| source.url == lock.source && source.protocol == lock.source_protocol)
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
    let root = package_root(&config.cache_root, &request);
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
      lock_path: path,
      target_framework: target_text,
      source: &lock.source,
      prune_fingerprint,
      source_protocol: lock.source_protocol,
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
    let root = resolution.cache_root().join(normalize_id(&id)?).join(normalize_version(&version)?);
    packages.push(LockPackage {
      id,
      version,
      sha512: resolution.package_hash(index).to_owned(),
      direct: package.direct,
      dependencies,
      compile_assets: relative_assets(&root, resolution.package_compile_assets(index))?,
      runtime_assets: relative_assets(&root, resolution.package_runtime_assets(index))?,
      analyzers: relative_assets(&root, resolution.package_analyzers(index))?,
      resource_assets: relative_assets(&root, resolution.package_resource_assets(index))?,
      content_files: relative_assets(&root, resolution.package_content_files(index))?,
      build_assets: relative_assets(&root, resolution.package_build_assets(index))?,
      build_multi_targeting_assets: relative_assets(&root, resolution.package_build_multi_targeting_assets(index))?,
      build_transitive_assets: relative_assets(&root, resolution.package_build_transitive_assets(index))?,
      native_assets: relative_assets(&root, resolution.package_native_assets(index))?,
      runtime_targets: resolution
        .package_runtime_targets(index)
        .map(|asset| {
          Ok(LockRuntimeTarget {
            path: relative_asset(&root, resolution.get(asset.path))?,
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
    let path = temp.write(
      "dv.lock.json",
      r#"{"schema_version":1,"target_framework":"net10.0","source":"https://api.nuget.org/v3/index.json","source_protocol":"v3","direct":[],"packages":[]}"#,
    );
    let config = NugetConfiguration {
      cache_root: temp.0.join("packages"),
      sources: vec![PackageSource {
        url: DEFAULT_SOURCE.into(),
        protocol: NugetProtocol::V3,
      }],
    };

    let result = read_warm_lock(&path, &config, &[], "net10.0", "current-table").unwrap();

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
    let mut sources = Vec::new();
    let mut disabled = BTreeSet::new();
    let mut cache = None;

    merge_config(&path, &mut sources, &mut disabled, &mut cache).unwrap();

    assert_eq!(sources[0].0, "legacy");
    assert_eq!(sources[0].1.protocol, NugetProtocol::V2);
    assert_eq!(sources[1].0, "modern");
    assert_eq!(sources[1].1.protocol, NugetProtocol::V3);
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
    let no_sources = discover_configuration(&temp.0.join("repository"), Some(&temp.0.join("packages")), Some(&explicit))
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
  fn exact_v3_package_uses_only_the_discovered_flat_container() {
    let service_index = serde_json::json!({
      "resources": [{
        "@id": "https://content.example.test/arbitrary/root",
        "@type": ["PackageBaseAddress/3.0.0", "Other/1.0.0"]
      }]
    });
    let package_base = package_base_from_service_index("https://feed.example.test/custom-index", &service_index).unwrap();

    let metadata = v3_package_metadata(&request(), &package_base);

    assert_eq!(package_base, "https://content.example.test/arbitrary/root/");
    assert_eq!(
      metadata.content_url,
      "https://content.example.test/arbitrary/root/sample.package/1.2.3/sample.package.1.2.3.nupkg"
    );
    assert_eq!(metadata.expected_hash, None);
    assert_eq!(metadata.expected_size, None);
    assert_eq!(metadata.requests, 0);
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
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup>
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
  fn direct_project_ranges_select_the_lowest_available_version_offline() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup>
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
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup>
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
