use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{Diagnostic, RuntimeTargetKind};

/// Current version of the JSON event protocol.
pub const EVENT_SCHEMA_VERSION: u16 = 16;

/// The result of a command or work item.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
  /// The operation completed successfully.
  Succeeded,
  /// The operation failed.
  Failed,
  /// The operation was deliberately skipped.
  Skipped,
  /// The operation was cancelled before completion.
  Cancelled,
}

/// The result of a cache lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheOutcome {
  /// The requested data was found and reused.
  Hit,
  /// The requested data was not found.
  Miss,
  /// Stored data existed but was not valid for this request.
  Invalid,
}

/// One event in a command's ordered event stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Event {
  /// Wire protocol version.
  pub schema_version: u16,
  /// Zero-based position in this command's event stream.
  pub sequence: u64,
  /// Microseconds elapsed since command start.
  pub elapsed_us: u64,
  /// Event-specific data.
  #[serde(flatten)]
  pub payload: EventPayload,
}

/// One selected NuGet service endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackageServiceEndpointEvent {
  /// Stable capability spelling.
  pub kind: String,
  /// Absolute endpoint URL advertised by the source.
  pub location: String,
}

/// One effective package source and its selected capabilities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackageSourceCapabilityEvent {
  /// Configuration key or redacted command-line source identity.
  pub name: String,
  /// Configured source URL or local path.
  pub location: String,
  /// `local`, `v2`, or `v3`.
  pub protocol: String,
  /// `none` or `basic`; never contains a username, password, or token.
  pub authentication: String,
  /// Whether this source explicitly permits unencrypted HTTP transport.
  pub allow_insecure_connections: bool,
  /// Whether TLS peer and hostname validation is disabled for this source.
  pub disable_tls_certificate_validation: bool,
  /// Capability-ordered endpoint batch.
  pub endpoints: Vec<PackageServiceEndpointEvent>,
  /// Actual HTTP attempts made against this source.
  pub requests: u32,
  /// HTTP response-body or local archive bytes read from this source.
  pub downloaded_bytes: u64,
  /// Cumulative source-work time; concurrent values may exceed command wall time.
  pub duration_us: u64,
}

/// Credential-free work attributed to one configured package source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackageSourceWorkEvent {
  /// Configuration key or redacted CLI source identity; never includes URL credentials.
  pub name: String,
  /// `local`, `v2`, or `v3`.
  pub protocol: String,
  /// Actual HTTP attempts, including retries and authentication retries.
  pub requests: u32,
  /// HTTP response-body or local archive bytes read from this source.
  pub downloaded_bytes: u64,
  /// Cumulative source-work time; concurrent values may exceed command wall time.
  pub duration_us: u64,
}

/// Redacted effective NuGet HTTP behavior for a package-source command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackageHttpPolicyEvent {
  /// Maximum attempts including the first request.
  pub max_tries: u8,
  /// Base retry delay in milliseconds.
  pub retry_delay_ms: u32,
  /// Maximum accepted `Retry-After` delay in seconds.
  pub max_retry_after_seconds: u32,
  /// Total request timeout in seconds.
  pub request_timeout_seconds: u16,
  /// Maximum response-body stall in seconds.
  pub download_timeout_seconds: u16,
  /// Configured concurrent request limit per source.
  pub max_requests_per_source: u16,
  /// Whether HTTP 429 is retried.
  pub retry_http_429: bool,
  /// Whether server `Retry-After` is observed.
  pub observe_retry_after: bool,
  /// Whether a proxy is configured; the address is deliberately omitted.
  pub proxy_configured: bool,
  /// Whether redacted proxy credentials are configured.
  pub proxy_authenticated: bool,
  /// Whether a proxy bypass list is configured; its hosts are omitted.
  pub no_proxy_configured: bool,
  /// Whether network work was disabled for this command.
  pub offline: bool,
  /// TLS peer and hostname validation is enabled.
  pub tls_validation: bool,
  /// Whether at least one source explicitly permits unencrypted HTTP transport.
  pub allow_insecure_connections: bool,
  /// Maximum redirects; HTTP targets require a per-source opt-in.
  pub max_redirects: u8,
}

