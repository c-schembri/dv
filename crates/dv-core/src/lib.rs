//! Stable data contracts shared by dv commands and reporters.

mod cancellation;
mod child_process;
mod compiler;
mod credential_provider;
mod diagnostic;
mod event;
mod framework;
mod framework_reference;
mod legacy_pruning;
mod pack_requirement;
mod package;
mod path;
mod project;
mod redaction;
mod reporter;
mod runtime_graph;
mod runtime_pack;
mod sdk;
mod workspace;

/// ASSUMPTION: current benchmark hosts expose 64-byte cache lines. This value
/// documents packing evidence only; it does not define a wire or ABI layout.
pub(crate) const BENCHMARK_CACHE_LINE_BYTES: usize = 64;

pub(crate) use path::absolute_lexical;

pub use cancellation::CancellationToken;
pub use child_process::{ChildExitPolicy, ChildProcessFailure, ChildProcessFailureStage, ChildTermination, classify_child_termination};
/// Compatibility name for the package-only cancellation handle shipped by
/// earlier dv releases.
pub type PackageCancellation = CancellationToken;
pub use compiler::{CompilerPlan, CompilerPlanError, CompilerPlanErrorKind, plan_compiler_inputs, plan_compiler_inputs_with_packages};
pub use credential_provider::CredentialProviderLogSink;
pub use diagnostic::{ContextField, Diagnostic, DiagnosticCode, DiagnosticCodeError, Severity};
pub use event::{
  CacheOutcome, CentralPackageVersionEvent, CompatibilityInputEvent, CompatibilityInvocationEvent, CompatibilitySupport, CompilerReferenceAliasEvent,
  ContentFileEvent, DirectPackagePolicyEvent, EVENT_SCHEMA_VERSION, Event, EventPayload, EventStreamError, Outcome, PackageHttpPolicyEvent,
  PackagePathPropertyEvent, PackageServiceEndpointEvent, PackageSourceCapabilityEvent, PackageSourceWorkEvent, ProjectFrameworkReferenceEvent,
  ProjectPackageEvent, ResolvedFrameworkReferenceEvent, ResolvedPackageEvent, RuntimeInstallationEvent, RuntimeTargetEvent, SdkInstallationEvent,
  validate_events,
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
  CentralPackageVersion, FrameworkReference, NugetAuditLevel, NugetAuditMode, PackageAssetFlags, PackageReference, ProjectClosureBatch, ProjectConfiguration,
  ProjectError, ProjectErrorKind, ProjectOutputType, ProjectSpec, RepositoryKind, RepositoryRoot, RuntimeRollForward, WorkspaceCandidate,
  WorkspaceCandidateKind, WorkspaceInventory, WorkspaceSelection, discover_repository_root, discover_workspace, evaluate_project, evaluate_project_closure,
  evaluate_project_closures, evaluate_project_path, select_workspace,
};
pub use redaction::redact_url_for_output;
pub use reporter::write_json_lines;
pub use runtime_graph::{RuntimeGraphError, RuntimeGraphErrorKind, RuntimeIdentifierGraph, load_portable_runtime_graph};
pub use runtime_pack::{RuntimePackError, RuntimePackErrorKind, RuntimePackPlan, plan_runtime_packs};
pub use sdk::{
  DotnetArchitecture, InstalledSdkInventory, RuntimeInstallation, RuntimeInventory, SdkError, SdkErrorKind, SdkInstallation, SdkInventory, SdkVersion,
  discover_installed_sdks, discover_installed_sdks_for_architecture, discover_runtimes, discover_runtimes_for_architecture, discover_runtimes_in_roots,
  discover_sdks, discover_sdks_in_roots,
};
pub use workspace::{
  AncestorInput, AncestorInputBatch, AncestorInputError, AncestorInputErrorKind, AncestorInputKind, AncestorInputRequest, discover_ancestor_inputs,
};
