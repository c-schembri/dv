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
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering as AtomicOrdering},
  },
  thread,
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use quick_xml::{Reader, XmlVersion, events::Event};
use reqwest::header::{AUTHORIZATION, HeaderValue, RETRY_AFTER};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use tokio::{
  io::AsyncWriteExt,
  sync::{Mutex, OwnedSemaphorePermit, Semaphore},
  task::JoinSet,
};
use zeroize::Zeroizing;
use zip::ZipArchive;

use package_signature::{FingerprintAlgorithm, SignaturePolicy, TrustedCertificate, TrustedSigner, TrustedSignerKind};

use crate::{
  BENCHMARK_CACHE_LINE_BYTES, CacheOutcome, CredentialProviderLogSink, FrameworkFamily, NugetAuditLevel, NugetAuditMode, PackageAssetFlags,
  PackageCancellation, ProjectSpec, RuntimeIdentifierGraph, SdkInventory, TargetFramework,
  credential_provider::{self, CredentialProviderError, CredentialProviderErrorKind, CredentialProviderOptions},
  discover_sdks,
  framework_reference::package_pruning_runtime_names,
  legacy_pruning::{LegacyPrunePackage, PruningFramework, exact_legacy_pruning, nearest_legacy_pruning},
  load_portable_runtime_graph, redact_url_for_output,
};

#[path = "package_signature.rs"]
mod package_signature;

const DEFAULT_SOURCE: &str = "https://api.nuget.org/v3/index.json";
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CLIENT_CERTIFICATE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PRUNE_DATA_BYTES: u64 = 1024 * 1024;
const MAX_PRUNE_PACKAGES: usize = 10_000;
const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_CONTENT_FILE_RULES: usize = 4_096;
const MAX_CONTENT_PATTERN_BYTES: usize = 1_024;
const MAX_DOWNLOAD_WORKERS: usize = 24;
const DEFAULT_GLOBAL_NETWORK_REQUESTS: u16 = MAX_DOWNLOAD_WORKERS as u16;
const DEFAULT_MAX_HTTP_REQUESTS_PER_SOURCE: u16 = 64;
const MAX_NETWORK_TRIES: u8 = 32;
const MAX_RETRY_DELAY_MS: u32 = 60_000;
const MAX_RETRY_AFTER_SECONDS: u32 = 86_400;
const ASYNC_RUNTIME_WORKERS: usize = 2;
const MAX_EXTRACTION_WORKERS: usize = 4;
const MIN_PARALLEL_EXTRACTION_ENTRIES: usize = 8;
const MAX_GRAPH_REVISIONS: u32 = 64;
const PUBLISH_RETRY_DELAYS: [Duration; 3] = [Duration::from_millis(1), Duration::from_millis(4), Duration::from_millis(16)];
const LOCK_SCHEMA_VERSION: u16 = 8;
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

const HTTP_RETRY_429: u8 = 1 << 0;
const HTTP_OBSERVE_RETRY_AFTER: u8 = 1 << 1;
const HTTP_PROXY_CONFIGURED: u8 = 1 << 2;
const HTTP_PROXY_AUTHENTICATED: u8 = 1 << 3;
const HTTP_NO_PROXY_CONFIGURED: u8 = 1 << 4;
const HTTP_OFFLINE: u8 = 1 << 5;
const HTTP_TLS_VALIDATION: u8 = 1 << 6;
const HTTP_INSECURE_CONNECTIONS: u8 = 1 << 7;

const SOURCE_ALLOW_INSECURE_CONNECTIONS: u8 = 1 << 0;
const SOURCE_DISABLE_TLS_VALIDATION: u8 = 1 << 1;

const CONTENT_COPY_TO_OUTPUT: u8 = 1 << 0;
const CONTENT_FLATTEN: u8 = 1 << 1;
const CONTENT_HAS_BUILD_ACTION: u8 = 1 << 0;
const CONTENT_HAS_COPY_TO_OUTPUT: u8 = 1 << 1;
const CONTENT_HAS_FLATTEN: u8 = 1 << 2;
const NO_CONTENT_BUILD_ACTION: u32 = u32::MAX;
const DEFAULT_CONTENT_BUILD_ACTION: &str = "Compile";

/// Compact immutable NuGet transport policy selected from config and environment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageHttpPolicy {
  retry_delay_ms: u32,
  max_retry_after_seconds: u32,
  request_timeout_seconds: u16,
  download_timeout_seconds: u16,
  max_requests_per_source: u16,
  max_tries: u8,
  flags: u8,
}

const DEFAULT_HTTP_POLICY: PackageHttpPolicy = PackageHttpPolicy {
  retry_delay_ms: 1_000,
  max_retry_after_seconds: 3_600,
  request_timeout_seconds: 100,
  download_timeout_seconds: 60,
  max_requests_per_source: DEFAULT_MAX_HTTP_REQUESTS_PER_SOURCE,
  max_tries: 6,
  flags: HTTP_RETRY_429 | HTTP_OBSERVE_RETRY_AFTER | HTTP_TLS_VALIDATION,
};

const _: () = assert!(size_of::<PackageHttpPolicy>() == 16);
const _: () = assert!(align_of::<PackageHttpPolicy>() == 4);

/// Command-wide network task budget, clamped to the measured scheduler ceiling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageRequestBudget {
  global_requests: u16,
}

const DEFAULT_REQUEST_BUDGET: PackageRequestBudget = PackageRequestBudget {
  global_requests: DEFAULT_GLOBAL_NETWORK_REQUESTS,
};

const _: () = assert!(MAX_DOWNLOAD_WORKERS <= u16::MAX as usize);
const _: () = assert!(size_of::<PackageRequestBudget>() == 2);
const _: () = assert!(align_of::<PackageRequestBudget>() == 2);

/// Response-local accounting returned through task results without shared writes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct HttpWork {
  downloaded_bytes: u64,
  duration_us: u64,
  requests: u32,
}

/// Resolver accounting keyed by the deterministic configured-source index.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SourceWork {
  downloaded_bytes: u64,
  duration_us: u64,
  source_index: u32,
  requests: u32,
}

struct HttpPayload<T> {
  value: T,
  work: HttpWork,
}

const _: () = assert!(size_of::<HttpWork>() == 24);
const _: () = assert!(align_of::<HttpWork>() == 8);
const _: () = assert!(size_of::<SourceWork>() == 24);
const _: () = assert!(align_of::<SourceWork>() == 8);

impl HttpWork {
  fn merge(&mut self, other: Self, context: &str) -> Result<(), PackageError> {
    self.requests = self
      .requests
      .checked_add(other.requests)
      .ok_or_else(|| network_error(context, "HTTP request count overflow"))?;
    self.downloaded_bytes = self
      .downloaded_bytes
      .checked_add(other.downloaded_bytes)
      .ok_or_else(|| network_error(context, "HTTP response byte count overflow"))?;
    self.duration_us = self
      .duration_us
      .checked_add(other.duration_us)
      .ok_or_else(|| network_error(context, "HTTP request duration overflow"))?;
    Ok(())
  }
}

impl SourceWork {
  const fn new(source_index: u32) -> Self {
    Self {
      downloaded_bytes: 0,
      duration_us: 0,
      source_index,
      requests: 0,
    }
  }

  fn merge_http(&mut self, work: HttpWork, context: &str) -> Result<(), PackageError> {
    let mut total = HttpWork {
      downloaded_bytes: self.downloaded_bytes,
      duration_us: self.duration_us,
      requests: self.requests,
    };
    total.merge(work, context)?;
    self.downloaded_bytes = total.downloaded_bytes;
    self.duration_us = total.duration_us;
    self.requests = total.requests;
    Ok(())
  }
}

fn elapsed_us(started: Instant) -> u64 {
  started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

fn source_work_table(source_count: usize) -> Result<Vec<SourceWork>, PackageError> {
  (0..source_count)
    .map(|index| u32_len(index, "NuGet package-source index").map(SourceWork::new))
    .collect()
}

fn merge_source_work(target: &mut [SourceWork], work: SourceWork, context: &str) -> Result<(), PackageError> {
  let slot = target.get_mut(work.source_index as usize).ok_or_else(|| {
    PackageError::new(
      PackageErrorKind::TextOverflow,
      context,
      format!("NuGet source-work index {} is outside the configured source batch", work.source_index),
    )
  })?;
  if slot.source_index != work.source_index {
    return Err(PackageError::new(
      PackageErrorKind::TextOverflow,
      context,
      "NuGet source-work table is not indexed contiguously",
    ));
  }
  slot.merge_http(
    HttpWork {
      downloaded_bytes: work.downloaded_bytes,
      duration_us: work.duration_us,
      requests: work.requests,
    },
    context,
  )
}

impl PackageRequestBudget {
  const fn global_limit(self) -> usize {
    self.global_requests as usize
  }
}

impl PackageHttpPolicy {
  /// Maximum attempts for retryable HTTP work, including the first request.
  pub const fn max_tries(self) -> u8 {
    self.max_tries
  }

  /// Base delay used when a retryable response has no accepted `Retry-After`.
  pub const fn retry_delay_ms(self) -> u32 {
    self.retry_delay_ms
  }

  /// Maximum accepted server-directed retry delay.
  pub const fn max_retry_after_seconds(self) -> u32 {
    self.max_retry_after_seconds
  }

  /// Total request timeout in seconds.
  pub const fn request_timeout_seconds(self) -> u16 {
    self.request_timeout_seconds
  }

  /// Maximum idle interval between response-body chunks in seconds.
  pub const fn download_timeout_seconds(self) -> u16 {
    self.download_timeout_seconds
  }

  /// Configured concurrent-request limit for each package source.
  pub const fn max_requests_per_source(self) -> u16 {
    self.max_requests_per_source
  }

  /// Whether HTTP 429 responses are retried.
  pub const fn retries_http_429(self) -> bool {
    self.flags & HTTP_RETRY_429 != 0
  }

  /// Whether `Retry-After` response headers control retry delay.
  pub const fn observes_retry_after(self) -> bool {
    self.flags & HTTP_OBSERVE_RETRY_AFTER != 0
  }

  /// Whether an explicit proxy is configured.
  pub const fn proxy_configured(self) -> bool {
    self.flags & HTTP_PROXY_CONFIGURED != 0
  }

  /// Whether the proxy has redacted Basic credentials.
  pub const fn proxy_authenticated(self) -> bool {
    self.flags & HTTP_PROXY_AUTHENTICATED != 0
  }

  /// Whether the proxy has an explicit bypass list.
  pub const fn no_proxy_configured(self) -> bool {
    self.flags & HTTP_NO_PROXY_CONFIGURED != 0
  }

  /// Whether all network work is disabled for this operation.
  pub const fn offline(self) -> bool {
    self.flags & HTTP_OFFLINE != 0
  }

  /// Whether every configured HTTPS source validates TLS peers and hostnames.
  pub const fn tls_validation(self) -> bool {
    self.flags & HTTP_TLS_VALIDATION != 0
  }

  /// Whether at least one configured source explicitly permits HTTP.
  pub const fn allows_insecure_connections(self) -> bool {
    self.flags & HTTP_INSECURE_CONNECTIONS != 0
  }

  /// Maximum redirects for the general HTTP client.
  pub const fn max_redirects(self) -> u8 {
    10
  }

  const fn effective_request_limit(self, global_limit: usize) -> usize {
    let configured = self.max_requests_per_source as usize;
    if configured < global_limit { configured } else { global_limit }
  }

  const fn with_offline(mut self, offline: bool) -> Self {
    if offline {
      self.flags |= HTTP_OFFLINE;
    } else {
      self.flags &= !HTTP_OFFLINE;
    }
    self
  }

  fn with_source_security(mut self, sources: &[(String, PackageSource)]) -> Self {
    if sources.iter().any(|(_, source)| source.allow_insecure_connections()) {
      self.flags |= HTTP_INSECURE_CONNECTIONS;
    }
    if sources.iter().any(|(_, source)| !source.tls_validation()) {
      self.flags &= !HTTP_TLS_VALIDATION;
    }
    self
  }
}

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
  central_transitive: bool,
  cache_hit: bool,
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

/// Rare package-provided framework metadata, keyed into the package batch.
///
/// Three fields occupy 20 bytes at four-byte alignment. The common package has
/// no row; names live in the resolution text table and each range is linear.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageFrameworkAssets {
  references: ItemRange,
  assemblies: ItemRange,
  package_index: u32,
}

const _: () = assert!(size_of::<PackageFrameworkAssets>() == 20);
const _: () = assert!(align_of::<PackageFrameworkAssets>() == 4);

/// Cold direct-reference policy kept separate from graph and asset hot rows.
///
/// Six fields occupy 32 bytes at four-byte alignment. Two rows fit in one
/// assumed 64-byte benchmark-host cache line; 51 direct references retain
/// 1,632 bytes plus the one contiguous allocation header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DirectPackagePolicy {
  no_warn: TextSpan,
  aliases: TextSpan,
  path_property: TextSpan,
  package_index: u32,
  include_assets: PackageAssetFlags,
  private_assets: PackageAssetFlags,
}

const _: () = assert!(size_of::<DirectPackagePolicy>() == 32);
const _: () = assert!(align_of::<DirectPackagePolicy>() == 4);
const _: () = assert!(BENCHMARK_CACHE_LINE_BYTES / size_of::<DirectPackagePolicy>() == 2);

/// One cold successful-restore downgrade warning.
///
/// The four spans occupy 32 bytes at four-byte alignment. Warning text shares
/// the resolution text table and the empty common case retains no row storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageDowngrade {
  package_id: TextSpan,
  selected_version: TextSpan,
  requested_range: TextSpan,
  requesting_package: TextSpan,
}

const _: () = assert!(size_of::<PackageDowngrade>() == 32);
const _: () = assert!(align_of::<PackageDowngrade>() == 4);
const _: () = assert!(BENCHMARK_CACHE_LINE_BYTES / size_of::<PackageDowngrade>() == 2);

/// The role assigned to an RID-specific runtime target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTargetKind {
  /// A managed runtime assembly.
  Runtime,
  /// A runtime-specific satellite resource assembly.
  Resource,
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

/// Metadata parallel to the contiguous content-asset path range.
///
/// The action shares the resolution text table. Two Boolean decisions occupy
/// inline bytes; the record remains 12 bytes at four-byte alignment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ContentFileMetadata {
  build_action: TextSpan,
  copy_to_output: bool,
  flatten: bool,
}

const _: () = assert!(size_of::<ContentFileMetadata>() == 12);
const _: () = assert!(align_of::<ContentFileMetadata>() == 4);

type AssetFlags = PackageAssetFlags;

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
  /// Permit a credential provider to show interactive login instructions.
  /// The default is false so CI never blocks waiting for input.
  pub interactive: bool,
  /// Acquire provider credentials while inspecting sources without making an
  /// HTTP request. Intended for diagnostics and like-for-like measurement.
  pub probe_credentials: bool,
  /// Cooperative cancellation observed by credential-provider subprocesses.
  pub cancellation: Option<PackageCancellation>,
  /// Receives provider log messages only in interactive mode.
  pub credential_provider_log_sink: Option<CredentialProviderLogSink>,
}

impl PackageResolveOptions {
  fn credential_provider_options(&self) -> CredentialProviderOptions {
    CredentialProviderOptions {
      configured: credential_provider::is_configured(),
      interactive: self.interactive,
      cancellation: self.cancellation.clone(),
      log_sink: self.credential_provider_log_sink,
    }
  }
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

/// Authentication attached to one effective package source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PackageSourceAuthentication {
  /// The source has no static credential.
  None,
  /// The source uses HTTP Basic authentication, including PAT-as-password feeds.
  Basic,
  /// The source presents an X.509 client certificate during TLS negotiation.
  ClientCertificate,
  /// The source combines HTTP Basic authentication with a client certificate.
  BasicAndClientCertificate,
}

impl PackageSourceAuthentication {
  /// Returns the stable event and human-output spelling.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::None => "none",
      Self::Basic => "basic",
      Self::ClientCertificate => "client_certificate",
      Self::BasicAndClientCertificate => "basic_and_client_certificate",
    }
  }
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
  authentication: PackageSourceAuthentication,
  security_flags: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageServiceEndpointRecord {
  location: TextSpan,
  kind: PackageServiceKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PackageSourceWorkRecord {
  downloaded_bytes: u64,
  duration_us: u64,
  name: TextSpan,
  requests: u32,
  protocol: NugetProtocol,
}

const _: () = assert!(size_of::<PackageSourceRecord>() == 28);
const _: () = assert!(align_of::<PackageSourceRecord>() == 4);
const _: () = assert!(size_of::<PackageServiceEndpointRecord>() == 12);
const _: () = assert!(align_of::<PackageServiceEndpointRecord>() == 4);
const _: () = assert!(size_of::<PackageSourceWorkRecord>() == 32);
const _: () = assert!(align_of::<PackageSourceWorkRecord>() == 8);

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
  source_work: Box<[SourceWork]>,
  http_policy: PackageHttpPolicy,
  network_requests: u32,
  downloaded_bytes: u64,
}

const _: () = assert!(size_of::<PackageSourceInventory>() == 96);
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

  /// Returns a credential-free configured source URL or local path.
  pub fn source_location(&self, source: usize) -> &str {
    self.get(self.sources[source].location)
  }

  /// Returns `local`, `v2`, or `v3`.
  pub fn source_protocol(&self, source: usize) -> &'static str {
    self.sources[source].protocol.as_str()
  }

  /// Returns the selected source authentication policy without exposing credentials.
  pub fn source_authentication(&self, source: usize) -> PackageSourceAuthentication {
    self.sources[source].authentication
  }

  /// Returns whether this source explicitly permits insecure HTTP transport.
  pub const fn source_allows_insecure_connections(&self, source: usize) -> bool {
    self.sources[source].security_flags & SOURCE_ALLOW_INSECURE_CONNECTIONS != 0
  }

  /// Returns whether this source validates TLS peers and hostnames.
  pub const fn source_tls_validation(&self, source: usize) -> bool {
    self.sources[source].security_flags & SOURCE_DISABLE_TLS_VALIDATION == 0
  }

  /// Returns the effective redacted transport policy used by source discovery.
  pub const fn http_policy(&self) -> PackageHttpPolicy {
    self.http_policy
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

  /// Returns actual HTTP attempts made against one source.
  pub fn source_requests(&self, source: usize) -> u32 {
    self.source_work[source].requests
  }

  /// Returns HTTP response-body or local archive bytes read from one source.
  pub fn source_downloaded_bytes(&self, source: usize) -> u64 {
    self.source_work[source].downloaded_bytes
  }

  /// Returns cumulative source-work microseconds for one source.
  pub fn source_duration_us(&self, source: usize) -> u64 {
    self.source_work[source].duration_us
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
  runtime_identifier: TextSpan,
  runtime_graph_fingerprint: TextSpan,
  source_name: TextSpan,
  source_location: TextSpan,
  prune_fingerprint: TextSpan,
  central_package_fingerprint: TextSpan,
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
  package_framework_assets: Box<[PackageFrameworkAssets]>,
  framework_items: Box<[TextSpan]>,
  direct_policies: Box<[DirectPackagePolicy]>,
  downgrades: Box<[PackageDowngrade]>,
  dependencies: Box<[u32]>,
  assets: Box<[TextSpan]>,
  asset_ranges: PackageAssetRanges,
  runtime_targets: Box<[RuntimeTargetAsset]>,
  content_file_metadata: Box<[ContentFileMetadata]>,
  source_work: Box<[PackageSourceWorkRecord]>,
  cache_hits: u32,
  downloaded_packages: u32,
  network_requests: u32,
  shared_metadata_hits: u32,
  downloaded_bytes: u64,
}

const _: () = assert!(size_of::<PackageResolution>() == 432);
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

  /// Returns the selected runtime identifier used for package assets.
  pub fn runtime_identifier(&self) -> Option<&str> {
    (self.runtime_identifier.len != 0).then(|| self.get(self.runtime_identifier))
  }

  /// Returns the credential-free configuration key for the selected source.
  pub fn source(&self) -> &str {
    self.get(self.source_name)
  }

  fn source_location(&self) -> &str {
    self.get(self.source_location)
  }

  /// Returns the selected NuGet protocol generation.
  pub fn source_protocol(&self) -> &'static str {
    self.source_protocol.as_str()
  }

  /// Returns configured sources in deterministic configuration order.
  pub fn source_work(&self) -> std::ops::Range<usize> {
    0..self.source_work.len()
  }

  /// Returns a credential-free configured source identity.
  pub fn source_work_name(&self, source: usize) -> &str {
    self.get(self.source_work[source].name)
  }

  /// Returns `local`, `v2`, or `v3` for one source-work row.
  pub fn source_work_protocol(&self, source: usize) -> &'static str {
    self.source_work[source].protocol.as_str()
  }

  /// Returns actual HTTP attempts made against one source.
  pub fn source_work_requests(&self, source: usize) -> u32 {
    self.source_work[source].requests
  }

  /// Returns HTTP response-body or local archive bytes read from one source.
  pub fn source_work_downloaded_bytes(&self, source: usize) -> u64 {
    self.source_work[source].downloaded_bytes
  }

  /// Returns cumulative source-work microseconds for one source.
  pub fn source_work_duration_us(&self, source: usize) -> u64 {
    self.source_work[source].duration_us
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

  /// Returns whether a transitive dependency was promoted by a central pin.
  pub fn package_is_central_transitive(&self, package: ResolvedPackage) -> bool {
    package.central_transitive
  }

  /// Returns the successful cache classification for a resolved package.
  pub fn package_cache_outcome(&self, package: ResolvedPackage) -> CacheOutcome {
    if package.cache_hit { CacheOutcome::Hit } else { CacheOutcome::Miss }
  }

  /// Iterates dependency package indices.
  pub fn package_dependencies(&self, package: ResolvedPackage) -> impl ExactSizeIterator<Item = u32> + '_ {
    let range = range(package.dependencies);
    self.dependencies[range].iter().copied()
  }

  /// Iterates sparse package-framework rows in package identity order.
  pub fn package_frameworks(&self) -> std::ops::Range<usize> {
    0..self.package_framework_assets.len()
  }

  /// Returns the resolved package index which owns one framework row.
  pub fn package_framework_package(&self, framework: usize) -> u32 {
    self.package_framework_assets[framework].package_index
  }

  /// Iterates shared-framework references selected from one package manifest.
  pub fn package_framework_references(&self, framework: usize) -> impl ExactSizeIterator<Item = &str> {
    let selected = self.package_framework_assets[framework];
    self.framework_items[range(selected.references)].iter().map(|span| self.get(*span))
  }

  /// Iterates legacy framework assembly names selected from one package manifest.
  pub fn package_framework_assemblies(&self, framework: usize) -> impl ExactSizeIterator<Item = &str> {
    let selected = self.package_framework_assets[framework];
    self.framework_items[range(selected.assemblies)].iter().map(|span| self.get(*span))
  }

  /// Iterates direct-reference policy rows in package identity order.
  pub fn direct_policies(&self) -> std::ops::Range<usize> {
    0..self.direct_policies.len()
  }

  /// Returns successful direct-wins downgrade warning rows.
  pub fn downgrades(&self) -> std::ops::Range<usize> {
    0..self.downgrades.len()
  }

  /// Returns the selected package identity for one downgrade.
  pub fn downgrade_package_id(&self, downgrade: usize) -> &str {
    self.get(self.downgrades[downgrade].package_id)
  }

  /// Returns the selected lower version for one downgrade.
  pub fn downgrade_selected_version(&self, downgrade: usize) -> &str {
    self.get(self.downgrades[downgrade].selected_version)
  }

  /// Returns the higher dependency range which was overridden.
  pub fn downgrade_requested_range(&self, downgrade: usize) -> &str {
    self.get(self.downgrades[downgrade].requested_range)
  }

  /// Returns the package which requested the overridden range.
  pub fn downgrade_requesting_package(&self, downgrade: usize) -> &str {
    self.get(self.downgrades[downgrade].requesting_package)
  }

  /// Returns the resolved package index owned by a direct policy row.
  pub fn direct_policy_package(&self, policy: usize) -> u32 {
    self.direct_policies[policy].package_index
  }

  /// Returns the effective asset families consumed through a direct reference.
  pub fn direct_policy_include_assets(&self, policy: usize) -> PackageAssetFlags {
    self.direct_policies[policy].include_assets
  }

  /// Returns the asset families hidden from consuming projects.
  pub fn direct_policy_private_assets(&self, policy: usize) -> PackageAssetFlags {
    self.direct_policies[policy].private_assets
  }

  /// Returns the package-scoped warning suppression list.
  pub fn direct_policy_no_warn(&self, policy: usize) -> Option<&str> {
    let span = self.direct_policies[policy].no_warn;
    (span.len != 0).then(|| self.get(span))
  }

  /// Returns compiler aliases applied to this package's compile assemblies.
  pub fn direct_policy_aliases(&self, policy: usize) -> Option<&str> {
    let span = self.direct_policies[policy].aliases;
    (span.len != 0).then(|| self.get(span))
  }

  /// Returns the generated MSBuild-compatible property and package root.
  pub fn direct_policy_path_property(&self, policy: usize) -> Option<(&str, &Path)> {
    let policy = self.direct_policies[policy];
    (policy.path_property.len != 0).then(|| {
      (
        self.get(policy.path_property),
        Path::new(self.get(self.package_roots[policy.package_index as usize])),
      )
    })
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

  /// Iterates selected content paths and their build metadata in path order.
  pub fn content_files_with_metadata(&self) -> impl ExactSizeIterator<Item = (&Path, &str, bool, bool)> {
    self
      .content_files()
      .zip(self.content_file_metadata.iter())
      .map(|(path, metadata)| (path, self.get(metadata.build_action), metadata.copy_to_output, metadata.flatten))
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

  #[cfg(test)]
  fn shared_metadata_hits(&self) -> u32 {
    self.shared_metadata_hits
  }

  /// Returns HTTP response-body and local source archive bytes read.
  pub fn downloaded_bytes(&self) -> u64 {
    self.downloaded_bytes
  }

  pub(crate) fn package_compile_assets(&self, index: usize) -> impl ExactSizeIterator<Item = &str> {
    let range = range(self.package_assets[index].compile);
    self.assets[range].iter().map(|span| self.get(*span))
  }

  pub(crate) fn package_compile_reference_range(&self, index: usize) -> std::ops::Range<usize> {
    let selected = self.package_assets[index].compile;
    let start = (selected.start - self.asset_ranges.compile.start) as usize;
    start..start + selected.len as usize
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

  fn package_content_files_with_metadata(&self, index: usize) -> impl ExactSizeIterator<Item = (&str, &str, bool, bool)> {
    let selected = self.package_extended_assets[index].content_files;
    let metadata_start = (selected.start - self.asset_ranges.content.start) as usize;
    let metadata_end = metadata_start + selected.len as usize;
    self
      .package_content_files(index)
      .zip(self.content_file_metadata[metadata_start..metadata_end].iter())
      .map(|(path, metadata)| (path, self.get(metadata.build_action), metadata.copy_to_output, metadata.flatten))
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
    let direct_count = self.direct_policies.len();
    self.target_framework() == project.target_framework()
      && self.runtime_identifier() == project.runtime_identifier()
      && self.lock_path() == project.project_directory().join("dv.lock.json")
      && self.get(self.central_package_fingerprint) == project.central_package_fingerprint()
      && direct_count == project.package_references().len()
      && project.package_references().iter().all(|reference| {
        let Ok(range) = VersionRange::parse(project.package_version(*reference)) else {
          return false;
        };
        let Some(package_index) = self.packages.iter().copied().position(|package| {
          package.direct
            && self.package_id(package).eq_ignore_ascii_case(project.package_id(*reference))
            && PackageVersion::parse(self.package_version(package)).is_ok_and(|version| range.contains(&version))
        }) else {
          return false;
        };
        let Some(policy) = self.direct_policies.iter().find(|policy| policy.package_index as usize == package_index) else {
          return false;
        };
        let policy_text = |span: TextSpan| (span.len != 0).then(|| self.get(span));
        policy.include_assets == project.package_effective_assets(*reference)
          && policy.private_assets == project.package_private_assets(*reference)
          && policy_text(policy.no_warn) == project.package_no_warn(*reference)
          && policy_text(policy.aliases) == project.package_aliases(*reference)
          && (policy.path_property.len != 0) == project.package_generate_path_property(*reference)
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
  /// Active dependency constraints have no common version.
  ConstraintConflict,
  /// A central package pin forces a lower or otherwise incompatible version.
  Downgrade,
  /// The dependency graph contains a cycle.
  DependencyCycle,
  /// No enabled source contains a requested package identity.
  PackageNotFound,
  /// Sources contain the identity but not a version in the requested range.
  VersionNotFound,
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
  /// A credential provider could not be discovered or violated its protocol.
  CredentialProvider,
  /// Package work was cooperatively cancelled.
  Cancelled,
  /// Package-source mapping selected no enabled source for an identity.
  UnmappedIdentity,
}

/// A package failure with stable path or source context.
#[derive(Debug)]
pub struct PackageError {
  kind: PackageErrorKind,
  context: String,
  message: String,
  diagnostic: Option<Box<PackageDiagnosticData>>,
  http_work: Option<HttpWork>,
  source_work: Vec<SourceWork>,
}

#[derive(Debug, Default)]
struct PackageDiagnosticData {
  context: Vec<(&'static str, String)>,
  causes: Vec<String>,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<PackageError>() <= 2 * BENCHMARK_CACHE_LINE_BYTES);

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
      diagnostic: None,
      http_work: None,
      source_work: Vec::new(),
    }
  }

  fn with_context(mut self, name: &'static str, value: impl Into<String>) -> Self {
    self
      .diagnostic
      .get_or_insert_with(|| Box::new(PackageDiagnosticData::default()))
      .context
      .push((name, value.into()));
    self
  }

  fn with_cause(mut self, cause: impl Into<String>) -> Self {
    self
      .diagnostic
      .get_or_insert_with(|| Box::new(PackageDiagnosticData::default()))
      .causes
      .push(cause.into());
    self
  }

  /// Iterates ordered machine-readable context retained on the cold error path.
  pub fn diagnostic_context(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
    self
      .diagnostic
      .as_ref()
      .map_or(&[][..], |diagnostic| diagnostic.context.as_slice())
      .iter()
      .map(|(name, value)| (*name, value.as_str()))
  }

  /// Iterates deterministic causal messages from nearest to root cause.
  pub fn causes(&self) -> impl ExactSizeIterator<Item = &str> {
    self
      .diagnostic
      .as_ref()
      .map_or(&[][..], |diagnostic| diagnostic.causes.as_slice())
      .iter()
      .map(String::as_str)
  }

  fn with_http_work(mut self, work: HttpWork) -> Self {
    debug_assert_eq!(self.kind, PackageErrorKind::Network);
    self.http_work = Some(work);
    self
  }

  fn take_http_work(&mut self) -> Option<HttpWork> {
    self.http_work.take()
  }

  fn with_source_work(mut self, work: Vec<SourceWork>) -> Self {
    debug_assert_eq!(self.kind, PackageErrorKind::Network);
    self.source_work = work;
    self
  }

  fn take_source_work(&mut self) -> Vec<SourceWork> {
    std::mem::take(&mut self.source_work)
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
  central_transitive: bool,
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

struct CentralPackagePin {
  lower_id: String,
  version: PackageVersion,
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

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<PackageVersion>() == 48);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<PackageVersion>() == align_of::<usize>());

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
  float_prefix_len: u16,
  inclusive: bool,
  float_behavior: VersionFloatBehavior,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
enum VersionFloatBehavior {
  #[default]
  None,
  Prerelease,
  Revision,
  Patch,
  Minor,
  Major,
  AbsoluteLatest,
  PrereleaseRevision,
  PrereleasePatch,
  PrereleaseMinor,
  PrereleaseMajor,
}

impl VersionFloatBehavior {
  fn includes_prerelease(self) -> bool {
    matches!(
      self,
      Self::Prerelease | Self::AbsoluteLatest | Self::PrereleaseRevision | Self::PrereleasePatch | Self::PrereleaseMinor | Self::PrereleaseMajor
    )
  }
}

impl VersionBound {
  fn new(version: PackageVersion, inclusive: bool) -> Self {
    Self {
      version,
      float_prefix_len: 0,
      inclusive,
      float_behavior: VersionFloatBehavior::None,
    }
  }

  fn floating(version: PackageVersion, inclusive: bool, float_behavior: VersionFloatBehavior, float_prefix_len: usize) -> Self {
    Self {
      version,
      float_prefix_len: float_prefix_len as u16,
      inclusive,
      float_behavior,
    }
  }

  fn float_prefix(&self) -> &str {
    &self.version.prerelease().expect("prerelease floating bounds have a release label")[..usize::from(self.float_prefix_len)]
  }

  fn float_satisfies(&self, version: &PackageVersion) -> bool {
    let stable = version.prerelease().is_none();
    match self.float_behavior {
      VersionFloatBehavior::None => false,
      VersionFloatBehavior::AbsoluteLatest => true,
      VersionFloatBehavior::Major => stable,
      VersionFloatBehavior::Minor => stable && self.version.numbers[0] == version.numbers[0],
      VersionFloatBehavior::Patch => stable && self.version.numbers[..2] == version.numbers[..2],
      VersionFloatBehavior::Revision => stable && self.version.numbers[..3] == version.numbers[..3],
      VersionFloatBehavior::Prerelease => {
        self.version.numbers == version.numbers && (stable || version.prerelease().is_some_and(|release| release.starts_with(self.float_prefix())))
      },
      VersionFloatBehavior::PrereleaseMajor => stable || version.prerelease().is_some_and(|release| release.starts_with(self.float_prefix())),
      VersionFloatBehavior::PrereleaseMinor => {
        self.version.numbers[0] == version.numbers[0] && (stable || version.prerelease().is_some_and(|release| release.starts_with(self.float_prefix())))
      },
      VersionFloatBehavior::PrereleasePatch => {
        self.version.numbers[..2] == version.numbers[..2] && (stable || version.prerelease().is_some_and(|release| release.starts_with(self.float_prefix())))
      },
      VersionFloatBehavior::PrereleaseRevision => {
        self.version.numbers[..3] == version.numbers[..3] && (stable || version.prerelease().is_some_and(|release| release.starts_with(self.float_prefix())))
      },
    }
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VersionRange {
  lower: Option<VersionBound>,
  upper: Option<VersionBound>,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<VersionBound>() == 56);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<VersionBound>() == align_of::<usize>());
#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<VersionRange>() == 112);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<VersionRange>() == align_of::<usize>());

impl VersionRange {
  #[cfg(test)]
  fn exact(version: PackageVersion) -> Self {
    Self {
      lower: Some(VersionBound::new(version.clone(), true)),
      upper: Some(VersionBound::new(version, true)),
    }
  }

  fn parse(value: &str) -> Result<Self, PackageError> {
    let value = value.trim();
    if value.is_empty() {
      return Err(unsupported_version_range(value));
    }
    if !value.starts_with(['[', '(']) {
      return Ok(Self {
        lower: Some(parse_version_bound(value, true, true)?),
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
        lower: Some(VersionBound::new(version.clone(), true)),
        upper: Some(VersionBound::new(version, true)),
      });
    }
    let (lower, upper) = body.split_once(',').expect("a checked range contains a comma");
    let lower = (!lower.trim().is_empty())
      .then(|| parse_version_bound(lower.trim(), lower_inclusive, true))
      .transpose()?;
    let upper = (!upper.trim().is_empty())
      .then(|| parse_version_bound(upper.trim(), upper_inclusive, false))
      .transpose()?;
    if lower.is_none() && upper.is_none() {
      return Err(unsupported_version_range(value));
    }
    if let (Some(lower), Some(upper)) = (&lower, &upper)
      && (lower.version > upper.version || (lower.version == upper.version && (!lower.inclusive || !upper.inclusive)))
    {
      return Err(PackageError::new(
        PackageErrorKind::Resolution,
        value,
        format!("dependency range {value:?} contains no versions"),
      ));
    }
    Ok(Self { lower, upper })
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
    self
      .lower
      .as_ref()
      .is_some_and(|bound| bound.float_behavior.includes_prerelease() || bound.version.prerelease().is_some())
      || self.upper.as_ref().is_some_and(|bound| bound.version.prerelease().is_some())
  }

  fn is_floating(&self) -> bool {
    self.lower.as_ref().is_some_and(|bound| bound.float_behavior != VersionFloatBehavior::None)
  }

  #[cfg(test)]
  fn floating_satisfies(&self, version: &PackageVersion) -> bool {
    self.lower.as_ref().is_some_and(|bound| bound.float_satisfies(version))
  }

  fn is_better(&self, current: &PackageVersion, considering: &PackageVersion) -> bool {
    let Some(lower) = self.lower.as_ref().filter(|bound| bound.float_behavior != VersionFloatBehavior::None) else {
      return current > considering;
    };
    let current_floats = lower.float_satisfies(current);
    let considering_floats = lower.float_satisfies(considering);
    match (current_floats, considering_floats) {
      (true, false) => false,
      (false, true) => true,
      (true, true) => current < considering,
      (false, false) => match (current < &lower.version, considering < &lower.version) {
        (true, false) => true,
        (false, true) => false,
        (false, false) => current > considering,
        (true, true) => current < considering,
      },
    }
  }

  fn diagnostic_text(&self) -> String {
    if let (Some(lower), Some(upper)) = (&self.lower, &self.upper)
      && lower.inclusive
      && upper.inclusive
      && lower.version == upper.version
    {
      return format!("[{}]", lower.version.normalized);
    }
    if self.is_floating() {
      return format!(
        "{} (floating)",
        self.lower.as_ref().expect("a floating range has a lower bound").version.normalized
      );
    }
    let mut text = String::new();
    text.push(if self.lower.as_ref().is_some_and(|bound| bound.inclusive) { '[' } else { '(' });
    if let Some(lower) = &self.lower {
      text.push_str(&lower.version.normalized);
    }
    text.push(',');
    if let Some(upper) = &self.upper {
      text.push_str(&upper.version.normalized);
    }
    text.push(if self.upper.as_ref().is_some_and(|bound| bound.inclusive) { ']' } else { ')' });
    text
  }
}

fn parse_version_bound(value: &str, inclusive: bool, allow_floating: bool) -> Result<VersionBound, PackageError> {
  if !value.contains('*') {
    return Ok(VersionBound::new(PackageVersion::parse(value)?, inclusive));
  }
  if !allow_floating {
    return Err(unsupported_version_range(value));
  }
  let (version, behavior, prefix_len) = parse_floating_version(value)?;
  Ok(VersionBound::floating(version, inclusive, behavior, prefix_len))
}

fn parse_floating_version(value: &str) -> Result<(PackageVersion, VersionFloatBehavior, usize), PackageError> {
  if value.len() > 256 || value.contains('+') || !value.ends_with('*') {
    return Err(unsupported_version_range(value));
  }
  if value == "*" {
    return Ok((PackageVersion::parse("0.0.0")?, VersionFloatBehavior::Major, 0));
  }
  if value == "*-*" {
    return Ok((PackageVersion::parse("0.0.0-0")?, VersionFloatBehavior::AbsoluteLatest, 0));
  }

  let first_star = value.find('*').expect("a floating version contains a wildcard");
  let last_star = value.rfind('*').expect("a floating version contains a wildcard");
  if first_star != last_star {
    let dash = value.find('-').ok_or_else(|| unsupported_version_range(value))?;
    if first_star + 1 != dash || last_star + 1 != value.len() {
      return Err(unsupported_version_range(value));
    }
    let mut numeric = value[..first_star].to_owned();
    numeric.push('0');
    let behavior = match numeric.bytes().filter(|byte| *byte == b'.').count() + 1 {
      1 => VersionFloatBehavior::PrereleaseMajor,
      2 => VersionFloatBehavior::PrereleaseMinor,
      3 => VersionFloatBehavior::PrereleasePatch,
      4 => VersionFloatBehavior::PrereleaseRevision,
      _ => return Err(unsupported_version_range(value)),
    };
    let prefix = &value[dash + 1..last_star];
    let mut actual = numeric;
    actual.push('-');
    actual.push_str(prefix);
    if prefix.is_empty() || prefix.ends_with('.') {
      actual.push('0');
    }
    return Ok((
      PackageVersion::parse(&actual).map_err(|_| unsupported_version_range(value))?,
      behavior,
      prefix.len(),
    ));
  }

  let mut actual = value[..last_star].to_owned();
  if let Some(dash) = value.find('-') {
    let explicit_prefix = (dash == value.rfind('-').expect("a checked prerelease has a dash")).then_some(&value[dash + 1..last_star]);
    if explicit_prefix.is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('.')) {
      actual.push('0');
    }
    let version = PackageVersion::parse(&actual).map_err(|_| unsupported_version_range(value))?;
    let prefix_len = explicit_prefix.map_or_else(|| version.prerelease().map_or(0, str::len), str::len);
    return Ok((version, VersionFloatBehavior::Prerelease, prefix_len));
  }

  actual.push('0');
  let behavior = match actual.bytes().filter(|byte| *byte == b'.').count() + 1 {
    1 => VersionFloatBehavior::None,
    2 => VersionFloatBehavior::Minor,
    3 => VersionFloatBehavior::Patch,
    4 => VersionFloatBehavior::Revision,
    _ => return Err(unsupported_version_range(value)),
  };
  Ok((PackageVersion::parse(&actual).map_err(|_| unsupported_version_range(value))?, behavior, 0))
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
  content_files: Vec<WorkContentFile>,
  content_actions: Vec<String>,
  build_assets: Vec<PathBuf>,
  build_multi_targeting_assets: Vec<PathBuf>,
  build_transitive_assets: Vec<PathBuf>,
  native_assets: Vec<PathBuf>,
  runtime_targets: Vec<WorkRuntimeTarget>,
  framework_references: Vec<String>,
  framework_assemblies: Vec<String>,
  cache_hit: bool,
  origin: Option<PackageSource>,
}

struct WorkRuntimeTarget {
  path: PathBuf,
  runtime_identifier: String,
  kind: RuntimeTargetKind,
}

struct RuntimeAssetSelection {
  targets: Vec<WorkRuntimeTarget>,
  runtime: Option<Vec<PathBuf>>,
  resources: Option<Vec<PathBuf>>,
  native: Option<Vec<PathBuf>>,
}

#[derive(Clone, Copy)]
struct PackageAssetContext<'a> {
  target: TargetFramework,
  target_text: &'a str,
  runtime_identifier: Option<&'a str>,
  runtime_graph: Option<&'a RuntimeIdentifierGraph>,
  flags: AssetFlags,
}

struct WorkContentFile {
  path: PathBuf,
  build_action: u32,
  copy_to_output: bool,
  flatten: bool,
}

struct ResolutionContext<'a> {
  project: &'a ProjectSpec,
  direct: &'a [PackageRequirement],
  cache_root: &'a Path,
  http_cache_root: &'a Path,
  temp_root: &'a Path,
  fallback_roots: &'a [PathBuf],
  lock_path: &'a Path,
  target_framework: &'a str,
  runtime_identifier: Option<&'a str>,
  runtime_graph_fingerprint: &'a str,
  source_name: &'a str,
  source_location: &'a str,
  sources: &'a [(String, PackageSource)],
  prune_fingerprint: &'a str,
  central_package_fingerprint: &'a str,
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct PackageFrameworkMetadata {
  references: Vec<String>,
  assemblies: Vec<String>,
}

impl PackageFrameworkMetadata {
  fn is_empty(&self) -> bool {
    self.references.is_empty() && self.assemblies.is_empty()
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ContentFileRule {
  excludes: ItemRange,
  include_pattern: u32,
  build_action: u32,
  values: u8,
  present: u8,
}

const _: () = assert!(size_of::<ContentFileRule>() == 20);
const _: () = assert!(align_of::<ContentFileRule>() == 4);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ContentFileRules {
  patterns: Vec<String>,
  actions: Vec<String>,
  rules: Vec<ContentFileRule>,
  present: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct PackageColdMetadata {
  frameworks: PackageFrameworkMetadata,
  content_rules: ContentFileRules,
}

impl PackageColdMetadata {
  fn is_empty(&self) -> bool {
    self.frameworks.is_empty() && !self.content_rules.present
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedPackageMetadata {
  dependencies: Vec<PackageRequirement>,
  cold: PackageColdMetadata,
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
  metadata: Option<ParsedPackageMetadata>,
  cache_hit: bool,
  source_work: Option<SourceWork>,
  failed_source_work: Box<[SourceWork]>,
  origin: Option<PackageSource>,
}

struct ResolvedGraph {
  packages: BTreeMap<String, WorkPackage>,
  source_work: Vec<SourceWork>,
  downgrades: Vec<ResolvedDowngrade>,
  shared_metadata_hits: u32,
}

struct ResolvedDowngrade {
  package_id: String,
  selected_version: String,
  requested_range: String,
  requesting_package: String,
}

struct GraphRoots<'a> {
  direct: &'a [PackageRequirement],
  central_pins: &'a [CentralPackagePin],
}

struct GraphContext<'a> {
  config: &'a NugetConfiguration,
  options: &'a PackageResolveOptions,
  target: TargetFramework,
  target_text: &'a str,
  runtime_identifier: Option<&'a str>,
  runtime_graph: Option<&'a RuntimeIdentifierGraph>,
  pruning: &'a PackagePruning,
  batch_metadata: &'a mut BatchMetadataCache,
}

/// One command-local parsed dependency row. The sorted batch is searched by
/// target, identity, then exact version. Each row is 40 bytes; eight rows span
/// five assumed 64-byte cache lines. Identity/version text shares one scope
/// buffer, while variable external dependencies own one boxed batch and are
/// cloned only into a project's independently mutable graph.
struct BatchMetadataEntry {
  target: TargetFramework,
  lower_id: TextSpan,
  version: TextSpan,
  dependencies: Box<[PackageRequirement]>,
}

const _: () = assert!(size_of::<BatchMetadataEntry>() == 40);
const _: () = assert!(align_of::<BatchMetadataEntry>() == align_of::<usize>());

struct BatchMetadataScope {
  cache_root: PathBuf,
  fallback_roots: Arc<[PathBuf]>,
  text: TextTable,
  entries: Vec<BatchMetadataEntry>,
  cold_indices: Vec<u32>,
  cold: Vec<PackageColdMetadata>,
  packages: Vec<Option<BatchCachedPackage>>,
}

struct BatchCachedPackage {
  root: PathBuf,
  hash: String,
  origin: Option<PackageSource>,
}

#[cfg(all(target_pointer_width = "64", target_os = "windows"))]
const _: () = assert!(size_of::<BatchCachedPackage>() == 88);
#[cfg(all(target_pointer_width = "64", not(target_os = "windows")))]
const _: () = assert!(size_of::<BatchCachedPackage>() == 80);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<BatchCachedPackage>() == align_of::<usize>());

impl BatchCachedPackage {
  fn from_cached(package: &CachedPackage) -> Self {
    Self {
      root: package.root.clone(),
      hash: package.hash.clone(),
      origin: package.origin.clone(),
    }
  }

  fn from_task(package: &TaskCachedPackage) -> Self {
    Self {
      root: package.root.clone(),
      hash: package.hash.clone(),
      origin: package.origin.clone(),
    }
  }

  fn materialize(&self) -> CachedPackage {
    CachedPackage {
      root: self.root.clone(),
      hash: self.hash.clone(),
      metadata: None,
      cache_hit: true,
      source_work: None,
      failed_source_work: Box::new([]),
      origin: self.origin.clone(),
    }
  }
}

struct TaskCachedPackage {
  root: PathBuf,
  hash: String,
  origin: Option<PackageSource>,
  cache_hit: bool,
}

#[cfg(all(target_pointer_width = "64", target_os = "windows"))]
const _: () = assert!(size_of::<TaskCachedPackage>() == 96);
#[cfg(all(target_pointer_width = "64", not(target_os = "windows")))]
const _: () = assert!(size_of::<TaskCachedPackage>() == 88);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<TaskCachedPackage>() == align_of::<usize>());

impl TaskCachedPackage {
  fn from_cached(package: CachedPackage) -> Self {
    Self {
      root: package.root,
      hash: package.hash,
      origin: package.origin,
      cache_hit: package.cache_hit,
    }
  }

  fn materialize(self) -> CachedPackage {
    CachedPackage {
      root: self.root,
      hash: self.hash,
      metadata: None,
      cache_hit: self.cache_hit,
      source_work: None,
      failed_source_work: Box::new([]),
      origin: self.origin,
    }
  }
}

#[derive(Default)]
struct BatchMetadataCache {
  scopes: Vec<BatchMetadataScope>,
}

impl BatchMetadataCache {
  fn scope_mut(&mut self, config: &NugetConfiguration) -> &mut BatchMetadataScope {
    let index = self
      .scopes
      .iter()
      .position(|scope| scope.cache_root == config.cache_root && scope.fallback_roots == config.fallback_roots)
      .unwrap_or_else(|| {
        self.scopes.push(BatchMetadataScope {
          cache_root: config.cache_root.clone(),
          fallback_roots: Arc::clone(&config.fallback_roots),
          text: TextTable::with_capacity(0),
          entries: Vec::new(),
          cold_indices: Vec::new(),
          cold: Vec::new(),
          packages: Vec::new(),
        });
        self.scopes.len() - 1
      });
    &mut self.scopes[index]
  }

  fn get(&mut self, config: &NugetConfiguration, target: TargetFramework, lower_id: &str, version: &str) -> Option<ParsedPackageMetadata> {
    let scope = self.scope_mut(config);
    let index = scope
      .entries
      .binary_search_by(|entry| {
        entry
          .target
          .cmp(&target)
          .then_with(|| scope.text.get(entry.lower_id).cmp(lower_id))
          .then_with(|| scope.text.get(entry.version).cmp(version))
      })
      .ok()?;
    let cold_index = scope.cold_indices[index];
    let cold = if cold_index == u32::MAX {
      PackageColdMetadata::default()
    } else {
      scope.cold[cold_index as usize].clone()
    };
    Some(ParsedPackageMetadata {
      dependencies: scope.entries[index].dependencies.to_vec(),
      cold,
    })
  }

  fn package(&mut self, config: &NugetConfiguration, target: TargetFramework, lower_id: &str, version: &str) -> Option<CachedPackage> {
    let scope = self.scope_mut(config);
    let index = scope
      .entries
      .binary_search_by(|entry| {
        entry
          .target
          .cmp(&target)
          .then_with(|| scope.text.get(entry.lower_id).cmp(lower_id))
          .then_with(|| scope.text.get(entry.version).cmp(version))
      })
      .ok()?;
    scope.packages[index].as_ref().map(BatchCachedPackage::materialize)
  }

  fn insert(
    &mut self,
    config: &NugetConfiguration,
    target: TargetFramework,
    lower_id: &str,
    version: &str,
    metadata: &ParsedPackageMetadata,
    package: Option<&TaskCachedPackage>,
  ) -> Result<(), PackageError> {
    let scope = self.scope_mut(config);
    let index = scope.entries.binary_search_by(|entry| {
      entry
        .target
        .cmp(&target)
        .then_with(|| scope.text.get(entry.lower_id).cmp(lower_id))
        .then_with(|| scope.text.get(entry.version).cmp(version))
    });
    if let Ok(index) = index {
      if scope.packages[index].is_none() {
        scope.packages[index] = package.map(BatchCachedPackage::from_task);
      }
      return Ok(());
    }
    let index = index.expect_err("an absent batch metadata key has an insertion index");
    let lower_id = scope.text.push(lower_id)?;
    let version = scope.text.push(version)?;
    scope.entries.insert(
      index,
      BatchMetadataEntry {
        target,
        lower_id,
        version,
        dependencies: metadata.dependencies.as_slice().into(),
      },
    );
    let cold_index = if metadata.cold.is_empty() {
      u32::MAX
    } else {
      let index = u32::try_from(scope.cold.len()).map_err(|_| {
        PackageError::new(
          PackageErrorKind::TextOverflow,
          "package cold metadata",
          "package cold metadata count exceeds u32",
        )
      })?;
      scope.cold.push(metadata.cold.clone());
      index
    };
    scope.cold_indices.insert(index, cold_index);
    scope.packages.insert(index, package.map(BatchCachedPackage::from_task));
    Ok(())
  }

  fn insert_package(&mut self, config: &NugetConfiguration, target: TargetFramework, lower_id: &str, version: &str, package: &CachedPackage) {
    let scope = self.scope_mut(config);
    if let Ok(index) = scope.entries.binary_search_by(|entry| {
      entry
        .target
        .cmp(&target)
        .then_with(|| scope.text.get(entry.lower_id).cmp(lower_id))
        .then_with(|| scope.text.get(entry.version).cmp(version))
    }) {
      scope.packages[index] = Some(BatchCachedPackage::from_cached(package));
    }
  }
}

#[derive(Default)]
struct PackageBatchContext {
  runtime: Option<tokio::runtime::Runtime>,
  metadata: BatchMetadataCache,
}

/// Cold graph state is identity-ordered and owned by the resolver task. Parent
/// identities key constraints so replacing a package version can retract its
/// previous edges without retaining an object graph.
struct ConstraintNode {
  id: String,
  direct: Option<VersionRange>,
  central_pin: Option<PackageVersion>,
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

#[derive(Clone, Copy)]
enum ConstraintView<'a> {
  All(&'a BTreeMap<String, VersionRange>),
  Active(&'a [&'a VersionRange]),
}

impl<'a> ConstraintView<'a> {
  fn any(self, predicate: impl FnMut(&VersionRange) -> bool) -> bool {
    match self {
      Self::All(constraints) => constraints.values().any(predicate),
      Self::Active(constraints) => constraints.iter().copied().any(predicate),
    }
  }

  fn all(self, predicate: impl FnMut(&VersionRange) -> bool) -> bool {
    match self {
      Self::All(constraints) => constraints.values().all(predicate),
      Self::Active(constraints) => constraints.iter().copied().all(predicate),
    }
  }

  fn first(self, mut predicate: impl FnMut(&VersionRange) -> bool) -> Option<&'a VersionRange> {
    match self {
      Self::All(constraints) => constraints.values().find(|range| predicate(range)),
      Self::Active(constraints) => constraints.iter().copied().find(|range| predicate(range)),
    }
  }

  fn highest_inclusive_lower(self) -> Option<&'a PackageVersion> {
    let mut candidate = None::<&'a PackageVersion>;
    let mut consider = |range: &'a VersionRange| {
      if let Some(lower) = &range.lower
        && lower.inclusive
        && candidate.is_none_or(|candidate| lower.version > *candidate)
      {
        candidate = Some(&lower.version);
      }
    };
    match self {
      Self::All(constraints) => constraints.values().for_each(&mut consider),
      Self::Active(constraints) => constraints.iter().copied().for_each(consider),
    }
    candidate
  }

  fn has_empty_intersection(self) -> bool {
    let mut lower = None::<&VersionBound>;
    let mut upper = None::<&VersionBound>;
    let mut inspect = |range: &'a VersionRange| {
      if let Some(candidate) = &range.lower
        && lower
          .is_none_or(|current| candidate.version > current.version || (candidate.version == current.version && !candidate.inclusive && current.inclusive))
      {
        lower = Some(candidate);
      }
      if let Some(candidate) = &range.upper
        && upper
          .is_none_or(|current| candidate.version < current.version || (candidate.version == current.version && !candidate.inclusive && current.inclusive))
      {
        upper = Some(candidate);
      }
    };
    match self {
      Self::All(constraints) => constraints.values().for_each(&mut inspect),
      Self::Active(constraints) => constraints.iter().copied().for_each(inspect),
    }
    matches!((lower, upper), (Some(lower), Some(upper)) if lower.version > upper.version || (lower.version == upper.version && (!lower.inclusive || !upper.inclusive)))
  }

  fn diagnostic_ranges(self) -> String {
    let mut text = String::new();
    let mut append = |range: &'a VersionRange| {
      if !text.is_empty() {
        text.push_str("; ");
      }
      text.push_str(&range.diagnostic_text());
    };
    match self {
      Self::All(constraints) => constraints.values().for_each(&mut append),
      Self::Active(constraints) => constraints.iter().copied().for_each(append),
    }
    text
  }
}

// Inline metadata avoids one heap allocation per package. The scheduler keeps
// no more than MAX_DOWNLOAD_WORKERS results live concurrently.
#[allow(clippy::large_enum_variant)]
enum MetadataTaskResult {
  Requirements {
    metadata: ParsedPackageMetadata,
    source_work: Option<SourceWork>,
    failed_source_work: Box<[SourceWork]>,
    package: Option<TaskCachedPackage>,
  },
  Versions {
    versions: Vec<PackageVersion>,
    source_work: Vec<SourceWork>,
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
  security_flags: u8,
}

const _: () = assert!(size_of::<PackageSource>() == 32);
const _: () = assert!(align_of::<PackageSource>() == 8);

impl PackageSource {
  fn parse(
    value: String,
    protocol: Option<&str>,
    allow_insecure_connections: bool,
    disable_tls_validation: bool,
    context: &Path,
    relative_to: &Path,
  ) -> Result<Self, PackageError> {
    if value.trim().is_empty() {
      return Err(config_error(context, "package source cannot be empty"));
    }
    let is_https = value.get(..8).is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"));
    let is_http = value.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"));
    if is_https || is_http {
      let parsed = reqwest::Url::parse(&value).map_err(|error| config_error(context, format!("invalid HTTP package source {value:?}: {error}")))?;
      if !parsed.has_host() {
        return Err(config_error(context, format!("HTTP package source {value:?} must include a host")));
      }
      if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(config_error(
          context,
          "package-source URLs must not embed credentials; use packageSourceCredentials or NuGetPackageSourceCredentials_{name}",
        ));
      }
      if is_http && !allow_insecure_connections {
        return Err(config_error(
          context,
          format!("insecure HTTP package source {value:?} requires allowInsecureConnections=true"),
        ));
      }
      return Ok(Self {
        protocol: NugetProtocol::parse_http(protocol, &value, context)?,
        url: value,
        security_flags: source_security_flags(allow_insecure_connections, disable_tls_validation),
      });
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
    let path = if value.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://")) {
      reqwest::Url::parse(&value)
        .map_err(|error| config_error(context, format!("invalid local package-source URI {value:?}: {error}")))?
        .to_file_path()
        .map_err(|()| config_error(context, format!("local package-source URI {value:?} does not identify a filesystem path")))?
    } else {
      if value.contains("://") {
        return Err(config_error(
          context,
          format!("package source {value:?} must be HTTP, HTTPS, file://, or a local folder path"),
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
      security_flags: source_security_flags(allow_insecure_connections, disable_tls_validation),
    })
  }

  const fn allow_insecure_connections(&self) -> bool {
    self.security_flags & SOURCE_ALLOW_INSECURE_CONNECTIONS != 0
  }

  const fn tls_validation(&self) -> bool {
    self.security_flags & SOURCE_DISABLE_TLS_VALIDATION == 0
  }
}

const fn source_security_flags(allow_insecure_connections: bool, disable_tls_validation: bool) -> u8 {
  (if allow_insecure_connections { SOURCE_ALLOW_INSECURE_CONNECTIONS } else { 0 }) | if disable_tls_validation { SOURCE_DISABLE_TLS_VALIDATION } else { 0 }
}

enum StoredCredentialPassword {
  Clear(Zeroizing<String>),
  Encrypted(String),
}

struct MergedSourceCredential {
  source: String,
  username: Zeroizing<String>,
  password: StoredCredentialPassword,
  valid_authentication_types: Option<String>,
}

enum MergedClientCertificate {
  File {
    source: String,
    path: PathBuf,
    password: Option<StoredCredentialPassword>,
  },
  Store {
    source: String,
    location: String,
    name: String,
    find_by: String,
    find_value: String,
  },
}

impl MergedClientCertificate {
  fn source(&self) -> &str {
    match self {
      Self::File { source, .. } | Self::Store { source, .. } => source,
    }
  }

  const fn is_file(&self) -> bool {
    matches!(self, Self::File { .. })
  }
}

struct PendingSourceCredential {
  source: String,
  username: Option<Zeroizing<String>>,
  password: Option<StoredCredentialPassword>,
  valid_authentication_types: Option<String>,
}

struct SourceCredential {
  authorization: Option<HeaderValue>,
  origin: HttpOrigin,
  provider: Option<Box<SourceProviderCredential>>,
  client: Option<reqwest::Client>,
  transport_client: Option<reqwest::Client>,
  global_limiter: Option<Arc<Semaphore>>,
  source_limiter: Option<Arc<Semaphore>>,
  http_policy: PackageHttpPolicy,
  security_flags: u8,
}

struct SourceProviderCredential {
  source: Box<str>,
  provider_options: CredentialProviderOptions,
  acquired: AtomicBool,
  state: Mutex<ProviderCredentialState>,
}

#[derive(Default)]
struct ProviderCredentialState {
  authorization: Option<HeaderValue>,
  provider_index: Option<usize>,
  generation: u32,
}

#[cfg(target_pointer_width = "64")]
const _: () = {
  assert!(size_of::<HttpOrigin>() == 24);
  assert!(align_of::<HttpOrigin>() == 8);
  assert!(size_of::<SourceCredential>() == 128);
  assert!(align_of::<SourceCredential>() == 8);
};

impl SourceCredential {
  #[cfg(test)]
  fn authorization(&self) -> Option<&HeaderValue> {
    self.authorization.as_ref()
  }

  fn authentication(&self) -> PackageSourceAuthentication {
    let basic = self.authorization.is_some() || self.provider.as_deref().is_some_and(|provider| provider.acquired.load(AtomicOrdering::Acquire));
    match (basic, self.client.is_some()) {
      (false, false) => PackageSourceAuthentication::None,
      (true, false) => PackageSourceAuthentication::Basic,
      (false, true) => PackageSourceAuthentication::ClientCertificate,
      (true, true) => PackageSourceAuthentication::BasicAndClientCertificate,
    }
  }

  async fn authorization_snapshot(&self) -> (Option<HeaderValue>, u32, bool) {
    let Some(provider) = &self.provider else {
      return (self.authorization.clone(), 0, false);
    };
    let state = provider.state.lock().await;
    match &state.authorization {
      Some(authorization) => (Some(authorization.clone()), state.generation, true),
      None => (self.authorization.clone(), state.generation, false),
    }
  }

  async fn acquire_provider(&self, observed_generation: u32, is_retry: bool) -> Result<Option<HeaderValue>, PackageError> {
    let Some(provider) = &self.provider else {
      return Ok(None);
    };
    let mut state = provider.state.lock().await;
    if state.generation != observed_generation {
      return Ok(state.authorization.clone());
    }
    let acquired = credential_provider::acquire(&provider.source, &provider.provider_options, is_retry, state.provider_index)
      .await
      .map_err(package_credential_provider_error)?;
    let Some(acquired) = acquired else {
      return Ok(None);
    };
    state.authorization = Some(acquired.authorization.clone());
    state.provider_index = Some(acquired.provider_index);
    state.generation = state.generation.wrapping_add(1);
    provider.acquired.store(true, AtomicOrdering::Release);
    Ok(Some(acquired.authorization))
  }
}

#[derive(Eq, PartialEq)]
struct HttpOrigin {
  host: Box<str>,
  port: u16,
  secure: bool,
}

impl HttpOrigin {
  fn parse(url: &str, context: &Path) -> Result<Self, PackageError> {
    let url = reqwest::Url::parse(url).map_err(|error| config_error(context, format!("invalid HTTP package source {url:?}: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
      return Err(config_error(context, format!("package source {url:?} must use HTTP or HTTPS")));
    }
    let host = url
      .host_str()
      .ok_or_else(|| config_error(context, format!("HTTP package source {url:?} must include a host")))?;
    Ok(Self {
      host: host.to_ascii_lowercase().into_boxed_str(),
      port: url.port_or_known_default().expect("HTTP and HTTPS have known default ports"),
      secure: url.scheme() == "https",
    })
  }

  fn matches(&self, url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
      return false;
    };
    url.scheme() == if self.secure { "https" } else { "http" }
      && url.host_str().is_some_and(|host| host.eq_ignore_ascii_case(&self.host))
      && url.port_or_known_default() == Some(self.port)
  }
}

#[derive(Default)]
struct SourceCredentialBatch {
  entries: Box<[Option<Arc<SourceCredential>>]>,
}

impl SourceCredentialBatch {
  fn get(&self, source: usize) -> Option<&Arc<SourceCredential>> {
    self.entries.get(source).and_then(Option::as_ref)
  }

  fn authentication(&self, source: usize) -> PackageSourceAuthentication {
    self
      .get(source)
      .map_or(PackageSourceAuthentication::None, |credential| credential.authentication())
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
    credential: Option<Arc<SourceCredential>>,
    source_index: u32,
  },
  V3 {
    source: String,
    services: Arc<NugetServiceEndpoints>,
    credential: Option<Arc<SourceCredential>>,
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

  fn credential(&self) -> Option<&SourceCredential> {
    match self {
      Self::Local { .. } => None,
      Self::V2 { credential, .. } | Self::V3 { credential, .. } => credential.as_deref(),
    }
  }
}

struct NugetConfiguration {
  cache_root: PathBuf,
  http_cache_root: PathBuf,
  temp_root: PathBuf,
  fallback_roots: Arc<[PathBuf]>,
  sources: Vec<(String, PackageSource)>,
  credentials: SourceCredentialBatch,
  // Audit resolution consumes this batch without reopening configuration
  // files once vulnerability endpoints are wired into restore.
  #[allow(dead_code)]
  audit_sources: Vec<(String, PackageSource)>,
  source_mapping: Option<Arc<PackageSourceMapping>>,
  signature_validation: SignatureValidationMode,
  signature_policy: Arc<SignaturePolicy>,
  proxy: Option<ProxySettings>,
  http_policy: PackageHttpPolicy,
}

#[derive(Default)]
struct NugetConfigMerge {
  sources: Vec<(String, PackageSource)>,
  credentials: Vec<MergedSourceCredential>,
  client_certificates: Vec<MergedClientCertificate>,
  disabled: Vec<String>,
  audit_sources: Vec<(String, PackageSource)>,
  source_mapping: MergedSourceMapping,
  global_packages: Option<PathBuf>,
  fallback_folders: Vec<FallbackFolder>,
  config_priority: u32,
  signature_validation: Option<SignatureValidationMode>,
  trusted_signers: Vec<TrustedSigner>,
  proxy_url: Option<String>,
  proxy_user: Option<String>,
  proxy_password: Option<String>,
  no_proxy: Option<String>,
  max_http_requests_per_source: Option<String>,
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
  username: Option<Zeroizing<String>>,
  password: Option<Zeroizing<String>>,
}

struct ProxyCredential {
  username: Zeroizing<String>,
  password: Zeroizing<String>,
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

const _: () = {
  assert!(size_of::<PackageSourceMapping>() == 48);
  assert!(align_of::<PackageSourceMapping>() == 8);
  assert!(size_of::<SourceMappingEntry>() == 12);
  assert!(align_of::<SourceMappingEntry>() == 4);
  assert!(size_of::<SourcePattern>() == 12);
  assert!(align_of::<SourcePattern>() == 4);
};

struct PendingSourceMapping {
  source: String,
  pattern_start: usize,
}

#[derive(Serialize, Deserialize)]
struct LockFile {
  schema_version: u16,
  target_framework: String,
  #[serde(default)]
  runtime_identifier: Option<String>,
  #[serde(default)]
  runtime_graph_fingerprint: String,
  source: String,
  source_protocol: NugetProtocol,
  #[serde(default)]
  prune_fingerprint: String,
  #[serde(default)]
  central_package_fingerprint: String,
  direct: Vec<LockDirect>,
  packages: Vec<LockPackage>,
  #[serde(default)]
  downgrades: Vec<LockDowngrade>,
}

#[derive(Serialize, Deserialize, Eq, PartialEq)]
struct LockDirect {
  id: String,
  version: String,
  include_assets: u8,
}

#[derive(Serialize, Deserialize, Eq, PartialEq)]
struct LockDependency {
  id: String,
  version: String,
}

#[derive(Serialize, Deserialize)]
struct LockDowngrade {
  package_id: String,
  selected_version: String,
  requested_range: String,
  requesting_package: String,
}

#[derive(Serialize, Deserialize)]
struct LockPackage {
  id: String,
  version: String,
  sha512: String,
  direct: bool,
  #[serde(default)]
  central_transitive: bool,
  dependencies: Vec<LockDependency>,
  compile_assets: Vec<String>,
  runtime_assets: Vec<String>,
  analyzers: Vec<String>,
  #[serde(default)]
  resource_assets: Vec<String>,
  #[serde(default)]
  content_files: Vec<LockContentFile>,
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
  #[serde(default)]
  framework_references: Vec<String>,
  #[serde(default)]
  framework_assemblies: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct LockRuntimeTarget {
  path: String,
  runtime_identifier: String,
  kind: RuntimeTargetKind,
}

#[derive(Serialize, Deserialize)]
struct LockContentFile {
  path: String,
  build_action: String,
  copy_to_output: bool,
  flatten: bool,
}

/// Resolves exact package graphs for an evaluated project batch.
///
/// Restore supplies one root-first project-reference closure. Empty or
/// package-free projects do not read configuration, inspect caches, or access
/// the network. Cold projects share one runtime and parsed metadata session.
pub fn resolve_package_inputs(projects: &[&ProjectSpec], options: &PackageResolveOptions) -> Result<Vec<PackageResolution>, PackageError> {
  let mut resolutions = Vec::with_capacity(projects.len());
  let mut batch = PackageBatchContext::default();
  for project in projects {
    if project.package_references().is_empty() {
      resolutions.push(empty_resolution(project)?);
      continue;
    }
    let inventory = discover_project_sdk(project)?;
    let runtime_graph = project
      .runtime_identifier()
      .map(|_| {
        load_portable_runtime_graph(inventory.as_ref().expect("RID projects discover an SDK")).map_err(|error| {
          PackageError::new(
            PackageErrorKind::Configuration,
            project.project_path().display().to_string(),
            format!("failed to load the selected SDK runtime graph: {error}"),
          )
        })
      })
      .transpose()?;
    resolutions.push(resolve_project(project, options, runtime_graph.as_ref(), inventory.as_ref(), &mut batch)?);
  }
  Ok(resolutions)
}

/// Resolves package graphs with the selected SDK's portable RID graph.
///
/// The graph is required only when a package-bearing project selects one
/// runtime identifier. Portable projects never read or traverse it.
pub fn resolve_package_inputs_with_runtime_graph(
  projects: &[&ProjectSpec],
  options: &PackageResolveOptions,
  runtime_graph: Option<&RuntimeIdentifierGraph>,
  inventory: Option<&SdkInventory>,
) -> Result<Vec<PackageResolution>, PackageError> {
  let mut resolutions = Vec::with_capacity(projects.len());
  let mut batch = PackageBatchContext::default();
  for project in projects {
    if project.package_references().is_empty() {
      resolutions.push(empty_resolution(project)?);
    } else {
      if project.runtime_identifier().is_some() && runtime_graph.is_none() {
        return Err(PackageError::new(
          PackageErrorKind::Configuration,
          project.project_path().display().to_string(),
          "RID-specific package selection requires the selected SDK portable runtime graph",
        ));
      }
      resolutions.push(resolve_project(project, options, runtime_graph, inventory, &mut batch)?);
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
      options.credential_provider_options(),
    )?;
    let client = http_client(config.proxy.as_ref())?;
    inventories.push(runtime.block_on(inspect_source_batch(
      &client,
      &config.sources,
      &config.credentials,
      !options.offline,
      options.probe_credentials,
      config.http_policy.with_offline(options.offline),
    ))?);
  }
  Ok(inventories)
}

async fn inspect_source_batch(
  client: &reqwest::Client,
  sources: &[(String, PackageSource)],
  credentials: &SourceCredentialBatch,
  allow_network: bool,
  probe_credentials: bool,
  http_policy: PackageHttpPolicy,
) -> Result<PackageSourceInventory, PackageError> {
  let mut discovered = std::iter::repeat_with(|| None)
    .take(sources.len())
    .collect::<Vec<Option<(NugetServiceEndpoints, HttpWork)>>>();
  if probe_credentials {
    for (index, (_, source)) in sources.iter().enumerate() {
      if source.protocol != NugetProtocol::Local
        && let Some(credential) = credentials.get(index)
      {
        let (_, generation, _) = credential.authorization_snapshot().await;
        credential.acquire_provider(generation, false).await?;
      }
    }
  }
  if allow_network {
    let jobs = sources
      .iter()
      .enumerate()
      .filter(|(_, (_, source))| source.protocol == NugetProtocol::V3)
      .map(|(index, (_, source))| (index, source.url.clone(), credentials.get(index).cloned()))
      .collect::<Vec<_>>();
    let mut tasks = JoinSet::new();
    let mut next = 0usize;
    while next < jobs.len() || !tasks.is_empty() {
      while next < jobs.len() && tasks.len() < MAX_DOWNLOAD_WORKERS {
        let (index, source, credential) = jobs[next].clone();
        let client = client.clone();
        tasks.spawn(async move { (index, fetch_v3_service_index(&client, credential.as_deref(), &source).await) });
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
  let mut source_work = (0..sources.len())
    .map(|index| u32_len(index, "NuGet package-source index").map(SourceWork::new))
    .collect::<Result<Vec<_>, _>>()?;
  let mut network_requests = 0u32;
  let mut downloaded_bytes = 0u64;
  for (index, (name, source)) in sources.iter().enumerate() {
    let start = u32_len(endpoint_rows.len(), "package-source endpoint range")?;
    if let Some((services, work)) = discovered[index].take() {
      source_work[index].merge_http(work, &source.url)?;
      network_requests = network_requests
        .checked_add(work.requests)
        .ok_or_else(|| network_error(&source.url, "package-source request count overflow"))?;
      downloaded_bytes = downloaded_bytes
        .checked_add(work.downloaded_bytes)
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
            location: text.push(redact_url_for_output(endpoint).as_ref())?,
            kind,
          });
        }
      }
    }
    source_rows.push(PackageSourceRecord {
      name: text.push(redact_url_for_output(name).as_ref())?,
      location: text.push(redact_url_for_output(&source.url).as_ref())?,
      endpoints: ItemRange {
        start,
        len: u32_len(endpoint_rows.len() - start as usize, "package-source endpoint range")?,
      },
      protocol: source.protocol,
      authentication: credentials.authentication(index),
      security_flags: source.security_flags,
    });
  }
  Ok(PackageSourceInventory {
    text: text.text.into_boxed_str(),
    sources: source_rows.into_boxed_slice(),
    endpoints: endpoint_rows.into_boxed_slice(),
    source_work: source_work.into_boxed_slice(),
    http_policy,
    network_requests,
    downloaded_bytes,
  })
}

fn resolve_project(
  project: &ProjectSpec,
  options: &PackageResolveOptions,
  runtime_graph: Option<&RuntimeIdentifierGraph>,
  inventory: Option<&SdkInventory>,
  batch: &mut PackageBatchContext,
) -> Result<PackageResolution, PackageError> {
  let mut config = discover_configuration(
    project.project_directory(),
    options.packages_directory.as_deref(),
    options.config_file.as_deref(),
    &options.sources,
    options.credential_provider_options(),
  )?;
  if let Some(inventory) = inventory {
    Arc::get_mut(&mut config.signature_policy)
      .expect("a newly discovered signature policy has one owner")
      .set_sdk_root(inventory.installation_path(inventory.selected()));
  }
  validate_audit_policy(project)?;
  let lock_path = project.project_directory().join("dv.lock.json");
  let direct = direct_requests(project)?;
  let central_pins = central_package_pins(project)?;
  let target = project.target();
  let target_text = project.target_framework();
  let pruning = discover_package_pruning(project, inventory)?;
  let runtime_graph_fingerprint = runtime_compatibility_fingerprint(project.runtime_identifier(), runtime_graph)?;
  if let Some(resolution) = read_warm_lock(&lock_path, &config, &direct, project, &pruning.fingerprint, &runtime_graph_fingerprint)? {
    return Ok(resolution);
  }

  let client = http_client(config.proxy.as_ref())?;
  if batch.runtime.is_none() {
    batch.runtime = Some(
      tokio::runtime::Builder::new_multi_thread()
        .worker_threads(ASYNC_RUNTIME_WORKERS)
        .enable_all()
        .build()
        .map_err(|error| PackageError::new(PackageErrorKind::Io, "package scheduler", format!("failed to create async runtime: {error}")))?,
    );
  }
  let runtime = batch.runtime.as_ref().expect("the package runtime was initialized");
  let graph = runtime.block_on(resolve_streaming_graph(
    &client,
    GraphRoots {
      direct: &direct,
      central_pins: &central_pins,
    },
    GraphContext {
      config: &config,
      options,
      target,
      target_text,
      runtime_identifier: project.runtime_identifier(),
      runtime_graph,
      pruning: &pruning,
      batch_metadata: &mut batch.metadata,
    },
  ))?;
  let ResolvedGraph {
    packages: resolved,
    source_work,
    downgrades,
    shared_metadata_hits,
  } = graph;

  validate_acyclic(&resolved)?;
  let origin = resolved.values().find_map(|package| package.origin.as_ref());
  let selected_source = origin
    .and_then(|origin| {
      config
        .sources
        .iter()
        .find(|(_, source)| source.url == origin.url && source.protocol == origin.protocol)
    })
    .or_else(|| config.sources.first());
  let (source_name, raw_source_location, source_protocol) = selected_source.map_or(("", DEFAULT_SOURCE, NugetProtocol::V3), |(name, source)| {
    (name.as_str(), source.url.as_str(), source.protocol)
  });
  let source_location = redact_url_for_output(raw_source_location);
  let resolution = materialize_resolution(
    ResolutionContext {
      project,
      direct: &direct,
      cache_root: &config.cache_root,
      http_cache_root: &config.http_cache_root,
      temp_root: &config.temp_root,
      fallback_roots: &config.fallback_roots,
      lock_path: &lock_path,
      target_framework: target_text,
      runtime_identifier: project.runtime_identifier(),
      runtime_graph_fingerprint: &runtime_graph_fingerprint,
      source_name,
      source_location: source_location.as_ref(),
      sources: &config.sources,
      prune_fingerprint: &pruning.fingerprint,
      central_package_fingerprint: project.central_package_fingerprint(),
      source_protocol,
      signature_validation: config.signature_validation,
      audit_enabled: project.nuget_audit_enabled(),
      audit_mode: project.nuget_audit_mode(),
      audit_level: project.nuget_audit_level(),
      proxy_configured: config.proxy.is_some(),
    },
    &resolved,
    &source_work,
    &downgrades,
    shared_metadata_hits,
  )?;
  if options.write_lock {
    write_lock(&resolution)?;
  }
  Ok(resolution)
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
    if let Some((existing_range, existing_text)) = seen.insert(lower_id.clone(), (range.clone(), version_text.to_owned())) {
      let message = if existing_range == range {
        format!("package {id} is directly referenced more than once; consolidate its metadata into one PackageReference")
      } else {
        format!("package {id} is directly referenced with conflicting versions {existing_text} and {version_text}")
      };
      return Err(PackageError::new(PackageErrorKind::Resolution, id, message));
    }
    direct.push(PackageRequirement {
      id: id.into(),
      lower_id,
      range,
      direct: true,
      include_assets: project.package_effective_assets(*package),
      suppress_parent: project.package_private_assets(*package),
    });
  }
  direct.sort_unstable_by(|left, right| left.lower_id.cmp(&right.lower_id));
  Ok(direct)
}

fn central_package_pins(project: &ProjectSpec) -> Result<Vec<CentralPackagePin>, PackageError> {
  if !project.central_package_transitive_pinning_enabled() {
    return Ok(Vec::new());
  }
  project
    .central_package_versions()
    .iter()
    .map(|package| {
      let id = project.central_package_id(*package);
      Ok(CentralPackagePin {
        lower_id: normalize_id(id)?,
        version: minimum_version_from_range(&VersionRange::parse(project.central_package_version(*package).trim())?)?,
      })
    })
    .collect()
}

fn discover_project_sdk(project: &ProjectSpec) -> Result<Option<SdkInventory>, PackageError> {
  let pruning_needs_sdk = project.restore_package_pruning_enabled()
    && ((project.target().family() == FrameworkFamily::Net && project.target().major() >= 10) || !project.framework_references().is_empty());
  if project.runtime_identifier().is_none() && !pruning_needs_sdk {
    return Ok(None);
  }
  discover_sdks(project.project_directory()).map(Some).map_err(|error| {
    PackageError::new(
      PackageErrorKind::Configuration,
      project.project_directory().display().to_string(),
      format!("failed to select the SDK needed for package assets: {error}"),
    )
  })
}

fn runtime_compatibility_fingerprint(runtime_identifier: Option<&str>, runtime_graph: Option<&RuntimeIdentifierGraph>) -> Result<String, PackageError> {
  let Some(runtime_identifier) = runtime_identifier else {
    return Ok(String::new());
  };
  let graph = runtime_graph.ok_or_else(|| {
    PackageError::new(
      PackageErrorKind::Configuration,
      runtime_identifier,
      "RID-specific package selection requires the selected SDK portable runtime graph",
    )
  })?;
  let mut hasher = Sha512::new();
  for compatible in graph.compatible_rids(runtime_identifier) {
    hasher.update((compatible.len() as u64).to_le_bytes());
    hasher.update(compatible.as_bytes());
  }
  Ok(BASE64.encode(hasher.finalize()))
}

fn discover_package_pruning(project: &ProjectSpec, inventory: Option<&SdkInventory>) -> Result<PackagePruning, PackageError> {
  if !project.restore_package_pruning_enabled() {
    return Ok(PackagePruning::default());
  }

  let target = project.target();
  if target.family() == FrameworkFamily::NetFramework {
    if project.allow_missing_prune_package_data() {
      return compact_package_pruning(Vec::new());
    }
    return Err(missing_pruning_data(project, PruningFramework::Default));
  }

  let mut packages = Vec::new();
  if target.family() == FrameworkFamily::NetStandard && project.framework_references().is_empty() {
    let found = extend_legacy_pruning(&mut packages, project, PruningFramework::Default, false)?;
    if !found && !project.allow_missing_prune_package_data() {
      return Err(missing_pruning_data(project, PruningFramework::Default));
    }
    return compact_package_pruning(packages);
  }

  let needs_sdk = (target.family() == FrameworkFamily::Net && target.major() >= 10) || !project.framework_references().is_empty();
  if !needs_sdk {
    if target.family() != FrameworkFamily::NetStandard {
      let found = extend_legacy_pruning(&mut packages, project, PruningFramework::Core, true)?;
      if !found && !project.allow_missing_prune_package_data() {
        return Err(missing_pruning_data(project, PruningFramework::Core));
      }
    }
    return compact_package_pruning(packages);
  }

  let discovered;
  let inventory = if let Some(inventory) = inventory {
    inventory
  } else {
    discovered = discover_sdks(project.project_directory()).map_err(|error| {
      PackageError::new(
        PackageErrorKind::Configuration,
        project.project_directory().display().to_string(),
        format!("failed to select the SDK needed for package pruning: {error}"),
      )
    })?;
    &discovered
  };

  let frameworks = pruning_framework_batch(project, Some(inventory))?;

  if target.family() != FrameworkFamily::Net || target.major() < 10 {
    for kind in frameworks.iter() {
      let found = extend_legacy_pruning(&mut packages, project, kind, target.family() != FrameworkFamily::NetStandard)?;
      if !found && !project.allow_missing_prune_package_data() {
        return Err(missing_pruning_data(project, kind));
      }
    }
    return compact_package_pruning(packages);
  }

  for kind in frameworks.iter() {
    let found = extend_modern_pruning(&mut packages, project, inventory, kind)?;
    if !found && !project.allow_missing_prune_package_data() {
      return Err(missing_pruning_data(project, kind));
    }
  }
  compact_package_pruning(packages)
}

#[derive(Clone, Copy)]
struct PruningFrameworkBatch {
  values: [PruningFramework; 3],
  len: u8,
}

const _: () = assert!(size_of::<PruningFrameworkBatch>() == 4);
const _: () = assert!(align_of::<PruningFrameworkBatch>() == 1);

impl PruningFrameworkBatch {
  fn empty() -> Self {
    Self {
      values: [PruningFramework::Default; 3],
      len: 0,
    }
  }

  fn push(&mut self, framework: PruningFramework) {
    if !self.iter().any(|existing| existing == framework) {
      debug_assert!(usize::from(self.len) < self.values.len());
      self.values[usize::from(self.len)] = framework;
      self.len += 1;
    }
  }

  fn iter(&self) -> impl Iterator<Item = PruningFramework> + '_ {
    self.values[..usize::from(self.len)].iter().copied()
  }
}

fn pruning_framework_batch(project: &ProjectSpec, inventory: Option<&SdkInventory>) -> Result<PruningFrameworkBatch, PackageError> {
  let mut batch = PruningFrameworkBatch::empty();
  match project.target().family() {
    FrameworkFamily::Net | FrameworkFamily::NetCoreApp => batch.push(PruningFramework::Core),
    FrameworkFamily::NetStandard => batch.push(PruningFramework::Default),
    FrameworkFamily::NetFramework => return Ok(batch),
  }
  let mut needs_manifest = false;
  for reference in project.framework_references() {
    if let Some(framework) = pruning_framework_kind(project.framework_reference_id(*reference)) {
      batch.push(framework);
    } else {
      needs_manifest = true;
    }
  }
  if needs_manifest {
    let inventory = inventory.ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::Configuration,
        project.project_path().display().to_string(),
        "framework-profile pruning requires the selected SDK inventory",
      )
    })?;
    let runtime_names = package_pruning_runtime_names(project, inventory).map_err(|error| {
      PackageError::new(
        PackageErrorKind::Configuration,
        error.path().display().to_string(),
        format!("failed to map package-pruning framework references: {error}"),
      )
    })?;
    for runtime_name in runtime_names {
      if let Some(framework) = pruning_framework_kind(&runtime_name) {
        batch.push(framework);
      }
    }
  }
  Ok(batch)
}

fn extend_modern_pruning(
  packages: &mut Vec<ParsedPrunedPackage>,
  project: &ProjectSpec,
  inventory: &SdkInventory,
  framework: PruningFramework,
) -> Result<bool, PackageError> {
  let target = project.target();
  let selected = inventory.selected();
  let sdk_data_root = inventory.installation_path(selected).join("PrunePackageData");
  let pack_root = inventory
    .root(selected)
    .join("packs")
    .join(format!("{}.Ref", pruning_framework_name(framework)));
  let sdk_data = sdk_data_root
    .join(target.framework_version())
    .join(pruning_framework_name(framework))
    .join("PackageOverrides.txt");
  if framework == PruningFramework::Core && sdk_data.is_file() {
    extend_parsed_pruning(packages, parse_package_pruning(&sdk_data)?)?;
    return Ok(true);
  }
  if let Some(pack_data) = select_pruning_pack_data(&pack_root, target.major(), target.minor())? {
    extend_parsed_pruning(packages, parse_package_pruning(&pack_data)?)?;
    return Ok(true);
  }
  if framework == PruningFramework::WindowsDesktop
    && let Some(table) = nearest_legacy_pruning(target.family(), target.major(), target.minor(), framework)
  {
    extend_legacy_packages(packages, table, true)?;
    return Ok(true);
  }
  Ok(false)
}

fn select_pruning_pack_data(root: &Path, major: u16, minor: u16) -> Result<Option<PathBuf>, PackageError> {
  let entries = match fs::read_dir(root) {
    Ok(entries) => entries,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
    Err(error) => return Err(package_io("enumerate package-pruning packs", root, error)),
  };
  let mut selected = None::<(PackageVersion, PathBuf)>;
  for entry in entries {
    let entry = entry.map_err(|error| package_io("enumerate package-pruning packs", root, error))?;
    if !entry
      .file_type()
      .map_err(|error| package_io("inspect package-pruning pack", &entry.path(), error))?
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
    let data = entry.path().join("data").join("PackageOverrides.txt");
    if version.prerelease().is_some() || version.numbers[0] != u32::from(major) || version.numbers[1] != u32::from(minor) || !data.is_file() {
      continue;
    }
    if selected.as_ref().is_none_or(|(current, _)| version > *current) {
      selected = Some((version, data));
    }
  }
  Ok(selected.map(|(_, path)| path))
}

fn extend_parsed_pruning(packages: &mut Vec<ParsedPrunedPackage>, selected: Vec<ParsedPrunedPackage>) -> Result<(), PackageError> {
  if packages.len().saturating_add(selected.len()) > MAX_PRUNE_PACKAGES {
    return Err(PackageError::new(
      PackageErrorKind::Configuration,
      "package-pruning data",
      format!("merged package-pruning data exceeds {MAX_PRUNE_PACKAGES} entries"),
    ));
  }
  packages.extend(selected);
  Ok(())
}

fn pruning_framework_kind(runtime_name: &str) -> Option<PruningFramework> {
  if runtime_name.eq_ignore_ascii_case("Microsoft.NETCore.App") {
    Some(PruningFramework::Core)
  } else if runtime_name.eq_ignore_ascii_case("Microsoft.AspNetCore.App") {
    Some(PruningFramework::AspNetCore)
  } else if runtime_name.eq_ignore_ascii_case("Microsoft.WindowsDesktop.App") {
    Some(PruningFramework::WindowsDesktop)
  } else {
    None
  }
}

fn extend_legacy_pruning(
  packages: &mut Vec<ParsedPrunedPackage>,
  project: &ProjectSpec,
  framework: PruningFramework,
  stable_patch_ceiling: bool,
) -> Result<bool, PackageError> {
  let target = project.target();
  if let Some(table) = exact_legacy_pruning(target.family(), target.major(), target.minor(), framework) {
    extend_legacy_packages(packages, table, stable_patch_ceiling)?;
    return Ok(true);
  }
  Ok(false)
}

fn missing_pruning_data(project: &ProjectSpec, framework: PruningFramework) -> PackageError {
  PackageError::new(
    PackageErrorKind::Configuration,
    project.project_path().display().to_string(),
    format!(
      "selected SDK has no package-pruning data for {} {}",
      project.target_framework(),
      pruning_framework_name(framework)
    ),
  )
}

fn pruning_framework_name(framework: PruningFramework) -> &'static str {
  match framework {
    PruningFramework::Default => "",
    PruningFramework::Core => "Microsoft.NETCore.App",
    PruningFramework::AspNetCore => "Microsoft.AspNetCore.App",
    PruningFramework::WindowsDesktop => "Microsoft.WindowsDesktop.App",
  }
}

fn extend_legacy_packages(packages: &mut Vec<ParsedPrunedPackage>, table: &[LegacyPrunePackage], stable_patch_ceiling: bool) -> Result<(), PackageError> {
  if packages.len().saturating_add(table.len()) > MAX_PRUNE_PACKAGES {
    return Err(PackageError::new(
      PackageErrorKind::Configuration,
      "generated package-pruning data",
      format!("merged package-pruning data exceeds {MAX_PRUNE_PACKAGES} entries"),
    ));
  }
  packages.reserve(table.len());
  for package in table {
    let numbers = if stable_patch_ceiling {
      [package.numbers[0], package.numbers[1], 32_767, 0]
    } else {
      package.numbers
    };
    let normalized = if numbers[3] == 0 {
      format!("{}.{}.{}", numbers[0], numbers[1], numbers[2])
    } else {
      format!("{}.{}.{}.{}", numbers[0], numbers[1], numbers[2], numbers[3])
    };
    packages.push(ParsedPrunedPackage {
      lower_id: package.id.to_owned(),
      upper: PackageVersion {
        normalized,
        numbers,
        prerelease_start: None,
      },
    });
  }
  Ok(())
}

fn parse_package_pruning(path: &Path) -> Result<Vec<ParsedPrunedPackage>, PackageError> {
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
  Ok(packages)
}

#[cfg(test)]
fn read_package_pruning(path: &Path) -> Result<PackagePruning, PackageError> {
  compact_package_pruning(parse_package_pruning(path)?)
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
  provider_options: CredentialProviderOptions,
) -> Result<NugetConfiguration, PackageError> {
  let config_paths = discover_config_paths(project_directory, explicit_config, &NugetConfigRoots::from_environment())?;

  let mut merged = NugetConfigMerge::default();
  if config_paths.is_empty() {
    merged.sources.push((
      "nuget.org".to_owned(),
      PackageSource {
        url: DEFAULT_SOURCE.to_owned(),
        protocol: NugetProtocol::V3,
        security_flags: 0,
      },
    ));
  }
  for path in config_paths {
    merge_config(&path, &mut merged)?;
  }
  let proxy = effective_proxy(&merged)?;
  let http_policy = effective_http_policy(&merged, proxy.as_ref());
  let request_budget = effective_request_budget();
  let signature_validation = merged.signature_validation.unwrap_or(SignatureValidationMode::Accept);
  let signature_policy = Arc::new(SignaturePolicy::new(signature_validation, std::mem::take(&mut merged.trusted_signers)));
  signature_policy.validate()?;
  for key in merged.disabled {
    merged.sources.retain(|(name, _)| !name.eq_ignore_ascii_case(&key));
  }
  let sources = command_line_sources(explicit_sources, merged.sources, project_directory)?;
  let http_policy = http_policy.with_source_security(&sources);
  let mut credentials = resolve_source_credentials(&sources, merged.credentials, project_directory, provider_options)?;
  attach_client_certificates(&sources, merged.client_certificates, proxy.as_ref(), project_directory, &mut credentials)?;
  attach_http_policy(&sources, http_policy, request_budget, proxy.as_ref(), project_directory, &mut credentials)?;
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
    credentials,
    audit_sources: merged.audit_sources,
    source_mapping,
    signature_validation,
    signature_policy,
    proxy,
    http_policy,
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
    let selected = if let Some((name, source)) = configured.iter().rev().find(|(_, source)| source.url == *value) {
      (name.clone(), source.clone())
    } else {
      (
        redact_url_for_output(value).into_owned(),
        PackageSource::parse(value.clone(), None, false, false, Path::new("--source"), project_directory)?,
      )
    };
    if sources.iter().any(|(_, source): &(String, PackageSource)| source.url == selected.1.url) {
      continue;
    }
    sources.push(selected);
  }
  Ok(sources)
}

fn resolve_source_credentials(
  sources: &[(String, PackageSource)],
  configured: Vec<MergedSourceCredential>,
  context: &Path,
  provider_options: CredentialProviderOptions,
) -> Result<SourceCredentialBatch, PackageError> {
  resolve_source_credentials_with(sources, configured, context, provider_options, |name| env::var(name).ok())
}

fn resolve_source_credentials_with(
  sources: &[(String, PackageSource)],
  configured: Vec<MergedSourceCredential>,
  context: &Path,
  provider_options: CredentialProviderOptions,
  mut environment: impl FnMut(&str) -> Option<String>,
) -> Result<SourceCredentialBatch, PackageError> {
  let mut configured = configured;
  let provider_configured = provider_options.configured;
  let mut entries = None::<Vec<Option<Arc<SourceCredential>>>>;
  for (source_index, (name, source)) in sources.iter().enumerate() {
    if source.protocol == NugetProtocol::Local {
      if let Some(entries) = &mut entries {
        entries.push(None);
      }
      continue;
    }
    let selected = environment_source_credential(name, &mut environment).or_else(|| {
      configured
        .iter()
        .position(|credential| credential.source == *name)
        .map(EnvironmentOrConfigCredential::Config)
    });
    let authorization = match selected {
      Some(EnvironmentOrConfigCredential::Environment(credential)) => Some(materialize_source_credential(credential, source, context)?),
      Some(EnvironmentOrConfigCredential::Config(index)) => {
        let credential = configured.swap_remove(index);
        Some(materialize_source_credential(credential, source, context)?)
      },
      None => None,
    };
    let provider = provider_configured.then(|| {
      Box::new(SourceProviderCredential {
        source: source.url.clone().into_boxed_str(),
        provider_options: provider_options.clone(),
        acquired: AtomicBool::new(false),
        state: Mutex::new(ProviderCredentialState::default()),
      })
    });
    let credential = if authorization.is_some() || provider.is_some() {
      Some(Arc::new(SourceCredential {
        authorization,
        origin: HttpOrigin::parse(&source.url, context)?,
        provider,
        client: None,
        transport_client: None,
        global_limiter: None,
        source_limiter: None,
        http_policy: DEFAULT_HTTP_POLICY,
        security_flags: source.security_flags,
      }))
    } else {
      None
    };
    if credential.is_some() && entries.is_none() {
      let mut initialized = Vec::with_capacity(sources.len());
      initialized.resize_with(source_index, || None);
      entries = Some(initialized);
    }
    if let Some(entries) = &mut entries {
      entries.push(credential);
    }
  }
  Ok(SourceCredentialBatch {
    entries: entries.map_or_else(Box::default, Vec::into_boxed_slice),
  })
}

fn attach_client_certificates(
  sources: &[(String, PackageSource)],
  certificates: Vec<MergedClientCertificate>,
  proxy: Option<&ProxySettings>,
  context: &Path,
  credentials: &mut SourceCredentialBatch,
) -> Result<(), PackageError> {
  if certificates.is_empty() {
    return Ok(());
  }
  for (index, certificate) in certificates.iter().enumerate() {
    if certificates[index + 1..]
      .iter()
      .any(|candidate| candidate.source().eq_ignore_ascii_case(certificate.source()))
    {
      return Err(config_error(
        context,
        format!(
          "NuGet package source {:?} has more than one client certificate configuration",
          certificate.source()
        ),
      ));
    }
  }
  let mut entries = std::mem::take(&mut credentials.entries).into_vec();
  entries.resize_with(sources.len(), || None);
  for (source_index, (name, source)) in sources.iter().enumerate() {
    if source.protocol == NugetProtocol::Local {
      continue;
    }
    let Some(certificate) = certificates.iter().find(|certificate| certificate.source() == name) else {
      continue;
    };
    let client = client_certificate_http_client(certificate, proxy, source.security_flags, context)?;
    if let Some(credential) = entries[source_index].as_mut() {
      Arc::get_mut(credential)
        .expect("source authentication is uniquely owned during configuration")
        .client = Some(client);
    } else {
      entries[source_index] = Some(Arc::new(SourceCredential {
        authorization: None,
        origin: HttpOrigin::parse(&source.url, context)?,
        provider: None,
        client: Some(client),
        transport_client: None,
        global_limiter: None,
        source_limiter: None,
        http_policy: DEFAULT_HTTP_POLICY,
        security_flags: source.security_flags,
      }));
    }
  }
  credentials.entries = entries.into_boxed_slice();
  Ok(())
}

fn attach_http_policy(
  sources: &[(String, PackageSource)],
  policy: PackageHttpPolicy,
  request_budget: PackageRequestBudget,
  proxy: Option<&ProxySettings>,
  context: &Path,
  credentials: &mut SourceCredentialBatch,
) -> Result<(), PackageError> {
  let global_limit = request_budget.global_limit();
  let global_changed = global_limit < MAX_DOWNLOAD_WORKERS;
  let source_limit = policy.effective_request_limit(global_limit);
  let runtime_changed = policy.max_tries != DEFAULT_HTTP_POLICY.max_tries
    || policy.retry_delay_ms != DEFAULT_HTTP_POLICY.retry_delay_ms
    || policy.max_retry_after_seconds != DEFAULT_HTTP_POLICY.max_retry_after_seconds
    || (policy.flags ^ DEFAULT_HTTP_POLICY.flags) & (HTTP_RETRY_429 | HTTP_OBSERVE_RETRY_AFTER) != 0
    || global_changed
    || source_limit < global_limit;
  let source_security_changed = sources.iter().any(|(_, source)| source.security_flags != 0);
  if credentials.entries.is_empty() && !runtime_changed && !source_security_changed {
    return Ok(());
  }

  let global_limiter = global_changed.then(|| Arc::new(Semaphore::new(global_limit)));
  let mut entries = std::mem::take(&mut credentials.entries).into_vec();
  entries.resize_with(sources.len(), || None);
  for (source_index, (_, source)) in sources.iter().enumerate() {
    if source.protocol == NugetProtocol::Local {
      continue;
    }
    let transport_client = if source.security_flags == 0 {
      None
    } else {
      Some(source_http_client(proxy, source.security_flags)?)
    };
    let source_limiter = (source_limit < global_limit).then(|| Arc::new(Semaphore::new(source_limit)));
    if let Some(credential) = entries[source_index].as_mut() {
      let credential = Arc::get_mut(credential).expect("source policy is uniquely owned during configuration");
      credential.http_policy = policy;
      credential.global_limiter = global_limiter.clone();
      credential.source_limiter = source_limiter;
      credential.security_flags = source.security_flags;
      credential.transport_client = transport_client;
    } else if runtime_changed || source.security_flags != 0 {
      entries[source_index] = Some(Arc::new(SourceCredential {
        authorization: None,
        origin: HttpOrigin::parse(&source.url, context)?,
        provider: None,
        client: None,
        transport_client,
        global_limiter: global_limiter.clone(),
        source_limiter,
        http_policy: policy,
        security_flags: source.security_flags,
      }));
    }
  }
  credentials.entries = entries.into_boxed_slice();
  Ok(())
}

fn client_certificate_http_client(
  certificate: &MergedClientCertificate,
  proxy: Option<&ProxySettings>,
  security_flags: u8,
  context: &Path,
) -> Result<reqwest::Client, PackageError> {
  let identity = match certificate {
    MergedClientCertificate::File { source, path, password } => {
      let bytes = Zeroizing::new(read_bounded_client_certificate(path)?);
      let password = match password {
        Some(StoredCredentialPassword::Clear(password)) => Zeroizing::new(password.as_str().to_owned()),
        Some(StoredCredentialPassword::Encrypted(password)) => decrypt_source_password(source, StoredCredentialPassword::Encrypted(password.clone()), context)?,
        None => Zeroizing::new(String::new()),
      };
      reqwest::Identity::from_pkcs12_der(&bytes, &password).map_err(|error| {
        config_error(
          path,
          format!("failed to load PKCS#12 client certificate for package source {source:?}: {error}"),
        )
      })?
    },
    MergedClientCertificate::Store {
      source,
      location,
      name,
      find_by,
      find_value,
    } => platform_store_identity(source, location, name, find_by, find_value, context)?,
  };
  configured_http_client_builder(proxy, security_flags)?
    .tls_backend_native()
    .redirect(reqwest::redirect::Policy::none())
    .identity(identity)
    .build()
    .map_err(|error| network_error("client-certificate HTTP client", format!("failed to create HTTP client: {error}")))
}

fn read_bounded_client_certificate(path: &Path) -> Result<Vec<u8>, PackageError> {
  let file = fs::File::open(path).map_err(|error| package_io("open NuGet client certificate", path, error))?;
  let mut bytes = Vec::with_capacity(
    file
      .metadata()
      .map_err(|error| package_io("inspect NuGet client certificate", path, error))?
      .len()
      .min(MAX_CLIENT_CERTIFICATE_BYTES) as usize,
  );
  file
    .take(MAX_CLIENT_CERTIFICATE_BYTES + 1)
    .read_to_end(&mut bytes)
    .map_err(|error| package_io("read NuGet client certificate", path, error))?;
  if bytes.len() as u64 > MAX_CLIENT_CERTIFICATE_BYTES {
    return Err(config_error(
      path,
      format!("NuGet client certificate exceeds the {MAX_CLIENT_CERTIFICATE_BYTES}-byte limit"),
    ));
  }
  Ok(bytes)
}

#[cfg(windows)]
fn platform_store_identity(
  source: &str,
  location: &str,
  name: &str,
  find_by: &str,
  find_value: &str,
  context: &Path,
) -> Result<reqwest::Identity, PackageError> {
  use schannel::{
    cert_context::HashAlgorithm,
    cert_store::{CertAdd, CertStore, Memory},
  };

  const STORE_NAMES: [&str; 8] = [
    "AddressBook",
    "AuthRoot",
    "CertificateAuthority",
    "Disallowed",
    "My",
    "Root",
    "TrustedPeople",
    "TrustedPublisher",
  ];
  let store_name = STORE_NAMES
    .iter()
    .find(|candidate| candidate.eq_ignore_ascii_case(name))
    .ok_or_else(|| config_error(context, format!("NuGet client certificate store name {name:?} is unsupported")))?;
  if !find_by.eq_ignore_ascii_case("Thumbprint") {
    return Err(config_error(
      context,
      format!("NuGet client certificate selector {find_by:?} is unsupported; dv currently supports Thumbprint"),
    ));
  }
  let thumbprint = parse_certificate_thumbprint(find_value, context)?;
  let store = if location.eq_ignore_ascii_case("CurrentUser") {
    CertStore::open_current_user(store_name)
  } else if location.eq_ignore_ascii_case("LocalMachine") {
    CertStore::open_local_machine(store_name)
  } else {
    return Err(config_error(
      context,
      format!("NuGet client certificate store location {location:?} is unsupported; expected CurrentUser or LocalMachine"),
    ));
  }
  .map_err(|error| config_error(context, format!("failed to open {location}\\{store_name} certificate store: {error}")))?;
  let selected = store
    .certs()
    .find(|certificate| certificate.fingerprint(HashAlgorithm::sha1()).is_ok_and(|candidate| candidate == thumbprint))
    .ok_or_else(|| {
      config_error(
        context,
        format!("NuGet client certificate for package source {source:?} was not found in {location}\\{store_name}"),
      )
    })?;
  selected.private_key().acquire().map_err(|error| {
    config_error(
      context,
      format!("NuGet client certificate for package source {source:?} has no accessible private key: {error}"),
    )
  })?;
  let mut memory = Memory::new()
    .map_err(|error| config_error(context, format!("failed to create temporary client certificate store: {error}")))?
    .into_store();
  memory
    .add_cert(&selected, CertAdd::Always)
    .map_err(|error| config_error(context, format!("failed to stage client certificate for package source {source:?}: {error}")))?;
  const TRANSIENT_PASSWORD: &str = "dv-client-certificate";
  let pkcs12 = Zeroizing::new(
    memory
      .export_pkcs12(TRANSIENT_PASSWORD)
      .map_err(|error| config_error(context, format!("failed to export client certificate for package source {source:?}: {error}")))?,
  );
  reqwest::Identity::from_pkcs12_der(&pkcs12, TRANSIENT_PASSWORD)
    .map_err(|error| config_error(context, format!("failed to load client certificate for package source {source:?}: {error}")))
}

#[cfg(windows)]
fn parse_certificate_thumbprint(value: &str, context: &Path) -> Result<Vec<u8>, PackageError> {
  let mut bytes = Vec::with_capacity(20);
  let mut high = None::<u8>;
  for byte in value.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
    let digit = match byte {
      b'0'..=b'9' => byte - b'0',
      b'a'..=b'f' => byte - b'a' + 10,
      b'A'..=b'F' => byte - b'A' + 10,
      _ => return Err(config_error(context, "NuGet client certificate thumbprint must contain hexadecimal digits")),
    };
    if let Some(high) = high.take() {
      bytes.push((high << 4) | digit);
    } else {
      high = Some(digit);
    }
  }
  if high.is_some() || bytes.len() != 20 {
    return Err(config_error(
      context,
      "NuGet client certificate SHA-1 thumbprint must contain exactly 40 hexadecimal digits",
    ));
  }
  Ok(bytes)
}

#[cfg(not(windows))]
fn platform_store_identity(
  source: &str,
  _location: &str,
  _name: &str,
  _find_by: &str,
  _find_value: &str,
  context: &Path,
) -> Result<reqwest::Identity, PackageError> {
  Err(config_error(
    context,
    format!("platform certificate stores for package source {source:?} are currently supported only on Windows; use fileCert on this platform"),
  ))
}

enum EnvironmentOrConfigCredential {
  Environment(MergedSourceCredential),
  Config(usize),
}

fn environment_source_credential(source: &str, environment: &mut impl FnMut(&str) -> Option<String>) -> Option<EnvironmentOrConfigCredential> {
  let variable = format!("NuGetPackageSourceCredentials_{source}");
  let raw = environment(&variable)?;
  let raw = Zeroizing::new(raw);
  let (username, password, valid_authentication_types) = parse_environment_credential(&raw)?;
  if username.is_empty() || password.is_empty() {
    return None;
  }
  Some(EnvironmentOrConfigCredential::Environment(MergedSourceCredential {
    source: source.to_owned(),
    username: Zeroizing::new(username.to_owned()),
    password: StoredCredentialPassword::Clear(Zeroizing::new(password.to_owned())),
    valid_authentication_types: valid_authentication_types.map(str::to_owned),
  }))
}

fn parse_environment_credential(value: &str) -> Option<(&str, &str, Option<&str>)> {
  let value = value.trim();
  value.get(..9).filter(|prefix| prefix.eq_ignore_ascii_case("Username="))?;
  let rest = &value[9..];
  let (username, password_and_types) = rest.match_indices(';').find_map(|(separator, _)| {
    let after_separator = rest[separator + 1..].trim_start_matches(char::is_whitespace);
    let prefix = after_separator.get(..9)?;
    prefix.eq_ignore_ascii_case("Password=").then_some((&rest[..separator], &after_separator[9..]))
  })?;
  let auth_marker = ";ValidAuthenticationTypes=";
  if let Some(index) = find_ascii_case_insensitive(password_and_types, auth_marker) {
    Some((username, &password_and_types[..index], Some(&password_and_types[index + auth_marker.len()..])))
  } else {
    Some((username, password_and_types, None))
  }
}

fn find_ascii_case_insensitive(value: &str, needle: &str) -> Option<usize> {
  value
    .as_bytes()
    .windows(needle.len())
    .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn materialize_source_credential(credential: MergedSourceCredential, _source: &PackageSource, context: &Path) -> Result<HeaderValue, PackageError> {
  if credential.username.is_empty() {
    return Err(config_error(
      context,
      format!("package source {:?} has an empty credential username", credential.source),
    ));
  }
  let password = decrypt_source_password(&credential.source, credential.password, context)?;
  if password.is_empty() {
    return Err(config_error(
      context,
      format!("package source {:?} has an empty credential password", credential.source),
    ));
  }
  if let Some(types) = credential.valid_authentication_types.as_deref()
    && !types.trim().is_empty()
    && !types.split(',').any(|kind| kind.trim().eq_ignore_ascii_case("basic"))
  {
    return Err(config_error(
      context,
      format!(
        "package source {:?} does not allow Basic authentication; other mechanisms remain tracked by NUGET-009",
        credential.source
      ),
    ));
  }
  let authorization = basic_authorization(&credential.username, &password, &credential.source, context)?;
  Ok(authorization)
}

fn basic_authorization(username: &str, password: &str, source: &str, context: &Path) -> Result<HeaderValue, PackageError> {
  let mut plaintext = Zeroizing::new(Vec::with_capacity(username.len().saturating_add(password.len()).saturating_add(1)));
  plaintext.extend_from_slice(username.as_bytes());
  plaintext.push(b':');
  plaintext.extend_from_slice(password.as_bytes());
  let encoded = Zeroizing::new(BASE64.encode(&*plaintext));
  let mut header_bytes = Zeroizing::new(Vec::with_capacity(encoded.len().saturating_add(6)));
  header_bytes.extend_from_slice(b"Basic ");
  header_bytes.extend_from_slice(encoded.as_bytes());
  let mut authorization = HeaderValue::from_bytes(&header_bytes).map_err(|_| {
    config_error(
      context,
      format!("package source {source:?} has a credential which cannot form an HTTP Basic header"),
    )
  })?;
  authorization.set_sensitive(true);
  Ok(authorization)
}

fn decrypt_source_password(source: &str, password: StoredCredentialPassword, context: &Path) -> Result<Zeroizing<String>, PackageError> {
  match password {
    StoredCredentialPassword::Clear(password) => Ok(password),
    StoredCredentialPassword::Encrypted(password) => decrypt_nuget_password(source, &password, context),
  }
}

#[cfg(windows)]
fn decrypt_nuget_password(source: &str, password: &str, context: &Path) -> Result<Zeroizing<String>, PackageError> {
  let encrypted = BASE64
    .decode(password)
    .map_err(|error| config_error(context, format!("package source {source:?} has an invalid encrypted password: {error}")))?;
  let decrypted = windows_dpapi::decrypt_data(&encrypted, windows_dpapi::Scope::User, Some(b"NuGet")).map_err(|_| {
    config_error(
      context,
      format!("package source {source:?} password could not be decrypted for the current Windows user"),
    )
  })?;
  let decrypted = Zeroizing::new(decrypted);
  let password = std::str::from_utf8(&decrypted).map_err(|_| config_error(context, format!("package source {source:?} decrypted password is not UTF-8")))?;
  Ok(Zeroizing::new(password.to_owned()))
}

#[cfg(not(windows))]
fn decrypt_nuget_password(source: &str, _password: &str, context: &Path) -> Result<Zeroizing<String>, PackageError> {
  Err(config_error(
    context,
    format!("package source {source:?} uses a Windows-encrypted password; use ClearTextPassword with an environment expansion on this platform"),
  ))
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
  effective_proxy_with(merged, environment_value)
}

fn effective_proxy_with(merged: &NugetConfigMerge, mut environment: impl FnMut(&str, &str) -> Option<String>) -> Result<Option<ProxySettings>, PackageError> {
  let configured = merged.proxy_url.as_ref().filter(|url| !url.is_empty());
  if let Some(url) = configured {
    let credentials = configured_proxy_credentials(merged)?;
    return proxy_settings(url.clone(), merged.no_proxy.clone(), credentials);
  }
  let Some(url) = environment("http_proxy", "HTTP_PROXY") else {
    return Ok(None);
  };
  proxy_settings(url, environment("no_proxy", "NO_PROXY"), None)
}

fn environment_value(primary: &str, fallback: &str) -> Option<String> {
  env::var(primary)
    .ok()
    .filter(|value| !value.is_empty())
    .or_else(|| env::var(fallback).ok().filter(|value| !value.is_empty()))
}

#[cfg(windows)]
fn configured_proxy_credentials(merged: &NugetConfigMerge) -> Result<Option<ProxyCredential>, PackageError> {
  let Some(username) = merged.proxy_user.as_ref().filter(|value| !value.is_empty()) else {
    return Ok(None);
  };
  let Some(password) = merged.proxy_password.as_ref().filter(|value| !value.is_empty()) else {
    return Ok(None);
  };
  Ok(Some(ProxyCredential {
    username: Zeroizing::new(username.clone()),
    password: decrypt_nuget_password("http_proxy", password, Path::new("http_proxy.password"))?,
  }))
}

#[cfg(not(windows))]
fn configured_proxy_credentials(_merged: &NugetConfigMerge) -> Result<Option<ProxyCredential>, PackageError> {
  Ok(None)
}

fn proxy_settings(raw_url: String, no_proxy: Option<String>, configured_credentials: Option<ProxyCredential>) -> Result<Option<ProxySettings>, PackageError> {
  let raw_url = Zeroizing::new(raw_url);
  let mut parsed = reqwest::Url::parse(&raw_url).map_err(|error| config_error(Path::new("http_proxy"), format!("invalid NuGet proxy address: {error}")))?;
  if !parsed.has_host() || !matches!(parsed.scheme(), "http" | "https") {
    return Err(config_error(
      Path::new("http_proxy"),
      "NuGet proxy address must be an absolute HTTP or HTTPS URL",
    ));
  }
  let embedded = if parsed.username().is_empty() && parsed.password().is_none() {
    None
  } else {
    let username = percent_encoding::percent_decode_str(parsed.username())
      .decode_utf8()
      .map_err(|_| config_error(Path::new("http_proxy"), "NuGet proxy username is not valid UTF-8"))?;
    let password = percent_encoding::percent_decode_str(parsed.password().unwrap_or_default())
      .decode_utf8()
      .map_err(|_| config_error(Path::new("http_proxy"), "NuGet proxy password is not valid UTF-8"))?;
    Some(ProxyCredential {
      username: Zeroizing::new(username.into_owned()),
      password: Zeroizing::new(password.into_owned()),
    })
  };
  parsed
    .set_username("")
    .map_err(|()| config_error(Path::new("http_proxy"), "failed to remove proxy credentials from the retained URL"))?;
  parsed
    .set_password(None)
    .map_err(|()| config_error(Path::new("http_proxy"), "failed to remove proxy credentials from the retained URL"))?;
  let (username, password) = configured_credentials
    .or(embedded)
    .map_or((None, None), |credential| (Some(credential.username), Some(credential.password)));
  Ok(Some(ProxySettings {
    url: parsed.into(),
    no_proxy: no_proxy.filter(|value| !value.is_empty()),
    username,
    password,
  }))
}

fn effective_http_policy(merged: &NugetConfigMerge, proxy: Option<&ProxySettings>) -> PackageHttpPolicy {
  effective_http_policy_with(merged, proxy, |name| env::var(name).ok())
}

fn effective_request_budget() -> PackageRequestBudget {
  effective_request_budget_with(|name| env::var(name).ok())
}

fn effective_request_budget_with(mut environment: impl FnMut(&str) -> Option<String>) -> PackageRequestBudget {
  let global_requests = environment("NUGET_CONCURRENCY_LIMIT")
    .and_then(|value| value.trim().parse::<i32>().ok())
    .filter(|value| *value > 0)
    .map_or(DEFAULT_REQUEST_BUDGET.global_requests, |value| {
      (value as u32).min(u32::from(DEFAULT_REQUEST_BUDGET.global_requests)) as u16
    });
  PackageRequestBudget { global_requests }
}

fn effective_http_policy_with(
  merged: &NugetConfigMerge,
  proxy: Option<&ProxySettings>,
  mut environment: impl FnMut(&str) -> Option<String>,
) -> PackageHttpPolicy {
  let mut policy = DEFAULT_HTTP_POLICY;
  policy.max_requests_per_source = merged
    .max_http_requests_per_source
    .as_deref()
    .and_then(|value| value.parse::<u16>().ok())
    .filter(|value| *value > 0)
    .unwrap_or(DEFAULT_MAX_HTTP_REQUESTS_PER_SOURCE);
  policy.max_tries = environment("NUGET_ENHANCED_MAX_NETWORK_TRY_COUNT")
    .and_then(|value| value.parse::<u8>().ok())
    .filter(|value| (1..=MAX_NETWORK_TRIES).contains(value))
    .unwrap_or(DEFAULT_HTTP_POLICY.max_tries);
  policy.retry_delay_ms = environment("NUGET_ENHANCED_NETWORK_RETRY_DELAY_MILLISECONDS")
    .and_then(|value| value.parse::<u32>().ok())
    .filter(|value| *value <= MAX_RETRY_DELAY_MS)
    .unwrap_or(DEFAULT_HTTP_POLICY.retry_delay_ms);
  policy.max_retry_after_seconds = environment("NUGET_MAX_RETRY_AFTER_DELAY_SECONDS")
    .and_then(|value| value.parse::<u32>().ok())
    .filter(|value| *value <= MAX_RETRY_AFTER_SECONDS)
    .unwrap_or(DEFAULT_HTTP_POLICY.max_retry_after_seconds);
  set_http_flag(
    &mut policy.flags,
    HTTP_RETRY_429,
    environment("NUGET_RETRY_HTTP_429").and_then(|value| value.parse::<bool>().ok()).unwrap_or(true),
  );
  set_http_flag(
    &mut policy.flags,
    HTTP_OBSERVE_RETRY_AFTER,
    environment("NUGET_OBSERVE_RETRY_AFTER")
      .and_then(|value| value.parse::<bool>().ok())
      .unwrap_or(true),
  );
  if let Some(proxy) = proxy {
    policy.flags |= HTTP_PROXY_CONFIGURED;
    if proxy.username.is_some() {
      policy.flags |= HTTP_PROXY_AUTHENTICATED;
    }
    if proxy.no_proxy.is_some() {
      policy.flags |= HTTP_NO_PROXY_CONFIGURED;
    }
  }
  policy
}

fn set_http_flag(flags: &mut u8, mask: u8, enabled: bool) {
  if enabled {
    *flags |= mask;
  } else {
    *flags &= !mask;
  }
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
  let path = names.iter().map(|name| directory.join(name)).find(|path| path.is_file())?;
  #[cfg(target_os = "macos")]
  {
    // The default macOS filesystem is case-insensitive but case-preserving.
    // Preserve the real directory entry rather than the spelling of our probe.
    let mut selected = None::<(usize, PathBuf)>;
    if let Ok(entries) = fs::read_dir(directory) {
      for entry in entries.flatten() {
        let Some(name) = entry
          .file_name()
          .to_str()
          .and_then(|name| names.iter().position(|candidate| *candidate == name))
        else {
          continue;
        };
        if selected.as_ref().is_none_or(|(rank, _)| name < *rank) {
          selected = Some((name, entry.path()));
        }
      }
    }
    selected.map_or(Some(path), |(_, actual)| Some(actual))
  }
  #[cfg(not(target_os = "macos"))]
  {
    Some(path)
  }
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
  discover_configuration(project_directory, None, None, &[], CredentialProviderOptions::default()).map(|configuration| configuration.cache_root)
}

struct PendingTrustedSigner {
  name: String,
  service_index: Option<String>,
  owners: Vec<String>,
  certificates: Vec<TrustedCertificate>,
  kind: TrustedSignerKind,
}

fn merge_trusted_signers(bytes: &[u8], path: &Path, merged: &mut NugetConfigMerge) -> Result<(), PackageError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  let mut in_section = false;
  let mut in_owners = false;
  let mut owner_text = None::<String>;
  let mut pending = None::<PendingTrustedSigner>;
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) if local_name(element.name().as_ref()) == b"trustedSigners" => {
        if in_section {
          return Err(config_error(path, "NuGet trustedSigners sections cannot be nested"));
        }
        in_section = true;
      },
      Ok(Event::Start(element)) if in_section && matches!(local_name(element.name().as_ref()), b"author" | b"repository") => {
        begin_trusted_signer(&reader, &element, path, &mut pending)?;
      },
      Ok(Event::Start(element)) if in_section && local_name(element.name().as_ref()) == b"certificate" => {
        append_trusted_certificate(&reader, &element, path, pending.as_mut())?;
      },
      Ok(Event::Start(element)) if in_section && local_name(element.name().as_ref()) == b"owners" => {
        let signer = pending
          .as_ref()
          .ok_or_else(|| config_error(path, "NuGet trusted repository owners must be inside a repository"))?;
        if signer.kind != TrustedSignerKind::Repository || in_owners || owner_text.is_some() {
          return Err(config_error(path, "NuGet trusted repository must contain at most one owners element"));
        }
        in_owners = true;
      },
      Ok(Event::Empty(element)) if in_section && local_name(element.name().as_ref()) == b"clear" && pending.is_none() => {
        merged.trusted_signers.clear();
      },
      Ok(Event::Empty(element)) if in_section && local_name(element.name().as_ref()) == b"certificate" => {
        append_trusted_certificate(&reader, &element, path, pending.as_mut())?;
      },
      Ok(Event::Empty(element)) if in_section && matches!(local_name(element.name().as_ref()), b"author" | b"repository") => {
        return Err(config_error(path, "NuGet trusted signer requires at least one certificate"));
      },
      Ok(Event::Text(text)) if in_owners => {
        let value = text
          .xml_content(XmlVersion::Implicit1_0)
          .map_err(|error| config_error(path, format!("invalid NuGet trusted repository owners text: {error}")))?
          .into_owned();
        if owner_text.replace(value).is_some() {
          return Err(config_error(path, "NuGet trusted repository owners must contain one text value"));
        }
      },
      Ok(Event::End(element)) if in_section && local_name(element.name().as_ref()) == b"owners" => {
        let value = owner_text
          .take()
          .ok_or_else(|| config_error(path, "NuGet trusted repository owners cannot be empty"))?;
        let owners = value.split(';').map(str::trim).collect::<Vec<_>>();
        if owners.is_empty() || owners.iter().any(|owner| owner.is_empty()) {
          return Err(config_error(
            path,
            "NuGet trusted repository owners must be semicolon-delimited non-empty names",
          ));
        }
        pending.as_mut().expect("owners start required a signer").owners = owners.into_iter().map(str::to_owned).collect();
        in_owners = false;
      },
      Ok(Event::End(element)) if in_section && matches!(local_name(element.name().as_ref()), b"author" | b"repository") => {
        finish_trusted_signer(path, &mut merged.trusted_signers, &mut pending)?;
      },
      Ok(Event::End(element)) if local_name(element.name().as_ref()) == b"trustedSigners" => {
        if pending.is_some() || in_owners {
          return Err(config_error(path, "NuGet trusted signer did not close"));
        }
        in_section = false;
      },
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(config_error(path, format!("invalid NuGet configuration XML: {error}"))),
    }
  }
  if in_section || pending.is_some() || in_owners {
    return Err(config_error(path, "NuGet trustedSigners section did not close"));
  }
  Ok(())
}

fn begin_trusted_signer(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  path: &Path,
  pending: &mut Option<PendingTrustedSigner>,
) -> Result<(), PackageError> {
  if pending.is_some() {
    return Err(config_error(path, "NuGet trusted signers cannot be nested"));
  }
  let name = config_attribute(reader, element, b"name", path)?.ok_or_else(|| config_error(path, "NuGet trusted signer requires a name"))?;
  if name.is_empty() {
    return Err(config_error(path, "NuGet trusted signer name cannot be empty"));
  }
  let kind = if local_name(element.name().as_ref()) == b"author" {
    TrustedSignerKind::Author
  } else {
    TrustedSignerKind::Repository
  };
  let service_index = if kind == TrustedSignerKind::Repository {
    let value =
      config_attribute(reader, element, b"serviceIndex", path)?.ok_or_else(|| config_error(path, "NuGet trusted repository requires serviceIndex"))?;
    let url = reqwest::Url::parse(&value).map_err(|error| config_error(path, format!("invalid trusted repository serviceIndex: {error}")))?;
    if url.scheme() != "https" || !url.has_host() {
      return Err(config_error(path, "NuGet trusted repository serviceIndex must use HTTPS"));
    }
    Some(value)
  } else {
    None
  };
  *pending = Some(PendingTrustedSigner {
    name,
    service_index,
    owners: Vec::new(),
    certificates: Vec::new(),
    kind,
  });
  Ok(())
}

fn append_trusted_certificate(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  path: &Path,
  pending: Option<&mut PendingTrustedSigner>,
) -> Result<(), PackageError> {
  let signer = pending.ok_or_else(|| config_error(path, "NuGet trusted certificate must be inside an author or repository"))?;
  let fingerprint =
    config_attribute(reader, element, b"fingerprint", path)?.ok_or_else(|| config_error(path, "NuGet trusted certificate requires fingerprint"))?;
  let algorithm = config_attribute(reader, element, b"hashAlgorithm", path)?
    .and_then(|value| FingerprintAlgorithm::parse(&value))
    .ok_or_else(|| config_error(path, "NuGet trusted certificate hashAlgorithm must be SHA256, SHA384, or SHA512"))?;
  let allow_untrusted_root = config_attribute(reader, element, b"allowUntrustedRoot", path)?
    .ok_or_else(|| config_error(path, "NuGet trusted certificate requires allowUntrustedRoot"))?
    .parse::<bool>()
    .map_err(|_| config_error(path, "NuGet trusted certificate allowUntrustedRoot must be true or false"))?;
  let certificate = TrustedCertificate::parse(&fingerprint, algorithm, allow_untrusted_root)
    .map_err(|error| config_error(path, format!("invalid NuGet trusted certificate: {error}")))?;
  signer.certificates.push(certificate);
  Ok(())
}

fn finish_trusted_signer(path: &Path, signers: &mut Vec<TrustedSigner>, pending: &mut Option<PendingTrustedSigner>) -> Result<(), PackageError> {
  let pending = pending.take().ok_or_else(|| config_error(path, "NuGet trusted signer ended without a start"))?;
  if pending.certificates.is_empty() {
    return Err(config_error(path, "NuGet trusted signer requires at least one certificate"));
  }
  let signer = TrustedSigner {
    name: pending.name,
    service_index: pending.service_index,
    owners: pending.owners.into_boxed_slice(),
    certificates: pending.certificates.into_boxed_slice(),
    kind: pending.kind,
  };
  let existing = signers.iter_mut().find(|existing| match signer.kind {
    TrustedSignerKind::Author => existing.kind == TrustedSignerKind::Author && existing.name.eq_ignore_ascii_case(&signer.name),
    TrustedSignerKind::Repository => {
      existing.kind == TrustedSignerKind::Repository
        && existing
          .service_index
          .as_deref()
          .zip(signer.service_index.as_deref())
          .is_some_and(|(existing, candidate)| {
            reqwest::Url::parse(existing)
              .ok()
              .zip(reqwest::Url::parse(candidate).ok())
              .is_some_and(|(existing, candidate)| existing == candidate)
          })
    },
  });
  if let Some(existing) = existing {
    *existing = signer;
  } else {
    signers.push(signer);
  }
  Ok(())
}

fn merge_config(path: &Path, merged: &mut NugetConfigMerge) -> Result<(), PackageError> {
  let bytes = Zeroizing::new(fs::read(path).map_err(|error| package_io("read NuGet configuration", path, error))?);
  merge_trusted_signers(bytes.as_slice(), path, merged)?;
  let config_priority = merged.config_priority;
  let mut reader = Reader::from_reader(bytes.as_slice());
  reader.config_mut().trim_text(true);
  let mut section = ConfigSection::Other;
  let mut pending_mapping = None::<PendingSourceMapping>;
  let mut pending_credential = None::<PendingSourceCredential>;
  let mut mapping_sources_in_file = Vec::<String>::new();
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => match local_name(element.name().as_ref()) {
        _ if matches!(section, ConfigSection::Credentials) => {
          begin_source_credential(&reader, &element, path, &mut pending_credential)?;
        },
        b"fileCert" | b"storeCert" if matches!(section, ConfigSection::ClientCertificates) => {
          append_client_certificate(&reader, &element, path, &mut merged.client_certificates)?;
        },
        b"packageSources" => section = ConfigSection::Sources,
        b"packageSourceCredentials" => section = ConfigSection::Credentials,
        b"clientCertificates" => section = ConfigSection::ClientCertificates,
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
        ConfigSection::Credentials if pending_credential.is_none() => merged.credentials.clear(),
        ConfigSection::Credentials => return Err(config_error(path, "NuGet source credential groups do not support clear")),
        ConfigSection::ClientCertificates => merged.client_certificates.clear(),
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
            let allow_insecure_connections = config_attribute(&reader, &element, b"allowInsecureConnections", path)?
              .map(|value| expand_config_value(value, path))
              .transpose()?
              .as_deref()
              .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"));
            let disable_tls_validation = config_attribute(&reader, &element, b"disableTLSCertificateValidation", path)?
              .map(|value| expand_config_value(value, path))
              .transpose()?
              .as_deref()
              .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"));
            let source = PackageSource::parse(
              value,
              protocol.as_deref(),
              allow_insecure_connections,
              disable_tls_validation,
              path,
              path.parent().unwrap_or(Path::new(".")),
            )?;
            let sources = if matches!(section, ConfigSection::Sources) {
              &mut merged.sources
            } else {
              &mut merged.audit_sources
            };
            add_or_replace_source(sources, key, source);
          },
          ConfigSection::Credentials => {
            append_source_credential(&key, value, path, pending_credential.as_mut())?;
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
          ConfigSection::Config if key.eq_ignore_ascii_case("maxHttpRequestsPerSource") => {
            merged.max_http_requests_per_source = Some(value);
          },
          ConfigSection::Other | ConfigSection::ClientCertificates | ConfigSection::SourceMapping | ConfigSection::Config => {},
        }
      },
      Ok(Event::Empty(element))
        if matches!(section, ConfigSection::ClientCertificates) && matches!(local_name(element.name().as_ref()), b"fileCert" | b"storeCert") =>
      {
        append_client_certificate(&reader, &element, path, &mut merged.client_certificates)?;
      },
      Ok(Event::Empty(element)) if matches!(section, ConfigSection::ClientCertificates) => {
        return Err(config_error(
          path,
          format!(
            "unsupported NuGet clientCertificates element {:?}; expected fileCert or storeCert",
            String::from_utf8_lossy(local_name(element.name().as_ref()))
          ),
        ));
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
          ConfigSection::Credentials => return Err(config_error(path, "NuGet source credential groups do not support remove")),
          ConfigSection::ClientCertificates => return Err(config_error(path, "NuGet clientCertificates does not support remove")),
          ConfigSection::Disabled => merged.disabled.retain(|name| !name.eq_ignore_ascii_case(&key)),
          ConfigSection::AuditSources => merged.audit_sources.retain(|(name, _)| !name.eq_ignore_ascii_case(&key)),
          ConfigSection::SourceMapping => merged.source_mapping.remove(&key),
          ConfigSection::FallbackFolders => merged.fallback_folders.retain(|folder| !folder.name.eq_ignore_ascii_case(&key)),
          ConfigSection::Config => merged.remove_config(&key),
          ConfigSection::Other => {},
        }
      },
      Ok(Event::Empty(element)) if matches!(section, ConfigSection::Credentials) => {
        begin_source_credential(&reader, &element, path, &mut pending_credential)?;
        finish_source_credential(path, &mut merged.credentials, &mut pending_credential)?;
      },
      Ok(Event::End(element)) => match local_name(element.name().as_ref()) {
        _ if matches!(section, ConfigSection::Credentials) && pending_credential.is_some() => {
          finish_source_credential(path, &mut merged.credentials, &mut pending_credential)?;
        },
        b"packageSource" if matches!(section, ConfigSection::SourceMapping) => {
          let pending = pending_mapping
            .take()
            .ok_or_else(|| config_error(path, "NuGet package-source mapping ended without a source"))?;
          merged.source_mapping.finish_source(pending, path)?;
        },
        b"packageSourceCredentials" if matches!(section, ConfigSection::Credentials) => {
          if pending_credential.is_some() {
            return Err(config_error(path, "NuGet package-source credential group did not close"));
          }
          section = ConfigSection::Other;
        },
        b"packageSources"
        | b"clientCertificates"
        | b"disabledPackageSources"
        | b"auditSources"
        | b"packageSourceMapping"
        | b"fallbackPackageFolders"
        | b"config" => {
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
  if pending_credential.is_some() {
    return Err(config_error(path, "NuGet package-source credential group did not close"));
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

fn append_client_certificate(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  config_path: &Path,
  certificates: &mut Vec<MergedClientCertificate>,
) -> Result<(), PackageError> {
  let source = required_expanded_attribute(reader, element, b"packageSource", config_path)?;
  if source.is_empty() {
    return Err(config_error(config_path, "NuGet client certificate packageSource cannot be empty"));
  }
  let certificate = match local_name(element.name().as_ref()) {
    b"fileCert" => {
      let configured_path = required_expanded_attribute(reader, element, b"path", config_path)?;
      if configured_path.is_empty() {
        return Err(config_error(
          config_path,
          format!("NuGet file certificate for source {source:?} has an empty path"),
        ));
      }
      let password = config_attribute(reader, element, b"password", config_path)?;
      let clear_password = config_attribute(reader, element, b"clearTextPassword", config_path)?;
      if password.is_some() && clear_password.is_some() {
        return Err(config_error(
          config_path,
          format!("NuGet file certificate for source {source:?} cannot specify both password and clearTextPassword"),
        ));
      }
      let password = match (password, clear_password) {
        (Some(value), None) => Some(StoredCredentialPassword::Encrypted(expand_config_value(value, config_path)?)),
        (None, Some(value)) => Some(StoredCredentialPassword::Clear(Zeroizing::new(expand_config_value(value, config_path)?))),
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!("the conflicting password attributes returned above"),
      };
      MergedClientCertificate::File {
        source,
        path: resolve_config_path(config_path, &configured_path),
        password,
      }
    },
    b"storeCert" => MergedClientCertificate::Store {
      source,
      location: optional_expanded_attribute(reader, element, b"storeLocation", config_path)?.unwrap_or_else(|| "CurrentUser".to_owned()),
      name: optional_expanded_attribute(reader, element, b"storeName", config_path)?.unwrap_or_else(|| "My".to_owned()),
      find_by: optional_expanded_attribute(reader, element, b"findBy", config_path)?.unwrap_or_else(|| "Thumbprint".to_owned()),
      find_value: required_expanded_attribute(reader, element, b"findValue", config_path)?,
    },
    _ => return Err(config_error(config_path, "unsupported NuGet client certificate element")),
  };
  let file = certificate.is_file();
  if let Some(existing) = certificates
    .iter_mut()
    .find(|existing| existing.source() == certificate.source() && existing.is_file() == file)
  {
    *existing = certificate;
  } else {
    certificates.push(certificate);
  }
  Ok(())
}

fn required_expanded_attribute(reader: &Reader<&[u8]>, element: &quick_xml::events::BytesStart<'_>, name: &[u8], path: &Path) -> Result<String, PackageError> {
  let value = config_attribute(reader, element, name, path)?
    .ok_or_else(|| config_error(path, format!("NuGet client certificate requires attribute {:?}", String::from_utf8_lossy(name))))?;
  expand_config_value(value, path)
}

fn optional_expanded_attribute(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  name: &[u8],
  path: &Path,
) -> Result<Option<String>, PackageError> {
  config_attribute(reader, element, name, path)?
    .map(|value| expand_config_value(value, path))
    .transpose()
}

fn begin_source_credential(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  path: &Path,
  pending: &mut Option<PendingSourceCredential>,
) -> Result<(), PackageError> {
  if pending.is_some() {
    return Err(config_error(path, "NuGet package-source credential groups cannot be nested"));
  }
  let name = element.name();
  let encoded = reader
    .decoder()
    .decode(name.as_ref())
    .map_err(|error| config_error(path, format!("invalid NuGet credential source name: {error}")))?;
  let source = decode_xml_name(&encoded).map_err(|error| config_error(path, error))?;
  if source.is_empty() {
    return Err(config_error(path, "NuGet package-source credential name cannot be empty"));
  }
  *pending = Some(PendingSourceCredential {
    source,
    username: None,
    password: None,
    valid_authentication_types: None,
  });
  Ok(())
}

fn append_source_credential(key: &str, value: String, path: &Path, pending: Option<&mut PendingSourceCredential>) -> Result<(), PackageError> {
  let credential = pending.ok_or_else(|| config_error(path, "NuGet credential add must be inside a package-source group"))?;
  if key.eq_ignore_ascii_case("Username") {
    if credential.username.replace(Zeroizing::new(value)).is_some() {
      return Err(config_error(
        path,
        format!("NuGet package source {:?} has more than one Username", credential.source),
      ));
    }
  } else if key.eq_ignore_ascii_case("Password") {
    if credential.password.replace(StoredCredentialPassword::Encrypted(value)).is_some() {
      return Err(config_error(
        path,
        format!("NuGet package source {:?} has more than one password", credential.source),
      ));
    }
  } else if key.eq_ignore_ascii_case("ClearTextPassword") {
    if credential.password.replace(StoredCredentialPassword::Clear(Zeroizing::new(value))).is_some() {
      return Err(config_error(
        path,
        format!("NuGet package source {:?} has more than one password", credential.source),
      ));
    }
  } else if key.eq_ignore_ascii_case("ValidAuthenticationTypes") && credential.valid_authentication_types.replace(value).is_some() {
    return Err(config_error(
      path,
      format!("NuGet package source {:?} has more than one ValidAuthenticationTypes value", credential.source),
    ));
  }
  Ok(())
}

fn finish_source_credential(
  path: &Path,
  credentials: &mut Vec<MergedSourceCredential>,
  pending: &mut Option<PendingSourceCredential>,
) -> Result<(), PackageError> {
  let credential = pending
    .take()
    .ok_or_else(|| config_error(path, "NuGet package-source credential group ended without a start"))?;
  let username = credential
    .username
    .ok_or_else(|| config_error(path, format!("NuGet package source {:?} credential requires Username", credential.source)))?;
  let password = credential.password.ok_or_else(|| {
    config_error(
      path,
      format!("NuGet package source {:?} credential requires Password or ClearTextPassword", credential.source),
    )
  })?;
  let merged = MergedSourceCredential {
    source: credential.source,
    username,
    password,
    valid_authentication_types: credential.valid_authentication_types,
  };
  if let Some(existing) = credentials.iter_mut().find(|existing| existing.source == merged.source) {
    *existing = merged;
  } else {
    credentials.push(merged);
  }
  Ok(())
}

fn decode_xml_name(value: &str) -> Result<String, String> {
  let bytes = value.as_bytes();
  let mut units = Vec::with_capacity(value.encode_utf16().count());
  let mut offset = 0usize;
  while offset < bytes.len() {
    if offset + 7 <= bytes.len()
      && bytes[offset] == b'_'
      && matches!(bytes[offset + 1], b'x' | b'X')
      && bytes[offset + 6] == b'_'
      && let Ok(hex) = std::str::from_utf8(&bytes[offset + 2..offset + 6])
      && let Ok(unit) = u16::from_str_radix(hex, 16)
    {
      units.push(unit);
      offset += 7;
      continue;
    }
    let character = value[offset..].chars().next().expect("offset is inside the source name");
    let mut encoded = [0u16; 2];
    units.extend_from_slice(character.encode_utf16(&mut encoded));
    offset += character.len_utf8();
  }
  String::from_utf16(&units).map_err(|_| format!("NuGet credential source name {value:?} contains an invalid UTF-16 escape"))
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
    self.max_http_requests_per_source = None;
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
    } else if key.eq_ignore_ascii_case("maxHttpRequestsPerSource") {
      self.max_http_requests_per_source = None;
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
  #[cfg(test)]
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

  fn required_rank(&self, package_id: &str) -> Option<usize> {
    let mut best = 0usize;
    for source in &self.sources {
      for pattern in &self.patterns[range(source.patterns)] {
        let Some(rank) = self.pattern_rank(*pattern, package_id) else {
          continue;
        };
        best = best.max(rank);
      }
    }
    (best != 0).then_some(best)
  }

  fn enabled_rank(&self, package_id: &str) -> Option<usize> {
    let required = self.required_rank(package_id)?;
    self.source_indices_at_rank(package_id, required).next().map(|_| required)
  }

  fn source_indices_at_rank<'a>(&'a self, package_id: &'a str, required: usize) -> impl Iterator<Item = u32> + 'a {
    self.sources.iter().filter_map(move |source| {
      (source.source_index != u32::MAX
        && self.patterns[range(source.patterns)]
          .iter()
          .any(|pattern| self.pattern_rank(*pattern, package_id) == Some(required)))
      .then_some(source.source_index)
    })
  }

  fn source_matches_rank(&self, source_index: u32, package_id: &str, required: usize) -> bool {
    self
      .sources
      .iter()
      .filter(|source| source.source_index == source_index)
      .flat_map(|source| &self.patterns[range(source.patterns)])
      .any(|pattern| self.pattern_rank(*pattern, package_id) == Some(required))
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
  Credentials,
  ClientCertificates,
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
  configured_http_client_builder(proxy, 0)?
    .tls_backend_rustls()
    .build()
    .map_err(|error| network_error("HTTP client", format!("failed to create HTTP client: {error}")))
}

fn source_http_client(proxy: Option<&ProxySettings>, security_flags: u8) -> Result<reqwest::Client, PackageError> {
  configured_http_client_builder(proxy, security_flags)?
    .tls_backend_rustls()
    .build()
    .map_err(|error| network_error("package-source HTTP client", format!("failed to create HTTP client: {error}")))
}

fn configured_http_client_builder(proxy: Option<&ProxySettings>, security_flags: u8) -> Result<reqwest::ClientBuilder, PackageError> {
  let allow_insecure = security_flags & SOURCE_ALLOW_INSECURE_CONNECTIONS != 0;
  let disable_tls_validation = security_flags & SOURCE_DISABLE_TLS_VALIDATION != 0;
  let mut builder = reqwest::Client::builder()
    .https_only(!allow_insecure)
    .tls_danger_accept_invalid_certs(disable_tls_validation)
    .tls_danger_accept_invalid_hostnames(disable_tls_validation)
    .timeout(Duration::from_secs(DEFAULT_HTTP_POLICY.request_timeout_seconds as u64))
    .redirect(source_redirect_policy(allow_insecure));
  if let Some(settings) = proxy {
    let mut configured =
      reqwest::Proxy::all(&settings.url).map_err(|error| config_error(Path::new("http_proxy"), format!("invalid NuGet proxy address: {error}")))?;
    configured = configured.no_proxy(settings.no_proxy.as_deref().and_then(reqwest::NoProxy::from_string));
    if let (Some(username), Some(password)) = (&settings.username, &settings.password) {
      configured = configured.basic_auth(username, password);
    }
    builder = builder.no_proxy().proxy(configured);
  }
  Ok(builder)
}

fn source_redirect_policy(allow_insecure: bool) -> reqwest::redirect::Policy {
  reqwest::redirect::Policy::custom(move |attempt| {
    if attempt.previous().len() >= 10 {
      return attempt.error("NuGet redirect limit exceeded");
    }
    if attempt.url().scheme() != "https" && !(allow_insecure && attempt.url().scheme() == "http") {
      return attempt.error("NuGet redirects must use HTTPS unless allowInsecureConnections is true");
    }
    attempt.follow()
  })
}

struct LazyServiceEndpoints {
  slots: Vec<Option<ServiceEndpoint>>,
  snapshot: Arc<[ServiceEndpoint]>,
}

struct ServiceDiscoveryOptions<'a> {
  source_work: &'a mut [SourceWork],
  worker_budget: u8,
  allow_network: bool,
}

const _: () = assert!(size_of::<LazyServiceEndpoints>() == 40);
const _: () = assert!(align_of::<LazyServiceEndpoints>() == 8);
const _: () = assert!(size_of::<ServiceDiscoveryOptions>() == 24);
const _: () = assert!(align_of::<ServiceDiscoveryOptions>() == 8);

impl LazyServiceEndpoints {
  fn new(source_count: usize) -> Self {
    Self {
      slots: std::iter::repeat_with(|| None).take(source_count).collect(),
      snapshot: Arc::from([]),
    }
  }

  async fn ensure_identity(
    &mut self,
    client: &reqwest::Client,
    sources: &[(String, PackageSource)],
    credentials: &SourceCredentialBatch,
    mapping: Option<&PackageSourceMapping>,
    package_id: &str,
    options: ServiceDiscoveryOptions<'_>,
  ) -> Result<(), PackageError> {
    debug_assert!(options.worker_budget > 0);
    let worker_budget = usize::from(options.worker_budget.max(1));
    let allow_network = options.allow_network;
    let source_work = options.source_work;
    let required_rank = mapping.and_then(|mapping| mapping.required_rank(package_id));
    if mapping.is_some() && required_rank.is_none() {
      return Err(unmapped_identity(package_id));
    }

    let mut matched = false;
    let mut changed = false;
    let mut jobs = Vec::new();
    let mut select_source = |index: usize| -> Result<(), PackageError> {
      let (_, source) = &sources[index];
      let source_index = u32_len(index, "NuGet package-source index")?;
      matched = true;
      if self.slots[index].is_some() {
        return Ok(());
      }
      match source.protocol {
        NugetProtocol::Local => {
          let root = PathBuf::from(&source.url);
          self.slots[index] = Some(ServiceEndpoint::Local {
            source: source.url.clone(),
            layout: detect_local_feed_layout(&root)?,
            root,
            source_index,
          });
          changed = true;
        },
        NugetProtocol::V2 if allow_network => {
          self.slots[index] = Some(ServiceEndpoint::V2 {
            source: source.url.clone(),
            base: with_trailing_slash(source.url.clone()),
            credential: credentials.get(index).cloned(),
            source_index,
          });
          changed = true;
        },
        NugetProtocol::V3 if allow_network => jobs.push((index, source.url.clone(), credentials.get(index).cloned())),
        NugetProtocol::V2 | NugetProtocol::V3 => {},
      }
      Ok(())
    };
    if let Some(mapping) = mapping {
      for source_index in mapping.source_indices_at_rank(package_id, required_rank.expect("mapped identity has a rank")) {
        select_source(source_index as usize)?;
      }
    } else {
      for index in 0..sources.len() {
        select_source(index)?;
      }
    }
    if !matched {
      return if mapping.is_some() { Err(unmapped_identity(package_id)) } else { Ok(()) };
    }

    let completed_capacity = jobs.len();
    let mut pending = jobs.into_iter();
    let mut tasks = JoinSet::new();
    let mut completed = Vec::with_capacity(completed_capacity);
    loop {
      while tasks.len() < worker_budget {
        let Some((index, source, credential)) = pending.next() else {
          break;
        };
        let client = client.clone();
        tasks.spawn(async move {
          let result = fetch_v3_service_index(&client, credential.as_deref(), &source).await;
          (index, source, credential, result)
        });
      }
      let Some(result) = tasks.join_next().await else {
        break;
      };
      completed
        .push(result.map_err(|error| PackageError::new(PackageErrorKind::Io, "package-source scheduler", format!("service-index task failed: {error}")))?);
    }
    completed.sort_unstable_by_key(|(index, ..)| *index);
    for (index, source, credential, result) in completed {
      let (services, work) = result?;
      merge_source_work(
        source_work,
        SourceWork {
          downloaded_bytes: work.downloaded_bytes,
          duration_us: work.duration_us,
          source_index: u32_len(index, "NuGet package-source index")?,
          requests: work.requests,
        },
        &source,
      )?;
      if services.package_base_address().is_none() {
        return Err(network_error(&source, "NuGet v3 source has no compatible PackageBaseAddress resource"));
      }
      self.slots[index] = Some(ServiceEndpoint::V3 {
        source,
        services: Arc::new(services),
        credential,
        source_index: u32_len(index, "NuGet package-source index")?,
      });
      changed = true;
    }
    if changed {
      self.snapshot = endpoint_snapshot(&self.slots);
    }
    Ok(())
  }

  fn snapshot(&self) -> Arc<[ServiceEndpoint]> {
    Arc::clone(&self.snapshot)
  }
}

fn endpoint_snapshot(slots: &[Option<ServiceEndpoint>]) -> Arc<[ServiceEndpoint]> {
  slots
    .iter()
    .flatten()
    .filter(|endpoint| endpoint.protocol() == NugetProtocol::Local)
    .chain(slots.iter().flatten().filter(|endpoint| endpoint.protocol() != NugetProtocol::Local))
    .cloned()
    .collect::<Vec<_>>()
    .into()
}

fn unmapped_identity(package_id: &str) -> PackageError {
  PackageError::new(
    PackageErrorKind::UnmappedIdentity,
    package_id,
    format!("package source mapping selects no enabled source for package {package_id}"),
  )
}

fn source_mapping_selects(mapping: Option<&PackageSourceMapping>, selected_rank: Option<usize>, package_id: &str, source_index: u32) -> bool {
  mapping.is_none_or(|mapping| selected_rank.is_some_and(|required| mapping.source_matches_rank(source_index, package_id, required)))
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

async fn fetch_v3_service_index(
  client: &reqwest::Client,
  credential: Option<&SourceCredential>,
  source: &str,
) -> Result<(NugetServiceEndpoints, HttpWork), PackageError> {
  let payload = get_bytes(client, credential, source, MAX_JSON_BYTES, "NuGet service index").await?;
  let document: serde_json::Value =
    serde_json::from_slice(&payload.value).map_err(|error| network_error(source, format!("invalid NuGet service-index JSON: {error}")))?;
  let security_flags = credential.map_or(0, |credential| credential.security_flags);
  let endpoints = parse_v3_service_index(source, &document, security_flags)?;
  Ok((endpoints, payload.work))
}

fn parse_v3_service_index(source: &str, document: &serde_json::Value, security_flags: u8) -> Result<NugetServiceEndpoints, PackageError> {
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
    append_selected_service_endpoints(
      resources,
      service_types(capability),
      &supported_client,
      security_flags,
      &mut text,
      &mut endpoints,
    )?;
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
  security_flags: u8,
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
      if url.scheme() != "https" && !(url.scheme() == "http" && security_flags & SOURCE_ALLOW_INSECURE_CONNECTIONS != 0) {
        return Err(network_error(
          location,
          format!("NuGet service resource {resource_type} uses insecure HTTP; set allowInsecureConnections=true on its package source to opt in"),
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

async fn resolve_streaming_graph(client: &reqwest::Client, roots: GraphRoots<'_>, context: GraphContext<'_>) -> Result<ResolvedGraph, PackageError> {
  let GraphContext {
    config,
    options,
    target,
    target_text,
    runtime_identifier,
    runtime_graph,
    pruning,
    batch_metadata,
  } = context;
  let mut nodes = BTreeMap::<String, ConstraintNode>::new();
  let mut dirty = BTreeSet::new();
  for request in roots.direct {
    nodes.insert(
      request.lower_id.clone(),
      ConstraintNode {
        id: request.id.clone(),
        direct: Some(request.range.clone()),
        central_pin: None,
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
  let mut endpoints: Option<LazyServiceEndpoints> = None;
  let mut source_work = source_work_table(config.sources.len())?;
  let mut metadata_packages = BTreeMap::<(String, String), TaskCachedPackage>::new();
  let mut package_cold = BTreeMap::<(String, String), PackageColdMetadata>::new();
  let mut shared_metadata_hits = 0u32;
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
        central_transitive: node.direct.is_none() && node.central_pin.is_some(),
      });
      let generation = node.generation;
      if let Some(request) = &request
        && let Some(metadata) = batch_metadata.get(config, target, &lower_id, &request.version)
      {
        shared_metadata_hits = shared_metadata_hits
          .checked_add(1)
          .ok_or_else(|| resolution_error(&lower_id, "shared package metadata hit count overflow"))?;
        let ParsedPackageMetadata { dependencies, cold } = metadata;
        if !cold.is_empty() {
          package_cold.insert((lower_id.clone(), request.version.clone()), cold);
        }
        install_node_dependencies(&lower_id, generation, dependencies, roots.central_pins, &mut nodes, &mut dirty)?;
        stabilize_constraint_nodes(&mut nodes, &mut dirty, &mut ready, pruning)?;
        continue;
      }
      // Exact cache misses can delegate selection directly to endpoint
      // discovery. Ranged identities need this precheck so an unmapped but
      // already cached version batch remains source-independent like NuGet.
      let mapping_available = request.is_some() || config.source_mapping.as_ref().is_none_or(|mapping| mapping.enabled_rank(&lower_id).is_some());
      let needs_sources = match request.as_ref() {
        Some(request) => find_package_root(&config.cache_root, &config.fallback_roots, request).is_none(),
        None if !mapping_available => enumerate_cached_versions(&config.cache_root, &config.fallback_roots, &lower_id)?.is_empty(),
        None => true,
      };
      if needs_sources {
        if !mapping_available {
          return Err(unmapped_identity(&lower_id));
        }
        if !config.cache_root.is_dir() {
          fs::create_dir_all(&config.cache_root).map_err(|error| package_io("create package cache", &config.cache_root, error))?;
        }
        if endpoints.is_none() {
          endpoints = Some(LazyServiceEndpoints::new(config.sources.len()));
        }
        endpoints
          .as_mut()
          .expect("lazy endpoint state was initialized")
          .ensure_identity(
            client,
            &config.sources,
            &config.credentials,
            config.source_mapping.as_deref(),
            &lower_id,
            ServiceDiscoveryOptions {
              worker_budget: u8::try_from(MAX_DOWNLOAD_WORKERS - tasks.len()).expect("package worker budget fits u8"),
              allow_network: !options.offline,
              source_work: &mut source_work,
            },
          )
          .await?;
      }

      let task_client = client.clone();
      let task_cache_root = config.cache_root.clone();
      let task_fallback_roots = Arc::clone(&config.fallback_roots);
      let task_temp_root = config.temp_root.clone();
      let task_endpoints = endpoints.as_ref().map(LazyServiceEndpoints::snapshot).unwrap_or_else(|| Arc::from([]));
      let task_source_mapping = config.source_mapping.clone();
      let task_signature_policy = Arc::clone(&config.signature_policy);
      let task_version = request.as_ref().map(|request| request.version.clone());
      let task_target = target;
      in_flight.insert(lower_id.clone());
      tasks.spawn(async move {
        let storage = PackageStorage {
          cache_root: &task_cache_root,
          fallback_roots: &task_fallback_roots,
          temp_root: &task_temp_root,
          signature_policy: &task_signature_policy,
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

    if tasks.is_empty() {
      continue;
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
      MetadataTaskResult::Versions {
        versions,
        source_work: task_work,
      } => {
        for work in task_work {
          merge_source_work(&mut source_work, work, &lower_id)?;
        }
        if let Some(node) = nodes.get_mut(&lower_id) {
          node.available_versions = Some(versions);
          dirty.insert(lower_id);
        }
      },
      MetadataTaskResult::Requirements {
        metadata,
        source_work: task_work,
        failed_source_work,
        package,
      } => {
        for work in failed_source_work {
          merge_source_work(&mut source_work, work, &lower_id)?;
        }
        if let Some(work) = task_work {
          merge_source_work(&mut source_work, work, &lower_id)?;
        }
        if let Some(version) = task_version.as_deref() {
          batch_metadata.insert(config, target, &lower_id, version, &metadata, package.as_ref())?;
        }
        let ParsedPackageMetadata { dependencies, cold } = metadata;
        if !cold.is_empty()
          && let Some(version) = task_version.as_deref()
        {
          package_cold.insert((lower_id.clone(), version.to_owned()), cold);
        }
        if let Some(package) = package
          && let Some(version) = task_version.as_ref()
        {
          metadata_packages.insert((lower_id.clone(), version.clone()), package);
        }
        if stale {
          if nodes.get(&lower_id).is_some_and(|node| !node.pruned) {
            ready.insert(lower_id.clone());
          }
        } else {
          install_node_dependencies(&lower_id, generation, dependencies, roots.central_pins, &mut nodes, &mut dirty)?;
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
        central_transitive: node.direct.is_none() && node.central_pin.is_some(),
      },
    );
  }
  let downgrades = collect_downgrades(&nodes);

  let mut acquisition = BTreeMap::new();
  let mut acquired = BTreeMap::<String, (PackageRequest, CachedPackage)>::new();
  for (lower_id, request) in exact {
    match metadata_packages.remove(&(lower_id.clone(), request.version.clone())) {
      Some(cached) => {
        acquired.insert(lower_id, (request, cached.materialize()));
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
      if let Some(cached) = batch_metadata.package(config, target, &request.lower_id, &request.version) {
        acquired.insert(request.lower_id.clone(), (request, cached));
        continue;
      }
      let task_client = client.clone();
      let task_cache_root = config.cache_root.clone();
      let task_fallback_roots = Arc::clone(&config.fallback_roots);
      let task_temp_root = config.temp_root.clone();
      let task_endpoints = endpoints.as_ref().map(LazyServiceEndpoints::snapshot).unwrap_or_else(|| Arc::from([]));
      let task_source_mapping = config.source_mapping.clone();
      let task_signature_policy = Arc::clone(&config.signature_policy);
      let parallel_extract = acquisition_tasks.is_empty() && acquisition.is_empty();
      acquisition_tasks.spawn(async move {
        let storage = PackageStorage {
          cache_root: &task_cache_root,
          fallback_roots: &task_fallback_roots,
          temp_root: &task_temp_root,
          signature_policy: &task_signature_policy,
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
    if acquisition_tasks.is_empty() {
      continue;
    }
    let (request, cached) = acquisition_tasks
      .join_next()
      .await
      .ok_or_else(package_worker_stopped)?
      .map_err(package_blocking_task_error)?;
    let cached = cached?;
    for work in &cached.failed_source_work {
      merge_source_work(&mut source_work, *work, &request.lower_id)?;
    }
    if let Some(work) = cached.source_work {
      merge_source_work(&mut source_work, work, &request.lower_id)?;
    }
    batch_metadata.insert_package(config, target, &request.lower_id, &request.version, &cached);
    acquired.insert(request.lower_id.clone(), (request, cached));
  }

  let asset_flags = flatten_asset_flags(&nodes, roots.direct);
  let mut resolved = BTreeMap::<String, WorkPackage>::new();
  for (lower_id, (request, cached)) in acquired {
    let dependencies = concrete_dependencies(&nodes, &request.lower_id)?;
    let flags = asset_flags.get(&lower_id).copied().unwrap_or(AssetFlags::NONE);
    let cold = package_cold.remove(&(lower_id.clone(), request.version.clone())).unwrap_or_default();
    let parsed = parse_cached_package(
      request.clone(),
      cached,
      PackageAssetContext {
        target,
        target_text,
        runtime_identifier,
        runtime_graph,
        flags,
      },
      dependencies,
      cold,
    )?;
    resolved.insert(lower_id, parsed);
  }

  Ok(ResolvedGraph {
    packages: resolved,
    source_work,
    downgrades,
    shared_metadata_hits,
  })
}

fn flatten_asset_flags(nodes: &BTreeMap<String, ConstraintNode>, direct: &[PackageRequirement]) -> BTreeMap<String, AssetFlags> {
  let mut result = BTreeMap::<String, AssetFlags>::new();
  let mut queue = VecDeque::<(&str, AssetFlags)>::new();
  for reference in direct {
    if nodes.get(&reference.lower_id).is_some_and(|node| !node.pruned) {
      queue.push_back((&reference.lower_id, reference.include_assets));
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

fn select_node_version_with_constraints(node: &ConstraintNode, constraints: ConstraintView<'_>) -> Result<NodeSelection, PackageError> {
  fn consider_lower<'a>(candidate: &mut Option<&'a PackageVersion>, range: &'a VersionRange) {
    if let Some(lower) = &range.lower
      && lower.inclusive
      && candidate.is_none_or(|candidate| lower.version > *candidate)
    {
      *candidate = Some(&lower.version);
    }
  }

  if node.direct.is_none()
    && let Some(pin) = &node.central_pin
  {
    if let Some(required) = node.constraints.values().find(|range| !range.contains(pin)) {
      return Err(
        PackageError::new(
          PackageErrorKind::Downgrade,
          &node.id,
          format!(
            "central package pin selects {} {}, which does not satisfy {}",
            node.id,
            pin.normalized,
            required.diagnostic_text()
          ),
        )
        .with_context("package_id", &node.id)
        .with_context("selected_version", &pin.normalized)
        .with_context("required_range", required.diagnostic_text())
        .with_cause("CentralPackageTransitivePinning forced the selected version"),
      );
    }
    if node.available_versions.as_ref().is_some_and(|versions| versions.binary_search(pin).is_err()) {
      return Err(version_not_found_error(
        &node.id,
        &format!("[{}]", pin.normalized),
        node.available_versions.as_deref(),
      ));
    }
    return Ok(NodeSelection::Version(pin.clone()));
  }

  if node.direct.is_none() && constraints.has_empty_intersection() {
    return Err(
      PackageError::new(
        PackageErrorKind::ConstraintConflict,
        &node.id,
        format!("dependency constraints for {} have no common version", node.id),
      )
      .with_context("package_id", &node.id)
      .with_context("required_ranges", constraints.diagnostic_ranges()),
    );
  }

  let preferred = node.direct.as_ref();
  let allows_prerelease = preferred.map_or_else(|| constraints.any(VersionRange::allows_prerelease), VersionRange::allows_prerelease);
  let accepts = |version: &PackageVersion| {
    (version.prerelease().is_none() || allows_prerelease)
      && node
        .direct
        .as_ref()
        .map_or_else(|| constraints.all(|range| range.contains(version)), |direct| direct.contains(version))
  };
  if let Some(versions) = &node.available_versions {
    let preference = preferred
      .filter(|range| range.is_floating())
      .or_else(|| constraints.first(VersionRange::is_floating));
    let mut selected = None;
    for version in versions.iter().filter(|version| accepts(version)) {
      if preference.is_none() {
        return Ok(NodeSelection::Version(version.clone()));
      }
      if selected.is_none_or(|current| preference.expect("a checked preference exists").is_better(current, version)) {
        selected = Some(version);
      }
    }
    return selected.cloned().map(NodeSelection::Version).ok_or_else(|| {
      let requested = if let Some(direct) = &node.direct {
        direct.diagnostic_text()
      } else {
        constraints.diagnostic_ranges()
      };
      version_not_found_error(&node.id, &requested, node.available_versions.as_deref())
    });
  }
  if preferred.is_some_and(VersionRange::is_floating) || constraints.any(VersionRange::is_floating) {
    return Ok(NodeSelection::Enumerate);
  }
  let candidate = if let Some(direct) = &node.direct {
    let mut candidate = None;
    consider_lower(&mut candidate, direct);
    candidate
  } else {
    constraints.highest_inclusive_lower()
  };
  match candidate {
    Some(candidate) if accepts(candidate) => Ok(NodeSelection::Version(candidate.clone())),
    _ => Ok(NodeSelection::Enumerate),
  }
}

#[cfg(test)]
fn select_node_version(node: &ConstraintNode) -> Result<NodeSelection, PackageError> {
  select_node_version_with_constraints(node, ConstraintView::All(&node.constraints))
}

fn constraint_parent_is_ancestor<'a>(
  nodes: &'a BTreeMap<String, ConstraintNode>,
  target: &str,
  ancestor: &'a str,
  descendant: &'a str,
  stack: &mut Vec<&'a str>,
  visited: &mut Vec<&'a str>,
) -> bool {
  stack.clear();
  visited.clear();
  stack.push(descendant);
  while let Some(current) = stack.pop() {
    if current == ancestor {
      return true;
    }
    if current == target || visited.contains(&current) {
      continue;
    }
    visited.push(current);
    if let Some(node) = nodes.get(current) {
      stack.extend(node.constraints.keys().map(String::as_str).filter(|parent| *parent != target));
    }
  }
  false
}

fn constraint_parent_has_alternate_root_path<'a>(
  nodes: &'a BTreeMap<String, ConstraintNode>,
  target: &str,
  blocked: &str,
  descendant: &str,
  stack: &mut Vec<&'a str>,
  visited: &mut Vec<&'a str>,
) -> bool {
  stack.clear();
  visited.clear();
  stack.extend(
    nodes
      .iter()
      .filter(|(id, node)| node.direct.is_some() && id.as_str() != blocked)
      .map(|(id, _)| id.as_str()),
  );
  while let Some(current) = stack.pop() {
    if current == descendant {
      return true;
    }
    if current == target || current == blocked || visited.contains(&current) {
      continue;
    }
    visited.push(current);
    if let Some(node) = nodes.get(current) {
      stack.extend(node.dependencies.iter().map(|dependency| dependency.lower_id.as_str()));
    }
  }
  false
}

fn collect_active_constraints<'a>(
  nodes: &'a BTreeMap<String, ConstraintNode>,
  target: &str,
  active: &mut Vec<&'a VersionRange>,
  stack: &mut Vec<&'a str>,
  visited: &mut Vec<&'a str>,
) {
  active.clear();
  let node = &nodes[target];
  for (parent, range) in &node.constraints {
    if !constraint_is_dominated(nodes, target, parent, stack, visited) {
      active.push(range);
    }
  }
}

fn constraint_is_dominated<'a>(
  nodes: &'a BTreeMap<String, ConstraintNode>,
  target: &str,
  parent: &'a str,
  stack: &mut Vec<&'a str>,
  visited: &mut Vec<&'a str>,
) -> bool {
  nodes[target].constraints.keys().any(|candidate| {
    candidate != parent
      && constraint_parent_is_ancestor(nodes, target, candidate, parent, stack, visited)
      && !constraint_parent_is_ancestor(nodes, target, parent, candidate, stack, visited)
      && !constraint_parent_has_alternate_root_path(nodes, target, candidate, parent, stack, visited)
  })
}

fn collect_downgrades(nodes: &BTreeMap<String, ConstraintNode>) -> Vec<ResolvedDowngrade> {
  let mut downgrades = Vec::new();
  let mut stack = Vec::new();
  let mut visited = Vec::new();
  for (lower_id, node) in nodes {
    let Some(selected) = node.selected.as_ref().filter(|_| !node.pruned) else {
      continue;
    };
    for (parent, range) in &node.constraints {
      if range.contains(selected) {
        continue;
      }
      if node.direct.is_none() && !constraint_is_dominated(nodes, lower_id, parent, &mut stack, &mut visited) {
        continue;
      }
      downgrades.push(ResolvedDowngrade {
        package_id: node.id.clone(),
        selected_version: selected.normalized.clone(),
        requested_range: range.diagnostic_text(),
        requesting_package: nodes.get(parent).map_or_else(|| parent.clone(), |parent| parent.id.clone()),
      });
    }
  }
  downgrades
}

fn mark_descendant_constraint_targets_dirty(nodes: &BTreeMap<String, ConstraintNode>, root: &str, dirty: &mut BTreeSet<String>) {
  // Topology changes are uncommon and external graph depth is unbounded, so a
  // dynamically sized borrowed-identity traversal is required on this path.
  let mut stack = vec![root];
  let mut visited = Vec::new();
  while let Some(current) = stack.pop() {
    if visited.contains(&current) {
      continue;
    }
    visited.push(current);
    if let Some(node) = nodes.get(current) {
      for dependency in &node.dependencies {
        dirty.insert(dependency.lower_id.clone());
        stack.push(&dependency.lower_id);
      }
    }
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
      // Removing the last incoming edge can change which constraints dominate
      // anywhere below this node even when every selected version stays equal.
      for dependency in &node.dependencies {
        mark_descendant_constraint_targets_dirty(nodes, &dependency.lower_id, dirty);
      }
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
    let selection = if node.direct.is_none() && node.central_pin.is_none() && node.constraints.len() > 1 {
      // Parent count and ancestry depth come from external package metadata.
      // Allocate only on the multi-parent path; zero/one-parent selection stays
      // allocation-free and straight-line.
      let mut active_constraints = Vec::with_capacity(node.constraints.len());
      let mut ancestry_stack = Vec::new();
      let mut ancestry_visited = Vec::new();
      collect_active_constraints(nodes, &lower_id, &mut active_constraints, &mut ancestry_stack, &mut ancestry_visited);
      select_node_version_with_constraints(node, ConstraintView::Active(&active_constraints))?
    } else {
      select_node_version_with_constraints(node, ConstraintView::All(&node.constraints))?
    };
    let next = match selection {
      NodeSelection::Version(version) => Some(version),
      NodeSelection::Enumerate => None,
    };
    let pruned = next
      .as_ref()
      .is_some_and(|version| node.direct.is_none() && node.central_pin.is_none() && pruning.contains(&lower_id, version));
    if next.is_some() && nodes.get(&lower_id).is_some_and(|node| node.selected == next && node.pruned == pruned) {
      continue;
    }
    for dependency in &node.dependencies {
      mark_descendant_constraint_targets_dirty(nodes, &dependency.lower_id, dirty);
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
  central_pins: &[CentralPackagePin],
  nodes: &mut BTreeMap<String, ConstraintNode>,
  dirty: &mut BTreeSet<String>,
) -> Result<(), PackageError> {
  let mut dependencies = dependencies;
  dependencies.sort_unstable_by(|left, right| left.lower_id.cmp(&right.lower_id));
  for duplicate in dependencies.windows(2) {
    if duplicate[0].lower_id == duplicate[1].lower_id && duplicate[0].range != duplicate[1].range {
      return Err(resolution_error(
        &duplicate[1].id,
        "a package dependency group contains duplicate identities with different ranges",
      ));
    }
  }
  dependencies.dedup_by(|left, right| left.lower_id == right.lower_id);
  {
    let node = nodes
      .get_mut(lower_id)
      .ok_or_else(|| resolution_error(lower_id, "package graph node disappeared"))?;
    if node.generation != generation {
      return Ok(());
    }
    node.metadata_version = node.selected.clone();
  }
  for dependency in &dependencies {
    if nodes.get(&dependency.lower_id).is_some_and(|node| node.metadata_version.is_some()) {
      mark_descendant_constraint_targets_dirty(nodes, &dependency.lower_id, dirty);
    }
    let central_pin = central_pins
      .binary_search_by(|pin| pin.lower_id.as_str().cmp(&dependency.lower_id))
      .ok()
      .map(|index| central_pins[index].version.clone());
    let child = nodes.entry(dependency.lower_id.clone()).or_insert_with(|| ConstraintNode {
      id: dependency.id.clone(),
      direct: None,
      central_pin: central_pin.clone(),
      constraints: BTreeMap::new(),
      selected: None,
      metadata_version: None,
      dependencies: Vec::new(),
      available_versions: None,
      pruned: false,
      generation: 0,
    });
    if child.central_pin.is_none() {
      child.central_pin = central_pin;
    }
    child.constraints.insert(lower_id.to_owned(), dependency.range.clone());
    dirty.insert(dependency.lower_id.clone());
  }
  nodes.get_mut(lower_id).expect("a checked node exists").dependencies = dependencies;
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
        central_transitive: node.direct.is_none() && node.central_pin.is_some(),
      }))
    })
    .collect()
}

fn resolution_error(context: impl Into<String>, message: impl Into<String>) -> PackageError {
  PackageError::new(PackageErrorKind::Resolution, context, message)
}

fn version_not_found_error(id: &str, requested: &str, available: Option<&[PackageVersion]>) -> PackageError {
  let mut error = PackageError::new(
    PackageErrorKind::VersionNotFound,
    id,
    format!("package {id} has no available version satisfying {requested}"),
  )
  .with_context("package_id", id)
  .with_context("required_range", requested);
  if let Some(versions) = available {
    error = error.with_context("available_versions", versions.len().to_string());
    if let Some(nearest) = versions.last() {
      error = error.with_context("nearest_version", &nearest.normalized);
    }
  }
  error
}

#[derive(Clone, Copy)]
struct PackageStorage<'a> {
  cache_root: &'a Path,
  fallback_roots: &'a [PathBuf],
  temp_root: &'a Path,
  signature_policy: &'a Arc<SignaturePolicy>,
}

const _: () = assert!(size_of::<PackageStorage<'_>>() == 7 * size_of::<usize>());
const _: () = assert!(align_of::<PackageStorage<'_>>() == align_of::<usize>());

const _: () = assert!(size_of::<PackageStorage<'static>>() == 56);
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
  if let Some(request) = request
    && let Some(root) = find_package_root(storage.cache_root, storage.fallback_roots, request)
  {
    let request = request.clone();
    let signature_policy = Arc::clone(storage.signature_policy);
    let metadata = tokio::task::spawn_blocking(move || {
      if signature_policy.mode == SignatureValidationMode::Require {
        validate_cached_package(&root, &request, true, &signature_policy)?;
      }
      read_cached_metadata(&root, &request, target)
    })
    .await
    .map_err(package_blocking_task_error)??;
    return Ok(MetadataTaskResult::Requirements {
      metadata,
      source_work: None,
      failed_source_work: Box::new([]),
      package: None,
    });
  }
  let selected_rank = source_mapping.and_then(|mapping| mapping.enabled_rank(lower_id));
  let unmapped = source_mapping.is_some() && selected_rank.is_none();
  let has_selected_endpoint = endpoints
    .iter()
    .any(|endpoint| source_mapping_selects(source_mapping, selected_rank, lower_id, endpoint.source_index()));
  if request.is_none() && !has_selected_endpoint {
    let cached_versions = enumerate_cached_versions(storage.cache_root, storage.fallback_roots, lower_id)?;
    if !cached_versions.is_empty() {
      return Ok(MetadataTaskResult::Versions {
        versions: cached_versions,
        source_work: Vec::new(),
      });
    }
  }
  if unmapped {
    return Err(unmapped_identity(lower_id));
  }
  if !has_selected_endpoint {
    if request.is_some() {
      let cached_versions = enumerate_cached_versions(storage.cache_root, storage.fallback_roots, lower_id)?;
      if !cached_versions.is_empty() {
        return Ok(MetadataTaskResult::Versions {
          versions: cached_versions,
          source_work: Vec::new(),
        });
      }
    }
    return Err(PackageError::new(
      PackageErrorKind::OfflineMiss,
      lower_id,
      format!("package {lower_id} has no compatible version in the global package cache"),
    ));
  }

  let mut exact_source_work = Vec::new();
  if let Some(request) = request {
    match ensure_package(client, request, storage, endpoints, source_mapping, target, false).await {
      Ok(mut cached) => {
        let metadata = match cached.metadata.take() {
          Some(metadata) => metadata,
          None => read_cached_metadata(&cached.root, request, target)?,
        };
        let source_work = cached.source_work.take();
        let failed_source_work = std::mem::take(&mut cached.failed_source_work);
        return Ok(MetadataTaskResult::Requirements {
          metadata,
          source_work,
          failed_source_work,
          package: Some(TaskCachedPackage::from_cached(cached)),
        });
      },
      Err(mut error) if error.kind() == PackageErrorKind::Network => exact_source_work = error.take_source_work(),
      Err(error) => return Err(error),
    }
  }

  // NuGet considers the global packages folder alongside enabled sources for
  // floating/ranged requests. Local sources must not hide already cached
  // versions merely because they remain available in offline mode.
  let mut versions = enumerate_cached_versions(storage.cache_root, storage.fallback_roots, lower_id)?;
  exact_source_work.reserve(endpoints.len());
  let mut source_work = exact_source_work;
  for endpoint in endpoints {
    if !source_mapping_selects(source_mapping, selected_rank, lower_id, endpoint.source_index()) {
      continue;
    }
    match endpoint {
      ServiceEndpoint::Local { .. } => versions.extend(enumerate_local_versions(endpoint, lower_id)?),
      ServiceEndpoint::V3 { services, .. } => {
        let package_base = services.package_base_address().expect("v3 endpoint discovery requires package content");
        let separator = if package_base.ends_with('/') { "" } else { "/" };
        let url = format!("{package_base}{separator}{lower_id}/index.json");
        let payload = get_optional_bytes(client, endpoint.credential(), &url, MAX_JSON_BYTES, "NuGet package version index").await?;
        source_work.push(SourceWork {
          downloaded_bytes: payload.work.downloaded_bytes,
          duration_us: payload.work.duration_us,
          source_index: endpoint.source_index(),
          requests: payload.work.requests,
        });
        let Some(body) = payload.value else {
          continue;
        };
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
        let batch = enumerate_v2_versions(client, endpoint.credential(), base, lower_id).await?;
        versions.extend(batch.versions);
        source_work.push(SourceWork {
          downloaded_bytes: batch.work.downloaded_bytes,
          duration_us: batch.work.duration_us,
          source_index: endpoint.source_index(),
          requests: batch.work.requests,
        });
      },
    }
  }
  versions.sort_unstable();
  versions.dedup();
  if versions.is_empty() {
    return Err(
      PackageError::new(
        PackageErrorKind::PackageNotFound,
        lower_id,
        format!("no enabled source contains package {lower_id}"),
      )
      .with_context("package_id", lower_id),
    );
  }
  Ok(MetadataTaskResult::Versions { versions, source_work })
}

#[derive(Deserialize)]
struct V3VersionIndex {
  versions: Vec<String>,
}

struct VersionBatch {
  versions: Vec<PackageVersion>,
  work: HttpWork,
}

async fn enumerate_v2_versions(
  client: &reqwest::Client,
  credential: Option<&SourceCredential>,
  base: &str,
  lower_id: &str,
) -> Result<VersionBatch, PackageError> {
  let security_flags = credential.map_or(0, |credential| credential.security_flags);
  let mut url = format!("{base}FindPackagesById()?id='{lower_id}'&semVerLevel=2.0.0");
  let mut visited = HashSet::new();
  let mut versions = Vec::new();
  let mut work = HttpWork::default();
  loop {
    if !visited.insert(url.clone()) {
      return Err(network_error(&url, "NuGet v2 version enumeration contains a continuation cycle"));
    }
    if visited.len() > MAX_ARCHIVE_ENTRIES {
      return Err(network_error(&url, "NuGet v2 version enumeration exceeds the page count limit"));
    }
    let payload = get_optional_bytes(client, credential, &url, MAX_JSON_BYTES, "NuGet v2 version page").await?;
    work.merge(payload.work, &url)?;
    let Some(body) = payload.value else {
      break;
    };
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
    if !continuation.username().is_empty() || continuation.password().is_some() {
      return Err(network_error(continuation.as_str(), "NuGet v2 continuation URL must not embed credentials"));
    }
    if continuation.scheme() != "https" && !(continuation.scheme() == "http" && security_flags & SOURCE_ALLOW_INSECURE_CONNECTIONS != 0) {
      return Err(network_error(
        continuation.as_str(),
        "NuGet v2 continuation URL must use HTTPS unless allowInsecureConnections is true",
      ));
    }
    url = continuation.into();
  }
  Ok(VersionBatch { versions, work })
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

fn read_cached_metadata(root: &Path, request: &PackageRequest, target: TargetFramework) -> Result<ParsedPackageMetadata, PackageError> {
  let nuspec_path = find_nuspec(root)?;
  let nuspec = fs::read(&nuspec_path).map_err(|error| package_io("read package manifest", &nuspec_path, error))?;
  parse_nuspec_metadata(&nuspec_path, &nuspec, request, target)
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

async fn get_optional_bytes(
  client: &reqwest::Client,
  credential: Option<&SourceCredential>,
  url: &str,
  limit: u64,
  kind: &str,
) -> Result<HttpPayload<Option<Vec<u8>>>, PackageError> {
  let mut response = send_authenticated(client, credential, url, "HTTP request").await?;
  if response.status() == reqwest::StatusCode::NOT_FOUND {
    let work = response.work(0);
    return Ok(HttpPayload { value: None, work });
  }
  if let Err(error) = response.error_for_status_ref() {
    let work = response.work(0);
    return Err(network_error(url, format!("HTTP request failed: {error}")).with_http_work(work));
  }
  if response.content_length().is_some_and(|length| length > limit) {
    let work = response.work(0);
    return Err(network_error(url, format!("{kind} response exceeds the {limit} byte limit")).with_http_work(work));
  }
  let mut bytes = Vec::with_capacity(response.content_length().unwrap_or(0).min(limit) as usize);
  loop {
    let chunk = match response.chunk(url, kind).await {
      Ok(Some(chunk)) => chunk,
      Ok(None) => break,
      Err(error) => return Err(error.with_http_work(response.work(bytes.len() as u64))),
    };
    if bytes.len().checked_add(chunk.len()).is_none_or(|length| length as u64 > limit) {
      let downloaded = bytes.len().saturating_add(chunk.len()).min(u64::MAX as usize) as u64;
      return Err(network_error(url, format!("{kind} response exceeds the {limit} byte limit")).with_http_work(response.work(downloaded)));
    }
    bytes.extend_from_slice(&chunk);
  }
  let work = response.work(bytes.len() as u64);
  Ok(HttpPayload { value: Some(bytes), work })
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
    let signature_policy = Arc::clone(storage.signature_policy);
    return tokio::task::spawn_blocking(move || validate_cached_package(&root, &request, true, &signature_policy))
      .await
      .map_err(package_blocking_task_error)?;
  }
  let selected_rank = source_mapping.and_then(|mapping| mapping.enabled_rank(&request.lower_id));
  if source_mapping.is_some() && selected_rank.is_none() {
    return Err(unmapped_identity(&request.id));
  }
  let mut last_error = None;
  let mut failed_source_work = Vec::new();
  for endpoint in endpoints {
    if !source_mapping_selects(source_mapping, selected_rank, &request.lower_id, endpoint.source_index()) {
      continue;
    }
    match download_and_publish(client, request, storage, endpoint, target, parallel_extract).await {
      Ok(mut package) => {
        package.failed_source_work = failed_source_work.into_boxed_slice();
        return Ok(package);
      },
      Err(mut error) if error.kind() == PackageErrorKind::Network => {
        if let Some(work) = error.take_http_work() {
          failed_source_work.push(SourceWork {
            downloaded_bytes: work.downloaded_bytes,
            duration_us: work.duration_us,
            source_index: endpoint.source_index(),
            requests: work.requests,
          });
        }
        last_error = Some(error);
      },
      Err(error) => return Err(error),
    }
  }
  Err(
    last_error
      .unwrap_or_else(|| {
        PackageError::new(
          PackageErrorKind::Network,
          format!("{} {}", request.id, request.version),
          format!("no enabled source could provide package {} {}", request.id, request.version),
        )
      })
      .with_source_work(failed_source_work),
  )
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
  work: HttpWork,
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
  work: HttpWork,
  signature_policy: Arc<SignaturePolicy>,
  target: TargetFramework,
  parallel_extract: bool,
}

async fn download_and_publish(
  client: &reqwest::Client,
  request: &PackageRequest,
  storage: PackageStorage<'_>,
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
    let cache_root = storage.cache_root.to_owned();
    let endpoint = endpoint.clone();
    let signature_policy = Arc::clone(storage.signature_policy);
    return tokio::task::spawn_blocking(move || install_local_package(&archive, request, cache_root, endpoint, signature_policy, target, parallel_extract))
      .await
      .map_err(package_blocking_task_error)?;
  }
  let metadata = match endpoint {
    ServiceEndpoint::Local { .. } => unreachable!("local package acquisition returned above"),
    ServiceEndpoint::V2 { base, .. } => v2_package_metadata(client, endpoint.credential(), request, base).await?,
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

  let scratch_root = unique_temp_root(storage.temp_root, request);
  tokio::fs::create_dir_all(&scratch_root)
    .await
    .map_err(|error| package_io("create package scratch directory", &scratch_root, error))?;
  let scratch_guard = TempGuard(Some(scratch_root.clone()));
  let nupkg_name = format!("{}.{}.nupkg", request.lower_id, request.version);
  let scratch_nupkg = scratch_root.join(&nupkg_name);
  let (hash, package_work) = download_package(client, endpoint.credential(), &metadata.content_url, &scratch_nupkg).await?;
  let bytes = package_work.downloaded_bytes;
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
  let staging_root = unique_temp_root(storage.cache_root, request);
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
  let mut work = metadata.work;
  work.merge(package_work, &metadata.content_url)?;
  let downloaded = DownloadedPackage {
    request: request.clone(),
    cache_root: storage.cache_root.to_owned(),
    endpoint: endpoint.clone(),
    temp_root: staging_root,
    nupkg_name,
    nupkg_path,
    hash,
    work,
    signature_policy: Arc::clone(storage.signature_policy),
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
  signature_policy: Arc<SignaturePolicy>,
  target: TargetFramework,
  parallel_extract: bool,
) -> Result<CachedPackage, PackageError> {
  let source_started = Instant::now();
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
    work: HttpWork {
      downloaded_bytes: bytes,
      duration_us: elapsed_us(source_started),
      requests: 0,
    },
    signature_policy,
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
  package_signature::verify_package(&downloaded.nupkg_path, &downloaded.signature_policy)?;
  validate_and_extract_archive(&downloaded.nupkg_path, &downloaded.temp_root, downloaded.parallel_extract)?;
  normalize_nuspec_name(&downloaded.temp_root, &downloaded.request)?;
  let nuspec_path = downloaded.temp_root.join(format!("{}.nuspec", downloaded.request.lower_id));
  let nuspec = fs::read(&nuspec_path).map_err(|error| package_io("read package manifest", &nuspec_path, error))?;
  let metadata = parse_nuspec_metadata(&nuspec_path, &nuspec, &downloaded.request, downloaded.target)?;
  fs::write(
    downloaded.temp_root.join(format!("{}.sha512", downloaded.nupkg_name)),
    downloaded.hash.as_bytes(),
  )
  .map_err(|error| package_io("write package hash", &downloaded.temp_root, error))?;
  let package_metadata = serde_json::json!({
    "schemaVersion": 1,
    "sha512": &downloaded.hash,
    "source": redact_url_for_output(downloaded.endpoint.source()),
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
      metadata: Some(metadata),
      cache_hit: false,
      source_work: Some(SourceWork {
        downloaded_bytes: downloaded.work.downloaded_bytes,
        duration_us: downloaded.work.duration_us,
        source_index: downloaded.endpoint.source_index(),
        requests: downloaded.work.requests,
      }),
      failed_source_work: Box::new([]),
      origin: None,
    }
  } else {
    let mut cached = validate_cached_package(&final_root, &downloaded.request, false, &downloaded.signature_policy)?;
    cached.source_work = Some(SourceWork {
      downloaded_bytes: downloaded.work.downloaded_bytes,
      duration_us: downloaded.work.duration_us,
      source_index: downloaded.endpoint.source_index(),
      requests: downloaded.work.requests,
    });
    cached
  };
  cached.origin = Some(PackageSource {
    url: downloaded.endpoint.source().to_owned(),
    protocol: downloaded.endpoint.protocol(),
    security_flags: 0,
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
    work: HttpWork::default(),
  }
}

async fn v2_package_metadata(
  client: &reqwest::Client,
  credential: Option<&SourceCredential>,
  request: &PackageRequest,
  base: &str,
) -> Result<PackageMetadata, PackageError> {
  let metadata_url = format!("{base}Packages(Id='{}',Version='{}')", request.id, request.version);
  let payload = get_bytes(client, credential, &metadata_url, MAX_JSON_BYTES, "NuGet v2 metadata").await?;
  let security_flags = credential.map_or(0, |credential| credential.security_flags);
  let mut metadata = parse_v2_package_metadata(request, &metadata_url, &payload.value, security_flags)?;
  metadata.work = payload.work;
  Ok(metadata)
}

fn parse_v2_package_metadata(request: &PackageRequest, metadata_url: &str, bytes: &[u8], security_flags: u8) -> Result<PackageMetadata, PackageError> {
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
  let parsed_content = reqwest::Url::parse(&content_url).map_err(|error| {
    PackageError::new(
      PackageErrorKind::Integrity,
      &content_url,
      format!("invalid NuGet v2 package content URL: {error}"),
    )
  })?;
  if !parsed_content.has_host() {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      &content_url,
      "NuGet v2 package content URL must include a host",
    ));
  }
  if !parsed_content.username().is_empty() || parsed_content.password().is_some() {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      &content_url,
      "NuGet v2 package content URL must not embed credentials",
    ));
  }
  if parsed_content.scheme() != "https" && !(parsed_content.scheme() == "http" && security_flags & SOURCE_ALLOW_INSECURE_CONNECTIONS != 0) {
    return Err(PackageError::new(
      PackageErrorKind::Integrity,
      &content_url,
      "NuGet v2 package content URL must use HTTPS unless allowInsecureConnections is true",
    ));
  }
  Ok(PackageMetadata {
    content_url,
    expected_hash: Some(hash.ok_or_else(|| network_error(metadata_url, "NuGet v2 metadata has no package hash"))?),
    expected_size: Some(size.ok_or_else(|| network_error(metadata_url, "NuGet v2 metadata has no valid package size"))?),
    work: HttpWork::default(),
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

async fn download_package(
  client: &reqwest::Client,
  credential: Option<&SourceCredential>,
  url: &str,
  destination: &Path,
) -> Result<(String, HttpWork), PackageError> {
  let mut response = send_authenticated(client, credential, url, "package download").await?;
  if let Err(error) = response.error_for_status_ref() {
    let work = response.work(0);
    return Err(network_error(url, format!("package download failed: {error}")).with_http_work(work));
  }
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
  loop {
    let chunk = match response.chunk(url, "package").await {
      Ok(Some(chunk)) => chunk,
      Ok(None) => break,
      Err(error) => return Err(error.with_http_work(response.work(total))),
    };
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
  let work = response.work(total);
  Ok((BASE64.encode(hasher.finalize()), work))
}

async fn get_bytes(
  client: &reqwest::Client,
  credential: Option<&SourceCredential>,
  url: &str,
  limit: u64,
  kind: &str,
) -> Result<HttpPayload<Vec<u8>>, PackageError> {
  let mut response = send_authenticated(client, credential, url, "HTTP request").await?;
  if let Err(error) = response.error_for_status_ref() {
    let work = response.work(0);
    return Err(network_error(url, format!("HTTP request failed: {error}")).with_http_work(work));
  }
  if response.content_length().is_some_and(|length| length > limit) {
    let work = response.work(0);
    return Err(network_error(url, format!("{kind} response exceeds the {limit} byte limit")).with_http_work(work));
  }
  let capacity = response.content_length().unwrap_or(0).min(limit) as usize;
  let mut bytes = Vec::with_capacity(capacity);
  loop {
    let chunk = match response.chunk(url, kind).await {
      Ok(Some(chunk)) => chunk,
      Ok(None) => break,
      Err(error) => return Err(error.with_http_work(response.work(bytes.len() as u64))),
    };
    let Some(next) = bytes.len().checked_add(chunk.len()).filter(|length| *length as u64 <= limit) else {
      let downloaded = bytes.len().saturating_add(chunk.len()).min(u64::MAX as usize) as u64;
      return Err(network_error(url, format!("{kind} response exceeds the {limit} byte limit")).with_http_work(response.work(downloaded)));
    };
    bytes.extend_from_slice(&chunk);
    debug_assert_eq!(bytes.len(), next);
  }
  let work = response.work(bytes.len() as u64);
  Ok(HttpPayload { value: bytes, work })
}

#[cfg(test)]
fn authenticated_get(client: &reqwest::Client, credential: Option<&SourceCredential>, url: &str) -> reqwest::RequestBuilder {
  let source = credential;
  let credential = source.filter(|credential| credential.origin.matches(url));
  let selected_client = credential
    .and_then(|credential| credential.client.as_ref())
    .or_else(|| source.and_then(|credential| credential.transport_client.as_ref()))
    .unwrap_or(client);
  let request = selected_client.get(url);
  match credential.and_then(SourceCredential::authorization) {
    Some(authorization) => request.header(AUTHORIZATION, authorization.clone()),
    None => request,
  }
}

async fn send_authenticated(
  client: &reqwest::Client,
  credential: Option<&SourceCredential>,
  url: &str,
  operation: &str,
) -> Result<AuthenticatedResponse, PackageError> {
  let source = credential;
  let credential = source.filter(|credential| credential.origin.matches(url));
  let client = credential
    .and_then(|credential| credential.client.as_ref())
    .or_else(|| source.and_then(|credential| credential.transport_client.as_ref()))
    .unwrap_or(client);
  let policy = source.map_or(DEFAULT_HTTP_POLICY, |credential| credential.http_policy);
  // Acquire the narrower source budget first so a busy source cannot reserve
  // global slots while it waits for its own permits.
  let source_permit = match source.and_then(|credential| credential.source_limiter.as_ref()) {
    Some(limiter) => Some(
      Arc::clone(limiter)
        .acquire_owned()
        .await
        .map_err(|_| network_error(url, "package-source request limiter closed"))?,
    ),
    None => None,
  };
  let global_permit = match source.and_then(|credential| credential.global_limiter.as_ref()) {
    Some(limiter) => Some(
      Arc::clone(limiter)
        .acquire_owned()
        .await
        .map_err(|_| network_error(url, "global package request limiter closed"))?,
    ),
    None => None,
  };
  send_with_policy(client, credential, url, operation, policy, global_permit, source_permit).await
}

struct AuthenticatedResponse {
  response: reqwest::Response,
  _global_permit: Option<OwnedSemaphorePermit>,
  _source_permit: Option<OwnedSemaphorePermit>,
  started: Instant,
  download_timeout: Duration,
  requests: u32,
}

impl std::ops::Deref for AuthenticatedResponse {
  type Target = reqwest::Response;

  fn deref(&self) -> &Self::Target {
    &self.response
  }
}

impl std::ops::DerefMut for AuthenticatedResponse {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.response
  }
}

impl AuthenticatedResponse {
  async fn chunk(&mut self, url: &str, kind: &str) -> Result<Option<bytes::Bytes>, PackageError> {
    tokio::time::timeout(self.download_timeout, self.response.chunk())
      .await
      .map_err(|_| network_error(url, format!("{kind} response stalled for {} seconds", self.download_timeout.as_secs())))?
      .map_err(|error| network_error(url, format!("read {kind} response: {error}")))
  }

  fn work(&self, downloaded_bytes: u64) -> HttpWork {
    HttpWork {
      downloaded_bytes,
      duration_us: elapsed_us(self.started),
      requests: self.requests,
    }
  }
}

async fn send_with_policy(
  client: &reqwest::Client,
  credential: Option<&SourceCredential>,
  url: &str,
  operation: &str,
  policy: PackageHttpPolicy,
  global_permit: Option<OwnedSemaphorePermit>,
  source_permit: Option<OwnedSemaphorePermit>,
) -> Result<AuthenticatedResponse, PackageError> {
  let started = Instant::now();
  let mut requests = 0u32;
  'network: for network_attempt in 0..policy.max_tries {
    for authentication_attempt in 0..=2 {
      let (authorization, generation, provider_was_used) = match credential {
        Some(credential) => credential.authorization_snapshot().await,
        None => (None, 0, false),
      };
      let mut request = client.get(url);
      if let Some(authorization) = authorization {
        request = request.header(AUTHORIZATION, authorization);
      }
      requests = requests.checked_add(1).ok_or_else(|| network_error(url, "HTTP request count overflow"))?;
      let sent = tokio::time::timeout(Duration::from_secs(policy.request_timeout_seconds as u64), request.send()).await;
      let response = match sent {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
          if network_attempt + 1 == policy.max_tries {
            return Err(
              network_error(url, format!("{operation} failed after {} attempts: {error}", policy.max_tries)).with_http_work(HttpWork {
                downloaded_bytes: 0,
                duration_us: elapsed_us(started),
                requests,
              }),
            );
          }
          tokio::time::sleep(exponential_retry_delay(policy, network_attempt)).await;
          continue 'network;
        },
        Err(_) => {
          if network_attempt + 1 == policy.max_tries {
            return Err(
              network_error(
                url,
                format!(
                  "{operation} timed out after {} seconds and {} attempts",
                  policy.request_timeout_seconds, policy.max_tries
                ),
              )
              .with_http_work(HttpWork {
                downloaded_bytes: 0,
                duration_us: elapsed_us(started),
                requests,
              }),
            );
          }
          tokio::time::sleep(exponential_retry_delay(policy, network_attempt)).await;
          continue 'network;
        },
      };
      if response.status() == reqwest::StatusCode::UNAUTHORIZED && authentication_attempt < 2 {
        let Some(credential) = credential else {
          return Ok(authenticated_response(response, global_permit, source_permit, policy, started, requests));
        };
        if credential.acquire_provider(generation, provider_was_used).await?.is_none() {
          return Ok(authenticated_response(response, global_permit, source_permit, policy, started, requests));
        }
        drop(response);
        continue;
      }
      if retryable_status(response.status(), policy) && network_attempt + 1 < policy.max_tries {
        let delay = response_retry_delay(&response, policy, network_attempt);
        drop(response);
        tokio::time::sleep(delay).await;
        continue 'network;
      }
      return Ok(authenticated_response(response, global_permit, source_permit, policy, started, requests));
    }
  }
  unreachable!("the bounded transport loop always returns")
}

fn authenticated_response(
  response: reqwest::Response,
  global_permit: Option<OwnedSemaphorePermit>,
  source_permit: Option<OwnedSemaphorePermit>,
  policy: PackageHttpPolicy,
  started: Instant,
  requests: u32,
) -> AuthenticatedResponse {
  AuthenticatedResponse {
    response,
    _global_permit: global_permit,
    _source_permit: source_permit,
    started,
    download_timeout: Duration::from_secs(policy.download_timeout_seconds as u64),
    requests,
  }
}

fn retryable_status(status: reqwest::StatusCode, policy: PackageHttpPolicy) -> bool {
  status.is_server_error()
    || ((status == reqwest::StatusCode::REQUEST_TIMEOUT || status == reqwest::StatusCode::TOO_MANY_REQUESTS) && policy.retries_http_429())
}

fn response_retry_delay(response: &reqwest::Response, policy: PackageHttpPolicy, attempt: u8) -> Duration {
  if policy.observes_retry_after()
    && let Some(value) = response.headers().get(RETRY_AFTER).and_then(|value| value.to_str().ok())
    && let Some(delay) = parse_retry_after(value, SystemTime::now())
  {
    return delay.min(Duration::from_secs(policy.max_retry_after_seconds as u64));
  }
  exponential_retry_delay(policy, attempt)
}

fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
  if let Ok(seconds) = value.parse::<u64>() {
    return Some(Duration::from_secs(seconds));
  }
  httpdate::parse_http_date(value).ok()?.duration_since(now).ok()
}

fn exponential_retry_delay(policy: PackageHttpPolicy, attempt: u8) -> Duration {
  let multiplier = 1u32.checked_shl(u32::from(attempt.min(15))).unwrap_or(u32::MAX);
  let milliseconds = policy
    .retry_delay_ms
    .saturating_mul(multiplier)
    .min(policy.max_retry_after_seconds.saturating_mul(1_000));
  Duration::from_millis(u64::from(milliseconds))
}

fn package_blocking_task_error(error: tokio::task::JoinError) -> PackageError {
  PackageError::new(
    PackageErrorKind::Io,
    "package scheduler",
    format!("blocking package task stopped before completion: {error}"),
  )
}

fn network_error(context: impl Into<String>, message: impl Into<String>) -> PackageError {
  let context = context.into();
  let redacted = redact_url_for_output(&context);
  let message = message.into();
  let message = if redacted == context { message } else { message.replace(&context, &redacted) };
  PackageError::new(PackageErrorKind::Network, redacted, message)
}

fn package_credential_provider_error(error: CredentialProviderError) -> PackageError {
  let kind = match error.kind() {
    CredentialProviderErrorKind::Cancelled => PackageErrorKind::Cancelled,
    CredentialProviderErrorKind::Discovery
    | CredentialProviderErrorKind::Protocol
    | CredentialProviderErrorKind::Timeout
    | CredentialProviderErrorKind::Process => PackageErrorKind::CredentialProvider,
  };
  PackageError::new(kind, error.context(), error.to_string())
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

fn validate_cached_package(root: &Path, request: &PackageRequest, cache_hit: bool, signature_policy: &SignaturePolicy) -> Result<CachedPackage, PackageError> {
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
  if signature_policy.mode == SignatureValidationMode::Require {
    package_signature::verify_package(&nupkg, signature_policy)?;
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
    metadata: None,
    cache_hit,
    source_work: None,
    failed_source_work: Box::new([]),
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
  context: PackageAssetContext<'_>,
  dependencies: Vec<PackageRequest>,
  mut cold: PackageColdMetadata,
) -> Result<WorkPackage, PackageError> {
  let PackageAssetContext {
    target,
    target_text,
    runtime_identifier,
    runtime_graph,
    flags,
  } = context;
  let compile_assets = select_if(flags.contains(AssetFlags::COMPILE), || select_compile_assets(&cached.root, target))?;
  let mut runtime_assets = select_if(flags.contains(AssetFlags::RUNTIME), || select_runtime_assets(&cached.root, target))?;
  if (flags.contains(AssetFlags::COMPILE) || flags.contains(AssetFlags::RUNTIME))
    && compile_assets.is_empty()
    && runtime_assets.is_empty()
    && let Some(supported) = incompatible_asset_frameworks(&cached.root, target)?
  {
    return Err(
      PackageError::new(
        PackageErrorKind::Incompatible,
        format!("{} {}", request.id, request.version),
        format!(
          "package {} {} is not compatible with {}; supported frameworks: {}",
          request.id,
          request.version,
          target_text,
          supported.join(", ")
        ),
      )
      .with_context("package_id", &request.id)
      .with_context("package_version", &request.version)
      .with_context("target_framework", target_text)
      .with_context("supported_frameworks", supported.join(";")),
    );
  }
  // Package analyzers are resolved graph-wide by ResolvePackageAssets rather
  // than serialized as a target-library family in project.assets.json.
  let analyzers = select_if(flags.contains(AssetFlags::ANALYZERS), || collect_analyzers(&cached.root))?;
  let mut resource_assets = select_if(flags.contains(AssetFlags::RUNTIME), || select_resource_assets(&cached.root, target))?;
  let content_files = select_content_files(&cached.root, target, flags.contains(AssetFlags::CONTENT_FILES), &mut cold.content_rules)?;
  let mut native_assets = Vec::new();
  let rid_assets = select_runtime_targets(&cached.root, target, flags, runtime_identifier, runtime_graph)?;
  if let Some(selected) = rid_assets.runtime {
    runtime_assets = selected;
  }
  if let Some(selected) = rid_assets.resources {
    resource_assets = selected;
  }
  if let Some(selected) = rid_assets.native {
    native_assets = selected;
  }
  let runtime_targets = rid_assets.targets;
  if !flags.contains(AssetFlags::RUNTIME) {
    cold.frameworks.assemblies.clear();
  }
  let selected_build = select_build_assets(&cached.root, "build", &request.id, target, true)?;
  let selected_build_transitive = select_build_assets(&cached.root, "buildTransitive", &request.id, target, true)?;
  let build_transitive_assets = if flags.contains(AssetFlags::BUILD_TRANSITIVE) {
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
  let build_multi_targeting_assets = if flags.contains(AssetFlags::BUILD_MULTI_TARGETING) {
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
    content_actions: cold.content_rules.actions,
    build_assets,
    build_multi_targeting_assets,
    build_transitive_assets,
    native_assets,
    runtime_targets,
    framework_references: cold.frameworks.references,
    framework_assemblies: cold.frameworks.assemblies,
    cache_hit: cached.cache_hit,
    origin: cached.origin,
  })
}

fn incompatible_asset_frameworks(root: &Path, target: TargetFramework) -> Result<Option<Vec<String>>, PackageError> {
  let mut supported = Vec::new();
  let mut compatible = false;
  for category in ["ref", "lib"] {
    let category = root.join(category);
    let entries = match fs::read_dir(&category) {
      Ok(entries) => entries,
      Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
      Err(error) => return Err(package_io("enumerate package framework assets", &category, error)),
    };
    for entry in entries {
      let entry = entry.map_err(|error| package_io("enumerate package framework assets", &category, error))?;
      if !entry
        .file_type()
        .map_err(|error| package_io("inspect package framework asset", &entry.path(), error))?
        .is_dir()
      {
        continue;
      }
      let name = entry.file_name().into_string().map_err(|_| {
        PackageError::new(
          PackageErrorKind::NonUnicodePath,
          entry.path().display().to_string(),
          "package framework directory is not valid Unicode",
        )
      })?;
      compatible |= framework_score(Some(&name), target).is_some();
      supported.push(name);
    }
  }
  supported.sort_unstable();
  supported.dedup();
  Ok((!compatible && !supported.is_empty()).then_some(supported))
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

struct FrameworkReferenceGroup {
  framework: String,
  references: Vec<String>,
}

struct RawFrameworkAssembly {
  name: String,
  frameworks: Option<String>,
}

struct FrameworkAssemblyGroup {
  framework: Option<TargetFramework>,
  assemblies: Vec<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum NuspecSection {
  None,
  Dependencies,
  FrameworkReferences,
  FrameworkAssemblies,
  ContentFiles,
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
        central_transitive: false,
      })
    })
    .collect()
}

#[cfg(test)]
fn parse_nuspec_requirements(path: &Path, bytes: &[u8], request: &PackageRequest, target: TargetFramework) -> Result<Vec<PackageRequirement>, PackageError> {
  Ok(parse_nuspec_metadata(path, bytes, request, target)?.dependencies)
}

fn parse_nuspec_metadata(path: &Path, bytes: &[u8], request: &PackageRequest, target: TargetFramework) -> Result<ParsedPackageMetadata, PackageError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  let mut current_text = NuspecText::None;
  let mut id = None;
  let mut version = None;
  let mut groups = Vec::<DependencyGroup>::new();
  let mut ungrouped = Vec::new();
  let mut reference_groups = Vec::<FrameworkReferenceGroup>::new();
  let mut framework_assemblies = Vec::<RawFrameworkAssembly>::new();
  let mut content_rules = ContentFileRules::default();
  let mut section = NuspecSection::None;
  let mut open_group = NuspecSection::None;
  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => match local_name(element.name().as_ref()) {
        b"id" if id.is_none() => current_text = NuspecText::Id,
        b"version" if version.is_none() => current_text = NuspecText::Version,
        b"dependencies" => section = NuspecSection::Dependencies,
        b"frameworkReferences" => section = NuspecSection::FrameworkReferences,
        b"frameworkAssemblies" => section = NuspecSection::FrameworkAssemblies,
        b"contentFiles" => {
          section = NuspecSection::ContentFiles;
          content_rules.present = true;
        },
        b"group" if section == NuspecSection::Dependencies => {
          if open_group != NuspecSection::None || !ungrouped.is_empty() {
            return Err(package_manifest_error(
              path,
              "dependency groups cannot be nested or mixed with ungrouped dependencies",
            ));
          }
          open_group = NuspecSection::Dependencies;
          groups.push(DependencyGroup {
            framework: nuspec_attribute(&reader, &element, b"targetFramework", path)?,
            dependencies: Vec::new(),
          });
        },
        b"group" if section == NuspecSection::FrameworkReferences => {
          if open_group != NuspecSection::None {
            return Err(package_manifest_error(path, "framework reference groups cannot be nested"));
          }
          open_group = NuspecSection::FrameworkReferences;
          let framework = required_nuspec_attribute(&reader, &element, b"targetFramework", "framework reference group", path)?;
          validate_nuspec_frameworks(&framework, path)?;
          reference_groups.push(FrameworkReferenceGroup {
            framework,
            references: Vec::new(),
          });
        },
        b"dependency" if section == NuspecSection::Dependencies => {
          push_raw_dependency(&reader, &element, path, open_group == NuspecSection::Dependencies, &mut groups, &mut ungrouped)?;
        },
        b"frameworkReference" if section == NuspecSection::FrameworkReferences => {
          push_framework_reference(&reader, &element, path, open_group == NuspecSection::FrameworkReferences, &mut reference_groups)?;
        },
        b"frameworkAssembly" if section == NuspecSection::FrameworkAssemblies => {
          framework_assemblies.push(parse_framework_assembly(&reader, &element, path)?);
        },
        b"files" if section == NuspecSection::ContentFiles => {
          push_content_file_rule(&reader, &element, path, &mut content_rules)?;
        },
        _ => {},
      },
      Ok(Event::Empty(element)) => match local_name(element.name().as_ref()) {
        b"contentFiles" => content_rules.present = true,
        b"group" if section == NuspecSection::Dependencies => {
          if open_group != NuspecSection::None || !ungrouped.is_empty() {
            return Err(package_manifest_error(
              path,
              "dependency groups cannot be nested or mixed with ungrouped dependencies",
            ));
          }
          groups.push(DependencyGroup {
            framework: nuspec_attribute(&reader, &element, b"targetFramework", path)?,
            dependencies: Vec::new(),
          });
        },
        b"group" if section == NuspecSection::FrameworkReferences => {
          if open_group != NuspecSection::None {
            return Err(package_manifest_error(path, "framework reference groups cannot be nested"));
          }
          let framework = required_nuspec_attribute(&reader, &element, b"targetFramework", "framework reference group", path)?;
          validate_nuspec_frameworks(&framework, path)?;
          reference_groups.push(FrameworkReferenceGroup {
            framework,
            references: Vec::new(),
          });
        },
        b"dependency" if section == NuspecSection::Dependencies => {
          push_raw_dependency(&reader, &element, path, open_group == NuspecSection::Dependencies, &mut groups, &mut ungrouped)?;
        },
        b"frameworkReference" if section == NuspecSection::FrameworkReferences => {
          push_framework_reference(&reader, &element, path, open_group == NuspecSection::FrameworkReferences, &mut reference_groups)?;
        },
        b"frameworkAssembly" if section == NuspecSection::FrameworkAssemblies => {
          framework_assemblies.push(parse_framework_assembly(&reader, &element, path)?);
        },
        b"files" if section == NuspecSection::ContentFiles => {
          push_content_file_rule(&reader, &element, path, &mut content_rules)?;
        },
        _ => {},
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
        b"group" => open_group = NuspecSection::None,
        b"dependencies" | b"frameworkReferences" | b"frameworkAssemblies" | b"contentFiles" => section = NuspecSection::None,
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
      let target_framework = canonical_target_framework(target);
      let supported_frameworks = groups.iter().filter_map(|group| group.framework.as_deref()).collect::<Vec<_>>().join(";");
      PackageError::new(
        PackageErrorKind::Incompatible,
        format!("{} {}", request.id, request.version),
        format!(
          "package {} {} has no dependency group compatible with {target_framework}",
          request.id, request.version
        ),
      )
      .with_context("package_id", &request.id)
      .with_context("package_version", &request.version)
      .with_context("target_framework", target_framework)
      .with_context("supported_frameworks", supported_frameworks)
    })?
  };
  let dependencies = selected
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
    .collect::<Result<Vec<_>, PackageError>>()?;
  let mut references = reference_groups
    .iter()
    .filter_map(|group| framework_score(Some(&group.framework), target).map(|score| (score, group)))
    .max_by_key(|(score, _)| *score)
    .map_or_else(Vec::new, |(_, group)| group.references.clone());
  sort_dedup_case_insensitive(&mut references);
  let assemblies = select_framework_assemblies(framework_assemblies, target);
  Ok(ParsedPackageMetadata {
    dependencies,
    cold: PackageColdMetadata {
      frameworks: PackageFrameworkMetadata { references, assemblies },
      content_rules,
    },
  })
}

fn select_framework_assemblies(rows: Vec<RawFrameworkAssembly>, target: TargetFramework) -> Vec<String> {
  if target.family() != FrameworkFamily::NetFramework {
    return Vec::new();
  }
  let mut groups = Vec::<FrameworkAssemblyGroup>::new();
  for row in rows {
    let frameworks = row.frameworks.as_deref().map(|frameworks| {
      frameworks
        .split(',')
        .filter_map(|framework| parse_nuspec_target_framework(framework.trim().trim_start_matches('.')))
        .collect::<Vec<_>>()
    });
    if frameworks.as_ref().is_some_and(Vec::is_empty) {
      continue;
    }
    if let Some(frameworks) = frameworks {
      for framework in frameworks {
        if let Some(group) = groups.iter_mut().find(|group| group.framework == Some(framework)) {
          group.assemblies.push(row.name.clone());
        } else {
          groups.push(FrameworkAssemblyGroup {
            framework: Some(framework),
            assemblies: vec![row.name.clone()],
          });
        }
      }
    } else if let Some(group) = groups.iter_mut().find(|group| group.framework.is_none()) {
      group.assemblies.push(row.name);
    } else {
      groups.push(FrameworkAssemblyGroup {
        framework: None,
        assemblies: vec![row.name],
      });
    }
  }
  let mut selected = groups
    .into_iter()
    .filter_map(|group| {
      group
        .framework
        .map_or(Some(0), |framework| framework_score_value(framework, target))
        .map(|score| (score, group.assemblies))
    })
    .max_by_key(|(score, _)| *score)
    .map_or_else(Vec::new, |(_, assemblies)| assemblies);
  sort_dedup_case_insensitive(&mut selected);
  selected
}

fn sort_dedup_case_insensitive(values: &mut Vec<String>) {
  values.sort_unstable_by(|left, right| {
    left
      .bytes()
      .map(|byte| byte.to_ascii_lowercase())
      .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
      .then_with(|| left.cmp(right))
  });
  values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
}

fn push_raw_dependency(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  path: &Path,
  grouped: bool,
  groups: &mut [DependencyGroup],
  ungrouped: &mut Vec<RawDependency>,
) -> Result<(), PackageError> {
  let dependency_id = required_nuspec_attribute(reader, element, b"id", "dependency", path)?;
  let dependency_version = required_nuspec_attribute(reader, element, b"version", "dependency", path)?;
  let include_assets = parse_asset_flags(nuspec_attribute(reader, element, b"include", path)?.as_deref(), AssetFlags::NO_CONTENT, path)?;
  let exclude_assets = parse_asset_flags(nuspec_attribute(reader, element, b"exclude", path)?.as_deref(), AssetFlags::NONE, path)?;
  let dependency = RawDependency {
    id: dependency_id,
    version: dependency_version,
    include_assets: include_assets.without(exclude_assets),
    suppress_parent: AssetFlags::NONE,
  };
  if grouped {
    let group = groups
      .last_mut()
      .ok_or_else(|| package_manifest_error(path, "dependency must be inside a declared dependency group"))?;
    group.dependencies.push(dependency);
  } else if groups.is_empty() {
    ungrouped.push(dependency);
  } else {
    return Err(package_manifest_error(path, "grouped and ungrouped dependencies cannot be mixed"));
  }
  Ok(())
}

fn push_framework_reference(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  path: &Path,
  grouped: bool,
  groups: &mut [FrameworkReferenceGroup],
) -> Result<(), PackageError> {
  if !grouped {
    return Err(package_manifest_error(path, "frameworkReference must be inside a frameworkReferences group"));
  }
  let reference = required_nuspec_attribute(reader, element, b"name", "framework reference", path)?;
  validate_framework_name(&reference, "framework reference", path)?;
  groups
    .last_mut()
    .ok_or_else(|| package_manifest_error(path, "frameworkReference must be inside a frameworkReferences group"))?
    .references
    .push(reference);
  Ok(())
}

fn parse_framework_assembly(reader: &Reader<&[u8]>, element: &quick_xml::events::BytesStart<'_>, path: &Path) -> Result<RawFrameworkAssembly, PackageError> {
  let name = required_nuspec_attribute(reader, element, b"assemblyName", "framework assembly", path)?;
  validate_framework_assembly_name(&name, path)?;
  let frameworks = nuspec_attribute(reader, element, b"targetFramework", path)?;
  if let Some(frameworks) = frameworks.as_deref() {
    validate_nuspec_frameworks(frameworks, path)?;
  }
  Ok(RawFrameworkAssembly { name, frameworks })
}

fn push_content_file_rule(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  path: &Path,
  content: &mut ContentFileRules,
) -> Result<(), PackageError> {
  if content.rules.len() >= MAX_CONTENT_FILE_RULES {
    return Err(package_manifest_error(
      path,
      format!("contentFiles exceeds the {MAX_CONTENT_FILE_RULES}-rule limit"),
    ));
  }
  let include = required_nuspec_attribute(reader, element, b"include", "content file rule", path)?;
  let include = normalize_content_pattern(&include, path)?;
  let include_pattern = u32::try_from(content.patterns.len()).map_err(|_| package_manifest_error(path, "content file pattern count exceeds u32"))?;
  content.patterns.push(include);
  let exclude_start = u32::try_from(content.patterns.len()).map_err(|_| package_manifest_error(path, "content file pattern count exceeds u32"))?;
  if let Some(exclude) = nuspec_attribute(reader, element, b"exclude", path)?.filter(|value| !value.is_empty()) {
    content.patterns.push(normalize_content_pattern(&exclude, path)?);
  }
  let exclude_len =
    u32::try_from(content.patterns.len() - exclude_start as usize).map_err(|_| package_manifest_error(path, "content file exclude count exceeds u32"))?;

  let mut present = 0u8;
  let mut values = 0u8;
  let build_action = if let Some(action) = nuspec_attribute(reader, element, b"buildAction", path)? {
    let action = action.trim();
    if action.is_empty() || action.len() > 256 || action.chars().any(char::is_control) {
      return Err(package_manifest_error(
        path,
        "content file buildAction is empty, too long, or contains control characters",
      ));
    }
    present |= CONTENT_HAS_BUILD_ACTION;
    let action = canonical_content_build_action(action).unwrap_or(action);
    if let Some(index) = content.actions.iter().position(|candidate| candidate.eq_ignore_ascii_case(action)) {
      u32::try_from(index).map_err(|_| package_manifest_error(path, "content file action count exceeds u32"))?
    } else {
      let index = u32::try_from(content.actions.len()).map_err(|_| package_manifest_error(path, "content file action count exceeds u32"))?;
      content.actions.push(action.to_owned());
      index
    }
  } else {
    NO_CONTENT_BUILD_ACTION
  };
  if let Some(value) = nuspec_attribute(reader, element, b"copyToOutput", path)? {
    present |= CONTENT_HAS_COPY_TO_OUTPUT;
    if parse_nuspec_bool(&value, "copyToOutput", path)? {
      values |= CONTENT_COPY_TO_OUTPUT;
    }
  }
  if let Some(value) = nuspec_attribute(reader, element, b"flatten", path)? {
    present |= CONTENT_HAS_FLATTEN;
    if parse_nuspec_bool(&value, "flatten", path)? {
      values |= CONTENT_FLATTEN;
    }
  }
  content.rules.push(ContentFileRule {
    excludes: ItemRange {
      start: exclude_start,
      len: exclude_len,
    },
    include_pattern,
    build_action,
    values,
    present,
  });
  Ok(())
}

fn normalize_content_pattern(value: &str, path: &Path) -> Result<String, PackageError> {
  let normalized = value.replace('\\', "/");
  let mut components = 0usize;
  if normalized.is_empty() || normalized.len() > MAX_CONTENT_PATTERN_BYTES || normalized.starts_with('/') || normalized.contains(':') {
    return Err(package_manifest_error(
      path,
      format!("content file pattern {value:?} is empty, absolute, or too long"),
    ));
  }
  for component in normalized.split('/') {
    components += 1;
    if component.is_empty() || matches!(component, "." | "..") || component.contains(['[', ']']) || (component.contains("**") && component != "**") {
      return Err(package_manifest_error(
        path,
        format!("content file pattern {value:?} uses an unsupported or unsafe component"),
      ));
    }
  }
  if components > 64 {
    return Err(package_manifest_error(
      path,
      format!("content file pattern {value:?} exceeds 64 path components"),
    ));
  }
  Ok(normalized)
}

fn parse_nuspec_bool(value: &str, attribute: &str, path: &Path) -> Result<bool, PackageError> {
  if value.eq_ignore_ascii_case("true") {
    Ok(true)
  } else if value.eq_ignore_ascii_case("false") {
    Ok(false)
  } else {
    Err(package_manifest_error(
      path,
      format!("content file {attribute} must be true or false, found {value:?}"),
    ))
  }
}

fn required_nuspec_attribute(
  reader: &Reader<&[u8]>,
  element: &quick_xml::events::BytesStart<'_>,
  name: &[u8],
  meaning: &str,
  path: &Path,
) -> Result<String, PackageError> {
  nuspec_attribute(reader, element, name, path)?
    .map(|value| value.trim().to_owned())
    .filter(|value| !value.is_empty())
    .ok_or_else(|| package_manifest_error(path, format!("{meaning} requires {}", String::from_utf8_lossy(name))))
}

fn validate_framework_name(value: &str, meaning: &str, path: &Path) -> Result<(), PackageError> {
  if value.len() > 1024
    || !value
      .bytes()
      .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'|'))
  {
    return Err(package_manifest_error(
      path,
      format!("{meaning} name {value:?} is outside the supported identifier form"),
    ));
  }
  Ok(())
}

fn validate_framework_assembly_name(value: &str, path: &Path) -> Result<(), PackageError> {
  if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
    return Err(package_manifest_error(
      path,
      "framework assembly name is empty, too long, or contains control characters",
    ));
  }
  Ok(())
}

fn validate_nuspec_frameworks(value: &str, path: &Path) -> Result<(), PackageError> {
  if value.len() > 1024 || value.split(',').any(|framework| framework.trim().is_empty()) {
    return Err(package_manifest_error(path, "targetFramework is empty or exceeds 1024 bytes"));
  }
  Ok(())
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
    } else if token.eq_ignore_ascii_case("buildMultiTargeting") {
      AssetFlags::BUILD_MULTI_TARGETING
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

fn select_content_files(root: &Path, target: TargetFramework, included: bool, rules: &mut ContentFileRules) -> Result<Vec<WorkContentFile>, PackageError> {
  let content_root = root.join("contentFiles");
  if !content_root.is_dir() {
    return Ok(Vec::new());
  }
  if !included {
    let none = content_action_index(&mut rules.actions, "None")?;
    return Ok(vec![WorkContentFile {
      path: content_root.join("any/any/_._"),
      build_action: none,
      copy_to_output: false,
      flatten: false,
    }]);
  }
  let mut paths = Vec::new();
  let languages = fs::read_dir(&content_root).map_err(|error| package_io("enumerate package content languages", &content_root, error))?;
  for language in languages {
    let language = language.map_err(|error| package_io("enumerate package content languages", &content_root, error))?;
    let language_root = language.path();
    if !language
      .file_type()
      .map_err(|error| package_io("inspect package content language", &language_root, error))?
      .is_dir()
    {
      continue;
    }
    let directory = select_framework_directory(&language_root, target)?.or_else(|| {
      let any = language_root.join("any");
      any.is_dir().then_some(any)
    });
    if let Some(directory) = directory {
      collect_files(&directory, &mut paths, |_| true)?;
    }
  }
  paths.sort_unstable();
  paths.dedup();
  let mut selected = Vec::with_capacity(paths.len());
  for path in paths {
    let relative = path.strip_prefix(&content_root).expect("selected content paths are rooted below contentFiles");
    let relative = relative.to_str().ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::NonUnicodePath,
        path.display().to_string(),
        "package content path is not valid Unicode",
      )
    })?;
    let (mut build_action, copy_to_output, flatten) = select_content_metadata(relative, rules)?;
    if path.file_name().is_some_and(|name| name == "_._") {
      build_action = content_action_index(&mut rules.actions, "None")?;
    }
    selected.push(WorkContentFile {
      path,
      build_action,
      copy_to_output,
      flatten,
    });
  }
  Ok(selected)
}

fn select_content_metadata(path: &str, rules: &ContentFileRules) -> Result<(u32, bool, bool), PackageError> {
  if !rules.present {
    return Ok((NO_CONTENT_BUILD_ACTION, false, false));
  }
  let mut build_action = None;
  let mut copy_to_output = None;
  let mut flatten = None;
  for rule in &rules.rules {
    if !glob_path_matches(&rules.patterns[rule.include_pattern as usize], path)
      || rules.patterns[range(rule.excludes)].iter().any(|exclude| glob_path_matches(exclude, path))
    {
      continue;
    }
    if rule.present & CONTENT_HAS_BUILD_ACTION != 0 {
      build_action = Some(rule.build_action);
    }
    if rule.present & CONTENT_HAS_COPY_TO_OUTPUT != 0 {
      copy_to_output = Some(rule.values & CONTENT_COPY_TO_OUTPUT != 0);
    }
    if rule.present & CONTENT_HAS_FLATTEN != 0 {
      flatten = Some(rule.values & CONTENT_FLATTEN != 0);
    }
  }
  if let Some(index) = build_action {
    let action = rules
      .actions
      .get(index as usize)
      .ok_or_else(|| PackageError::new(PackageErrorKind::TextOverflow, path, "content build-action index exceeds its action batch"))?;
    if canonical_content_build_action(action).is_none() {
      return Err(PackageError::new(
        PackageErrorKind::Integrity,
        path,
        format!("content file selects unknown build action {action:?}"),
      ));
    }
  }
  Ok((
    build_action.unwrap_or(NO_CONTENT_BUILD_ACTION),
    copy_to_output.unwrap_or(false),
    flatten.unwrap_or(false),
  ))
}

fn content_action_index(actions: &mut Vec<String>, action: &str) -> Result<u32, PackageError> {
  if let Some(index) = actions.iter().position(|candidate| candidate.eq_ignore_ascii_case(action)) {
    return u32_len(index, "package content build-action index");
  }
  let index = u32_len(actions.len(), "package content build-action index")?;
  actions.push(action.to_owned());
  Ok(index)
}

fn canonical_content_build_action(action: &str) -> Option<&'static str> {
  [
    "None",
    "Compile",
    "Content",
    "EmbeddedResource",
    "ApplicationDefinition",
    "Page",
    "Resource",
    "SplashScreen",
    "DesignData",
    "DesignDataWithDesignTimeCreatableTypes",
    "CodeAnalysisDictionary",
    "AndroidAsset",
    "AndroidResource",
    "BundleResource",
  ]
  .into_iter()
  .find(|candidate| candidate.eq_ignore_ascii_case(action))
}

fn glob_path_matches(pattern: &str, path: &str) -> bool {
  let mut pattern_offset = 0usize;
  let mut path_offset = 0usize;
  let mut recursive_pattern = None;
  let mut recursive_path = 0usize;
  loop {
    let pattern_segment = next_path_segment(pattern, pattern_offset);
    let path_segment = next_path_segment(path, path_offset);
    match (pattern_segment, path_segment) {
      (Some(("**", next_pattern)), _) => {
        recursive_pattern = Some(next_pattern);
        recursive_path = path_offset;
        pattern_offset = next_pattern;
      },
      (Some((pattern_segment, next_pattern)), Some((path_segment, next_path)))
        if glob_component_matches(pattern_segment.as_bytes(), path_segment.as_bytes()) =>
      {
        pattern_offset = next_pattern;
        path_offset = next_path;
      },
      (None, None) => return true,
      _ => {
        let Some(after_recursive) = recursive_pattern else {
          return false;
        };
        let Some((_, next_path)) = next_path_segment(path, recursive_path) else {
          return false;
        };
        recursive_path = next_path;
        path_offset = next_path;
        pattern_offset = after_recursive;
      },
    }
  }
}

fn next_path_segment(value: &str, offset: usize) -> Option<(&str, usize)> {
  if offset >= value.len() {
    return None;
  }
  let bytes = value.as_bytes();
  let mut end = offset;
  while end < bytes.len() && !matches!(bytes[end], b'/' | b'\\') {
    end += 1;
  }
  let mut next = end;
  while next < bytes.len() && matches!(bytes[next], b'/' | b'\\') {
    next += 1;
  }
  Some((&value[offset..end], next))
}

fn glob_component_matches(pattern: &[u8], value: &[u8]) -> bool {
  let (mut pattern_index, mut value_index) = (0usize, 0usize);
  let (mut star_pattern, mut star_value) = (None, 0usize);
  while value_index < value.len() {
    if pattern_index < pattern.len() && (pattern[pattern_index] == b'?' || pattern[pattern_index].eq_ignore_ascii_case(&value[value_index])) {
      pattern_index += 1;
      value_index += 1;
    } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
      pattern_index += 1;
      star_pattern = Some(pattern_index);
      star_value = value_index;
    } else if let Some(after_star) = star_pattern {
      star_value += 1;
      value_index = star_value;
      pattern_index = after_star;
    } else {
      return false;
    }
  }
  pattern[pattern_index..].iter().all(|byte| *byte == b'*')
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

fn select_runtime_targets(
  root: &Path,
  target: TargetFramework,
  flags: AssetFlags,
  requested_runtime: Option<&str>,
  runtime_graph: Option<&RuntimeIdentifierGraph>,
) -> Result<RuntimeAssetSelection, PackageError> {
  let runtimes = root.join("runtimes");
  if !runtimes.is_dir() || !flags.contains(AssetFlags::RUNTIME) && !flags.contains(AssetFlags::NATIVE) {
    return Ok(RuntimeAssetSelection {
      targets: Vec::new(),
      runtime: None,
      resources: None,
      native: None,
    });
  }
  let mut directories = Vec::new();
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
    let runtime_identifier = entry.file_name().into_string().map_err(|_| {
      PackageError::new(
        PackageErrorKind::NonUnicodePath,
        rid_root.display().to_string(),
        "package runtime identifier is not valid Unicode",
      )
    })?;
    let rank = requested_runtime.and_then(|requested| {
      runtime_graph
        .expect("a selected runtime was validated with a graph")
        .compatible_rids(requested)
        .position(|compatible| compatible == runtime_identifier)
    });
    if requested_runtime.is_none() || rank.is_some() {
      directories.push((rank.unwrap_or(usize::MAX), runtime_identifier, rid_root));
    }
  }
  directories.sort_unstable_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

  let mut selected = Vec::new();
  let mut selected_runtime = None;
  let mut selected_resources = None;
  let mut selected_native = None;
  for (_, runtime_identifier, rid_root) in directories {
    if selected_runtime.is_none()
      && flags.contains(AssetFlags::RUNTIME)
      && let Some(directory) = select_framework_directory(&rid_root.join("lib"), target)?
    {
      let runtime = dlls_in(&directory)?;
      let resources = resource_dlls_in(&directory)?;
      if requested_runtime.is_some() {
        selected_runtime = Some(runtime);
        selected_resources = Some(resources);
      } else {
        for path in runtime {
          selected.push(WorkRuntimeTarget {
            path,
            runtime_identifier: runtime_identifier.clone(),
            kind: RuntimeTargetKind::Runtime,
          });
        }
        for path in resources {
          selected.push(WorkRuntimeTarget {
            path,
            runtime_identifier: runtime_identifier.clone(),
            kind: RuntimeTargetKind::Resource,
          });
        }
      }
    }
    if selected_native.is_none() && flags.contains(AssetFlags::NATIVE) {
      let native_assets = select_framework_directory(&rid_root.join("nativeassets"), target)?;
      let native = native_assets.or_else(|| {
        let directory = rid_root.join("native");
        directory.is_dir().then_some(directory)
      });
      if let Some(directory) = native {
        let mut paths = Vec::new();
        collect_files(&directory, &mut paths, |_| true)?;
        paths.sort_unstable();
        if requested_runtime.is_some() {
          selected_native = Some(paths);
        } else {
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
    if requested_runtime.is_some()
      && (!flags.contains(AssetFlags::RUNTIME) || selected_runtime.is_some())
      && (!flags.contains(AssetFlags::NATIVE) || selected_native.is_some())
    {
      break;
    }
  }
  selected.sort_unstable_by(|left, right| {
    left
      .runtime_identifier
      .cmp(&right.runtime_identifier)
      .then_with(|| left.path.cmp(&right.path))
      .then_with(|| (left.kind as u8).cmp(&(right.kind as u8)))
  });
  Ok(RuntimeAssetSelection {
    targets: selected,
    runtime: selected_runtime,
    resources: selected_resources,
    native: selected_native,
  })
}

fn resource_dlls_in(directory: &Path) -> Result<Vec<PathBuf>, PackageError> {
  let mut resources = Vec::new();
  for entry in fs::read_dir(directory).map_err(|error| package_io("enumerate package resources", directory, error))? {
    let entry = entry.map_err(|error| package_io("enumerate package resources", directory, error))?;
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
  let candidate = parse_nuspec_target_framework(canonical)?;
  framework_score_value(candidate, target)
}

fn framework_score_value(candidate: TargetFramework, target: TargetFramework) -> Option<u32> {
  let version = framework_version_key(candidate);
  if candidate.family() == target.family() && version <= framework_version_key(target) {
    return Some(30_000 + version);
  }
  match candidate.family() {
    FrameworkFamily::NetCoreApp if target.family() == FrameworkFamily::Net && (candidate.major(), candidate.minor()) <= (3, 1) => Some(20_000 + version),
    FrameworkFamily::NetStandard if target.family() == FrameworkFamily::Net && (candidate.major(), candidate.minor()) <= (2, 1) => Some(10_000 + version),
    _ => None,
  }
}

fn parse_nuspec_target_framework(value: &str) -> Option<TargetFramework> {
  let lower = value.to_ascii_lowercase();
  let Some(version) = lower.strip_prefix("netframework") else {
    return TargetFramework::parse(value).ok();
  };
  let mut parts = version.split('.');
  let major = parts.next()?.parse::<u16>().ok()?;
  let minor = parts.next()?.parse::<u16>().ok()?;
  let patch = parts.next().map(str::parse::<u16>).transpose().ok()?.unwrap_or(0);
  if parts.next().is_some() || major > 9 || minor > 9 || patch > 9 {
    return None;
  }
  let compact = if patch == 0 {
    format!("net{major}{minor}")
  } else {
    format!("net{major}{minor}{patch}")
  };
  TargetFramework::parse(&compact).ok()
}

fn framework_version_key(framework: TargetFramework) -> u32 {
  if framework.family() == FrameworkFamily::NetFramework {
    let encoded = framework.minor();
    let (minor, patch) = if encoded < 10 { (encoded, 0) } else { (encoded / 10, encoded % 10) };
    u32::from(framework.major()) * 10_000 + u32::from(minor) * 100 + u32::from(patch)
  } else {
    u32::from(framework.major()) * 100 + u32::from(framework.minor())
  }
}

fn canonical_target_framework(target: TargetFramework) -> String {
  match target.family() {
    FrameworkFamily::Net => format!("net{}.{}", target.major(), target.minor()),
    FrameworkFamily::NetCoreApp => format!("netcoreapp{}.{}", target.major(), target.minor()),
    FrameworkFamily::NetStandard => format!("netstandard{}.{}", target.major(), target.minor()),
    FrameworkFamily::NetFramework => format!("net{}{}", target.major(), target.minor()),
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

fn minimum_version_from_range(range: &VersionRange) -> Result<PackageVersion, PackageError> {
  match &range.lower {
    Some(lower) if lower.inclusive && !range.is_floating() => Ok(lower.version.clone()),
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
  fn visit<'a>(
    id: &'a str,
    packages: &'a BTreeMap<String, WorkPackage>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
    stack: &mut Vec<&'a str>,
  ) -> Result<(), PackageError> {
    if visited.contains(id) {
      return Ok(());
    }
    if !visiting.insert(id) {
      let start = stack.iter().position(|entry| *entry == id).unwrap_or(0);
      let mut cycle = stack[start..].join(" -> ");
      cycle.push_str(" -> ");
      cycle.push_str(id);
      return Err(
        PackageError::new(PackageErrorKind::DependencyCycle, id, format!("package dependency cycle detected: {cycle}"))
          .with_context("package_id", id)
          .with_context("cycle", cycle)
          .with_cause("package metadata contains a circular dependency chain"),
      );
    }
    stack.push(id);
    if let Some(package) = packages.get(id) {
      for dependency in &package.dependencies {
        visit(&dependency.lower_id, packages, visiting, visited, stack)?;
      }
    }
    stack.pop();
    visiting.remove(id);
    visited.insert(id);
    Ok(())
  }

  let mut visiting = BTreeSet::new();
  let mut visited = BTreeSet::new();
  let mut stack = Vec::new();
  for id in packages.keys() {
    visit(id, packages, &mut visiting, &mut visited, &mut stack)?;
  }
  Ok(())
}

fn materialize_resolution(
  context: ResolutionContext<'_>,
  work: &BTreeMap<String, WorkPackage>,
  source_work: &[SourceWork],
  downgrades: &[ResolvedDowngrade],
  shared_metadata_hits: u32,
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
          .chain(&package.build_assets)
          .chain(&package.build_multi_targeting_assets)
          .chain(&package.build_transitive_assets)
          .chain(&package.native_assets)
          .map(|path| path.as_os_str().len())
          .sum::<usize>()
        + package
          .content_files
          .iter()
          .map(|content| {
            content.path.as_os_str().len()
              + if content.build_action == NO_CONTENT_BUILD_ACTION {
                DEFAULT_CONTENT_BUILD_ACTION.len()
              } else {
                package.content_actions[content.build_action as usize].len()
              }
          })
          .sum::<usize>()
        + package
          .runtime_targets
          .iter()
          .map(|asset| asset.path.as_os_str().len() + asset.runtime_identifier.len())
          .sum::<usize>()
        + package.framework_references.iter().map(String::len).sum::<usize>()
        + package.framework_assemblies.iter().map(String::len).sum::<usize>()
    })
    .sum::<usize>()
    + context.cache_root.as_os_str().len()
    + context.http_cache_root.as_os_str().len()
    + context.temp_root.as_os_str().len()
    + context.fallback_roots.iter().map(|path| path.as_os_str().len()).sum::<usize>()
    + context.lock_path.as_os_str().len()
    + context.target_framework.len()
    + context.runtime_identifier.map_or(0, str::len)
    + context.runtime_graph_fingerprint.len()
    + context.source_name.len()
    + context.source_location.len()
    + context.sources.iter().map(|(name, _)| name.len()).sum::<usize>()
    + context.prune_fingerprint.len()
    + context.central_package_fingerprint.len()
    + downgrades
      .iter()
      .map(|warning| warning.package_id.len() + warning.selected_version.len() + warning.requested_range.len() + warning.requesting_package.len())
      .sum::<usize>()
    + context
      .project
      .package_references()
      .iter()
      .map(|reference| {
        context.project.package_no_warn(*reference).map_or(0, str::len)
          + context.project.package_aliases(*reference).map_or(0, str::len)
          + usize::from(context.project.package_generate_path_property(*reference)) * (context.project.package_id(*reference).len() + 3)
      })
      .sum::<usize>();
  let mut table = TextTable::with_capacity(estimated);
  let cache_root_span = table.push_path(context.cache_root)?;
  let http_cache_root_span = table.push_path(context.http_cache_root)?;
  let temp_root_span = table.push_path(context.temp_root)?;
  let fallback_roots = context.fallback_roots.iter().map(|path| table.push_path(path)).collect::<Result<Box<_>, _>>()?;
  let lock_path_span = table.push_path(context.lock_path)?;
  let target_framework_span = table.push(context.target_framework)?;
  let runtime_identifier_span = table.push(context.runtime_identifier.unwrap_or(""))?;
  let runtime_graph_fingerprint_span = table.push(context.runtime_graph_fingerprint)?;
  let source_name_span = table.push(redact_url_for_output(context.source_name).as_ref())?;
  let source_location_span = table.push(context.source_location)?;
  let prune_fingerprint_span = table.push(context.prune_fingerprint)?;
  let central_package_fingerprint_span = table.push(context.central_package_fingerprint)?;
  if context.sources.len() != source_work.len() {
    return Err(PackageError::new(
      PackageErrorKind::TextOverflow,
      "NuGet source telemetry",
      "configured source and source-work batch lengths differ",
    ));
  }
  let mut materialized_source_work = Vec::with_capacity(source_work.len());
  let mut network_requests = 0u32;
  let mut downloaded_bytes = 0u64;
  for (index, ((name, source), work)) in context.sources.iter().zip(source_work).enumerate() {
    if work.source_index as usize != index {
      return Err(PackageError::new(
        PackageErrorKind::TextOverflow,
        "NuGet source telemetry",
        "source-work batch is not in configured source order",
      ));
    }
    network_requests = network_requests
      .checked_add(work.requests)
      .ok_or_else(|| network_error(name, "HTTP request count overflow"))?;
    downloaded_bytes = downloaded_bytes
      .checked_add(work.downloaded_bytes)
      .ok_or_else(|| network_error(name, "HTTP response byte count overflow"))?;
    materialized_source_work.push(PackageSourceWorkRecord {
      downloaded_bytes: work.downloaded_bytes,
      duration_us: work.duration_us,
      name: table.push(redact_url_for_output(name).as_ref())?,
      requests: work.requests,
      protocol: source.protocol,
    });
  }
  let mut packages = Vec::with_capacity(work.len());
  let mut package_roots = Vec::with_capacity(work.len());
  let mut package_assets = Vec::with_capacity(work.len());
  let mut package_extended_assets = Vec::with_capacity(work.len());
  let framework_row_count = work
    .values()
    .filter(|package| !package.framework_references.is_empty() || !package.framework_assemblies.is_empty())
    .count();
  let framework_item_count = work
    .values()
    .map(|package| package.framework_references.len() + package.framework_assemblies.len())
    .sum();
  let mut package_framework_assets = Vec::with_capacity(framework_row_count);
  let mut framework_items = Vec::with_capacity(framework_item_count);
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
  let mut content_file_metadata = Vec::with_capacity(asset_ranges.content.len as usize);
  let mut cache_hits = 0u32;

  for package in work.values() {
    let package_index = u32_len(packages.len(), "package framework owner")?;
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
    let content = push_content_asset_range(
      &mut table,
      &mut assets,
      &mut asset_cursors[4],
      &package.content_files,
      &package.content_actions,
      &mut content_file_metadata,
    )?;
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
    if !package.framework_references.is_empty() || !package.framework_assemblies.is_empty() {
      let reference_start = u32_len(framework_items.len(), "package framework reference range")?;
      for reference in &package.framework_references {
        framework_items.push(table.push(reference)?);
      }
      let assembly_start = u32_len(framework_items.len(), "package framework assembly range")?;
      for assembly in &package.framework_assemblies {
        framework_items.push(table.push(assembly)?);
      }
      package_framework_assets.push(PackageFrameworkAssets {
        references: ItemRange {
          start: reference_start,
          len: u32_len(package.framework_references.len(), "package framework reference range")?,
        },
        assemblies: ItemRange {
          start: assembly_start,
          len: u32_len(package.framework_assemblies.len(), "package framework assembly range")?,
        },
        package_index,
      });
    }
    packages.push(ResolvedPackage {
      id: table.push(&package.request.id)?,
      version: table.push(&package.request.version)?,
      dependencies: ItemRange {
        start: dependency_start,
        len: dependency_len,
      },
      direct: package.request.direct,
      central_transitive: package.request.central_transitive,
      cache_hit: package.cache_hit,
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

  let empty = TextSpan { start: 0, len: 0 };
  let mut direct_policies = Vec::with_capacity(context.direct.len());
  for requirement in context.direct {
    let package_index = *indices.get(requirement.lower_id.as_str()).ok_or_else(|| {
      PackageError::new(
        PackageErrorKind::Resolution,
        &requirement.id,
        format!("resolved graph omitted direct package {}", requirement.id),
      )
    })?;
    let reference = context
      .project
      .package_references()
      .iter()
      .copied()
      .find(|reference| context.project.package_id(*reference).eq_ignore_ascii_case(&requirement.id))
      .expect("direct requests originate from project package references");
    let no_warn = context
      .project
      .package_no_warn(reference)
      .map(|value| table.push(value))
      .transpose()?
      .unwrap_or(empty);
    let aliases = context
      .project
      .package_aliases(reference)
      .map(|value| table.push(value))
      .transpose()?
      .unwrap_or(empty);
    let path_property = if context.project.package_generate_path_property(reference) {
      let name = package_path_property(context.project.package_id(reference));
      table.push(&name)?
    } else {
      empty
    };
    direct_policies.push(DirectPackagePolicy {
      no_warn,
      aliases,
      path_property,
      package_index,
      include_assets: requirement.include_assets,
      private_assets: requirement.suppress_parent,
    });
  }
  let materialized_downgrades = downgrades
    .iter()
    .map(|warning| {
      Ok(PackageDowngrade {
        package_id: table.push(&warning.package_id)?,
        selected_version: table.push(&warning.selected_version)?,
        requested_range: table.push(&warning.requested_range)?,
        requesting_package: table.push(&warning.requesting_package)?,
      })
    })
    .collect::<Result<Box<_>, PackageError>>()?;

  Ok(PackageResolution {
    text: table.text.into_boxed_str(),
    cache_root: cache_root_span,
    http_cache_root: http_cache_root_span,
    temp_root: temp_root_span,
    lock_path: lock_path_span,
    target_framework: target_framework_span,
    runtime_identifier: runtime_identifier_span,
    runtime_graph_fingerprint: runtime_graph_fingerprint_span,
    source_name: source_name_span,
    source_location: source_location_span,
    prune_fingerprint: prune_fingerprint_span,
    central_package_fingerprint: central_package_fingerprint_span,
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
    package_framework_assets: package_framework_assets.into_boxed_slice(),
    framework_items: framework_items.into_boxed_slice(),
    direct_policies: direct_policies.into_boxed_slice(),
    downgrades: materialized_downgrades,
    dependencies: dependencies.into_boxed_slice(),
    assets: assets.into_boxed_slice(),
    asset_ranges,
    runtime_targets: runtime_targets.into_boxed_slice(),
    content_file_metadata: content_file_metadata.into_boxed_slice(),
    source_work: materialized_source_work.into_boxed_slice(),
    cache_hits,
    downloaded_packages: work.len() as u32 - cache_hits,
    network_requests,
    shared_metadata_hits,
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

fn package_path_property(id: &str) -> String {
  // The external package identifier determines this opted-in property length;
  // one exact cold allocation avoids reserving worst-case space per reference.
  let mut property = String::with_capacity(id.len() + 3);
  property.push_str("Pkg");
  property.extend(id.chars().map(|character| match character {
    '.' | '-' => '_',
    other => other,
  }));
  property
}

fn push_content_asset_range(
  table: &mut TextTable,
  target: &mut [TextSpan],
  cursor: &mut u32,
  content: &[WorkContentFile],
  actions: &[String],
  metadata: &mut Vec<ContentFileMetadata>,
) -> Result<ItemRange, PackageError> {
  let start = *cursor;
  let len = u32_len(content.len(), "package content asset range")?;
  let end = start
    .checked_add(len)
    .ok_or_else(|| PackageError::new(PackageErrorKind::TextOverflow, "package content assets", "content asset range overflowed u32"))?;
  let slots = target.get_mut(start as usize..end as usize).ok_or_else(|| {
    PackageError::new(
      PackageErrorKind::TextOverflow,
      "package content assets",
      "content asset range exceeds its family",
    )
  })?;
  for (slot, content) in slots.iter_mut().zip(content) {
    *slot = table.push_path(&content.path)?;
    let build_action = if content.build_action == NO_CONTENT_BUILD_ACTION {
      DEFAULT_CONTENT_BUILD_ACTION
    } else {
      actions.get(content.build_action as usize).ok_or_else(|| {
        PackageError::new(
          PackageErrorKind::TextOverflow,
          "package content assets",
          "content build-action index exceeds its package action batch",
        )
      })?
    };
    metadata.push(ContentFileMetadata {
      build_action: table.push(build_action)?,
      copy_to_output: content.copy_to_output,
      flatten: content.flatten,
    });
  }
  *cursor = end;
  Ok(ItemRange { start, len })
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
  let mut table = TextTable::with_capacity(
    project.project_path().as_os_str().len()
      + project.target_framework().len()
      + project.runtime_identifier().map_or(0, str::len)
      + project.central_package_fingerprint().len()
      + 32,
  );
  let empty = table.push("")?;
  let lock = table.push_path(&project.project_directory().join("dv.lock.json"))?;
  let target_framework = table.push(project.target_framework())?;
  let runtime_identifier = table.push(project.runtime_identifier().unwrap_or(""))?;
  let central_package_fingerprint = table.push(project.central_package_fingerprint())?;
  Ok(PackageResolution {
    text: table.text.into_boxed_str(),
    cache_root: empty,
    http_cache_root: empty,
    temp_root: empty,
    lock_path: lock,
    target_framework,
    runtime_identifier,
    runtime_graph_fingerprint: empty,
    source_name: empty,
    source_location: empty,
    prune_fingerprint: empty,
    central_package_fingerprint,
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
    package_framework_assets: Box::new([]),
    framework_items: Box::new([]),
    direct_policies: Box::new([]),
    downgrades: Box::new([]),
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
    content_file_metadata: Box::new([]),
    source_work: Box::new([]),
    cache_hits: 0,
    downloaded_packages: 0,
    network_requests: 0,
    shared_metadata_hits: 0,
    downloaded_bytes: 0,
  })
}

fn read_warm_lock(
  path: &Path,
  config: &NugetConfiguration,
  direct: &[PackageRequirement],
  project: &ProjectSpec,
  prune_fingerprint: &str,
  runtime_graph_fingerprint: &str,
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
        .is_some_and(|locked| {
          locked.include_assets == request.include_assets.bits() && PackageVersion::parse(&locked.version).is_ok_and(|version| request.range.contains(&version))
        })
    });
  let selected_source = config
    .sources
    .iter()
    .find(|(_, source)| redact_url_for_output(&source.url) == lock.source && source.protocol == lock.source_protocol);
  if lock.schema_version != LOCK_SCHEMA_VERSION
    || lock.target_framework != target_text
    || lock.runtime_identifier.as_deref() != project.runtime_identifier()
    || lock.runtime_graph_fingerprint != runtime_graph_fingerprint
    || lock.prune_fingerprint != prune_fingerprint
    || lock.central_package_fingerprint != project.central_package_fingerprint()
    || !direct_matches
    || selected_source.is_none()
  {
    return Ok(None);
  }
  let (source_name, _) = selected_source.expect("selected source was checked");
  let downgrades = lock
    .downgrades
    .into_iter()
    .map(|warning| ResolvedDowngrade {
      package_id: warning.package_id,
      selected_version: warning.selected_version,
      requested_range: warning.requested_range,
      requesting_package: warning.requesting_package,
    })
    .collect::<Vec<_>>();

  let mut work = BTreeMap::new();
  for package in lock.packages {
    let request = PackageRequest {
      lower_id: normalize_id(&package.id)?,
      version: normalize_version(&package.version)?,
      id: package.id,
      direct: package.direct,
      central_transitive: package.central_transitive,
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
    validate_locked_package(&root, &request, &package.sha512, &config.signature_policy)?;
    let compile_assets = lock_asset_paths(&root, &package.compile_assets)?;
    let runtime_assets = lock_asset_paths(&root, &package.runtime_assets)?;
    let analyzers = lock_asset_paths(&root, &package.analyzers)?;
    let resource_assets = lock_asset_paths(&root, &package.resource_assets)?;
    let (content_files, content_actions) = lock_content_files(&root, package.content_files)?;
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
          central_transitive: false,
        })
      })
      .collect::<Result<Vec<_>, PackageError>>()?;
    for reference in &package.framework_references {
      validate_framework_name(reference, "locked framework reference", path)?;
    }
    for assembly in &package.framework_assemblies {
      validate_framework_assembly_name(assembly, path)?;
    }
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
          content_actions,
          build_assets,
          build_multi_targeting_assets,
          build_transitive_assets,
          native_assets,
          runtime_targets,
          framework_references: package.framework_references,
          framework_assemblies: package.framework_assemblies,
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
  let source_work = source_work_table(config.sources.len())?;
  materialize_resolution(
    ResolutionContext {
      project,
      direct,
      cache_root: &config.cache_root,
      http_cache_root: &config.http_cache_root,
      temp_root: &config.temp_root,
      fallback_roots: &config.fallback_roots,
      lock_path: path,
      target_framework: target_text,
      runtime_identifier: project.runtime_identifier(),
      runtime_graph_fingerprint,
      source_name,
      source_location: &lock.source,
      sources: &config.sources,
      prune_fingerprint,
      central_package_fingerprint: project.central_package_fingerprint(),
      source_protocol: lock.source_protocol,
      signature_validation: config.signature_validation,
      audit_enabled: project.nuget_audit_enabled(),
      audit_mode: project.nuget_audit_mode(),
      audit_level: project.nuget_audit_level(),
      proxy_configured: config.proxy.is_some(),
    },
    &work,
    &source_work,
    &downgrades,
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

fn lock_content_files(root: &Path, values: Vec<LockContentFile>) -> Result<(Vec<WorkContentFile>, Vec<String>), PackageError> {
  let mut files = Vec::with_capacity(values.len());
  let mut actions = Vec::<String>::new();
  for value in values {
    let action = value.build_action.trim();
    let Some(action) = canonical_content_build_action(action) else {
      return Err(PackageError::new(
        PackageErrorKind::Integrity,
        root.display().to_string(),
        format!("locked content file has unknown build action {:?}", value.build_action),
      ));
    };
    let build_action = if action.eq_ignore_ascii_case(DEFAULT_CONTENT_BUILD_ACTION) {
      NO_CONTENT_BUILD_ACTION
    } else if let Some(index) = actions.iter().position(|candidate| candidate.eq_ignore_ascii_case(action)) {
      u32_len(index, "package content actions")?
    } else {
      let index = u32_len(actions.len(), "package content actions")?;
      actions.push(action.to_owned());
      index
    };
    files.push(WorkContentFile {
      path: lock_asset_path(root, &value.path)?,
      build_action,
      copy_to_output: value.copy_to_output,
      flatten: value.flatten,
    });
  }
  Ok((files, actions))
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

fn validate_locked_package(root: &Path, request: &PackageRequest, expected_hash: &str, signature_policy: &SignaturePolicy) -> Result<(), PackageError> {
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
  if signature_policy.mode == SignatureValidationMode::Require {
    let nupkg = root.join(format!("{}.{}.nupkg", request.lower_id, request.version));
    package_signature::verify_package(&nupkg, signature_policy)?;
  }
  Ok(())
}

fn write_lock(resolution: &PackageResolution) -> Result<(), PackageError> {
  if resolution.packages.is_empty() {
    return Ok(());
  }
  let mut direct = Vec::new();
  let mut packages = Vec::with_capacity(resolution.packages.len());
  let mut framework_cursor = 0usize;
  for (index, package) in resolution.packages.iter().copied().enumerate() {
    let id = resolution.package_id(package).to_owned();
    let version = resolution.package_version(package).to_owned();
    if package.direct
      && let Some(policy) = resolution.direct_policies.iter().find(|policy| policy.package_index as usize == index)
    {
      direct.push(LockDirect {
        id: id.clone(),
        version: version.clone(),
        include_assets: policy.include_assets.bits(),
      });
    }
    let dependencies = resolution
      .package_dependencies(package)
      .map(|dependency| {
        let dependency = resolution.packages[dependency as usize];
        LockDependency {
          id: resolution.package_id(dependency).to_owned(),
          version: resolution.package_version(dependency).to_owned(),
        }
      })
      .collect();
    let root = resolution.package_root_at(index);
    let framework_row = resolution
      .package_framework_assets
      .get(framework_cursor)
      .filter(|framework| framework.package_index as usize == index)
      .map(|_| {
        let row = framework_cursor;
        framework_cursor += 1;
        row
      });
    packages.push(LockPackage {
      id,
      version,
      sha512: resolution.package_hash(index).to_owned(),
      direct: package.direct,
      central_transitive: package.central_transitive,
      dependencies,
      compile_assets: relative_assets(root, resolution.package_compile_assets(index))?,
      runtime_assets: relative_assets(root, resolution.package_runtime_assets(index))?,
      analyzers: relative_assets(root, resolution.package_analyzers(index))?,
      resource_assets: relative_assets(root, resolution.package_resource_assets(index))?,
      content_files: resolution
        .package_content_files_with_metadata(index)
        .map(|(path, build_action, copy_to_output, flatten)| {
          Ok(LockContentFile {
            path: relative_asset(root, path)?,
            build_action: build_action.to_owned(),
            copy_to_output,
            flatten,
          })
        })
        .collect::<Result<Vec<_>, PackageError>>()?,
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
      framework_references: framework_row
        .map(|row| resolution.package_framework_references(row).map(str::to_owned).collect())
        .unwrap_or_default(),
      framework_assemblies: framework_row
        .map(|row| resolution.package_framework_assemblies(row).map(str::to_owned).collect())
        .unwrap_or_default(),
    });
  }
  debug_assert_eq!(framework_cursor, resolution.package_framework_assets.len());
  let lock = LockFile {
    schema_version: LOCK_SCHEMA_VERSION,
    target_framework: resolution.target_framework().into(),
    runtime_identifier: resolution.runtime_identifier().map(str::to_owned),
    runtime_graph_fingerprint: resolution.get(resolution.runtime_graph_fingerprint).to_owned(),
    source: resolution.source_location().into(),
    source_protocol: resolution.source_protocol,
    prune_fingerprint: resolution.get(resolution.prune_fingerprint).into(),
    central_package_fingerprint: resolution.get(resolution.central_package_fingerprint).into(),
    direct,
    packages,
    downgrades: resolution
      .downgrades()
      .map(|warning| LockDowngrade {
        package_id: resolution.downgrade_package_id(warning).to_owned(),
        selected_version: resolution.downgrade_selected_version(warning).to_owned(),
        requested_range: resolution.downgrade_requested_range(warning).to_owned(),
        requesting_package: resolution.downgrade_requesting_package(warning).to_owned(),
      })
      .collect(),
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

  fn get(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
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
    net::TcpListener,
    sync::atomic::{AtomicU64, Ordering},
  };

  use crate::{ProjectConfiguration, evaluate_project_path};
  use zip::{ZipWriter, write::SimpleFileOptions};

  use super::*;

  static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

  fn response_server(responses: Vec<String>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
      for response in responses {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).unwrap();
        stream.write_all(response.as_bytes()).unwrap();
      }
    });
    (format!("http://{address}/index.json"), worker)
  }

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
      central_transitive: false,
    }
  }

  fn requirement(id: &str, range: &str) -> PackageRequirement {
    PackageRequirement {
      id: id.into(),
      lower_id: id.to_ascii_lowercase(),
      range: VersionRange::parse(range).unwrap(),
      direct: false,
      include_assets: AssetFlags::ALL,
      suppress_parent: AssetFlags::NONE,
    }
  }

  fn constraint_node(id: &str) -> ConstraintNode {
    ConstraintNode {
      id: id.into(),
      direct: None,
      central_pin: None,
      constraints: BTreeMap::new(),
      selected: None,
      metadata_version: None,
      dependencies: Vec::new(),
      available_versions: None,
      pruned: false,
      generation: 0,
    }
  }

  fn work_package(id: &str, dependencies: &[&str]) -> WorkPackage {
    WorkPackage {
      request: PackageRequest {
        id: id.into(),
        lower_id: id.to_ascii_lowercase(),
        version: "1.0.0".into(),
        direct: false,
        central_transitive: false,
      },
      root: PathBuf::new(),
      hash: String::new(),
      dependencies: dependencies
        .iter()
        .map(|dependency| PackageRequest {
          id: (*dependency).into(),
          lower_id: dependency.to_ascii_lowercase(),
          version: "1.0.0".into(),
          direct: false,
          central_transitive: false,
        })
        .collect(),
      compile_assets: Vec::new(),
      runtime_assets: Vec::new(),
      analyzers: Vec::new(),
      resource_assets: Vec::new(),
      content_files: Vec::new(),
      content_actions: Vec::new(),
      build_assets: Vec::new(),
      build_multi_targeting_assets: Vec::new(),
      build_transitive_assets: Vec::new(),
      native_assets: Vec::new(),
      runtime_targets: Vec::new(),
      framework_references: Vec::new(),
      framework_assemblies: Vec::new(),
      cache_hit: true,
      origin: None,
    }
  }

  fn write_test_package(temp: &TempDirectory, relative: &str, id: &str, version: &str) -> PathBuf {
    let manifest = format!(r#"<package><metadata><id>{id}</id><version>{version}</version></metadata></package>"#);
    write_test_package_manifest(temp, relative, id, &manifest)
  }

  fn write_test_package_manifest(temp: &TempDirectory, relative: &str, id: &str, manifest: &str) -> PathBuf {
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
    archive.write_all(manifest.as_bytes()).unwrap();
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
  fn central_transitive_pin_rejects_a_dependency_downgrade() {
    let node = ConstraintNode {
      id: "Pinned.Package".into(),
      direct: None,
      central_pin: Some(PackageVersion::parse("1.0.0").unwrap()),
      constraints: [("parent.package".into(), VersionRange::parse("[2.0.0,)").unwrap())].into(),
      selected: None,
      metadata_version: None,
      dependencies: Vec::new(),
      available_versions: None,
      pruned: false,
      generation: 0,
    };

    let error = match select_node_version(&node) {
      Err(error) => error,
      Ok(_) => panic!("an incompatible central pin must fail"),
    };
    assert_eq!(error.kind(), PackageErrorKind::Downgrade);
    assert!(error.to_string().contains("central package pin"));
    assert!(error.diagnostic_context().any(|field| field == ("required_range", "[2.0.0,)")));
  }

  #[test]
  fn selector_distinguishes_conflicting_constraints_from_an_absent_version() {
    let mut conflict = constraint_node("Conflict.Leaf");
    conflict.constraints.insert("left".into(), VersionRange::parse("[1.0.0]").unwrap());
    conflict.constraints.insert("right".into(), VersionRange::parse("[2.0.0]").unwrap());
    conflict.available_versions = Some(["1.0.0", "2.0.0"].into_iter().map(|version| PackageVersion::parse(version).unwrap()).collect());
    let error = match select_node_version(&conflict) {
      Err(error) => error,
      Ok(_) => panic!("disjoint exact constraints must fail"),
    };
    assert_eq!(error.kind(), PackageErrorKind::ConstraintConflict);

    let mut missing = constraint_node("Missing.Version");
    missing.direct = Some(VersionRange::parse("[3.0.0]").unwrap());
    missing.available_versions = Some(["1.0.0", "2.0.0"].into_iter().map(|version| PackageVersion::parse(version).unwrap()).collect());
    let error = match select_node_version(&missing) {
      Err(error) => error,
      Ok(_) => panic!("an absent direct version must fail"),
    };
    assert_eq!(error.kind(), PackageErrorKind::VersionNotFound);
    assert!(error.diagnostic_context().any(|field| field == ("nearest_version", "2.0.0")));
  }

  #[test]
  fn incompatible_asset_groups_report_the_target_and_supported_frameworks() {
    let temp = TempDirectory::new();
    temp.write("package/lib/net11.0/Sample.dll", b"assembly");
    let supported = incompatible_asset_frameworks(&temp.0.join("package"), TargetFramework::parse("net10.0").unwrap())
      .unwrap()
      .expect("net11 assets are not compatible with net10");
    assert_eq!(supported, ["net11.0"]);
  }

  #[test]
  fn dependency_cycles_report_the_exact_deterministic_chain() {
    let packages = BTreeMap::from([
      ("cycle.a".into(), work_package("Cycle.A", &["Cycle.B"])),
      ("cycle.b".into(), work_package("Cycle.B", &["Cycle.C"])),
      ("cycle.c".into(), work_package("Cycle.C", &["Cycle.A"])),
    ]);
    let error = validate_acyclic(&packages).expect_err("a dependency cycle must fail");
    assert_eq!(error.kind(), PackageErrorKind::DependencyCycle);
    assert!(
      error
        .diagnostic_context()
        .any(|field| field == ("cycle", "cycle.a -> cycle.b -> cycle.c -> cycle.a"))
    );
  }

  #[test]
  fn direct_wins_downgrades_are_compacted_after_graph_convergence() {
    let mut leaf = constraint_node("Leaf.Package");
    leaf.direct = Some(VersionRange::parse("[1.0.0]").unwrap());
    leaf.selected = Some(PackageVersion::parse("1.0.0").unwrap());
    leaf.constraints.insert("top.package".into(), VersionRange::parse("[2.0.0]").unwrap());
    let top = constraint_node("Top.Package");
    let nodes = BTreeMap::from([("leaf.package".into(), leaf), ("top.package".into(), top)]);
    let warnings = collect_downgrades(&nodes);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].package_id, "Leaf.Package");
    assert_eq!(warnings[0].selected_version, "1.0.0");
    assert_eq!(warnings[0].requested_range, "[2.0.0]");
    assert_eq!(warnings[0].requesting_package, "Top.Package");
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
  fn floating_ranges_match_nuget_numeric_and_prerelease_prefixes() {
    let stable_any = VersionRange::parse("*").unwrap();
    let stable_minor = VersionRange::parse("1.2.*").unwrap();
    let absolute_latest = VersionRange::parse("*-*").unwrap();
    let release_candidate = VersionRange::parse("1.2.0-rc.*").unwrap();
    let patch_prerelease = VersionRange::parse("1.2.*-beta.*").unwrap();

    assert!(stable_any.floating_satisfies(&PackageVersion::parse("99.0.0").unwrap()));
    assert!(!stable_any.floating_satisfies(&PackageVersion::parse("99.0.0-preview.1").unwrap()));
    assert!(stable_minor.floating_satisfies(&PackageVersion::parse("1.2.99").unwrap()));
    assert!(!stable_minor.floating_satisfies(&PackageVersion::parse("1.3.0").unwrap()));
    assert!(absolute_latest.floating_satisfies(&PackageVersion::parse("99.0.0-preview.1").unwrap()));
    assert!(release_candidate.floating_satisfies(&PackageVersion::parse("1.2.0-rc.12").unwrap()));
    assert!(release_candidate.floating_satisfies(&PackageVersion::parse("1.2.0").unwrap()));
    assert!(!release_candidate.floating_satisfies(&PackageVersion::parse("1.2.0-beta.1").unwrap()));
    assert!(patch_prerelease.floating_satisfies(&PackageVersion::parse("1.2.7-beta.3").unwrap()));
    assert!(patch_prerelease.floating_satisfies(&PackageVersion::parse("1.2.8").unwrap()));
    assert!(!patch_prerelease.floating_satisfies(&PackageVersion::parse("1.2.7-rc.1").unwrap()));
  }

  #[test]
  fn nuget_floating_parser_accepts_numeric_prerelease_and_interval_forms() {
    for value in [
      "*",
      "*-*",
      "0*",
      "1.*",
      "1.2*",
      "1.2.*",
      "1.2.3.*",
      "1.2.3-*",
      "1.2.3-rc.*",
      "1.2.*-*",
      "1.2.*-preview.1.*",
      "*-rc.*",
      "[1.*,2.0)",
      "[1.2.0-rc.*, )",
    ] {
      assert!(VersionRange::parse(value).is_ok(), "{value} did not parse");
    }
  }

  #[test]
  fn malformed_floating_ranges_fail_instead_of_becoming_approximate_ranges() {
    for value in [
      "[*]",
      "1.2.*.3",
      "[1.0,2.*)",
      "1.2.*-beta",
      "1.2.*-*.*",
      "1.2.*+build",
      "1.0.0.0.*",
      "1.*.0",
      "1.0.**",
    ] {
      assert!(VersionRange::parse(value).is_err(), "{value} unexpectedly parsed");
    }
  }

  fn selected_version(range: &str, versions: &[&str]) -> Option<String> {
    let node = ConstraintNode {
      id: "Floating.Package".into(),
      direct: Some(VersionRange::parse(range).unwrap()),
      central_pin: None,
      constraints: BTreeMap::new(),
      selected: None,
      metadata_version: None,
      dependencies: Vec::new(),
      available_versions: Some(versions.iter().map(|version| PackageVersion::parse(version).unwrap()).collect()),
      pruned: false,
      generation: 0,
    };
    match select_node_version(&node) {
      Ok(NodeSelection::Version(version)) => Some(version.normalized),
      Ok(NodeSelection::Enumerate) | Err(_) => None,
    }
  }

  #[test]
  fn floating_selection_matches_nuget_best_match_fallback_rules() {
    let versions = ["1.1.0", "1.2.0-rc.1", "1.2.0-rc.2", "2.0.0", "3.0.0-beta.1"];
    assert_eq!(selected_version("*", &versions).as_deref(), Some("2.0.0"));
    assert_eq!(selected_version("1.*", &versions).as_deref(), Some("1.1.0"));
    assert_eq!(selected_version("1.2.0-*", &versions).as_deref(), Some("1.2.0-rc.2"));
    assert_eq!(selected_version("*-*", &versions).as_deref(), Some("3.0.0-beta.1"));
    assert_eq!(selected_version("1.*-*", &versions).as_deref(), Some("1.2.0-rc.2"));
    assert_eq!(selected_version("1.*", &["2.0.0", "3.0.0"]).as_deref(), Some("2.0.0"));
    assert_eq!(selected_version("[1.*,2.0)", &["1.1.0", "1.9.0", "2.0.0"]).as_deref(), Some("1.9.0"));
  }

  #[test]
  fn floating_direct_dependencies_select_the_highest_matching_version() {
    let node = ConstraintNode {
      id: "Floating.Package".into(),
      direct: Some(VersionRange::parse("1.2.*-rc.*").unwrap()),
      central_pin: None,
      constraints: BTreeMap::new(),
      selected: None,
      metadata_version: None,
      dependencies: Vec::new(),
      available_versions: Some(
        ["1.2.0-beta.1", "1.2.0-rc.1", "1.2.0", "1.2.1-rc.2", "1.2.1", "1.3.0"]
          .into_iter()
          .map(|version| PackageVersion::parse(version).unwrap())
          .collect(),
      ),
      pruned: false,
      generation: 0,
    };

    let NodeSelection::Version(selected) = select_node_version(&node).unwrap() else {
      panic!("floating dependencies require an enumerated version");
    };
    assert_eq!(selected.normalized, "1.2.1");
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
  fn generated_net8_and_net9_tables_match_the_sdk_oracle() {
    let mut net8 = Vec::new();
    let mut net9 = Vec::new();
    let net8_target = TargetFramework::parse("net8.0").unwrap();
    let net9_target = TargetFramework::parse("net9.0").unwrap();
    let net8_table = exact_legacy_pruning(net8_target.family(), net8_target.major(), net8_target.minor(), PruningFramework::Core).unwrap();
    let net9_table = exact_legacy_pruning(net9_target.family(), net9_target.major(), net9_target.minor(), PruningFramework::Core).unwrap();
    extend_legacy_packages(&mut net8, net8_table, true).unwrap();
    extend_legacy_packages(&mut net9, net9_table, true).unwrap();
    extend_legacy_packages(
      &mut net8,
      exact_legacy_pruning(net8_target.family(), net8_target.major(), net8_target.minor(), PruningFramework::AspNetCore).unwrap(),
      true,
    )
    .unwrap();
    extend_legacy_packages(
      &mut net9,
      exact_legacy_pruning(net9_target.family(), net9_target.major(), net9_target.minor(), PruningFramework::AspNetCore).unwrap(),
      true,
    )
    .unwrap();

    let net8 = compact_package_pruning(net8).unwrap();
    let net9 = compact_package_pruning(net9).unwrap();

    assert_eq!(net8.packages.len(), 418);
    assert_eq!(net9.packages.len(), 420);
    assert!(net8.contains("system.text.json", &PackageVersion::parse("8.0.32767").unwrap()));
    assert!(!net8.contains("system.text.json", &PackageVersion::parse("8.0.32768").unwrap()));
    assert!(net9.contains("system.text.json", &PackageVersion::parse("9.0.32767").unwrap()));
  }

  #[test]
  fn legacy_pruning_policy_rejects_missing_data_unless_explicitly_allowed() {
    let temp = TempDirectory::new();
    let missing_path = temp.write(
      "Missing.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>netstandard3.0</TargetFramework><RestoreEnablePackagePruning>true</RestoreEnablePackagePruning></PropertyGroup></Project>"#,
    );
    let allowed_path = temp.write(
      "Allowed.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>netstandard3.0</TargetFramework><RestoreEnablePackagePruning>true</RestoreEnablePackagePruning><AllowMissingPrunePackageData>true</AllowMissingPrunePackageData></PropertyGroup></Project>"#,
    );
    let framework_path = temp.write(
      "Framework.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net48</TargetFramework><RestoreEnablePackagePruning>true</RestoreEnablePackagePruning></PropertyGroup></Project>"#,
    );

    let missing = evaluate_project_path(&missing_path, ProjectConfiguration::Debug).unwrap();
    let allowed = evaluate_project_path(&allowed_path, ProjectConfiguration::Debug).unwrap();
    let framework = evaluate_project_path(&framework_path, ProjectConfiguration::Debug).unwrap();
    let allowed = discover_package_pruning(&allowed, None).unwrap();
    let missing_error = match discover_package_pruning(&missing, None) {
      Ok(_) => panic!("missing pruning data must fail"),
      Err(error) => error,
    };
    let framework_error = match discover_package_pruning(&framework, None) {
      Ok(_) => panic!(".NET Framework pruning without data must fail"),
      Err(error) => error,
    };

    assert_eq!(missing_error.kind, PackageErrorKind::Configuration);
    assert_eq!(framework_error.kind, PackageErrorKind::Configuration);
    assert!(allowed.packages.is_empty());
    assert!(!allowed.fingerprint.is_empty());
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
          security_flags: 0,
        },
      )],
      credentials: SourceCredentialBatch::default(),
      audit_sources: Vec::new(),
      source_mapping: None,
      signature_validation: SignatureValidationMode::Accept,
      signature_policy: Arc::new(SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new())),
      proxy: None,
      http_policy: DEFAULT_HTTP_POLICY,
    };

    let result = read_warm_lock(&path, &config, &[], &project, "current-table", "").unwrap();

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
          central_pin: None,
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
          central_pin: None,
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
          central_pin: None,
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
      central_pin: None,
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
  fn exclusive_lower_bound_enumerates_the_lowest_available_version() {
    let mut node = constraint_node("Common.Package");
    node.constraints = BTreeMap::from([
      ("a".into(), VersionRange::parse("(1.0.0,3.0.0)").unwrap()),
      ("b".into(), VersionRange::parse("[1.0.0,2.0.0]").unwrap()),
    ]);

    assert!(matches!(select_node_version(&node).unwrap(), NodeSelection::Enumerate));

    node.available_versions = Some(
      ["1.0.0", "1.1.0", "2.0.0", "3.0.0"]
        .into_iter()
        .map(|version| PackageVersion::parse(version).unwrap())
        .collect(),
    );
    let NodeSelection::Version(selected) = select_node_version(&node).unwrap() else {
      panic!("an exclusive lower bound must select from available versions");
    };
    assert_eq!(selected.normalized, "1.1.0");
  }

  #[test]
  fn direct_dependency_wins_over_a_transitive_minimum() {
    let node = ConstraintNode {
      id: "Direct.Package".into(),
      direct: Some(VersionRange::exact(PackageVersion::parse("1.0.0").unwrap())),
      central_pin: None,
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
  fn direct_dependency_wins_inside_a_package_subgraph() {
    let mut top = constraint_node("Top.Package");
    top.direct = Some(VersionRange::exact(PackageVersion::parse("1.0.0").unwrap()));
    top.selected = Some(PackageVersion::parse("1.0.0").unwrap());
    top.metadata_version = top.selected.clone();
    top.dependencies = vec![requirement("Common.Package", "1.0"), requirement("Deep.Package", "1.0")];

    let mut deep = constraint_node("Deep.Package");
    deep.constraints.insert("top.package".into(), VersionRange::parse("1.0").unwrap());
    deep.selected = Some(PackageVersion::parse("1.0.0").unwrap());
    deep.metadata_version = deep.selected.clone();
    deep.dependencies = vec![requirement("Common.Package", "2.0")];

    let mut common = constraint_node("Common.Package");
    common.constraints = BTreeMap::from([
      ("deep.package".into(), VersionRange::parse("2.0").unwrap()),
      ("top.package".into(), VersionRange::parse("1.0").unwrap()),
    ]);
    let mut nodes = BTreeMap::from([("common.package".into(), common), ("deep.package".into(), deep), ("top.package".into(), top)]);
    let mut dirty = BTreeSet::from(["common.package".into()]);
    let mut ready = BTreeSet::new();

    stabilize_constraint_nodes(&mut nodes, &mut dirty, &mut ready, &PackagePruning::default()).unwrap();

    assert_eq!(nodes["common.package"].selected.as_ref().unwrap().normalized, "1.0.0");
  }

  #[test]
  fn cousin_constraints_at_different_depths_still_converge() {
    let mut left = constraint_node("Left.Package");
    left.direct = Some(VersionRange::parse("1.0").unwrap());
    left.dependencies = vec![requirement("Common.Package", "1.0")];

    let mut right = constraint_node("Right.Package");
    right.direct = Some(VersionRange::parse("1.0").unwrap());
    right.dependencies = vec![requirement("Bridge.Package", "1.0")];

    let mut bridge = constraint_node("Bridge.Package");
    bridge.constraints.insert("right.package".into(), VersionRange::parse("1.0").unwrap());
    bridge.dependencies = vec![requirement("Common.Package", "2.0")];

    let mut common = constraint_node("Common.Package");
    common.constraints = BTreeMap::from([
      ("bridge.package".into(), VersionRange::parse("2.0").unwrap()),
      ("left.package".into(), VersionRange::parse("1.0").unwrap()),
    ]);
    let mut nodes = BTreeMap::from([
      ("bridge.package".into(), bridge),
      ("common.package".into(), common),
      ("left.package".into(), left),
      ("right.package".into(), right),
    ]);
    let mut dirty = BTreeSet::from(["common.package".into()]);
    let mut ready = BTreeSet::new();

    stabilize_constraint_nodes(&mut nodes, &mut dirty, &mut ready, &PackagePruning::default()).unwrap();

    assert_eq!(nodes["common.package"].selected.as_ref().unwrap().normalized, "2.0.0");
  }

  #[test]
  fn a_shared_direct_parent_stays_a_cousin_in_a_diamond_graph() {
    let mut provider = constraint_node("Provider.Package");
    provider.direct = Some(VersionRange::parse("1.0").unwrap());
    provider.dependencies = vec![requirement("Common.Package", "[1.0,3.0)"), requirement("Relational.Package", "1.0")];

    let mut relational = constraint_node("Relational.Package");
    relational.direct = Some(VersionRange::parse("1.0").unwrap());
    relational.constraints.insert("provider.package".into(), VersionRange::parse("1.0").unwrap());
    relational.dependencies = vec![requirement("Common.Package", "2.0")];

    let mut common = constraint_node("Common.Package");
    common.constraints = BTreeMap::from([
      ("provider.package".into(), VersionRange::parse("[1.0,3.0)").unwrap()),
      ("relational.package".into(), VersionRange::parse("2.0").unwrap()),
    ]);
    let mut nodes = BTreeMap::from([
      ("common.package".into(), common),
      ("provider.package".into(), provider),
      ("relational.package".into(), relational),
    ]);
    let mut dirty = BTreeSet::from(["common.package".into()]);
    let mut ready = BTreeSet::new();

    stabilize_constraint_nodes(&mut nodes, &mut dirty, &mut ready, &PackagePruning::default()).unwrap();

    assert_eq!(nodes["common.package"].selected.as_ref().unwrap().normalized, "2.0.0");
  }

  #[test]
  fn retracting_an_ancestor_edge_rechecks_descendant_conflicts() {
    let mut ancestor = constraint_node("Ancestor.Package");
    ancestor.dependencies = vec![requirement("Common.Package", "1.0"), requirement("Switch.Package", "1.0")];

    let mut switch = constraint_node("Switch.Package");
    switch.direct = Some(VersionRange::exact(PackageVersion::parse("2.0.0").unwrap()));
    switch.selected = Some(PackageVersion::parse("1.0.0").unwrap());
    switch.metadata_version = switch.selected.clone();
    switch.dependencies = vec![requirement("Deep.Package", "1.0")];
    switch.generation = 1;

    let mut deep = constraint_node("Deep.Package");
    deep.direct = Some(VersionRange::parse("1.0").unwrap());
    deep.constraints.insert("switch.package".into(), VersionRange::parse("1.0").unwrap());
    deep.dependencies = vec![requirement("Common.Package", "2.0")];
    deep.selected = Some(PackageVersion::parse("1.0.0").unwrap());
    deep.metadata_version = deep.selected.clone();

    let mut common = constraint_node("Common.Package");
    common.constraints = BTreeMap::from([
      ("ancestor.package".into(), VersionRange::parse("1.0").unwrap()),
      ("deep.package".into(), VersionRange::parse("2.0").unwrap()),
    ]);
    common.selected = Some(PackageVersion::parse("1.0.0").unwrap());
    common.generation = 1;
    let mut nodes = BTreeMap::from([
      ("ancestor.package".into(), ancestor),
      ("common.package".into(), common),
      ("deep.package".into(), deep),
      ("switch.package".into(), switch),
    ]);
    let mut dirty = BTreeSet::from(["switch.package".into()]);
    let mut ready = BTreeSet::new();

    stabilize_constraint_nodes(&mut nodes, &mut dirty, &mut ready, &PackagePruning::default()).unwrap();

    assert_eq!(nodes["common.package"].selected.as_ref().unwrap().normalized, "2.0.0");
  }

  #[test]
  fn stable_ranges_do_not_select_prerelease_versions_during_enumeration() {
    let node = ConstraintNode {
      id: "Stable.Package".into(),
      direct: Some(VersionRange::parse("[1.0,2.0)").unwrap()),
      central_pin: None,
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
          central_pin: None,
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
          central_pin: None,
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
  fn package_source_security_requires_explicit_http_opt_in_and_keeps_flags_per_source() {
    let temp = TempDirectory::new();
    let rejected = temp.write(
      "rejected.config",
      r#"<configuration><packageSources><clear />
<add key="rejected" value="http://packages.example.test/v3/index.json" protocolVersion="3" />
</packageSources></configuration>"#,
    );
    let error = merge_config(&rejected, &mut NugetConfigMerge::default()).unwrap_err();
    assert_eq!(error.kind(), PackageErrorKind::Configuration);
    assert!(error.to_string().contains("allowInsecureConnections=true"));

    let accepted = temp.write(
      "accepted.config",
      r#"<configuration><packageSources><clear />
<add key="http" value="HTTP://packages.example.test/v3/index.json" protocolVersion="3" allowInsecureConnections=" TRUE " />
<add key="tls" value="https://private.example.test/v3/index.json" protocolVersion="3" disableTLSCertificateValidation="true" />
<add key="invalid" value="https://secure.example.test/v3/index.json" protocolVersion="3" allowInsecureConnections="not-a-bool" disableTLSCertificateValidation="false" />
</packageSources></configuration>"#,
    );
    let mut merged = NugetConfigMerge::default();
    merge_config(&accepted, &mut merged).unwrap();

    assert!(merged.sources[0].1.allow_insecure_connections());
    assert!(merged.sources[0].1.tls_validation());
    assert!(!merged.sources[1].1.allow_insecure_connections());
    assert!(!merged.sources[1].1.tls_validation());
    assert_eq!(merged.sources[2].1.security_flags, 0);
    assert_eq!(size_of::<PackageSourceRecord>(), 28);
    assert_eq!(align_of::<PackageSourceRecord>(), 4);
  }

  #[test]
  fn command_line_http_source_must_match_an_opted_in_configured_source() {
    let configured = vec![(
      "private".to_owned(),
      PackageSource {
        url: "http://packages.example.test/v3/index.json".to_owned(),
        protocol: NugetProtocol::V3,
        security_flags: SOURCE_ALLOW_INSECURE_CONNECTIONS,
      },
    )];
    let selected = command_line_sources(&["http://packages.example.test/v3/index.json".to_owned()], configured, Path::new(".")).unwrap();
    assert_eq!(selected[0].0, "private");
    assert!(selected[0].1.allow_insecure_connections());

    let error = command_line_sources(&["http://other.example.test/v3/index.json".to_owned()], Vec::new(), Path::new("."))
      .err()
      .unwrap();
    assert!(error.to_string().contains("allowInsecureConnections=true"));

    let selected = command_line_sources(
      &["https://packages.example.test/v3/index.json?sig=cli-secret#fragment".to_owned()],
      Vec::new(),
      Path::new("."),
    )
    .unwrap();
    assert_eq!(selected[0].0, "https://packages.example.test/v3/index.json");
    assert!(selected[0].1.url.contains("cli-secret"));

    let origin = HttpOrigin::parse("http://packages.example.test:80/v3/index.json", Path::new("NuGet.Config")).unwrap();
    assert!(!origin.matches("ftp://packages.example.test:80/archive"));
  }

  #[test]
  fn package_source_rejects_embedded_credentials_before_reporting() {
    let error = PackageSource::parse(
      "https://user:secret@packages.example.test/v3/index.json".into(),
      Some("3"),
      false,
      false,
      Path::new("NuGet.Config"),
      Path::new("."),
    )
    .err()
    .expect("embedded credentials must be rejected");

    assert_eq!(error.kind(), PackageErrorKind::Configuration);
    assert!(error.message.contains("must not embed credentials"));
  }

  #[test]
  fn client_certificates_merge_by_kind_and_reject_ambiguous_sources() {
    let temp = TempDirectory::new();
    let lower = temp.write(
      "lower.config",
      r#"<configuration>
<packageSources><clear /><add key="private" value="https://packages.example.test/v3/index.json" protocolVersion="3" /></packageSources>
<clientCertificates>
  <fileCert packageSource="private" path="old.pfx" clearTextPassword="old-secret" />
</clientCertificates></configuration>"#,
    );
    let higher = temp.write(
      "higher.config",
      r#"<configuration><clientCertificates>
  <fileCert packageSource="private" path="new.pfx" clearTextPassword="new-secret" />
  <storeCert packageSource="private" findValue="00112233445566778899AABBCCDDEEFF00112233" />
</clientCertificates></configuration>"#,
    );
    let mut merged = NugetConfigMerge::default();

    merge_config(&lower, &mut merged).unwrap();
    merge_config(&higher, &mut merged).unwrap();

    assert_eq!(merged.client_certificates.len(), 2);
    let MergedClientCertificate::File { path, password, .. } = &merged.client_certificates[0] else {
      panic!("the higher fileCert replaces the lower fileCert");
    };
    assert_eq!(path, &temp.0.join("new.pfx"));
    assert!(matches!(password, Some(StoredCredentialPassword::Clear(value)) if value.as_str() == "new-secret"));
    let error = attach_client_certificates(
      &merged.sources,
      merged.client_certificates,
      None,
      &temp.0,
      &mut SourceCredentialBatch::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("more than one client certificate"));
    assert!(!error.to_string().contains("secret"));
  }

  #[test]
  fn source_credentials_follow_config_name_decoding_and_environment_precedence() {
    let temp = TempDirectory::new();
    let lower = temp.write(
      "lower.config",
      r#"<configuration>
<packageSources><clear /><add key="Private Feed" value="https://packages.example.test/v3/index.json" protocolVersion="3" /></packageSources>
<packageSourceCredentials><Private_x0020_Feed>
  <add key="Username" value="config-user" />
  <add key="ClearTextPassword" value="config-secret" />
  <add key="ValidAuthenticationTypes" value="negotiate" />
</Private_x0020_Feed></packageSourceCredentials>
</configuration>"#,
    );
    let higher = temp.write(
      "higher.config",
      r#"<configuration><packageSourceCredentials><Private_x0020_Feed>
  <add key="Username" value="higher-user" />
  <add key="ClearTextPassword" value="higher-secret" />
  <add key="ValidAuthenticationTypes" value="basic" />
</Private_x0020_Feed></packageSourceCredentials></configuration>"#,
    );
    let mut merged = NugetConfigMerge::default();
    merge_config(&lower, &mut merged).unwrap();
    merge_config(&higher, &mut merged).unwrap();
    assert_eq!(merged.credentials.len(), 1);
    assert_eq!(merged.credentials[0].source, "Private Feed");

    let sources = merged.sources.clone();
    let credentials = resolve_source_credentials_with(&sources, merged.credentials, &temp.0, CredentialProviderOptions::default(), |name| {
      assert_eq!(name, "NuGetPackageSourceCredentials_Private Feed");
      Some("Username=environment-user; Password=environment-pat;ValidAuthenticationTypes=basic".into())
    })
    .unwrap();
    let credential = credentials.get(0).unwrap();
    let request = authenticated_get(&reqwest::Client::new(), Some(credential), "https://packages.example.test/v3/index.json")
      .build()
      .unwrap();
    let authorization = request.headers().get(AUTHORIZATION).unwrap();
    assert_eq!(authorization, "Basic ZW52aXJvbm1lbnQtdXNlcjplbnZpcm9ubWVudC1wYXQ=");
    assert!(authorization.is_sensitive());

    let foreign = authenticated_get(&reqwest::Client::new(), Some(credential), "https://other.example.test/v3/index.json")
      .build()
      .unwrap();
    assert!(!foreign.headers().contains_key(AUTHORIZATION));

    let client = reqwest::Client::new();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let inventory = runtime
      .block_on(inspect_source_batch(
        &client,
        &sources,
        &credentials,
        false,
        false,
        DEFAULT_HTTP_POLICY.with_offline(true),
      ))
      .unwrap();
    assert_eq!(inventory.source_authentication(0), PackageSourceAuthentication::Basic);
    assert!(!inventory.text.contains("config-secret"));
    assert!(!inventory.text.contains("higher-secret"));
    assert!(!inventory.text.contains("environment-user"));
    assert!(!inventory.text.contains("environment-pat"));
  }

  #[test]
  fn source_inventory_redacts_query_credentials_from_reported_locations() {
    let document =
      r#"{"version":"3.0.0","resources":[{"@id":"https://content.example.test/flat/?sig=endpoint-secret#fragment","@type":"PackageBaseAddress/3.0.0"}]}"#;
    let response = format!(
      "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{document}",
      document.len()
    );
    let (url, worker) = response_server(vec![response]);
    let sources = vec![(
      "https://identity.example.test/index.json?sig=name-secret".to_owned(),
      PackageSource {
        url: format!("{url}?sig=config-secret#fragment"),
        protocol: NugetProtocol::V3,
        security_flags: SOURCE_ALLOW_INSECURE_CONNECTIONS,
      },
    )];
    let client = reqwest::Client::builder().build().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let inventory = runtime
      .block_on(inspect_source_batch(
        &client,
        &sources,
        &SourceCredentialBatch::default(),
        true,
        false,
        DEFAULT_HTTP_POLICY,
      ))
      .unwrap();

    assert_eq!(inventory.source_name(0), "https://identity.example.test/index.json");
    assert_eq!(inventory.source_location(0), url);
    assert_eq!(inventory.endpoint_location(0), "https://content.example.test/flat/");
    assert_eq!(inventory.source_requests(0), 1);
    assert!(inventory.source_downloaded_bytes(0) > 0);
    assert!(inventory.source_duration_us(0) > 0);
    assert!(!inventory.text.contains("secret"));
    assert!(matches!(redact_url_for_output(DEFAULT_SOURCE), std::borrow::Cow::Borrowed(_)));
    worker.join().unwrap();
  }

  #[test]
  fn network_diagnostics_redact_query_credentials() {
    let source = "https://packages.example.test/v3/index.json?sig=diagnostic-secret#fragment";
    let error = network_error(source, format!("request to {source} failed"));

    assert_eq!(error.context(), "https://packages.example.test/v3/index.json");
    assert!(!error.to_string().contains("diagnostic-secret"));
    assert!(error.to_string().contains("https://packages.example.test/v3/index.json"));
  }

  #[test]
  fn public_sources_without_credentials_retain_no_provider_batch() {
    let sources = vec![(
      "nuget.org".to_owned(),
      PackageSource {
        url: DEFAULT_SOURCE.to_owned(),
        protocol: NugetProtocol::V3,
        security_flags: 0,
      },
    )];

    let credentials = resolve_source_credentials_with(&sources, Vec::new(), Path::new("NuGet.Config"), CredentialProviderOptions::default(), |_| None).unwrap();

    assert!(credentials.entries.is_empty());
  }

  #[test]
  fn malformed_environment_credential_falls_back_without_exposing_config_secret() {
    let sources = vec![(
      "private".into(),
      PackageSource {
        url: "https://packages.example.test/v3/index.json".into(),
        protocol: NugetProtocol::V3,
        security_flags: 0,
      },
    )];
    let configured = vec![MergedSourceCredential {
      source: "private".into(),
      username: Zeroizing::new("config-user".into()),
      password: StoredCredentialPassword::Clear(Zeroizing::new("config-secret".into())),
      valid_authentication_types: Some("negotiate".into()),
    }];
    let error = resolve_source_credentials_with(&sources, configured, Path::new("NuGet.Config"), CredentialProviderOptions::default(), |_| {
      Some("malformed-secret-value".into())
    })
    .err()
    .unwrap();
    let rendered = error.to_string();
    assert!(rendered.contains("does not allow Basic authentication"));
    assert!(!rendered.contains("config-user"));
    assert!(!rendered.contains("config-secret"));
    assert!(!rendered.contains("malformed-secret-value"));
  }

  #[test]
  #[cfg(windows)]
  fn windows_encrypted_source_password_uses_nuget_dpapi_entropy() {
    let plaintext = b"encrypted-pat";
    let encrypted = windows_dpapi::encrypt_data(plaintext, windows_dpapi::Scope::User, Some(b"NuGet")).unwrap();
    let encoded = BASE64.encode(encrypted);

    let decrypted = decrypt_nuget_password("private", &encoded, Path::new("NuGet.Config")).unwrap();

    assert_eq!(decrypted.as_bytes(), plaintext);
  }

  #[test]
  fn malformed_environment_credentials_fall_back_to_config() {
    let source = PackageSource {
      url: "https://packages.example.test/v3/index.json".into(),
      protocol: NugetProtocol::V3,
      security_flags: 0,
    };
    let sources = vec![("private".to_owned(), source.clone())];
    let configured = vec![MergedSourceCredential {
      source: "private".into(),
      username: Zeroizing::new("config-user".into()),
      password: StoredCredentialPassword::Clear(Zeroizing::new("config-secret".into())),
      valid_authentication_types: Some("basic".into()),
    }];
    let credentials = resolve_source_credentials_with(&sources, configured, Path::new("NuGet.Config"), CredentialProviderOptions::default(), |_| {
      Some("Password=wrong-order;Username=ignored".into())
    })
    .unwrap();
    let request = authenticated_get(
      &reqwest::Client::new(),
      credentials.get(0).map(Arc::as_ref),
      "https://packages.example.test/v3/index.json",
    )
    .build()
    .unwrap();
    assert_eq!(
      request.headers()[AUTHORIZATION],
      format!("Basic {}", BASE64.encode("config-user:config-secret"))
    );
  }

  #[test]
  fn empty_source_credential_groups_fail_explicitly() {
    let temp = TempDirectory::new();
    let config = temp.write(
      "NuGet.Config",
      r#"<configuration><packageSourceCredentials><private /></packageSourceCredentials></configuration>"#,
    );

    let error = merge_config(&config, &mut NugetConfigMerge::default()).unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::Configuration);
    assert!(error.to_string().contains("requires Username"));
  }

  #[test]
  fn environment_credential_grammar_preserves_semicolons_in_passwords() {
    assert_eq!(
      parse_environment_credential(" Username=user; Password=pat;with;semicolons;ValidAuthenticationTypes=Basic "),
      Some(("user", "pat;with;semicolons", Some("Basic")))
    );
    assert_eq!(parse_environment_credential("Password=secret;Username=user"), None);
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
            security_flags: 0,
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
        security_flags: 0,
      },
    )];
    let mapping = PackageSourceMapping::compile(merged.source_mapping, &sources).unwrap();

    assert!(!mapping.allows(0, "Private.Package"));
    assert!(mapping.allows(0, "Public.Package"));
  }

  #[test]
  fn source_mapping_filters_service_discovery_before_network_io() {
    let document = r#"{"version":"3.0.0","resources":[{"@id":"https://content.example.test/v3-flat/","@type":"PackageBaseAddress/3.0.0"}]}"#;
    let response = format!(
      "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{document}",
      document.len()
    );
    let (selected_url, worker) = response_server(vec![response]);
    let sources = vec![
      (
        "decoy".to_owned(),
        PackageSource {
          url: "http://127.0.0.1:9/v3/index.json".to_owned(),
          protocol: NugetProtocol::V3,
          security_flags: SOURCE_ALLOW_INSECURE_CONNECTIONS,
        },
      ),
      (
        "selected".to_owned(),
        PackageSource {
          url: selected_url,
          protocol: NugetProtocol::V3,
          security_flags: SOURCE_ALLOW_INSECURE_CONNECTIONS,
        },
      ),
    ];
    let mapping = PackageSourceMapping::compile(
      MergedSourceMapping {
        sources: vec![MergedSourceMappingEntry {
          source: "selected".to_owned(),
          patterns: ItemRange { start: 0, len: 1 },
        }],
        patterns: vec!["Selected.*".to_owned()],
      },
      &sources,
    )
    .unwrap();
    let mut endpoints = LazyServiceEndpoints::new(sources.len());
    let mut source_work = source_work_table(sources.len()).unwrap();
    let client = reqwest::Client::builder().build().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    runtime
      .block_on(endpoints.ensure_identity(
        &client,
        &sources,
        &SourceCredentialBatch::default(),
        Some(&mapping),
        "selected.package",
        ServiceDiscoveryOptions {
          worker_budget: MAX_DOWNLOAD_WORKERS as u8,
          allow_network: true,
          source_work: &mut source_work,
        },
      ))
      .unwrap();

    assert_eq!(source_work[1].requests, 1);
    assert!(source_work[1].downloaded_bytes > 0);
    assert!(source_work[1].duration_us > 0);
    assert_eq!(endpoints.snapshot().len(), 1);
    assert_eq!(endpoints.snapshot()[0].source_index(), 1);
    worker.join().unwrap();
  }

  #[test]
  fn unmapped_identity_fails_before_service_discovery() {
    let sources = vec![(
      "decoy".to_owned(),
      PackageSource {
        url: "http://127.0.0.1:9/v3/index.json".to_owned(),
        protocol: NugetProtocol::V3,
        security_flags: SOURCE_ALLOW_INSECURE_CONNECTIONS,
      },
    )];
    let mapping = PackageSourceMapping::compile(
      MergedSourceMapping {
        sources: vec![MergedSourceMappingEntry {
          source: "decoy".to_owned(),
          patterns: ItemRange { start: 0, len: 1 },
        }],
        patterns: vec!["Mapped.*".to_owned()],
      },
      &sources,
    )
    .unwrap();
    let mut endpoints = LazyServiceEndpoints::new(sources.len());
    let mut source_work = source_work_table(sources.len()).unwrap();
    let client = reqwest::Client::builder().build().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let error = runtime
      .block_on(endpoints.ensure_identity(
        &client,
        &sources,
        &SourceCredentialBatch::default(),
        Some(&mapping),
        "unmapped.package",
        ServiceDiscoveryOptions {
          worker_budget: MAX_DOWNLOAD_WORKERS as u8,
          allow_network: true,
          source_work: &mut source_work,
        },
      ))
      .unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::UnmappedIdentity);
    assert!(endpoints.snapshot().is_empty());
  }

  #[test]
  fn empty_source_batch_without_mapping_is_not_an_unmapped_identity() {
    let mut endpoints = LazyServiceEndpoints::new(0);
    let mut source_work = Vec::new();
    let client = reqwest::Client::builder().build().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    runtime
      .block_on(endpoints.ensure_identity(
        &client,
        &[],
        &SourceCredentialBatch::default(),
        None,
        "missing.package",
        ServiceDiscoveryOptions {
          worker_budget: MAX_DOWNLOAD_WORKERS as u8,
          allow_network: true,
          source_work: &mut source_work,
        },
      ))
      .unwrap();

    assert!(source_work.is_empty());
    assert!(endpoints.snapshot().is_empty());
  }

  #[test]
  fn winning_pattern_on_a_disabled_source_is_unmapped() {
    let mapping = PackageSourceMapping::compile(
      MergedSourceMapping {
        sources: vec![MergedSourceMappingEntry {
          source: "disabled".to_owned(),
          patterns: ItemRange { start: 0, len: 1 },
        }],
        patterns: vec!["Selected.*".to_owned()],
      },
      &[],
    )
    .unwrap();
    let mut endpoints = LazyServiceEndpoints::new(0);
    let mut source_work = Vec::new();
    let client = reqwest::Client::builder().build().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let error = runtime
      .block_on(endpoints.ensure_identity(
        &client,
        &[],
        &SourceCredentialBatch::default(),
        Some(&mapping),
        "selected.package",
        ServiceDiscoveryOptions {
          worker_budget: MAX_DOWNLOAD_WORKERS as u8,
          allow_network: true,
          source_work: &mut source_work,
        },
      ))
      .unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::UnmappedIdentity);
    assert!(endpoints.snapshot().is_empty());
  }

  #[test]
  fn unmapped_identity_can_select_an_existing_cached_version() {
    let temp = TempDirectory::new();
    let cache_root = temp.0.join("packages");
    let temp_root = temp.0.join("temp");
    fs::create_dir_all(cache_root.join("unmapped.package/1.2.3")).unwrap();
    fs::create_dir_all(&temp_root).unwrap();
    let sources = vec![(
      "selected".to_owned(),
      PackageSource {
        url: "http://127.0.0.1:9/v3/index.json".to_owned(),
        protocol: NugetProtocol::V3,
        security_flags: SOURCE_ALLOW_INSECURE_CONNECTIONS,
      },
    )];
    let mapping = PackageSourceMapping::compile(
      MergedSourceMapping {
        sources: vec![MergedSourceMappingEntry {
          source: "selected".to_owned(),
          patterns: ItemRange { start: 0, len: 1 },
        }],
        patterns: vec!["Mapped.*".to_owned()],
      },
      &sources,
    )
    .unwrap();
    let unrelated_endpoint = ServiceEndpoint::V2 {
      source: sources[0].1.url.clone(),
      base: sources[0].1.url.clone(),
      credential: None,
      source_index: 0,
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let signature_policy = Arc::new(SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new()));

    let result = runtime
      .block_on(load_node_metadata(
        &reqwest::Client::new(),
        None,
        "unmapped.package",
        PackageStorage {
          cache_root: &cache_root,
          fallback_roots: &[],
          temp_root: &temp_root,
          signature_policy: &signature_policy,
        },
        &[unrelated_endpoint],
        Some(&mapping),
        TargetFramework::parse("net10.0").unwrap(),
      ))
      .unwrap();

    let MetadataTaskResult::Versions { versions, source_work } = result else {
      panic!("cached ranged identity should return its version batch");
    };
    assert_eq!(versions.iter().map(|version| version.normalized.as_str()).collect::<Vec<_>>(), ["1.2.3"]);
    assert!(source_work.is_empty());
  }

  #[test]
  fn unmapped_identity_can_read_an_exact_cached_package() {
    let temp = TempDirectory::new();
    let cache_root = temp.0.join("packages");
    let temp_root = temp.0.join("temp");
    temp.write(
      "packages/unmapped.package/1.2.3/Unmapped.Package.nuspec",
      r#"<package><metadata><id>Unmapped.Package</id><version>1.2.3</version></metadata></package>"#,
    );
    let sources = vec![(
      "selected".to_owned(),
      PackageSource {
        url: DEFAULT_SOURCE.to_owned(),
        protocol: NugetProtocol::V3,
        security_flags: 0,
      },
    )];
    let mapping = PackageSourceMapping::compile(
      MergedSourceMapping {
        sources: vec![MergedSourceMappingEntry {
          source: "selected".to_owned(),
          patterns: ItemRange { start: 0, len: 1 },
        }],
        patterns: vec!["Mapped.*".to_owned()],
      },
      &sources,
    )
    .unwrap();
    let request = PackageRequest {
      id: "Unmapped.Package".to_owned(),
      lower_id: "unmapped.package".to_owned(),
      version: "1.2.3".to_owned(),
      direct: true,
      central_transitive: false,
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let signature_policy = Arc::new(SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new()));

    let result = runtime
      .block_on(load_node_metadata(
        &reqwest::Client::new(),
        Some(&request),
        &request.lower_id,
        PackageStorage {
          cache_root: &cache_root,
          fallback_roots: &[],
          temp_root: &temp_root,
          signature_policy: &signature_policy,
        },
        &[],
        Some(&mapping),
        TargetFramework::parse("net10.0").unwrap(),
      ))
      .unwrap();

    let MetadataTaskResult::Requirements { metadata, source_work, .. } = result else {
      panic!("exact cached identity should return its dependency batch");
    };
    assert!(metadata.dependencies.is_empty());
    assert_eq!(source_work, None);
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
        security_flags: 0,
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

    let discovered = discover_configuration(
      &temp.0,
      Some(&temp.0.join("packages")),
      Some(&config),
      &overrides,
      CredentialProviderOptions::default(),
    )
    .unwrap();

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
  fn http_policy_matches_nuget_environment_and_bounds_untrusted_values() {
    let merged = NugetConfigMerge {
      max_http_requests_per_source: Some("7".into()),
      ..NugetConfigMerge::default()
    };
    let policy = effective_http_policy_with(&merged, None, |name| {
      Some(
        match name {
          "NUGET_ENHANCED_MAX_NETWORK_TRY_COUNT" => "9",
          "NUGET_ENHANCED_NETWORK_RETRY_DELAY_MILLISECONDS" => "250",
          "NUGET_MAX_RETRY_AFTER_DELAY_SECONDS" => "12",
          "NUGET_RETRY_HTTP_429" => "false",
          "NUGET_OBSERVE_RETRY_AFTER" => "false",
          _ => return None,
        }
        .into(),
      )
    });

    assert_eq!(policy.max_tries(), 9);
    assert_eq!(policy.retry_delay_ms(), 250);
    assert_eq!(policy.max_retry_after_seconds(), 12);
    assert_eq!(policy.max_requests_per_source(), 7);
    assert!(!policy.retries_http_429());
    assert!(!policy.observes_retry_after());

    let bounded = effective_http_policy_with(&NugetConfigMerge::default(), None, |name| {
      (name == "NUGET_ENHANCED_MAX_NETWORK_TRY_COUNT").then(|| "255".into())
    });
    assert_eq!(bounded.max_tries(), DEFAULT_HTTP_POLICY.max_tries());
  }

  #[test]
  fn global_request_budget_matches_nuget_environment_and_stays_bounded() {
    let selected = effective_request_budget_with(|name| (name == "NUGET_CONCURRENCY_LIMIT").then(|| "4".into()));
    let padded = effective_request_budget_with(|name| (name == "NUGET_CONCURRENCY_LIMIT").then(|| "  +4  ".into()));
    let oversized = effective_request_budget_with(|name| (name == "NUGET_CONCURRENCY_LIMIT").then(|| "1000".into()));

    assert_eq!(selected.global_requests, 4);
    assert_eq!(padded, selected);
    assert_eq!(oversized, DEFAULT_REQUEST_BUDGET);
    for invalid in ["0", "-1", "invalid", ""] {
      let budget = effective_request_budget_with(|name| (name == "NUGET_CONCURRENCY_LIMIT").then(|| invalid.into()));
      assert_eq!(budget, DEFAULT_REQUEST_BUDGET);
    }
    assert_eq!(effective_request_budget_with(|_| None), DEFAULT_REQUEST_BUDGET);
  }

  #[test]
  fn proxy_url_credentials_are_zeroized_and_never_retained_in_the_url() {
    let proxy = proxy_settings(
      "http://us%65r:s%65cret@proxy.example.test:8080".into(),
      Some("localhost,.example.test".into()),
      None,
    )
    .unwrap()
    .unwrap();

    assert_eq!(proxy.url, "http://proxy.example.test:8080/");
    assert_eq!(proxy.username.as_deref().map(String::as_str), Some("user"));
    assert_eq!(proxy.password.as_deref().map(String::as_str), Some("secret"));
    assert!(!proxy.url.contains("secret"));
    let policy = effective_http_policy_with(&NugetConfigMerge::default(), Some(&proxy), |_| None);
    assert!(policy.proxy_configured());
    assert!(policy.proxy_authenticated());
    assert!(policy.no_proxy_configured());
  }

  #[test]
  fn uppercase_proxy_environment_is_used_when_lowercase_is_absent() {
    let proxy = effective_proxy_with(&NugetConfigMerge::default(), |lower, upper| {
      Some(
        match (lower, upper) {
          ("http_proxy", "HTTP_PROXY") => "http://proxy.example.test:8080",
          ("no_proxy", "NO_PROXY") => "localhost,.example.test",
          _ => return None,
        }
        .into(),
      )
    })
    .unwrap()
    .unwrap();

    assert_eq!(proxy.url, "http://proxy.example.test:8080/");
    assert_eq!(proxy.no_proxy.as_deref(), Some("localhost,.example.test"));
  }

  #[test]
  #[cfg(windows)]
  fn windows_config_proxy_credentials_use_nuget_dpapi() {
    let encrypted = windows_dpapi::encrypt_data(b"proxy-secret", windows_dpapi::Scope::User, Some(b"NuGet")).unwrap();
    let merged = NugetConfigMerge {
      proxy_url: Some("http://proxy.example.test:8080".into()),
      proxy_user: Some("proxy-user".into()),
      proxy_password: Some(BASE64.encode(encrypted)),
      ..NugetConfigMerge::default()
    };

    let proxy = effective_proxy_with(&merged, |_, _| None).unwrap().unwrap();

    assert_eq!(proxy.username.as_deref().map(String::as_str), Some("proxy-user"));
    assert_eq!(proxy.password.as_deref().map(String::as_str), Some("proxy-secret"));
    assert!(!proxy.url.contains("proxy-user"));
    assert!(!proxy.url.contains("proxy-secret"));
  }

  #[test]
  fn retryable_http_status_is_retried_before_the_response_is_exposed() {
    let (url, worker) = response_server(vec![
      "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
      "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into(),
    ]);
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let policy = PackageHttpPolicy {
      max_tries: 2,
      retry_delay_ms: 0,
      download_timeout_seconds: 1,
      ..DEFAULT_HTTP_POLICY
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let mut response = runtime
      .block_on(send_with_policy(&client, None, &url, "test request", policy, None, None))
      .unwrap();
    let body = runtime.block_on(response.chunk(&url, "test")).unwrap().unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(body.as_ref(), b"ok");
    let work = response.work(body.len() as u64);
    assert_eq!(work.requests, 2);
    assert_eq!(work.downloaded_bytes, 2);
    assert!(work.duration_us > 0);
    worker.join().unwrap();
  }

  #[test]
  fn successful_source_fallback_retains_failed_request_work() {
    let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_owned();
    let (url, worker) = response_server(vec![response]);
    let service_index = serde_json::json!({
      "version": "3.0.0",
      "resources": [{ "@id": url, "@type": "PackageBaseAddress/3.0.0" }]
    });
    let services = parse_v3_service_index("http://feed.test/index.json", &service_index, SOURCE_ALLOW_INSECURE_CONNECTIONS).unwrap();
    let temp = TempDirectory::new();
    let local_root = temp.0.join("feed");
    write_test_package(&temp, "feed/Sample.Package.1.2.3.nupkg", "Sample.Package", "1.2.3");
    fs::create_dir_all(temp.0.join("packages")).unwrap();
    fs::create_dir_all(temp.0.join("scratch")).unwrap();
    let endpoints = [
      ServiceEndpoint::V3 {
        source: "http://feed.test/index.json".to_owned(),
        services: Arc::new(services),
        credential: None,
        source_index: 0,
      },
      ServiceEndpoint::Local {
        source: local_root.display().to_string(),
        layout: detect_local_feed_layout(&local_root).unwrap(),
        root: local_root,
        source_index: 1,
      },
    ];
    let client = reqwest::Client::builder().build().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let signature_policy = Arc::new(SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new()));

    let package = runtime
      .block_on(ensure_package(
        &client,
        &request(),
        PackageStorage {
          cache_root: &temp.0.join("packages"),
          fallback_roots: &[],
          temp_root: &temp.0.join("scratch"),
          signature_policy: &signature_policy,
        },
        &endpoints,
        None,
        TargetFramework::parse("net10.0").unwrap(),
        false,
      ))
      .unwrap();

    assert_eq!(package.failed_source_work.len(), 1);
    assert_eq!(package.failed_source_work[0].source_index, 0);
    assert_eq!(package.failed_source_work[0].requests, 1);
    assert_eq!(package.failed_source_work[0].downloaded_bytes, 0);
    assert!(package.failed_source_work[0].duration_us > 0);
    assert_eq!(package.source_work.unwrap().source_index, 1);
    worker.join().unwrap();
  }

  #[test]
  fn retry_429_switch_matches_nuget_for_request_timeout_and_rate_limit() {
    let disabled = PackageHttpPolicy {
      flags: DEFAULT_HTTP_POLICY.flags & !HTTP_RETRY_429,
      ..DEFAULT_HTTP_POLICY
    };

    assert!(retryable_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR, disabled));
    assert!(!retryable_status(reqwest::StatusCode::REQUEST_TIMEOUT, disabled));
    assert!(!retryable_status(reqwest::StatusCode::TOO_MANY_REQUESTS, disabled));
    assert!(retryable_status(reqwest::StatusCode::REQUEST_TIMEOUT, DEFAULT_HTTP_POLICY));
    assert!(retryable_status(reqwest::StatusCode::TOO_MANY_REQUESTS, DEFAULT_HTTP_POLICY));
  }

  #[test]
  fn secure_redirect_policy_rejects_an_http_destination() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let destination = format!("http://{address}/downgraded");
    let response = format!("HTTP/1.1 302 Found\r\nLocation: {destination}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let worker = thread::spawn(move || {
      let (mut stream, _) = listener.accept().unwrap();
      let mut request = [0u8; 1024];
      let _ = stream.read(&mut request).unwrap();
      stream.write_all(response.as_bytes()).unwrap();
    });
    let client = reqwest::Client::builder().redirect(source_redirect_policy(false)).build().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let error = runtime.block_on(client.get(format!("http://{address}/start")).send()).unwrap_err();

    assert!(error.is_redirect());
    worker.join().unwrap();
  }

  #[test]
  fn opted_in_redirect_policy_follows_http_without_relaxing_other_schemes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
      for response in [
        format!("HTTP/1.1 302 Found\r\nLocation: http://{address}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_owned(),
      ] {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).unwrap();
        stream.write_all(response.as_bytes()).unwrap();
      }
    });
    let client = reqwest::Client::builder().redirect(source_redirect_policy(true)).build().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let response = runtime.block_on(client.get(format!("http://{address}/start")).send()).unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    worker.join().unwrap();
  }

  #[test]
  fn opted_in_source_allocates_one_source_scoped_transport_client() {
    let sources = vec![(
      "insecure".into(),
      PackageSource {
        url: "http://packages.example.test/v3/index.json".into(),
        protocol: NugetProtocol::V3,
        security_flags: SOURCE_ALLOW_INSECURE_CONNECTIONS | SOURCE_DISABLE_TLS_VALIDATION,
      },
    )];
    let mut credentials = SourceCredentialBatch::default();
    let policy = DEFAULT_HTTP_POLICY.with_source_security(&sources);

    attach_http_policy(&sources, policy, DEFAULT_REQUEST_BUDGET, None, Path::new("NuGet.Config"), &mut credentials).unwrap();

    let source = credentials.get(0).unwrap();
    assert!(source.transport_client.is_some());
    assert_eq!(source.security_flags, SOURCE_ALLOW_INSECURE_CONNECTIONS | SOURCE_DISABLE_TLS_VALIDATION);
    assert!(!policy.tls_validation());
    assert!(policy.allows_insecure_connections());
  }

  #[test]
  fn default_request_budget_allocates_no_limiter_context() {
    let sources = vec![(
      "public".into(),
      PackageSource {
        url: DEFAULT_SOURCE.into(),
        protocol: NugetProtocol::V3,
        security_flags: 0,
      },
    )];
    let mut credentials = SourceCredentialBatch::default();

    attach_http_policy(
      &sources,
      DEFAULT_HTTP_POLICY,
      DEFAULT_REQUEST_BUDGET,
      None,
      Path::new("NuGet.Config"),
      &mut credentials,
    )
    .unwrap();

    assert!(credentials.entries.is_empty());
  }

  #[test]
  fn source_rate_limit_is_bounded_and_shared_by_all_source_requests() {
    let sources = vec![
      (
        "first".into(),
        PackageSource {
          url: "https://first.example.test/v3/index.json".into(),
          protocol: NugetProtocol::V3,
          security_flags: 0,
        },
      ),
      (
        "second".into(),
        PackageSource {
          url: "https://second.example.test/v3/index.json".into(),
          protocol: NugetProtocol::V3,
          security_flags: 0,
        },
      ),
    ];
    let mut credentials = SourceCredentialBatch::default();
    let policy = PackageHttpPolicy {
      max_requests_per_source: 2,
      ..DEFAULT_HTTP_POLICY
    };

    attach_http_policy(
      &sources,
      policy,
      PackageRequestBudget { global_requests: 4 },
      None,
      Path::new("NuGet.Config"),
      &mut credentials,
    )
    .unwrap();

    let first = credentials.get(0).unwrap();
    let second = credentials.get(1).unwrap();
    assert_eq!(first.global_limiter.as_ref().unwrap().available_permits(), 4);
    assert_eq!(second.global_limiter.as_ref().unwrap().available_permits(), 4);
    assert!(Arc::ptr_eq(first.global_limiter.as_ref().unwrap(), second.global_limiter.as_ref().unwrap()));
    assert_eq!(first.source_limiter.as_ref().unwrap().available_permits(), 2);
    assert_eq!(second.source_limiter.as_ref().unwrap().available_permits(), 2);
    assert!(!Arc::ptr_eq(first.source_limiter.as_ref().unwrap(), second.source_limiter.as_ref().unwrap()));
    assert_eq!(first.http_policy, policy);
    assert_eq!(second.http_policy, policy);
  }

  #[test]
  fn response_body_stall_uses_the_download_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
      let (mut stream, _) = listener.accept().unwrap();
      let mut request = [0u8; 1024];
      let _ = stream.read(&mut request).unwrap();
      stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n").unwrap();
      stream.flush().unwrap();
      thread::sleep(Duration::from_millis(1_250));
      let _ = stream.write_all(b"ok");
    });
    let url = format!("http://{address}/slow");
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let policy = PackageHttpPolicy {
      max_tries: 1,
      download_timeout_seconds: 1,
      ..DEFAULT_HTTP_POLICY
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut response = runtime
      .block_on(send_with_policy(&client, None, &url, "test request", policy, None, None))
      .unwrap();

    let error = runtime.block_on(response.chunk(&url, "test")).unwrap_err();

    assert!(error.to_string().contains("stalled for 1 seconds"));
    worker.join().unwrap();
  }

  #[test]
  fn required_signatures_need_at_least_one_trusted_signer() {
    let error = SignaturePolicy::new(SignatureValidationMode::Require, Vec::new()).validate().unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::Configuration);
    assert!(error.to_string().contains("trustedSigners"));
    SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new()).validate().unwrap();
  }

  #[test]
  fn signature_accept_allows_unsigned_packages_but_require_rejects_them() {
    let temp = TempDirectory::new();
    let package = write_test_package(&temp, "unsigned.nupkg", "Sample.Package", "1.2.3");
    let accept = SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new());
    assert!(!package_signature::verify_package(&package, &accept).unwrap());

    let certificate = TrustedCertificate::parse(&"00".repeat(32), FingerprintAlgorithm::Sha256, true).unwrap();
    let require = SignaturePolicy::new(
      SignatureValidationMode::Require,
      vec![TrustedSigner {
        name: "test".to_owned(),
        service_index: None,
        owners: Box::new([]),
        certificates: vec![certificate].into_boxed_slice(),
        kind: TrustedSignerKind::Author,
      }],
    );
    let error = package_signature::verify_package(&package, &require).unwrap_err();
    assert!(error.to_string().contains("package is unsigned"));
  }

  #[test]
  fn signature_accept_rejects_tampered_signed_packages() {
    let temp = TempDirectory::new();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/author-signed.nupkg");
    let mut bytes = fs::read(fixture).unwrap();
    bytes[100] ^= 1;
    let package = temp.write("tampered.nupkg", bytes);
    let accept = SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new());

    let error = package_signature::verify_package(&package, &accept).unwrap_err();
    assert!(error.to_string().contains("content hash does not match"));
  }

  #[test]
  fn trusted_signers_merge_as_typed_author_and_repository_policy() {
    let temp = TempDirectory::new();
    let fingerprint = "ab".repeat(32);
    let lower = temp.write(
      "lower.config",
      format!(
        r#"<configuration><trustedSigners>
<author name="Contoso"><certificate fingerprint="{fingerprint}" hashAlgorithm="SHA256" allowUntrustedRoot="false" /></author>
<repository name="Feed" serviceIndex="https://feed.example/v3/index.json"><owners>Alpha;Beta</owners><certificate fingerprint="{fingerprint}" hashAlgorithm="SHA256" allowUntrustedRoot="true" /></repository>
</trustedSigners></configuration>"#
      ),
    );
    let higher = temp.write(
      "higher.config",
      format!(
        r#"<configuration><trustedSigners><author name="CONTOSO"><certificate fingerprint="{fingerprint}" hashAlgorithm="SHA256" allowUntrustedRoot="true" /></author></trustedSigners></configuration>"#
      ),
    );
    let mut merged = NugetConfigMerge::default();

    merge_config(&lower, &mut merged).unwrap();
    merge_config(&higher, &mut merged).unwrap();

    assert_eq!(merged.trusted_signers.len(), 2);
    let author = &merged.trusted_signers[0];
    assert_eq!(author.name, "CONTOSO");
    assert!(author.certificates[0].allow_untrusted_root);
    let repository = &merged.trusted_signers[1];
    assert_eq!(repository.service_index.as_deref(), Some("https://feed.example/v3/index.json"));
    assert_eq!(repository.owners.as_ref(), ["Alpha", "Beta"]);
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
    let relative = paths
      .iter()
      .filter_map(|path| path.strip_prefix(&temp.0).ok())
      .map(|path| path.to_string_lossy().replace('\\', "/").to_ascii_lowercase())
      .collect::<Vec<_>>();

    assert_eq!(
      relative,
      [
        "machine/20.config",
        "machine/10.config",
        "user/config/20.config",
        "user/config/10.config",
        "user/nuget.config",
        "drive/nuget.config",
        "drive/repository/nuget.config",
        "drive/repository/src/nuget.config",
      ]
    );
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn nuget_config_discovery_preserves_macos_directory_entry_casing() {
    let temp = TempDirectory::new();
    temp.write("NuGet.Config", "<configuration />");

    let path = config_path_in(&temp.0).unwrap();

    assert_eq!(path.file_name(), Some(std::ffi::OsStr::new("NuGet.Config")));
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
    let no_sources = discover_configuration(
      &temp.0.join("repository"),
      Some(&temp.0.join("packages")),
      Some(&explicit),
      &[],
      CredentialProviderOptions::default(),
    )
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

    let parsed = parse_v2_package_metadata(&request(), "https://packages.example.test/api/v2/Packages(...)", metadata, 0).unwrap();

    assert_eq!(parsed.content_url, "https://packages.example.test/api/v2/package/Sample.Package/1.2.3");
    assert_eq!(parsed.expected_size, Some(42));
    assert_eq!(parsed.work.requests, 0);

    let insecure = String::from_utf8(metadata.to_vec()).unwrap().replace("https://packages", "http://packages");
    let error = parse_v2_package_metadata(&request(), "http://packages.example.test/api/v2/Packages(...)", insecure.as_bytes(), 0)
      .err()
      .unwrap();
    assert!(error.to_string().contains("allowInsecureConnections is true"));
    let parsed = parse_v2_package_metadata(
      &request(),
      "http://packages.example.test/api/v2/Packages(...)",
      insecure.as_bytes(),
      SOURCE_ALLOW_INSECURE_CONNECTIONS,
    )
    .unwrap();
    assert!(parsed.content_url.starts_with("http://"));
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
  fn rejects_embedded_credentials_in_v2_continuations() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let body = format!(r#"<feed><link rel="next" href="http://user:secret@{address}/next" /></feed>"#);
    let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let worker = thread::spawn(move || {
      let (mut stream, _) = listener.accept().unwrap();
      let mut request = [0u8; 1024];
      let _ = stream.read(&mut request).unwrap();
      stream.write_all(response.as_bytes()).unwrap();
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    let error = runtime
      .block_on(enumerate_v2_versions(&client, None, &format!("http://{address}/"), "sample.package"))
      .err()
      .unwrap();

    assert!(error.to_string().contains("must not embed credentials"));
    worker.join().unwrap();
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
    let services = parse_v3_service_index("https://feed.example.test/custom-index", &service_index, 0).unwrap();
    let package_base = services.package_base_address().unwrap();

    let metadata = v3_package_metadata(&request(), package_base);

    assert_eq!(package_base, "https://content.example.test/arbitrary/root");
    assert_eq!(
      metadata.content_url,
      "https://content.example.test/arbitrary/root/sample.package/1.2.3/sample.package.1.2.3.nupkg"
    );
    assert_eq!(metadata.expected_hash, None);
    assert_eq!(metadata.expected_size, None);
    assert_eq!(metadata.work.requests, 0);
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

    let services = parse_v3_service_index("https://feed.test/index.json", &document, 0).unwrap();

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
    let error = parse_v3_service_index("https://feed.test/index.json", &schema, 0).err().unwrap();
    assert_eq!(error.kind(), PackageErrorKind::Network);
    assert!(error.to_string().contains("expected major version 3"));

    let insecure = serde_json::json!({
      "version": "3.0.0",
      "resources": [{ "@id": "http://feed.test/content/", "@type": "PackageBaseAddress/3.0.0" }]
    });
    let error = parse_v3_service_index("https://feed.test/index.json", &insecure, 0).err().unwrap();
    assert_eq!(error.kind(), PackageErrorKind::Network);
    assert!(error.to_string().contains("allowInsecureConnections=true"));

    let services = parse_v3_service_index("http://feed.test/index.json", &insecure, SOURCE_ALLOW_INSECURE_CONNECTIONS).unwrap();
    assert_eq!(services.package_base_address(), Some("http://feed.test/content/"));
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
    let signature_policy = SignaturePolicy::new(SignatureValidationMode::Accept, Vec::new());

    validate_locked_package(&temp.0, &request(), &hash, &signature_policy).unwrap();

    let error = validate_locked_package(&temp.0, &request(), "not-base64", &signature_policy).unwrap_err();
    assert_eq!(error.kind(), PackageErrorKind::Integrity);
    fs::remove_file(temp.0.join(".dv.metadata.json")).unwrap();
    let error = validate_locked_package(&temp.0, &request(), &hash, &signature_policy).unwrap_err();
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
  fn nuspec_framework_reference_groups_are_isolated_from_dependency_groups() {
    let manifest = br#"<package><metadata><id>Sample.Package</id><version>1.2.3</version>
<dependencies>
  <group targetFramework="netstandard2.0"><dependency id="Base.Dependency" version="1.0" /></group>
  <group targetFramework="net10.0"><dependency id="Current.Dependency" version="[2.0]" /></group>
</dependencies>
<frameworkReferences>
  <group targetFramework="net8.0"><frameworkReference name="Ignored.Framework" /></group>
  <group targetFramework="net10.0"><frameworkReference name="Microsoft.AspNetCore.App" /><frameworkReference name="microsoft.aspnetcore.app" /></group>
</frameworkReferences>
<frameworkAssemblies><frameworkAssembly assemblyName="System.Xml" targetFramework="net10.0" /></frameworkAssemblies>
</metadata></package>"#;

    let metadata = parse_nuspec_metadata(
      Path::new("sample.package.nuspec"),
      manifest,
      &request(),
      TargetFramework::parse("net10.0").unwrap(),
    )
    .unwrap();

    assert_eq!(metadata.dependencies.len(), 1);
    assert_eq!(metadata.dependencies[0].id, "Current.Dependency");
    assert_eq!(metadata.cold.frameworks.references, ["Microsoft.AspNetCore.App"]);
    assert!(metadata.cold.frameworks.assemblies.is_empty());
  }

  #[test]
  fn nuspec_framework_assemblies_use_the_nearest_net_framework_group() {
    let manifest = br#"<package><metadata><id>Sample.Package</id><version>1.2.3</version>
<frameworkAssemblies>
  <frameworkAssembly assemblyName="System.Net" />
  <frameworkAssembly assemblyName="System.Xml" targetFramework="net45, .NETFramework4.8" />
  <frameworkAssembly assemblyName="System.Data" targetFramework="net48" />
  <frameworkAssembly assemblyName="System.Old" targetFramework="net472" />
</frameworkAssemblies>
</metadata></package>"#;

    let net48 = parse_nuspec_metadata(
      Path::new("sample.package.nuspec"),
      manifest,
      &request(),
      TargetFramework::parse("net48").unwrap(),
    )
    .unwrap();
    let net472 = parse_nuspec_metadata(
      Path::new("sample.package.nuspec"),
      manifest,
      &request(),
      TargetFramework::parse("net472").unwrap(),
    )
    .unwrap();

    assert_eq!(net48.cold.frameworks.assemblies, ["System.Data", "System.Xml"]);
    assert_eq!(net472.cold.frameworks.assemblies, ["System.Old"]);
    assert!(net48.cold.frameworks.references.is_empty());
  }

  #[test]
  fn excluding_runtime_keeps_shared_frameworks_but_removes_legacy_assemblies() {
    let temp = TempDirectory::new();
    let cached = CachedPackage {
      root: temp.0.clone(),
      hash: BASE64.encode([0u8; 64]),
      metadata: None,
      cache_hit: true,
      source_work: None,
      failed_source_work: Box::new([]),
      origin: None,
    };

    let package = parse_cached_package(
      request(),
      cached,
      PackageAssetContext {
        target: TargetFramework::parse("net10.0").unwrap(),
        target_text: "net10.0",
        runtime_identifier: None,
        runtime_graph: None,
        flags: AssetFlags::COMPILE,
      },
      Vec::new(),
      PackageColdMetadata {
        frameworks: PackageFrameworkMetadata {
          references: vec!["Microsoft.AspNetCore.App".into()],
          assemblies: vec!["System.Xml".into()],
        },
        content_rules: ContentFileRules::default(),
      },
    )
    .unwrap();

    assert_eq!(package.framework_references, ["Microsoft.AspNetCore.App"]);
    assert!(package.framework_assemblies.is_empty());
  }

  #[test]
  fn nuspec_sections_select_framework_metadata_without_crossing_group_boundaries() {
    let manifest = br#"<package><metadata><id>Sample.Package</id><version>1.2.3</version>
<dependencies>
  <group targetFramework=".NETFramework4.7.2"><dependency id="Legacy.Dependency" version="[1.0]" /></group>
  <group targetFramework="net10.0"><dependency id="Current.Dependency" version="[2.0]" /></group>
</dependencies>
<frameworkReferences>
  <group targetFramework="net8.0"><frameworkReference name="Microsoft.WindowsDesktop.App" /></group>
  <group targetFramework="net10.0"><frameworkReference name="Microsoft.AspNetCore.App" /></group>
</frameworkReferences>
<frameworkAssemblies>
  <frameworkAssembly assemblyName="System.Net.Http" targetFramework=".NETFramework4.7.2" />
  <frameworkAssembly assemblyName="System.Runtime" />
</frameworkAssemblies>
</metadata></package>"#;
    let path = Path::new("sample.package.nuspec");

    let modern = parse_nuspec_metadata(path, manifest, &request(), TargetFramework::parse("net10.0").unwrap()).unwrap();
    let legacy = parse_nuspec_metadata(path, manifest, &request(), TargetFramework::parse("net472").unwrap()).unwrap();

    assert_eq!(
      modern.dependencies.iter().map(|dependency| dependency.id.as_str()).collect::<Vec<_>>(),
      ["Current.Dependency"]
    );
    assert_eq!(modern.cold.frameworks.references, ["Microsoft.AspNetCore.App"]);
    assert!(modern.cold.frameworks.assemblies.is_empty());
    assert_eq!(
      legacy.dependencies.iter().map(|dependency| dependency.id.as_str()).collect::<Vec<_>>(),
      ["Legacy.Dependency"]
    );
    assert!(legacy.cold.frameworks.references.is_empty());
    assert_eq!(legacy.cold.frameworks.assemblies, ["System.Net.Http"]);
  }

  #[test]
  fn nuspec_framework_reference_requires_a_group() {
    let manifest = br#"<package><metadata><id>Sample.Package</id><version>1.2.3</version>
<frameworkReferences><frameworkReference name="Microsoft.AspNetCore.App" /></frameworkReferences>
</metadata></package>"#;

    let error = parse_nuspec_metadata(
      Path::new("sample.package.nuspec"),
      manifest,
      &request(),
      TargetFramework::parse("net10.0").unwrap(),
    )
    .unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::Integrity);
    assert!(error.to_string().contains("must be inside"));
  }

  #[test]
  fn package_framework_metadata_survives_the_warm_lock() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Framework.Metadata" Version="1.0.0" /></ItemGroup></Project>"#,
    );
    let manifest = r#"<package><metadata><id>Framework.Metadata</id><version>1.0.0</version>
<frameworkReferences><group targetFramework="net10.0"><frameworkReference name="Microsoft.AspNetCore.App" /></group></frameworkReferences>
<frameworkAssemblies><frameworkAssembly assemblyName="System.Runtime" /></frameworkAssemblies>
</metadata></package>"#;
    write_test_package_manifest(&temp, "feed/Framework.Metadata.1.0.0.nupkg", "Framework.Metadata", manifest);
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      sources: vec![temp.0.join("feed").to_string_lossy().into_owned()],
      offline: true,
      write_lock: true,
      ..PackageResolveOptions::default()
    };

    let cold = resolve_package_inputs(&[&project], &options).unwrap().remove(0);
    let warm = resolve_package_inputs(&[&project], &options).unwrap().remove(0);

    for resolution in [&cold, &warm] {
      let framework = resolution.package_frameworks().next().unwrap();
      assert_eq!(resolution.package_framework_package(framework), 0);
      assert_eq!(
        resolution.package_framework_references(framework).collect::<Vec<_>>(),
        ["Microsoft.AspNetCore.App"]
      );
      assert!(resolution.package_framework_assemblies(framework).next().is_none());
    }
    assert_eq!(cold.downloaded_packages(), 1);
    assert_eq!(warm.cache_hits(), 1);
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
      metadata: None,
      cache_hit: true,
      source_work: None,
      failed_source_work: Box::new([]),
      origin: None,
    };

    let package = parse_cached_package(
      request(),
      cached,
      PackageAssetContext {
        target: TargetFramework::parse("net10.0").unwrap(),
        target_text: "net10.0",
        runtime_identifier: None,
        runtime_graph: None,
        flags: AssetFlags::ALL,
      },
      Vec::new(),
      PackageColdMetadata::default(),
    )
    .unwrap();

    assert_eq!(package.compile_assets, [temp.0.join("ref/net10.0/Sample.Package.dll")]);
    assert_eq!(package.runtime_assets, [temp.0.join("lib/net10.0/Sample.Package.dll")]);
    assert_eq!(package.resource_assets, [temp.0.join("lib/net10.0/de/Sample.Package.resources.dll")]);
    assert_eq!(
      package.content_files.iter().map(|content| content.path.as_path()).collect::<Vec<_>>(),
      [temp.0.join("contentFiles/any/any/readme.txt")]
    );
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
  fn content_rules_apply_later_matching_attributes_and_preserve_unmatched_files() {
    let temp = TempDirectory::new();
    temp.write("contentFiles/any/any/docs/readme.md", []);
    temp.write("contentFiles/any/any/docs/private.txt", []);
    temp.write("contentFiles/cs/any/generated.cs", []);
    temp.write("contentFiles/vb/any/ignored.vb", []);
    let manifest = br#"<package><metadata><id>Sample.Package</id><version>1.2.3</version><contentFiles>
<files include="any/any/docs/**" exclude="any/any/docs/private.*" buildAction="Content" copyToOutput="true" flatten="true" />
<files include="any/any/**/*.md" buildAction="None" copyToOutput="false" flatten="false" />
<files include="cs/any/**" buildAction="EmbeddedResource" />
</contentFiles></metadata></package>"#;
    let metadata = parse_nuspec_metadata(
      Path::new("sample.package.nuspec"),
      manifest,
      &request(),
      TargetFramework::parse("net10.0").unwrap(),
    )
    .unwrap();
    let cached = CachedPackage {
      root: temp.0.clone(),
      hash: BASE64.encode([0u8; 64]),
      metadata: None,
      cache_hit: true,
      source_work: None,
      failed_source_work: Box::new([]),
      origin: None,
    };

    let package = parse_cached_package(
      request(),
      cached,
      PackageAssetContext {
        target: TargetFramework::parse("net10.0").unwrap(),
        target_text: "net10.0",
        runtime_identifier: None,
        runtime_graph: None,
        flags: AssetFlags::CONTENT_FILES,
      },
      Vec::new(),
      metadata.cold,
    )
    .unwrap();
    let selected = package
      .content_files
      .iter()
      .map(|content| {
        let action = if content.build_action == NO_CONTENT_BUILD_ACTION {
          DEFAULT_CONTENT_BUILD_ACTION
        } else {
          &package.content_actions[content.build_action as usize]
        };
        (
          content.path.strip_prefix(&temp.0).unwrap().to_string_lossy().replace('\\', "/"),
          action,
          content.copy_to_output,
          content.flatten,
        )
      })
      .collect::<Vec<_>>();

    assert_eq!(
      selected,
      [
        ("contentFiles/any/any/docs/private.txt".into(), "Compile", false, false),
        ("contentFiles/any/any/docs/readme.md".into(), "None", false, false),
        ("contentFiles/cs/any/generated.cs".into(), "EmbeddedResource", false, false),
        ("contentFiles/vb/any/ignored.vb".into(), "Compile", false, false),
      ]
    );
  }

  #[test]
  fn concrete_runtime_selects_each_asset_family_from_the_nearest_compatible_rid() {
    let temp = TempDirectory::new();
    temp.write("lib/net10.0/Portable.dll", []);
    temp.write("runtimes/linux-x64/lib/net10.0/Linux.dll", []);
    temp.write("runtimes/linux-x64/lib/net10.0/de/Linux.resources.dll", []);
    temp.write("runtimes/unix/native/libnative.so", []);
    temp.write("runtimes/win-x64/lib/net10.0/Windows.dll", []);
    temp.write("runtimes/win-x64/native/native.dll", []);
    let graph_path = temp.write(
      "PortableRuntimeIdentifierGraph.json",
      r##"{"runtimes":{"linux-musl-x64":{"#import":["linux-x64"]},"linux-x64":{"#import":["linux","unix-x64"]},"linux":{"#import":["unix"]},"unix-x64":{"#import":["unix"]},"unix":{},"win-x64":{}}}"##,
    );
    let graph = RuntimeIdentifierGraph::load(&graph_path).unwrap();
    let cached = CachedPackage {
      root: temp.0.clone(),
      hash: BASE64.encode([0u8; 64]),
      metadata: None,
      cache_hit: true,
      source_work: None,
      failed_source_work: Box::new([]),
      origin: None,
    };

    let package = parse_cached_package(
      request(),
      cached,
      PackageAssetContext {
        target: TargetFramework::parse("net10.0").unwrap(),
        target_text: "net10.0",
        runtime_identifier: Some("linux-musl-x64"),
        runtime_graph: Some(&graph),
        flags: AssetFlags::ALL,
      },
      Vec::new(),
      PackageColdMetadata::default(),
    )
    .unwrap();

    assert_eq!(package.runtime_assets, [temp.0.join("runtimes/linux-x64/lib/net10.0/Linux.dll")]);
    assert_eq!(package.resource_assets, [temp.0.join("runtimes/linux-x64/lib/net10.0/de/Linux.resources.dll")]);
    assert_eq!(package.native_assets, [temp.0.join("runtimes/unix/native/libnative.so")]);
    assert!(package.runtime_targets.is_empty());
  }

  #[test]
  fn malformed_content_metadata_fails_before_asset_materialization() {
    let manifest = br#"<package><metadata><id>Sample.Package</id><version>1.2.3</version><contentFiles>
<files include="../outside/**" buildAction="Content" />
</contentFiles></metadata></package>"#;

    let error = parse_nuspec_metadata(
      Path::new("sample.package.nuspec"),
      manifest,
      &request(),
      TargetFramework::parse("net10.0").unwrap(),
    )
    .unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::Integrity);
    assert!(error.to_string().contains("unsupported or unsafe"));
  }

  #[test]
  fn selected_content_metadata_rejects_unknown_actions_and_non_boolean_flags() {
    let temp = TempDirectory::new();
    temp.write("contentFiles/any/any/readme.txt", []);
    let cached_package = || CachedPackage {
      root: temp.0.clone(),
      hash: BASE64.encode([0u8; 64]),
      metadata: None,
      cache_hit: true,
      source_work: None,
      failed_source_work: Box::new([]),
      origin: None,
    };
    let context = PackageAssetContext {
      target: TargetFramework::parse("net10.0").unwrap(),
      target_text: "net10.0",
      runtime_identifier: None,
      runtime_graph: None,
      flags: AssetFlags::CONTENT_FILES,
    };

    for attribute in [r#"buildAction="CustomAsset""#, r#"copyToOutput="yes""#] {
      let manifest = format!(
        r#"<package><metadata><id>Sample.Package</id><version>1.2.3</version><contentFiles>
<files include="any/any/**" {attribute} />
</contentFiles></metadata></package>"#
      );
      let parsed = parse_nuspec_metadata(
        Path::new("sample.package.nuspec"),
        manifest.as_bytes(),
        &request(),
        TargetFramework::parse("net10.0").unwrap(),
      );
      let error = match parsed {
        Ok(metadata) => match parse_cached_package(request(), cached_package(), context, Vec::new(), metadata.cold) {
          Ok(_) => panic!("invalid selected content metadata was accepted: {attribute}"),
          Err(error) => error,
        },
        Err(error) => error,
      };
      assert_eq!(error.kind(), PackageErrorKind::Integrity);
    }
  }

  #[test]
  fn direct_package_policy_filters_assets_and_survives_the_warm_lock() {
    let temp = TempDirectory::new();
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup><ItemGroup><PackageReference Include="Sample.Package" Version="1.2.3" IncludeAssets="compile;runtime" ExcludeAssets="runtime" PrivateAssets="all" NoWarn="NU1603;NU1701" Aliases="SampleAlias" GeneratePathProperty="true" /></ItemGroup></Project>"#,
    );
    temp.write(
      "packages/sample.package/1.2.3/sample.package.nuspec",
      r#"<package><metadata><id>Sample.Package</id><version>1.2.3</version></metadata></package>"#,
    );
    temp.write("packages/sample.package/1.2.3/ref/net10.0/Sample.Package.dll", []);
    temp.write("packages/sample.package/1.2.3/lib/net10.0/Sample.Package.dll", []);
    temp.write("packages/sample.package/1.2.3/analyzers/dotnet/cs/Sample.Analyzer.dll", []);
    temp.write("packages/sample.package/1.2.3/sample.package.1.2.3.nupkg", []);
    temp.write("packages/sample.package/1.2.3/sample.package.1.2.3.nupkg.sha512", BASE64.encode([0u8; 64]));
    temp.write("packages/sample.package/1.2.3/.dv.metadata.json", "{}");
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      offline: true,
      write_lock: true,
      ..PackageResolveOptions::default()
    };

    let cold = resolve_package_inputs(&[&project], &options).unwrap().remove(0);

    assert_eq!(cold.compile_assets().len(), 1);
    assert_eq!(cold.runtime_assets().len(), 0);
    assert_eq!(cold.analyzers().len(), 0);
    assert_eq!(cold.direct_policies().len(), 1);
    let policy = cold.direct_policies().start;
    assert_eq!(cold.direct_policy_include_assets(policy), AssetFlags::COMPILE);
    assert_eq!(cold.direct_policy_private_assets(policy), AssetFlags::ALL);
    assert_eq!(cold.direct_policy_no_warn(policy), Some("NU1603;NU1701"));
    assert_eq!(cold.direct_policy_aliases(policy), Some("SampleAlias"));
    let (name, root) = cold.direct_policy_path_property(policy).unwrap();
    assert_eq!(name, "PkgSample_Package");
    assert_eq!(root, temp.0.join("packages/sample.package/1.2.3"));

    let warm = resolve_package_inputs(&[&project], &options).unwrap().remove(0);
    assert_eq!(warm.compile_assets().len(), 1);
    assert_eq!(warm.runtime_assets().len(), 0);
    assert_eq!(warm.direct_policy_aliases(0), Some("SampleAlias"));
    assert_eq!(warm.cache_hits(), 1);
    assert!(warm.matches_project(&project));

    temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup><ItemGroup><PackageReference Include="Sample.Package" Version="1.2.3" IncludeAssets="runtime" /></ItemGroup></Project>"#,
    );
    let changed = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    assert!(!warm.matches_project(&changed));
    let changed = resolve_package_inputs(&[&changed], &options).unwrap().remove(0);
    assert_eq!(changed.compile_assets().len(), 0);
    assert_eq!(changed.runtime_assets().len(), 1);
  }

  #[test]
  fn direct_wins_warning_survives_the_warm_lock_without_manifest_io() {
    let temp = TempDirectory::new();
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup><ItemGroup><PackageReference Include="Leaf.Package" Version="[1.0.0]" /><PackageReference Include="Top.Package" Version="[1.0.0]" /></ItemGroup></Project>"#,
    );
    for (id, manifest) in [
      (
        "leaf.package",
        r#"<package><metadata><id>Leaf.Package</id><version>1.0.0</version></metadata></package>"#,
      ),
      (
        "top.package",
        r#"<package><metadata><id>Top.Package</id><version>1.0.0</version><dependencies><group targetFramework="net10.0"><dependency id="Leaf.Package" version="[2.0.0]" /></group></dependencies></metadata></package>"#,
      ),
    ] {
      let root = format!("packages/{id}/1.0.0");
      temp.write(&format!("{root}/{id}.nuspec"), manifest);
      temp.write(&format!("{root}/{id}.1.0.0.nupkg"), []);
      temp.write(&format!("{root}/{id}.1.0.0.nupkg.sha512"), BASE64.encode([0u8; 64]));
      temp.write(&format!("{root}/.dv.metadata.json"), "{}");
      temp.write(&format!("{root}/lib/net10.0/{id}.dll"), []);
    }
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      offline: true,
      write_lock: true,
      ..PackageResolveOptions::default()
    };

    let cold = resolve_package_inputs(&[&project], &options).unwrap().remove(0);
    fs::remove_file(temp.0.join("packages/top.package/1.0.0/top.package.nuspec")).unwrap();
    let warm = resolve_package_inputs(&[&project], &options).unwrap().remove(0);

    for resolution in [&cold, &warm] {
      assert_eq!(resolution.downgrades().len(), 1);
      assert_eq!(resolution.downgrade_package_id(0), "Leaf.Package");
      assert_eq!(resolution.downgrade_selected_version(0), "1.0.0");
      assert_eq!(resolution.downgrade_requested_range(0), "[2.0.0]");
      assert_eq!(resolution.downgrade_requesting_package(0), "Top.Package");
    }
    assert_eq!(warm.cache_hits(), 2);
  }

  #[test]
  fn duplicate_direct_references_fail_before_metadata_can_be_merged_ambiguously() {
    let temp = TempDirectory::new();
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup><ItemGroup><PackageReference Include="Sample.Package" Version="1.2.3" IncludeAssets="compile" /><PackageReference Include="sample.package" Version="1.2.3" IncludeAssets="runtime" /></ItemGroup></Project>"#,
    );
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();

    let error = direct_requests(&project).unwrap_err();

    assert_eq!(error.kind(), PackageErrorKind::Resolution);
    assert!(error.to_string().contains("more than once"));
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
      metadata: None,
      cache_hit: true,
      source_work: None,
      failed_source_work: Box::new([]),
      origin: None,
    };

    let package = parse_cached_package(
      request(),
      cached,
      PackageAssetContext {
        target: TargetFramework::parse("net10.0").unwrap(),
        target_text: "net10.0",
        runtime_identifier: None,
        runtime_graph: None,
        flags: AssetFlags::ALL,
      },
      vec![PackageRequest {
        id: "Base.Dependency".into(),
        lower_id: "base.dependency".into(),
        version: "1.0.0".into(),
        direct: false,
        central_transitive: false,
      }],
      PackageColdMetadata::default(),
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
      ..PackageResolveOptions::default()
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
    assert!(
      resolution
        .packages()
        .iter()
        .copied()
        .all(|package| resolution.package_cache_outcome(package) == CacheOutcome::Hit)
    );
  }

  #[test]
  fn central_transitive_pinning_promotes_the_selected_dependency_and_survives_a_warm_lock() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    temp.write(
      "Directory.Packages.props",
      r#"<Project><PropertyGroup>
<ManagePackageVersionsCentrally>true</ManagePackageVersionsCentrally>
<CentralPackageTransitivePinningEnabled>true</CentralPackageTransitivePinningEnabled>
</PropertyGroup><ItemGroup>
<PackageVersion Include="Meta.Package" Version="1.0.0" />
<PackageVersion Include="Child.Package" Version="3.0.0" />
</ItemGroup></Project>"#,
    );
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Meta.Package" /></ItemGroup></Project>"#,
    );
    for (id, version, nuspec) in [
      (
        "meta.package",
        "1.0.0",
        r#"<package><metadata><id>Meta.Package</id><version>1.0.0</version><dependencies>
<group targetFramework="netstandard2.0"><dependency id="Child.Package" version="[2.0.0,4.0.0)" /></group>
</dependencies></metadata></package>"#,
      ),
      (
        "child.package",
        "3.0.0",
        r#"<package><metadata><id>Child.Package</id><version>3.0.0</version></metadata></package>"#,
      ),
    ] {
      let root = format!("packages/{id}/{version}");
      temp.write(&format!("{root}/{id}.nuspec"), nuspec);
      temp.write(&format!("{root}/{id}.{version}.nupkg"), []);
      temp.write(&format!("{root}/{id}.{version}.nupkg.sha512"), BASE64.encode([0u8; 64]));
      temp.write(&format!("{root}/.dv.metadata.json"), "{}");
    }
    temp.write("packages/child.package/3.0.0/lib/net6.0/Child.Package.dll", []);
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      config_file: None,
      sources: Vec::new(),
      offline: true,
      write_lock: true,
      ..PackageResolveOptions::default()
    };

    for resolution in [
      resolve_package_inputs(&[&project], &options).unwrap().remove(0),
      resolve_package_inputs(&[&project], &options).unwrap().remove(0),
    ] {
      let child = resolution
        .packages()
        .iter()
        .copied()
        .find(|package| resolution.package_id(*package) == "Child.Package")
        .unwrap();
      assert_eq!(resolution.package_version(child), "3.0.0");
      assert!(!resolution.package_is_direct(child));
      assert!(resolution.package_is_central_transitive(child));
    }
  }

  #[test]
  fn project_batch_reuses_parsed_metadata_and_publishes_one_archive() {
    let temp = TempDirectory::new();
    let project_xml = r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Shared.Package" Version="1.0.0" /></ItemGroup></Project>"#;
    temp.write("left/Program.cs", "");
    temp.write("right/Program.cs", "");
    let left_path = temp.write("left/Left.csproj", project_xml);
    let right_path = temp.write("right/Right.csproj", project_xml);
    write_test_package(&temp, "feed/Shared.Package.1.0.0.nupkg", "Shared.Package", "1.0.0");
    let left = evaluate_project_path(&left_path, ProjectConfiguration::Debug).unwrap();
    let right = evaluate_project_path(&right_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      sources: vec![temp.0.join("feed").to_string_lossy().into_owned()],
      offline: true,
      write_lock: false,
      ..PackageResolveOptions::default()
    };

    let resolutions = resolve_package_inputs(&[&left, &right], &options).unwrap();

    assert_eq!(resolutions[0].downloaded_packages(), 1);
    assert_eq!(resolutions[0].shared_metadata_hits(), 0);
    assert_eq!(resolutions[1].cache_hits(), 1);
    assert_eq!(resolutions[1].shared_metadata_hits(), 1);
    assert_eq!(resolutions[0].package_id(resolutions[0].packages()[0]), "Shared.Package");
    assert_eq!(resolutions[1].package_id(resolutions[1].packages()[0]), "Shared.Package");
  }

  #[test]
  fn project_batch_partitions_dependency_metadata_by_target() {
    let temp = TempDirectory::new();
    temp.write("net8/Program.cs", "");
    temp.write("net10/Program.cs", "");
    let net8_path = temp.write(
      "net8/Net8.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net8.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Meta.Package" Version="1.0.0" /></ItemGroup></Project>"#,
    );
    let net10_path = temp.write(
      "net10/Net10.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Meta.Package" Version="1.0.0" /></ItemGroup></Project>"#,
    );
    for (id, nuspec) in [
      (
        "meta.package",
        r#"<package><metadata><id>Meta.Package</id><version>1.0.0</version><dependencies>
<group targetFramework="net8.0"><dependency id="Child.Eight" version="1.0.0" /></group>
<group targetFramework="net10.0"><dependency id="Child.Ten" version="1.0.0" /></group>
</dependencies></metadata></package>"#,
      ),
      (
        "child.eight",
        r#"<package><metadata><id>Child.Eight</id><version>1.0.0</version></metadata></package>"#,
      ),
      (
        "child.ten",
        r#"<package><metadata><id>Child.Ten</id><version>1.0.0</version></metadata></package>"#,
      ),
    ] {
      let root = format!("packages/{id}/1.0.0");
      temp.write(&format!("{root}/{id}.nuspec"), nuspec);
      temp.write(&format!("{root}/{id}.1.0.0.nupkg"), "");
      temp.write(&format!("{root}/{id}.1.0.0.nupkg.sha512"), BASE64.encode([0u8; 64]));
      temp.write(&format!("{root}/.dv.metadata.json"), "{}");
    }
    temp.write("packages/child.eight/1.0.0/lib/net8.0/Child.Eight.dll", "");
    temp.write("packages/child.ten/1.0.0/lib/net10.0/Child.Ten.dll", "");
    let net8 = evaluate_project_path(&net8_path, ProjectConfiguration::Debug).unwrap();
    let net10 = evaluate_project_path(&net10_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      offline: true,
      write_lock: false,
      ..PackageResolveOptions::default()
    };

    let resolutions = resolve_package_inputs(&[&net8, &net10], &options).unwrap();
    let identities = resolutions
      .iter()
      .map(|resolution| {
        resolution
          .packages()
          .iter()
          .copied()
          .map(|package| resolution.package_id(package))
          .collect::<Vec<_>>()
      })
      .collect::<Vec<_>>();

    assert_eq!(identities[0], ["Child.Eight", "Meta.Package"]);
    assert_eq!(identities[1], ["Child.Ten", "Meta.Package"]);
    assert_eq!(resolutions[1].shared_metadata_hits(), 0);
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
      ..PackageResolveOptions::default()
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
      ..PackageResolveOptions::default()
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
      ..PackageResolveOptions::default()
    };

    let resolution = resolve_package_inputs(&[&project], &options).unwrap().remove(0);

    assert_eq!(resolution.packages().len(), 1);
    assert_eq!(resolution.package_version(resolution.packages()[0]), "2.0.0");
    assert_eq!(resolution.network_requests(), 0);
  }

  #[test]
  fn floating_project_versions_select_the_highest_stable_cache_match_cold_and_warm() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    let project_path = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Floating.Package" Version="1.2.*" /></ItemGroup></Project>"#,
    );
    for version in ["1.2.0-beta.1", "1.2.0", "1.2.1-rc.1", "1.2.1", "1.3.0"] {
      let root = format!("packages/floating.package/{version}");
      temp.write(
        &format!("{root}/floating.package.nuspec"),
        format!(r#"<package><metadata><id>Floating.Package</id><version>{version}</version></metadata></package>"#),
      );
      temp.write(&format!("{root}/floating.package.{version}.nupkg"), []);
      temp.write(&format!("{root}/floating.package.{version}.nupkg.sha512"), BASE64.encode([0u8; 64]));
      temp.write(&format!("{root}/.dv.metadata.json"), "{}");
      temp.write(&format!("{root}/lib/net10.0/Floating.Package.dll"), []);
    }
    let project = evaluate_project_path(&project_path, ProjectConfiguration::Debug).unwrap();
    let options = PackageResolveOptions {
      packages_directory: Some(temp.0.join("packages")),
      config_file: None,
      sources: Vec::new(),
      offline: true,
      write_lock: true,
      ..PackageResolveOptions::default()
    };

    let cold = resolve_package_inputs(&[&project], &options).unwrap().remove(0);
    let warm = resolve_package_inputs(&[&project], &options).unwrap().remove(0);

    assert_eq!(cold.package_version(cold.packages()[0]), "1.2.1");
    assert_eq!(warm.package_version(warm.packages()[0]), "1.2.1");
    assert_eq!(cold.network_requests(), 0);
    assert_eq!(warm.network_requests(), 0);
    assert_eq!(warm.cache_hits(), 1);
  }

  #[test]
  fn floating_versions_in_package_metadata_remain_typed_for_transitive_resolution() {
    let request = request();
    let manifest = br#"<package><metadata><id>Sample.Package</id><version>1.2.3</version><dependencies>
<dependency id="Child.Package" version="1.*" /></dependencies></metadata></package>"#;

    let requirements = parse_nuspec_requirements(
      Path::new("sample.package.nuspec"),
      manifest,
      &request,
      TargetFramework::parse("net10.0").unwrap(),
    )
    .unwrap();

    assert_eq!(requirements.len(), 1);
    assert!(requirements[0].range.is_floating());
  }

  #[test]
  fn warm_cache_and_lock_select_assets_for_the_evaluated_target_without_http() {
    let temp = TempDirectory::new();
    temp.write(
      "NuGet.Config",
      r#"<configuration><packageSources><clear /><add key="legacy" value="https://packages.example.test/api/v2/?sig=lock-secret#fragment" protocolVersion="2" /></packageSources></configuration>"#,
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
      ..PackageResolveOptions::default()
    };

    let first = resolve_package_inputs(&[&project], &options).unwrap().remove(0);
    let persisted_lock = fs::read_to_string(temp.0.join("dv.lock.json")).unwrap();
    let second = resolve_package_inputs(&[&project], &options).unwrap().remove(0);

    assert_eq!(first.target_framework(), "net8.0");
    assert_eq!(first.source_protocol(), "v2");
    assert!(persisted_lock.contains("https://packages.example.test/api/v2/"));
    assert!(!persisted_lock.contains("lock-secret"));
    assert_eq!(first.network_requests(), 0);
    assert_eq!(second.network_requests(), 0);
    assert_eq!(second.cache_hits(), 1);
    assert_eq!(second.package_cache_outcome(second.packages()[0]), CacheOutcome::Hit);
    assert_eq!(second.source_work().len(), 1);
    assert_eq!(second.source_work_name(0), "legacy");
    assert_eq!(second.source_work_requests(0), 0);
    assert_eq!(second.source_work_downloaded_bytes(0), 0);
    assert_eq!(second.source_work_duration_us(0), 0);
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
    assert!(second.native_assets().next().is_none());
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