impl Event {
  /// Creates an event using the current wire protocol version.
  pub fn new(sequence: u64, elapsed_us: u64, payload: EventPayload) -> Self {
    Self {
      schema_version: EVENT_SCHEMA_VERSION,
      sequence,
      elapsed_us,
      payload,
    }
  }
}

/// One SDK record materialized at the reporter boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SdkInstallationEvent {
  /// Installed SDK version.
  pub version: String,
  /// Full SDK directory.
  pub path: String,
  /// Whether this record was selected.
  pub selected: bool,
}

/// One RID-specific package runtime target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeTargetEvent {
  /// Full selected asset path.
  pub path: String,
  /// Runtime identifier associated with the asset.
  pub runtime_identifier: String,
  /// Whether the target is managed runtime code or native code.
  pub kind: RuntimeTargetKind,
}

/// One exact package dependency materialized at the reporter boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectPackageEvent {
  /// NuGet package identifier.
  pub id: String,
  /// Exact package version.
  pub version: String,
  /// Asset families made available to the project before exclusions.
  pub include_assets: Vec<String>,
  /// Asset families removed from the project.
  pub exclude_assets: Vec<String>,
  /// Asset families hidden from consuming projects.
  pub private_assets: Vec<String>,
  /// Package-scoped NuGet warning codes.
  pub no_warn: Vec<String>,
  /// Compiler aliases applied to direct package assemblies.
  pub aliases: Option<String>,
  /// Whether a `Pkg*` package-root property is generated.
  pub generate_path_property: bool,
}

/// One selected central package version materialized at the reporter boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CentralPackageVersionEvent {
  /// Case-preserving package identity.
  pub id: String,
  /// Selected version or range.
  pub version: String,
}

/// Sparse compiler alias attached to one materialized reference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompilerReferenceAliasEvent {
  /// Zero-based index in the compiler reference batch.
  pub reference_index: u32,
  /// Reference path at that index.
  pub reference: String,
  /// Alias text passed to Roslyn.
  pub aliases: String,
}

/// One generated package-root property.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackagePathPropertyEvent {
  /// MSBuild-compatible property name.
  pub name: String,
  /// Selected global-packages root for this package version.
  pub value: String,
}

/// One direct dependency policy keyed into the resolved package batch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DirectPackagePolicyEvent {
  /// Zero-based index in the resolved package batch.
  pub package_index: u32,
  /// Effective asset families consumed through this direct reference.
  pub include_assets: Vec<String>,
  /// Asset families hidden from consuming projects.
  pub private_assets: Vec<String>,
  /// Package-scoped NuGet warning codes.
  pub no_warn: Vec<String>,
  /// Compiler aliases for this package's direct compile assets.
  pub aliases: Option<String>,
  /// Generated package-root property, when requested.
  pub path_property: Option<PackagePathPropertyEvent>,
}

/// One explicit project framework reference at the reporter boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectFrameworkReferenceEvent {
  /// Framework-reference identity.
  pub id: String,
  /// Per-reference runtime version override.
  pub runtime_version: Option<String>,
  /// Per-reference targeting-pack version override.
  pub targeting_pack_version: Option<String>,
  /// Per-reference latest-runtime-patch preference.
  pub target_latest_runtime_patch: Option<bool>,
}

