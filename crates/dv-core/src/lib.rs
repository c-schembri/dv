//! Stable data contracts shared by dv commands and reporters.

mod compiler;
mod diagnostic;
mod event;
mod framework;
mod framework_reference;
mod pack_requirement;
mod package;
mod project;
mod reporter;
mod runtime_graph;
mod runtime_pack;
mod sdk;

pub use compiler::{CompilerPlan, CompilerPlanError, CompilerPlanErrorKind, plan_compiler_inputs, plan_compiler_inputs_with_packages};
pub use diagnostic::{ContextField, Diagnostic, DiagnosticCode, DiagnosticCodeError, Severity};
pub use event::{
  CacheOutcome, EVENT_SCHEMA_VERSION, Event, EventPayload, EventStreamError, Outcome, ProjectFrameworkReferenceEvent, ProjectPackageEvent,
  ResolvedFrameworkReferenceEvent, ResolvedPackageEvent, RuntimeTargetEvent, SdkInstallationEvent, validate_events,
};
pub use framework::{FrameworkFamily, TargetFramework, TargetFrameworkError};
pub use framework_reference::{
  FrameworkReferenceError, FrameworkReferenceErrorKind, FrameworkReferencePlan, ResolvedFrameworkReference, plan_framework_references,
};
pub use pack_requirement::{PackAcquisition, PackKind, PackRequirement};
pub use package::{
  PackageAssetFamily, PackageError, PackageErrorKind, PackageResolution, PackageResolveOptions, ResolvedPackage, RuntimeTargetKind, resolve_package_inputs,
};
pub use project::{
  FrameworkReference, PackageReference, ProjectConfiguration, ProjectError, ProjectErrorKind, ProjectOutputType, ProjectSpec, RuntimeRollForward,
  evaluate_project, evaluate_project_path,
};
pub use reporter::write_json_lines;
pub use runtime_graph::{RuntimeGraphError, RuntimeGraphErrorKind, RuntimeIdentifierGraph, load_portable_runtime_graph};
pub use runtime_pack::{RuntimePackError, RuntimePackErrorKind, RuntimePackPlan, plan_runtime_packs};
pub use sdk::{SdkError, SdkErrorKind, SdkInstallation, SdkInventory, SdkVersion, discover_sdks, discover_sdks_in_roots};
