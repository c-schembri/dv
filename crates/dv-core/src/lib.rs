//! Stable data contracts shared by dv commands and reporters.

mod compiler;
mod credential_provider;
mod diagnostic;
mod event;
mod framework;
mod framework_reference;
mod legacy_pruning;
mod pack_requirement;
mod package;
mod project;
mod redaction;
mod reporter;
mod runtime_graph;
mod runtime_pack;
mod sdk;

/// ASSUMPTION: current benchmark hosts expose 64-byte cache lines. This value
/// documents packing evidence only; it does not define a wire or ABI layout.
pub(crate) const BENCHMARK_CACHE_LINE_BYTES: usize = 64;

pub use compiler::{CompilerPlan, CompilerPlanError, CompilerPlanErrorKind, plan_compiler_inputs, plan_compiler_inputs_with_packages};
pub use credential_provider::{CredentialProviderLogSink, PackageCancellation};
pub use diagnostic::{ContextField, Diagnostic, DiagnosticCode, DiagnosticCodeError, Severity};
pub use event::{
  CacheOutcome, CentralPackageVersionEvent, CompilerReferenceAliasEvent, ContentFileEvent, DirectPackagePolicyEvent, EVENT_SCHEMA_VERSION, Event, EventPayload,
  EventStreamError, Outcome, PackageHttpPolicyEvent, PackagePathPropertyEvent, PackageServiceEndpointEvent, PackageSourceCapabilityEvent,
  PackageSourceWorkEvent, ProjectFrameworkReferenceEvent, ProjectPackageEvent, ResolvedFrameworkReferenceEvent, ResolvedPackageEvent, RuntimeTargetEvent,
  SdkInstallationEvent, validate_events,
};
pub use framework::{FrameworkFamily, TargetFramework, TargetFrameworkError};
pub use framework_reference::{
  FrameworkReferenceError, FrameworkReferenceErrorKind, FrameworkReferencePlan, ResolvedFrameworkReference, plan_framework_references,
};
pub use pack_requirement::{PackAcquisition, PackKind, PackRequirement};
pub use package::{
  PackageAssetFamily, PackageError, PackageErrorKind, PackageResolution, PackageResolveOptions, PackageServiceKind, PackageSourceAuthentication,
  PackageSourceInventory, ResolvedPackage, RuntimeTargetKind, SignatureValidationMode, inspect_package_sources, resolve_package_inputs,
  resolve_package_inputs_with_runtime_graph,
};
pub use project::{
  CentralPackageVersion, FrameworkReference, NugetAuditLevel, NugetAuditMode, PackageAssetFlags, PackageReference, ProjectConfiguration, ProjectError,
  ProjectErrorKind, ProjectOutputType, ProjectSpec, RuntimeRollForward, evaluate_project, evaluate_project_closure, evaluate_project_path,
};
pub use redaction::redact_url_for_output;
pub use reporter::write_json_lines;
pub use runtime_graph::{RuntimeGraphError, RuntimeGraphErrorKind, RuntimeIdentifierGraph, load_portable_runtime_graph};
pub use runtime_pack::{RuntimePackError, RuntimePackErrorKind, RuntimePackPlan, plan_runtime_packs};
pub use sdk::{SdkError, SdkErrorKind, SdkInstallation, SdkInventory, SdkVersion, discover_sdks, discover_sdks_in_roots};