/// One resolved framework-reference row at the reporter boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedFrameworkReferenceEvent {
  /// Project-facing framework-reference identity.
  pub reference: String,
  /// Runtimeconfig/shared-directory framework name.
  pub runtime_name: String,
  /// Minimum runtime version selected from project and SDK data.
  pub requested_version: String,
  /// Installed version selected by roll-forward, absent for self-contained projects.
  pub selected_version: Option<String>,
  /// Installed shared-framework directory, absent for self-contained projects.
  pub shared_root: Option<String>,
  /// Targeting-pack NuGet identity.
  pub targeting_pack_id: String,
  /// Targeting-pack version.
  pub targeting_pack_version: String,
  /// Installed or restored targeting-pack directory.
  pub targeting_pack_root: String,
  /// Optional framework profile.
  pub profile: Option<String>,
}

/// One resolved package materialized at the reporter boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPackageEvent {
  /// Package identity with source casing.
  pub id: String,
  /// Normalized exact version.
  pub version: String,
  /// Verified package archive SHA-512.
  pub sha512: String,
  /// Whether this package is directly referenced.
  pub direct: bool,
  /// Whether this transitive package was promoted by central pinning.
  pub central_transitive: bool,
  /// Number of outgoing dependency edges.
  pub dependency_count: u32,
  /// Shared-framework references selected from this package's nearest nuspec group.
  pub framework_references: Vec<String>,
  /// Legacy .NET Framework assemblies selected from this package's nearest nuspec group.
  pub framework_assemblies: Vec<String>,
  /// Whether the package was reused or acquired for this command.
  pub cache_outcome: CacheOutcome,
}

