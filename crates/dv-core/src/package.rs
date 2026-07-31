use std::{
  collections::{BTreeMap, BTreeSet, HashSet},
  env,
  error::Error,
  fmt, fs,
  io::{self, Read, Write},
  mem::{align_of, size_of},
  path::{Component, Path, PathBuf},
  sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
  },
  thread,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use quick_xml::{Reader, XmlVersion, events::Event};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use zip::ZipArchive;

use crate::{FrameworkFamily, ProjectSpec, TargetFramework};

const DEFAULT_SOURCE: &str = "https://api.nuget.org/v3/index.json";
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_DOWNLOAD_WORKERS: usize = 4;
const MAX_EXTRACTION_WORKERS: usize = 4;
const MIN_PARALLEL_EXTRACTION_ENTRIES: usize = 8;
const LOCK_SCHEMA_VERSION: u16 = 1;

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

/// Options controlling exact package resolution.
#[derive(Clone, Debug, Default)]
pub struct PackageResolveOptions {
  /// Explicit global-packages directory, overriding environment and config.
  pub packages_directory: Option<PathBuf>,
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
  source_protocol: NugetProtocol,
  packages: Box<[ResolvedPackage]>,
  package_assets: Box<[PackageAssets]>,
  dependencies: Box<[u32]>,
  compile_assets: Box<[TextSpan]>,
  runtime_assets: Box<[TextSpan]>,
  analyzers: Box<[TextSpan]>,
  cache_hits: u32,
  downloaded_packages: u32,
  network_requests: u32,
  downloaded_bytes: u64,
}

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
    self.compile_assets.iter().map(|span| Path::new(self.get(*span)))
  }

  /// Iterates selected runtime assemblies across the graph.
  pub fn runtime_assets(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.runtime_assets.iter().map(|span| Path::new(self.get(*span)))
  }

  /// Iterates package analyzers across the graph.
  pub fn analyzers(&self) -> impl ExactSizeIterator<Item = &Path> {
    self.analyzers.iter().map(|span| Path::new(self.get(*span)))
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
    self.compile_assets[range].iter().map(|span| self.get(*span))
  }

  fn package_runtime_assets(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_assets[index].runtime);
    self.runtime_assets[range].iter().map(|span| self.get(*span))
  }

  fn package_analyzers(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_assets[index].analyzers);
    self.analyzers[range].iter().map(|span| self.get(*span))
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
        let Ok(version) = normalize_version(project.package_version(*reference)) else {
          return false;
        };
        self.packages.iter().copied().any(|package| {
          package.direct && self.package_id(package).eq_ignore_ascii_case(project.package_id(*reference)) && self.package_version(package) == version
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

struct WorkPackage {
  request: PackageRequest,
  hash: String,
  dependencies: Vec<PackageRequest>,
  compile_assets: Vec<PathBuf>,
  runtime_assets: Vec<PathBuf>,
  analyzers: Vec<PathBuf>,
  cache_hit: bool,
  origin: Option<PackageSource>,
}

struct ResolutionContext<'a> {
  cache_root: &'a Path,
  lock_path: &'a Path,
  target_framework: &'a str,
  source: &'a str,
  source_protocol: NugetProtocol,
}

struct CachedPackage {
  root: PathBuf,
  hash: String,
  cache_hit: bool,
  requests: u32,
  bytes: u64,
  origin: Option<PackageSource>,
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
  let config = discover_configuration(project.project_directory(), options.packages_directory.as_deref())?;
  let lock_path = project.project_directory().join("dv.lock.json");
  let direct = direct_requests(project)?;
  let target = project.target();
  let target_text = project.target_framework();
  if let Some(resolution) = read_warm_lock(&lock_path, &config, &direct, target_text)? {
    return Ok(resolution);
  }

  let mut pending: BTreeMap<String, PackageRequest> = direct.iter().cloned().map(|request| (request.lower_id.clone(), request)).collect();
  let mut resolved = BTreeMap::<String, WorkPackage>::new();
  let agent = http_agent();
  let mut endpoints = None;
  let mut network_requests = 0;
  let mut downloaded_bytes = 0;

  while resolved.len() < pending.len() {
    let wave: Vec<PackageRequest> = pending.values().filter(|request| !resolved.contains_key(&request.lower_id)).cloned().collect();
    if wave.is_empty() {
      break;
    }

    let misses = wave.iter().filter(|request| !package_root(&config.cache_root, request).exists()).count();
    if misses > 0 && options.offline {
      let request = wave
        .iter()
        .find(|request| !package_root(&config.cache_root, request).exists())
        .expect("one miss exists");
      return Err(PackageError::new(
        PackageErrorKind::OfflineMiss,
        format!("{} {}", request.id, request.version),
        format!("package {} {} is not available in the global package cache", request.id, request.version),
      ));
    }
    if misses > 0 && endpoints.is_none() {
      let (discovered, requests) = discover_service_endpoints(&agent, &config.sources)?;
      endpoints = Some(discovered);
      network_requests += requests;
    }

    let cached = ensure_wave(&agent, &wave, &config.cache_root, endpoints.as_deref().unwrap_or(&[]))?;
    for (request, cached) in wave.into_iter().zip(cached) {
      network_requests += cached.requests;
      downloaded_bytes += cached.bytes;
      let parsed = parse_cached_package(request.clone(), cached, target, target_text)?;
      for dependency in &parsed.dependencies {
        match pending.get_mut(&dependency.lower_id) {
          Some(existing) if existing.version != dependency.version => {
            return Err(PackageError::new(
              PackageErrorKind::Resolution,
              &dependency.id,
              format!(
                "package {} requires conflicting exact versions {} and {}",
                dependency.id, existing.version, dependency.version
              ),
            ));
          },
          Some(existing) => existing.direct |= dependency.direct,
          None => {
            pending.insert(dependency.lower_id.clone(), dependency.clone());
          },
        }
      }
      resolved.insert(request.lower_id, parsed);
    }
  }

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
      source_protocol,
    },
    &resolved,
    network_requests,
    downloaded_bytes,
  )?;
  if options.write_lock {
    write_lock(&resolution)?;
  }
  Ok(resolution)
}