/// Event variants emitted by command execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
  /// The parsed command is about to execute.
  CommandStarted {
    /// Stable command name.
    command: String,
    /// Command arguments after the executable name.
    args: Vec<String>,
  },
  /// A batch of work is about to execute.
  WorkStarted {
    /// Stable operation name.
    operation: String,
    /// Number of items in the batch.
    item_count: u64,
  },
  /// A batch of work has completed.
  WorkFinished {
    /// Stable operation name matching the start event.
    operation: String,
    /// Number of items processed.
    item_count: u64,
    /// Time spent in the operation.
    duration_us: u64,
    /// Terminal result.
    outcome: Outcome,
  },
  /// A cache key was classified without exposing cache contents.
  CacheDecision {
    /// Cache namespace, such as `packages` or `build`.
    cache: String,
    /// Stable key or digest text.
    key: String,
    /// Lookup result.
    outcome: CacheOutcome,
  },
  /// The current SDK was selected.
  SdkSelected {
    /// Selected SDK version.
    version: String,
    /// Full SDK directory.
    path: String,
    /// Installation root.
    root: String,
    /// `global.json` used for selection, when present.
    global_json: Option<String>,
  },
  /// The installed SDK batch was discovered.
  SdkInventory {
    /// SDK records in deterministic resolver order.
    installations: Vec<SdkInstallationEvent>,
    /// `global.json` used for selection, when present.
    global_json: Option<String>,
  },
  /// One SDK-owned runtime identifier was expanded through the portable graph.
  RuntimeCompatibility {
    /// Selected SDK version that owns the graph.
    sdk_version: String,
    /// Full portable runtime-graph path.
    graph_path: String,
    /// Requested opaque runtime identifier.
    runtime_identifier: String,
    /// Compatible RIDs in breadth-first nearest-first order.
    compatible_runtimes: Vec<String>,
    /// Number of graph nodes.
    node_count: u32,
    /// Number of direct graph edges.
    edge_count: u32,
    /// Number of precomputed compatibility indices.
    compatibility_count: u32,
  },
  /// Runtime, host, native, and apphost inputs were selected for one RID.
  RuntimePackPlanCreated {
    /// Full project-file path.
    project: String,
    /// Selected SDK version.
    sdk_version: String,
    /// SDK manifest which supplied pack identities and versions.
    manifest: String,
    /// Evaluated target framework.
    target_framework: String,
    /// Runtime identifier requested by the project.
    requested_runtime_identifier: String,
    /// Nearest SDK-supported runtime-pack RID.
    runtime_identifier: String,
    /// Runtime-pack identity.
    runtime_pack_id: String,
    /// Runtime-pack version.
    runtime_pack_version: String,
    /// Resolved runtime-pack directory.
    runtime_pack_root: String,
    /// Nearest SDK-supported host-pack RID.
    host_runtime_identifier: String,
    /// Host-pack identity.
    host_pack_id: String,
    /// Host-pack version.
    host_pack_version: String,
    /// Resolved host-pack directory.
    host_pack_root: String,
    /// Selected platform apphost template.
    apphost_template: String,
    /// Managed runtime assets in pack-manifest order.
    managed_assets: Vec<String>,
    /// Native runtime assets in pack-manifest order.
    native_assets: Vec<String>,
  },
  /// Framework references, targeting packs, and shared runtimes were selected.
  FrameworkReferencePlanCreated {
    /// Full project-file path.
    project: String,
    /// Selected SDK version.
    sdk_version: String,
    /// SDK manifest which supplied framework and pack versions.
    manifest: String,
    /// Evaluated target framework.
    target_framework: String,
    /// Effective runtime-host roll-forward policy.
    roll_forward: String,
    /// Whether the project carries its runtime.
    self_contained: bool,
    /// Ordered implicit and explicit framework-reference batch.
    frameworks: Vec<ResolvedFrameworkReferenceEvent>,
  },
  /// One SDK-style project was discovered and evaluated.
  ProjectEvaluated {
    /// Full project-file path.
    project: String,
    /// SDK declared by the project.
    sdk: String,
    /// Single evaluated target framework.
    target_framework: String,
    /// Selected runtime identifier, when one inner runtime target is active.
    runtime_identifier: Option<String>,
    /// Ordered runtime-identifier expansion property.
    runtime_identifiers: Vec<String>,
    /// Unique runtime target dimensions materialized for downstream work.
    runtime_dimensions: Vec<String>,
    /// Managed output type.
    output_type: String,
    /// Selected build configuration.
    configuration: String,
    /// Output assembly name.
    assembly_name: String,
    /// Generated root namespace.
    root_namespace: String,
    /// Effective nullable mode.
    nullable: String,
    /// Effective implicit-usings mode.
    implicit_usings: String,
    /// Whether deterministic compiler output is required.
    deterministic: bool,
    /// Ordered source paths relative to the project.
    sources: Vec<String>,
    /// Ordered project-reference paths.
    project_references: Vec<String>,
    /// Ordered exact package references.
    package_references: Vec<ProjectPackageEvent>,
    /// Whether central package version management is active.
    central_package_management: bool,
    /// Whether selected central versions promote matching transitive packages.
    central_transitive_pinning: bool,
    /// Selected central package versions in case-insensitive identity order.
    central_package_versions: Vec<CentralPackageVersionEvent>,
    /// Ordered explicit framework references.
    framework_references: Vec<ProjectFrameworkReferenceEvent>,
    /// Project-wide runtime framework version override.
    runtime_framework_version: Option<String>,
    /// Explicit latest-runtime-patch preference.
    target_latest_runtime_patch: Option<bool>,
    /// Effective runtime-host roll-forward policy.
    roll_forward: String,
    /// Whether deployment includes its runtime.
    self_contained: bool,
  },
  /// A complete framework and Roslyn input plan was materialized.
  CompilerPlanCreated {
    /// Full project-file path.
    project: String,
    /// Selected SDK version.
    sdk_version: String,
    /// Selected Roslyn compiler assembly.
    compiler: String,
    /// Selected framework reference-pack version.
    framework_pack_version: String,
    /// Selected framework reference-pack directory.
    framework_pack: String,
    /// Fixed C# language version.
    language_version: String,
    /// Compiler warning level.
    warning_level: u16,
    /// Selected build configuration.
    configuration: String,
    /// Roslyn output kind.
    output_type: String,
    /// Whether nullable analysis is enabled.
    nullable: bool,
    /// Whether deterministic output is required.
    deterministic: bool,
    /// Planned output assembly.
    output_assembly: String,
    /// Planned portable PDB.
    output_pdb: String,
    /// Planned reference assembly.
    reference_output: String,
    /// Ordered user source paths.
    sources: Vec<String>,
    /// Ordered generated source paths.
    generated_sources: Vec<String>,
    /// Ordered framework reference assemblies.
    references: Vec<String>,
    /// Sparse aliases keyed into `references`.
    reference_aliases: Vec<CompilerReferenceAliasEvent>,
    /// Generated package-root properties.
    package_path_properties: Vec<PackagePathPropertyEvent>,
    /// Ordered SDK and framework analyzers.
    analyzers: Vec<String>,
    /// Ordered analyzer configuration files.
    analyzer_configs: Vec<String>,
    /// Ordered preprocessor symbols.
    defines: Vec<String>,
    /// Resolved package count.
    package_count: u32,
    /// Package compile-asset count included in references.
    package_compile_assets: u32,
    /// Packages reused from the global cache.
    package_cache_hits: u32,
    /// Packages downloaded during planning.
    downloaded_packages: u32,
    /// HTTP requests made during package planning.
    package_network_requests: u32,
    /// HTTP response-body and local source archive bytes read for packages.
    package_downloaded_bytes: u64,
  },
  /// One exact package graph was resolved and cached.
  PackageResolutionCreated {
    /// Full project-file path.
    project: String,
    /// Global-packages directory.
    cache_root: String,
    /// NuGet HTTP metadata-cache directory.
    http_cache_root: String,
    /// NuGet scratch directory.
    temp_root: String,
    /// Ordered read-only fallback package roots.
    fallback_roots: Vec<String>,
    /// Package-signature validation policy.
    signature_validation: String,
    /// Whether vulnerability auditing is enabled.
    audit_enabled: bool,
    /// Vulnerability-audit dependency scope.
    audit_mode: String,
    /// Minimum reported vulnerability severity.
    audit_level: String,
    /// Whether restore constructed an explicit proxy policy.
    proxy_configured: bool,
    /// Deterministic dv lock file.
    lock_path: String,
    /// Evaluated target framework.
    target_framework: String,
    /// Credential-free configuration key or redacted CLI identity for the selected source.
    source: String,
    /// Selected NuGet protocol generation.
    source_protocol: String,
    /// Credential-free work in configured source order.
    source_work: Vec<PackageSourceWorkEvent>,
    /// Packages sorted by case-insensitive identity.
    packages: Vec<ResolvedPackageEvent>,
    /// Direct-reference policy rows keyed into `packages`.
    direct_policies: Vec<DirectPackagePolicyEvent>,
    /// Ordered compile assemblies selected for the evaluated target.
    compile_assets: Vec<String>,
    /// Ordered runtime assemblies selected for the evaluated target.
    runtime_assets: Vec<String>,
    /// Ordered package analyzers.
    analyzers: Vec<String>,
    /// Ordered satellite resource assemblies.
    resource_assets: Vec<String>,
    /// Ordered package content files.
    content_files: Vec<String>,
    /// Ordered inner-build imports from `build`.
    build_assets: Vec<String>,
    /// Ordered outer-build imports from `buildMultiTargeting`.
    build_multi_targeting_assets: Vec<String>,
    /// Ordered transitive imports from `buildTransitive`.
    build_transitive_assets: Vec<String>,
    /// Ordered legacy native assets.
    native_assets: Vec<String>,
    /// Ordered RID-specific managed and native targets.
    runtime_targets: Vec<RuntimeTargetEvent>,
    /// Packages reused from the cache.
    cache_hits: u32,
    /// Packages downloaded and atomically published.
    downloaded_packages: u32,
    /// HTTP requests made.
    network_requests: u32,
    /// HTTP response-body and local source archive bytes read.
    downloaded_bytes: u64,
  },
  /// Effective package sources and v3 capabilities were inspected.
  PackageSourcesInspected {
    /// Full project-file path whose configuration hierarchy was used.
    project: String,
    /// Sources in merged configuration order.
    sources: Vec<PackageSourceCapabilityEvent>,
    /// Redacted effective HTTP behavior.
    http_policy: PackageHttpPolicyEvent,
    /// Service-index HTTP requests performed.
    network_requests: u32,
    /// Service-index response bytes read.
    downloaded_bytes: u64,
  },
  /// A structured diagnostic was produced.
  Diagnostic {
    /// Diagnostic data.
    diagnostic: Diagnostic,
  },
  /// The command reached a terminal state.
  CommandFinished {
    /// Stable command name matching the start event.
    command: String,
    /// Total command time.
    duration_us: u64,
    /// Terminal result.
    outcome: Outcome,
  },
}

/// A malformed event batch rejected before reporting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventStreamError {
  /// An event uses an unsupported schema.
  UnsupportedSchema {
    /// Position in the supplied batch.
    index: usize,
    /// Schema found at that position.
    found: u16,
  },
  /// Sequence numbers are not contiguous and zero-based.
  UnexpectedSequence {
    /// Position in the supplied batch.
    index: usize,
    /// Required sequence number.
    expected: u64,
    /// Sequence number found.
    found: u64,
  },
  /// Elapsed time moved backwards.
  ElapsedTimeRegressed {
    /// Position in the supplied batch.
    index: usize,
    /// Previous elapsed time.
    previous_us: u64,
    /// Current elapsed time.
    found_us: u64,
  },
}

impl fmt::Display for EventStreamError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::UnsupportedSchema { index, found } => write!(formatter, "event {index} uses schema {found}; expected {EVENT_SCHEMA_VERSION}"),
      Self::UnexpectedSequence { index, expected, found } => write!(formatter, "event {index} has sequence {found}; expected {expected}"),
      Self::ElapsedTimeRegressed { index, previous_us, found_us } => {
        write!(formatter, "event {index} elapsed time regressed from {previous_us}us to {found_us}us")
      },
    }
  }
}

impl Error for EventStreamError {}