fn direct_requests(project: &ProjectSpec) -> Result<Vec<PackageRequest>, PackageError> {
  let mut direct = Vec::with_capacity(project.package_references().len());
  let mut seen = BTreeMap::<String, String>::new();
  for package in project.package_references() {
    let id = project.package_id(*package);
    let lower_id = normalize_id(id)?;
    let version = normalize_version(project.package_version(*package))?;
    if let Some(existing) = seen.insert(lower_id.clone(), version.clone())
      && existing != version
    {
      return Err(PackageError::new(
        PackageErrorKind::Resolution,
        id,
        format!("package {id} is directly referenced with conflicting versions {existing} and {version}"),
      ));
    }
    direct.push(PackageRequest {
      id: id.into(),
      lower_id,
      version,
      direct: true,
    });
  }
  direct.sort_unstable_by(|left, right| left.lower_id.cmp(&right.lower_id));
  direct.dedup_by(|left, right| left.lower_id == right.lower_id);
  Ok(direct)
}

fn discover_configuration(project_directory: &Path, explicit_cache: Option<&Path>) -> Result<NugetConfiguration, PackageError> {
  let mut config_paths = Vec::new();
  if let Some(user) = user_config_path()
    && user.is_file()
  {
    config_paths.push(user);
  }
  let mut ancestors: Vec<&Path> = project_directory.ancestors().collect();
  ancestors.reverse();
  for directory in ancestors {
    for name in ["NuGet.Config", "nuget.config"] {
      let candidate = directory.join(name);
      if candidate.is_file() && !config_paths.contains(&candidate) {
        config_paths.push(candidate);
      }
    }
  }

  let mut sources = vec![(
    "nuget.org".to_owned(),
    PackageSource {
      url: DEFAULT_SOURCE.to_owned(),
      protocol: NugetProtocol::V3,
    },
  )];
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
  for source in &sources {
    if !source.url.starts_with("https://") {
      return Err(PackageError::new(
        PackageErrorKind::Configuration,
        &source.url,
        format!("package resolution supports HTTPS NuGet v2 and v3 sources; {:?} is unsupported", source.url),
      ));
    }
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

fn user_config_path() -> Option<PathBuf> {
  if cfg!(windows) {
    env::var_os("APPDATA").map(PathBuf::from).map(|path| path.join("NuGet/NuGet.Config"))
  } else {
    env::var_os("HOME").map(PathBuf::from).map(|path| path.join(".config/NuGet/NuGet.Config"))
  }
}

fn default_global_packages() -> Option<PathBuf> {
  env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
    .map(PathBuf::from)
    .map(|path| path.join(".nuget/packages"))
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

fn http_agent() -> ureq::Agent {
  ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into()
}

fn discover_service_endpoints(agent: &ureq::Agent, sources: &[PackageSource]) -> Result<(Vec<ServiceEndpoint>, u32), PackageError> {
  let mut endpoints = Vec::with_capacity(sources.len());
  let mut requests = 0;
  for source in sources {
    match source.protocol {
      NugetProtocol::V2 => endpoints.push(ServiceEndpoint::V2 {
        source: source.url.clone(),
        base: with_trailing_slash(source.url.clone()),
      }),
      NugetProtocol::V3 => {
        let document: serde_json::Value = get_json(agent, &source.url)?;
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

fn ensure_wave(agent: &ureq::Agent, requests: &[PackageRequest], cache_root: &Path, endpoints: &[ServiceEndpoint]) -> Result<Vec<CachedPackage>, PackageError> {
  if requests.len() <= 1 || requests.iter().all(|request| package_root(cache_root, request).exists()) {
    return requests
      .iter()
      .map(|request| ensure_package(agent, request, cache_root, endpoints, true))
      .collect();
  }

  let cursor = AtomicUsize::new(0);
  let results = Mutex::new(Vec::with_capacity(requests.len()));
  let worker_count = requests.len().min(MAX_DOWNLOAD_WORKERS);
  thread::scope(|scope| {
    for _ in 0..worker_count {
      let worker_agent = agent.clone();
      let results = &results;
      let cursor = &cursor;
      scope.spawn(move || {
        loop {
          let index = cursor.fetch_add(1, Ordering::Relaxed);
          if index >= requests.len() {
            break;
          }
          let result = ensure_package(&worker_agent, &requests[index], cache_root, endpoints, false);
          results.lock().expect("package worker result lock is not poisoned").push((index, result));
        }
      });
    }
  });
  let mut results = results.into_inner().expect("package worker result lock is not poisoned");
  results.sort_unstable_by_key(|(index, _)| *index);
  results.into_iter().map(|(_, result)| result).collect()
}

fn ensure_package(
  agent: &ureq::Agent,
  request: &PackageRequest,
  cache_root: &Path,
  endpoints: &[ServiceEndpoint],
  parallel_extract: bool,
) -> Result<CachedPackage, PackageError> {
  let root = package_root(cache_root, request);
  if root.exists() {
    return validate_cached_package(&root, request, true, 0, 0);
  }
  let mut last_error = None;
  for endpoint in endpoints {
    match download_and_publish(agent, request, cache_root, endpoint, parallel_extract) {
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

fn download_and_publish(
  agent: &ureq::Agent,
  request: &PackageRequest,
  cache_root: &Path,
  endpoint: &ServiceEndpoint,
  parallel_extract: bool,
) -> Result<CachedPackage, PackageError> {
  let metadata = match endpoint {
    ServiceEndpoint::V2 { base, .. } => v2_package_metadata(agent, request, base)?,
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

  fs::create_dir_all(cache_root).map_err(|error| package_io("create package cache", cache_root, error))?;
  let temp_root = unique_temp_root(cache_root, request);
  fs::create_dir(&temp_root).map_err(|error| package_io("create package staging directory", &temp_root, error))?;
  let mut guard = TempGuard(Some(temp_root.clone()));
  let nupkg_name = format!("{}.{}.nupkg", request.lower_id, request.version);
  let nupkg_path = temp_root.join(&nupkg_name);
  let (hash, bytes) = download_package(agent, &metadata.content_url, &nupkg_path)?;
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
  validate_and_extract_archive(&nupkg_path, &temp_root, parallel_extract)?;
  normalize_nuspec_name(&temp_root, request)?;
  validate_staged_nuspec_identity(&temp_root, request)?;
  fs::write(temp_root.join(format!("{nupkg_name}.sha512")), hash.as_bytes()).map_err(|error| package_io("write package hash", &temp_root, error))?;
  let package_metadata = serde_json::json!({
    "schemaVersion": 1,
    "sha512": hash,
    "source": endpoint.source(),
    "protocol": endpoint.protocol().as_str(),
  });
  fs::write(
    temp_root.join(".dv.metadata.json"),
    serde_json::to_vec_pretty(&package_metadata).expect("serializing package metadata succeeds"),
  )
  .map_err(|error| package_io("write package metadata", &temp_root, error))?;

  let final_root = package_root(cache_root, request);
  fs::create_dir_all(final_root.parent().expect("package version has an identity parent"))
    .map_err(|error| package_io("create package identity directory", &final_root, error))?;
  match fs::rename(&temp_root, &final_root) {
    Ok(()) => guard.0 = None,
    Err(_) if final_root.exists() => {},
    Err(error) => return Err(package_io("publish package atomically", &final_root, error)),
  }
  let mut cached = validate_cached_package(&final_root, request, false, metadata.requests + 1, bytes)?;
  cached.origin = Some(PackageSource {
    url: endpoint.source().to_owned(),
    protocol: endpoint.protocol(),
  });
  Ok(cached)
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

fn v2_package_metadata(agent: &ureq::Agent, request: &PackageRequest, base: &str) -> Result<PackageMetadata, PackageError> {
  let metadata_url = format!("{base}Packages(Id='{}',Version='{}')", request.id, request.version);
  let bytes = get_bytes(agent, &metadata_url, MAX_JSON_BYTES, "NuGet v2 metadata")?;
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

fn download_package(agent: &ureq::Agent, url: &str, destination: &Path) -> Result<(String, u64), PackageError> {
  let mut response = agent
    .get(url)
    .call()
    .map_err(|error| network_error(url, format!("package download failed: {error}")))?;
  if response.body().content_length().is_some_and(|length| length > MAX_PACKAGE_BYTES) {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      url,
      format!("package Content-Length exceeds the {MAX_PACKAGE_BYTES} byte limit"),
    ));
  }
  let mut input = response.body_mut().as_reader();
  let mut output = fs::File::create(destination).map_err(|error| package_io("create package archive", destination, error))?;
  let mut hasher = Sha512::new();
  let mut buffer = [0u8; 64 * 1024];
  let mut total = 0u64;
  loop {
    let read = input
      .read(&mut buffer)
      .map_err(|error| network_error(url, format!("read package response: {error}")))?;
    if read == 0 {
      break;
    }
    total = total
      .checked_add(read as u64)
      .filter(|total| *total <= MAX_PACKAGE_BYTES)
      .ok_or_else(|| PackageError::new(PackageErrorKind::Integrity, url, "package response exceeds the download limit"))?;
    hasher.update(&buffer[..read]);
    output
      .write_all(&buffer[..read])
      .map_err(|error| package_io("write package archive", destination, error))?;
  }
  output.sync_all().map_err(|error| package_io("flush package archive", destination, error))?;
  Ok((BASE64.encode(hasher.finalize()), total))
}

fn get_json<T: for<'de> Deserialize<'de>>(agent: &ureq::Agent, url: &str) -> Result<T, PackageError> {
  let bytes = get_bytes(agent, url, MAX_JSON_BYTES, "JSON")?;
  serde_json::from_slice(&bytes).map_err(|error| network_error(url, format!("invalid JSON response: {error}")))
}

fn get_bytes(agent: &ureq::Agent, url: &str, limit: u64, kind: &str) -> Result<Vec<u8>, PackageError> {
  let mut response = agent
    .get(url)
    .call()
    .map_err(|error| network_error(url, format!("HTTP request failed: {error}")))?;
  response
    .body_mut()
    .with_config()
    .limit(limit)
    .read_to_vec()
    .map_err(|error| network_error(url, format!("read {kind} response: {error}")))
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

fn parse_cached_package(request: PackageRequest, cached: CachedPackage, target: TargetFramework, target_text: &str) -> Result<WorkPackage, PackageError> {
  let nuspec_path = find_nuspec(&cached.root)?;
  let nuspec = fs::read(&nuspec_path).map_err(|error| package_io("read package manifest", &nuspec_path, error))?;
  let dependencies = parse_nuspec(&nuspec_path, &nuspec, &request, target)?;
  reject_unsupported_package_assets(&cached.root)?;
  let compile_assets = select_compile_assets(&cached.root, target)?;
  let runtime_assets = select_runtime_assets(&cached.root, target)?;
  let analyzers = collect_analyzers(&cached.root)?;
  if compile_assets.is_empty() {
    return Err(PackageError::new(
      PackageErrorKind::Incompatible,
      format!("{} {}", request.id, request.version),
      format!("package {} {} has no compatible compile assets for {target_text}", request.id, request.version,),
    ));
  }
  Ok(WorkPackage {
    request,
    hash: cached.hash,
    dependencies,
    compile_assets,
    runtime_assets,
    analyzers,
    cache_hit: cached.cache_hit,
    origin: cached.origin,
  })
}

struct DependencyGroup {
  framework: Option<String>,
  dependencies: Vec<(String, String)>,
}

fn parse_nuspec(path: &Path, bytes: &[u8], request: &PackageRequest, target: TargetFramework) -> Result<Vec<PackageRequest>, PackageError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  let mut current_text = NuspecText::None;
  let mut id = None;
  let mut version = None;
  let mut groups = Vec::<DependencyGroup>::new();
  let mut ungrouped = Vec::new();
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => match local_name(element.name().as_ref()) {
        b"id" if id.is_none() => current_text = NuspecText::Id,
        b"version" if version.is_none() => current_text = NuspecText::Version,
        b"group" => {
          groups.push(DependencyGroup {
            framework: nuspec_attribute(&reader, &element, b"targetFramework", path)?,
            dependencies: Vec::new(),
          });
        },
        _ => {},
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"group" => {
        groups.push(DependencyGroup {
          framework: nuspec_attribute(&reader, &element, b"targetFramework", path)?,
          dependencies: Vec::new(),
        });
      },
      Ok(Event::Empty(element)) if local_name(element.name().as_ref()) == b"dependency" => {
        let dependency_id = nuspec_attribute(&reader, &element, b"id", path)?.ok_or_else(|| package_manifest_error(path, "dependency requires id"))?;
        let dependency_version =
          nuspec_attribute(&reader, &element, b"version", path)?.ok_or_else(|| package_manifest_error(path, "dependency requires version"))?;
        if let Some(group) = groups.last_mut() {
          group.dependencies.push((dependency_id, dependency_version));
        } else {
          ungrouped.push((dependency_id, dependency_version));
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
      Ok(Event::End(element)) => {
        if matches!(local_name(element.name().as_ref()), b"id" | b"version") {
          current_text = NuspecText::None;
        }
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
    .map(|(id, range)| {
      Ok(PackageRequest {
        id: id.clone(),
        lower_id: normalize_id(id)?,
        version: minimum_version(range)?,
        direct: false,
      })
    })
    .collect()
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

fn reject_unsupported_package_assets(root: &Path) -> Result<(), PackageError> {
  for directory in ["build", "buildTransitive", "buildMultiTargeting", "runtimes"] {
    let path = root.join(directory);
    if path.is_dir() {
      return Err(PackageError::new(
        PackageErrorKind::Incompatible,
        path.display().to_string(),
        format!("package assets under {directory} are not supported by the initial resolver"),
      ));
    }
  }
  Ok(())
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

fn collect_analyzers(root: &Path) -> Result<Vec<PathBuf>, PackageError> {
  let analyzer_root = root.join("analyzers/dotnet/cs");
  if !analyzer_root.is_dir() {
    return Ok(Vec::new());
  }
  let mut directories = vec![analyzer_root];
  let mut analyzers = Vec::new();
  while let Some(directory) = directories.pop() {
    for entry in fs::read_dir(&directory).map_err(|error| package_io("enumerate package analyzers", &directory, error))? {
      let entry = entry.map_err(|error| package_io("enumerate package analyzers", &directory, error))?;
      let path = entry.path();
      let file_type = entry.file_type().map_err(|error| package_io("inspect package analyzer", &path, error))?;
      if file_type.is_dir() {
        directories.push(path);
      } else if file_type.is_file() && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dll")) {
        analyzers.push(path);
      }
    }
  }
  analyzers.sort_unstable();
  Ok(analyzers)
}

fn minimum_version(range: &str) -> Result<String, PackageError> {
  let trimmed = range.trim();
  let version = if trimmed.starts_with('[') && trimmed.ends_with(']') && !trimmed.contains(',') {
    &trimmed[1..trimmed.len() - 1]
  } else if let Some(remainder) = trimmed.strip_prefix('[') {
    remainder
      .split_once(',')
      .filter(|(_, upper)| upper.trim().is_empty() || upper.trim() == ")")
      .map(|(lower, _)| lower.trim())
      .ok_or_else(|| {
        PackageError::new(
          PackageErrorKind::Resolution,
          range,
          format!("dependency range {range:?} is outside the initial lowest-inclusive subset"),
        )
      })?
  } else if !trimmed.contains(['(', ')', '[', ']', ',', '*']) {
    trimmed
  } else {
    return Err(PackageError::new(
      PackageErrorKind::Resolution,
      range,
      format!("dependency range {range:?} is outside the initial lowest-inclusive subset"),
    ));
  };
  normalize_version(version)
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
  let precedence = value.split_once('+').map_or(value, |(precedence, _)| precedence);
  let (numbers, prerelease) = precedence
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
  let parts: Vec<&str> = numbers.split('.').collect();
  if parts.is_empty() || parts.len() > 4 {
    return Err(PackageError::new(
      PackageErrorKind::Resolution,
      value,
      format!("package version {value:?} must contain one to four numeric parts"),
    ));
  }
  let mut numeric = [0u32; 4];
  for (index, part) in parts.iter().enumerate() {
    numeric[index] = part.parse().map_err(|_| {
      PackageError::new(
        PackageErrorKind::Resolution,
        value,
        format!("package version {value:?} contains a non-numeric version part"),
      )
    })?;
  }
  let mut normalized = format!("{}.{}.{}", numeric[0], numeric[1], numeric[2]);
  if numeric[3] != 0 {
    normalized.push_str(&format!(".{}", numeric[3]));
  }
  if let Some(prerelease) = prerelease {
    normalized.push('-');
    normalized.push_str(&prerelease.to_ascii_lowercase());
  }
  Ok(normalized)
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
          .map(|path| path.as_os_str().len())
          .sum::<usize>()
    })
    .sum::<usize>()
    + context.cache_root.as_os_str().len()
    + context.lock_path.as_os_str().len()
    + context.target_framework.len()
    + context.source.len();
  let mut table = TextTable::with_capacity(estimated);
  let cache_root_span = table.push_path(context.cache_root)?;
  let lock_path_span = table.push_path(context.lock_path)?;
  let target_framework_span = table.push(context.target_framework)?;
  let source_span = table.push(context.source)?;
  let mut packages = Vec::with_capacity(work.len());
  let mut package_assets = Vec::with_capacity(work.len());
  let mut dependencies = Vec::new();
  let mut compile_assets = Vec::new();
  let mut runtime_assets = Vec::new();
  let mut analyzers = Vec::new();
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
    let compile = push_asset_range(&mut table, &mut compile_assets, &package.compile_assets)?;
    let runtime = push_asset_range(&mut table, &mut runtime_assets, &package.runtime_assets)?;
    let analyzer_range = push_asset_range(&mut table, &mut analyzers, &package.analyzers)?;
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
    cache_hits += u32::from(package.cache_hit);
  }

  Ok(PackageResolution {
    text: table.text.into_boxed_str(),
    cache_root: cache_root_span,
    lock_path: lock_path_span,
    target_framework: target_framework_span,
    source: source_span,
    source_protocol: context.source_protocol,
    packages: packages.into_boxed_slice(),
    package_assets: package_assets.into_boxed_slice(),
    dependencies: dependencies.into_boxed_slice(),
    compile_assets: compile_assets.into_boxed_slice(),
    runtime_assets: runtime_assets.into_boxed_slice(),
    analyzers: analyzers.into_boxed_slice(),
    cache_hits,
    downloaded_packages: work.len() as u32 - cache_hits,
    network_requests,
    downloaded_bytes,
  })
}

fn push_asset_range(table: &mut TextTable, target: &mut Vec<TextSpan>, paths: &[PathBuf]) -> Result<ItemRange, PackageError> {
  let start = u32_len(target.len(), "package asset range")?;
  for path in paths {
    target.push(table.push_path(path)?);
  }
  Ok(ItemRange {
    start,
    len: u32_len(paths.len(), "package asset range")?,
  })
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
    source_protocol: NugetProtocol::V3,
    packages: Box::new([]),
    package_assets: Box::new([]),
    dependencies: Box::new([]),
    compile_assets: Box::new([]),
    runtime_assets: Box::new([]),
    analyzers: Box::new([]),
    cache_hits: 0,
    downloaded_packages: 0,
    network_requests: 0,
    downloaded_bytes: 0,
  })
}

fn read_warm_lock(path: &Path, config: &NugetConfiguration, direct: &[PackageRequest], target_text: &str) -> Result<Option<PackageResolution>, PackageError> {
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
  let expected_direct: Vec<LockDirect> = direct
    .iter()
    .map(|request| LockDirect {
      id: request.id.clone(),
      version: request.version.clone(),
    })
    .collect();
  if lock.schema_version != LOCK_SCHEMA_VERSION
    || lock.target_framework != target_text
    || lock.direct != expected_direct
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
    let cached = validate_cached_package(&root, &request, true, 0, 0)?;
    if cached.hash != package.sha512 {
      return Err(PackageError::new(
        PackageErrorKind::Integrity,
        root.display().to_string(),
        format!("cached package hash for {} {} does not match dv.lock.json", request.id, request.version),
      ));
    }
    let compile_assets = lock_asset_paths(&root, &package.compile_assets)?;
    let runtime_assets = lock_asset_paths(&root, &package.runtime_assets)?;
    let analyzers = lock_asset_paths(&root, &package.analyzers)?;
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
      .is_some_and(|package| package.request.direct && package.request.version == request.version)
    {
      return Err(PackageError::new(
        PackageErrorKind::Integrity,
        path.display().to_string(),
        format!("dv package lock omits direct package {} {}", request.id, request.version),
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
    let path = root.join(relative);
    if !path.is_file() {
      return Err(PackageError::new(
        PackageErrorKind::Integrity,
        path.display().to_string(),
        "locked package asset is missing",
      ));
    }
    paths.push(path);
  }
  Ok(paths)
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
    });
  }
  let lock = LockFile {
    schema_version: LOCK_SCHEMA_VERSION,
    target_framework: resolution.target_framework().into(),
    source: resolution.source().into(),
    source_protocol: resolution.source_protocol,
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
  assets
    .map(|asset| {
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
    })
    .collect()
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
    temp.write("packages/sample.package/1.2.3/lib/net10.0/Sample.Package.dll", []);
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(cache),
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