/// Validates the schema, ordering, and monotonic time of an event batch.
pub fn validate_events(events: &[Event]) -> Result<(), EventStreamError> {
  let mut previous_us = 0;

  for (index, event) in events.iter().enumerate() {
    if event.schema_version != EVENT_SCHEMA_VERSION {
      return Err(EventStreamError::UnsupportedSchema {
        index,
        found: event.schema_version,
      });
    }

    let expected = index as u64;
    if event.sequence != expected {
      return Err(EventStreamError::UnexpectedSequence {
        index,
        expected,
        found: event.sequence,
      });
    }

    if index > 0 && event.elapsed_us < previous_us {
      return Err(EventStreamError::ElapsedTimeRegressed {
        index,
        previous_us,
        found_us: event.elapsed_us,
      });
    }
    previous_us = event.elapsed_us;
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn event(sequence: u64, elapsed_us: u64) -> Event {
    Event::new(
      sequence,
      elapsed_us,
      EventPayload::WorkStarted {
        operation: "parse_projects".into(),
        item_count: 3,
      },
    )
  }

  #[test]
  fn valid_batch_is_contiguous_and_monotonic() {
    assert!(validate_events(&[event(0, 0), event(1, 5)]).is_ok());
  }

  #[test]
  fn rejects_sequence_gaps() {
    assert!(matches!(
      validate_events(&[event(0, 0), event(2, 5)]),
      Err(EventStreamError::UnexpectedSequence { .. })
    ));
  }

  #[test]
  fn rejects_time_regression() {
    assert!(matches!(
      validate_events(&[event(0, 5), event(1, 4)]),
      Err(EventStreamError::ElapsedTimeRegressed { .. })
    ));
  }
}
