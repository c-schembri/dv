use std::{
  env,
  error::Error,
  ffi::OsStr,
  fmt::Write as _,
  fs,
  io::{self, IsTerminal, Read, Write},
  net::{SocketAddr, TcpListener, TcpStream},
  path::{Path, PathBuf},
  process::{Command, Output, Stdio},
  sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
  },
  thread::{self, JoinHandle},
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const PHASE_ONE_CI_TRACE: &str = include_str!("../../../compatibility/traces/phase1-ci.json");
const PHASE_ONE_CI_SCRIPT: &str = include_str!("../../../compatibility/scripts/phase1-github-actions.yml");
const GOLDEN_TRACE_ENVIRONMENT: &[(&str, &str)] = &[
  ("DOTNET_CLI_TELEMETRY_OPTOUT", "1"),
  ("DOTNET_NOLOGO", "1"),
  ("DOTNET_SKIP_FIRST_TIME_EXPERIENCE", "1"),
  ("NO_COLOR", "1"),
  ("HTTP_PROXY", "http://127.0.0.1:9"),
  ("HTTPS_PROXY", "http://127.0.0.1:9"),
  ("ALL_PROXY", "http://127.0.0.1:9"),
  ("NO_PROXY", ""),
  ("NUGET_PACKAGES", "$WORKSPACE/.packages"),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenTraceFile {
  schema_version: u16,
  cases: Box<[GoldenTraceCase]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenTraceCase {
  id: String,
  source: String,
  reference_argv: Box<[String]>,
  candidate_argv: Box<[String]>,
  environment_overrides: Box<[GoldenEnvironmentOverride]>,
  stdin_utf8: String,
  input_filesystem: String,
  expected: GoldenTraceExpected,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenEnvironmentOverride {
  name: String,
  value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenTraceExpected {
  reference: GoldenObservation,
  candidate: GoldenObservation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenObservation {
  stdout_utf8: String,
  stderr_utf8: String,
  exit_code: i32,
  filesystem_delta: Box<[GoldenFileChange]>,
  process_trace: String,
  network_trace: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct GoldenFileChange {
  path: String,
  kind: String,
  change: String,
}

#[derive(Clone, Copy)]
enum CaseKind {
  Startup,
  CliUnknownOption,
  CliOptionEffects,
  CliDriverInventory,
  CliGoldenTrace,
  CliCompatCheck,
  CliCommandNormalization,
  CliTransformEquivalence,
  CliModeClassification,
  CliExitPolicy,
  CliLexicalPreservation,
  CliRoutePrecedence,
  CliEnvironment,
  CliForwarding,
  CliChildExit,
  RidGraph,
  ProjectEvaluate,
  PackageReferenceConditions,
  RuntimeEvaluate,
  RuntimePackPlan,
  RuntimePackInventoryCold,
  FrameworkReferencePlan,
  PackDiagnostic,
  CompilerPlan,
  RestoreCold,
  PackageSyncCold,
  PackageGraphCold,
  PackageGraphMassive,
  PackageAssetPlan,
  PackageReferenceMetadata,
  PackagePruning,
  PackageSignatureCold,
  PackageSignatureWarm,
  NuspecFrameworkMetadata,
  PackageRidContentCold,
  PackageRidContentWarm,
  CentralPackageManagement,
  PackageConflictResolution,
  PackageBatchResolution,
  PackageDiagnostics,
  PackageSyncWarm,
  NugetConfigHierarchy,
  NugetConfigMerge,
  NugetSourceSections,
  NugetSourceMapping,
  NugetRequestBudget,
  NugetSourceTelemetry,
  NugetStoragePolicy,
  NugetCliOverrides,
  NugetLocalSources,
  NugetFloatingVersion,
  NugetServiceIndex,
  NugetCredentials,
  NugetCredentialProvider,
  NugetClientCertificates,
  NugetHttpPolicy,
  NugetSourceSecurity,
  BuildClean,
  BuildNoOp,
  RunWarm,
}

struct Case {
  name: &'static str,
  kind: CaseKind,
  args: &'static [&'static str],
  implemented: bool,
}

struct Fixtures<'a> {
  small: &'a Path,
  argument_forwarding: &'a Path,
  rid_graph: &'a Path,
  runtime: &'a Path,
  runtime_pack: &'a Path,
  framework_reference: &'a Path,
  unavailable_pack: &'a Path,
  package: &'a Path,
  package_reference_metadata: &'a Path,
  package_signature: &'a Path,
  nuspec_framework_metadata: &'a Path,
  package_rid_content: &'a Path,
  central_package_management: &'a Path,
  package_conflict_resolution: &'a Path,
  package_reference_conditions: &'a Path,
  nuget_config: &'a Path,
  nuget_config_merge: &'a Path,
  nuget_source_sections: &'a Path,
  nuget_source_mapping: &'a Path,
  nuget_request_budget: &'a Path,
  nuget_storage_policy: &'a Path,
  nuget_cli_overrides: &'a Path,
  nuget_local_sources: &'a Path,
  nuget_floating_version: &'a Path,
  nuget_service_index: &'a Path,
  nuget_credentials: &'a Path,
  nuget_credential_provider: &'a Path,
  nuget_client_certificates: &'a Path,
  nuget_http_policy: &'a Path,
  nuget_source_security: &'a Path,
  package_graph: &'a Path,
  package_graph_massive: &'a Path,
}

const DOTNET_CASES: &[Case] = &[
  Case {
    name: "sdk_current",
    kind: CaseKind::Startup,
    args: &["--version"],
    implemented: true,
  },
  Case {
    name: "sdk_current_globals",
    kind: CaseKind::Startup,
    args: &["--version"],
    implemented: true,
  },
  Case {
    name: "sdk_current_compat",
    kind: CaseKind::Startup,
    args: &["--version"],
    implemented: true,
  },
  Case {
    name: "cli_golden_trace",
    kind: CaseKind::CliGoldenTrace,
    args: &[
      "restore",
      "SmallConsole.csproj",
      "--packages",
      ".packages",
      "--source",
      "offline-source",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "cli_compat_help",
    kind: CaseKind::Startup,
    args: &["build", "-?"],
    implemented: true,
  },
  Case {
    name: "cli_cancellation",
    kind: CaseKind::Startup,
    args: &["--version"],
    implemented: true,
  },
  Case {
    name: "cli_unknown_option",
    kind: CaseKind::CliUnknownOption,
    args: &["build", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_option_effects",
    kind: CaseKind::CliOptionEffects,
    args: &["test", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "dotnet_runtime_inventory",
    kind: CaseKind::CliDriverInventory,
    args: &["--list-runtimes"],
    implemented: true,
  },
  Case {
    name: "cli_command_normalization",
    kind: CaseKind::CliCommandNormalization,
    args: &["restore", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_transform_equivalence",
    kind: CaseKind::CliTransformEquivalence,
    args: &["restore", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_mode_classification",
    kind: CaseKind::CliModeClassification,
    args: &["build", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_exit_policy",
    kind: CaseKind::CliExitPolicy,
    args: &["restore", "DefinitelyMissing.csproj"],
    implemented: true,
  },
  Case {
    name: "cli_lexical_preservation",
    kind: CaseKind::CliLexicalPreservation,
    args: &["build", "-c:Release", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_route_precedence",
    kind: CaseKind::CliRoutePrecedence,
    args: &["pack", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_environment",
    kind: CaseKind::CliEnvironment,
    args: &["build", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_forwarding",
    kind: CaseKind::CliForwarding,
    args: &["bin/Release/net10.0/ArgumentForwarding.dll", "alpha", "", "--color", "two words"],
    implemented: true,
  },
  Case {
    name: "cli_child_exit",
    kind: CaseKind::CliChildExit,
    args: &["bin/Release/net10.0/ArgumentForwarding.dll", "exit", "23"],
    implemented: true,
  },
  Case {
    name: "cli_protocol_version",
    kind: CaseKind::Startup,
    args: &[],
    implemented: false,
  },
  Case {
    name: "cli_compat_manifest",
    kind: CaseKind::Startup,
    args: &[],
    implemented: false,
  },
  Case {
    name: "cli_compat_check",
    kind: CaseKind::CliCompatCheck,
    args: &[],
    implemented: false,
  },
  Case {
    name: "rid_graph",
    kind: CaseKind::RidGraph,
    args: &["bin/Release/RidGraphOracle.dll", "linux-musl-x64"],
    implemented: true,
  },
  Case {
    name: "project_evaluate",
    kind: CaseKind::ProjectEvaluate,
    args: &[
      "msbuild",
      "SmallConsole.csproj",
      "--nologo",
      "-getProperty:TargetFramework,OutputType,Nullable,ImplicitUsings,AssemblyName,RootNamespace,Configuration,Deterministic",
      "-getItem:Compile,ProjectReference,PackageReference",
    ],
    implemented: true,
  },
  Case {
    name: "project_select_named",
    kind: CaseKind::ProjectEvaluate,
    args: &[
      "msbuild",
      "SmallConsole.csproj",
      "--nologo",
      "-getProperty:TargetFramework,OutputType,Nullable,ImplicitUsings,AssemblyName,RootNamespace,Configuration,Deterministic",
      "-getItem:Compile,ProjectReference,PackageReference",
    ],
    implemented: true,
  },
  Case {
    name: "package_reference_conditions",
    kind: CaseKind::PackageReferenceConditions,
    args: &[
      "msbuild",
      "ConditionalReferences.csproj",
      "--nologo",
      "-p:Configuration=Release",
      "-getProperty:TargetFramework,RuntimeIdentifier,Configuration",
      "-getItem:PackageReference,ProjectReference,FrameworkReference",
    ],
    implemented: true,
  },
  Case {
    name: "runtime_evaluate",
    kind: CaseKind::RuntimeEvaluate,
    args: &[
      "msbuild",
      "RuntimeProject.csproj",
      "--nologo",
      "-getProperty:TargetFramework,RuntimeIdentifier,RuntimeIdentifiers",
    ],
    implemented: true,
  },
  Case {
    name: "runtime_pack_plan",
    kind: CaseKind::RuntimePackPlan,
    args: &[
      "msbuild",
      "RuntimePackProject.csproj",
      "--nologo",
      "-p:SelfContained=true",
      "-p:UseAppHost=true",
      "-t:ProcessFrameworkReferences;ResolveFrameworkReferences;ResolveRuntimePackAssets;_GetAppHostPaths",
      "-getProperty:RuntimeIdentifier,AppHostSourcePath",
      "-getItem:ResolvedRuntimePack,RuntimePackAsset,ResolvedAppHostPack",
    ],
    implemented: true,
  },
  Case {
    name: "runtime_pack_inventory_cold",
    kind: CaseKind::RuntimePackInventoryCold,
    args: &[
      "msbuild",
      "RuntimePackProject.csproj",
      "--nologo",
      "-p:SelfContained=true",
      "-p:UseAppHost=true",
      "-t:ProcessFrameworkReferences;ResolveFrameworkReferences;ResolveRuntimePackAssets;_GetAppHostPaths",
      "-getProperty:RuntimeIdentifier,AppHostSourcePath",
      "-getItem:ResolvedRuntimePack,RuntimePackAsset,ResolvedAppHostPack",
    ],
    implemented: true,
  },
  Case {
    name: "framework_reference_plan",
    kind: CaseKind::FrameworkReferencePlan,
    args: &[
      "msbuild",
      "FrameworkReferenceProject.csproj",
      "--nologo",
      "-t:ResolveTargetingPackAssets",
      "-getProperty:TargetFramework,RollForward,SelfContained",
      "-getItem:RuntimeFramework,ResolvedFrameworkReference",
    ],
    implemented: true,
  },
  Case {
    name: "pack_diagnostic",
    kind: CaseKind::PackDiagnostic,
    args: &[
      "restore",
      "UnavailablePackProject.csproj",
      "--source",
      "offline-source",
      "--packages",
      ".packages",
      "--no-cache",
      "--disable-build-servers",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "minimal",
    ],
    implemented: true,
  },
  Case {
    name: "compiler_plan",
    kind: CaseKind::CompilerPlan,
    args: &[
      "msbuild",
      "SmallConsole.csproj",
      "--nologo",
      "-t:ResolveReferences",
      "-getProperty:LangVersion,DefineConstants",
      "-getItem:ReferencePath,Analyzer,Compile",
    ],
    implemented: true,
  },
  Case {
    name: "restore_cold",
    kind: CaseKind::RestoreCold,
    args: &["restore", "--nologo", "--verbosity", "quiet"],
    implemented: true,
  },
  Case {
    name: "package_sync_cold",
    kind: CaseKind::PackageSyncCold,
    args: &[
      "restore",
      "PackageConsole.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_sync_warm",
    kind: CaseKind::PackageSyncWarm,
    args: &[
      "restore",
      "PackageConsole.csproj",
      "--locked-mode",
      "--packages",
      ".packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_reference_metadata",
    kind: CaseKind::PackageReferenceMetadata,
    args: &[
      "restore",
      "MetadataProject.csproj",
      "--locked-mode",
      "--packages",
      ".packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_pruning",
    kind: CaseKind::PackagePruning,
    args: &[
      "restore",
      "LegacyPruningProject.csproj",
      "--locked-mode",
      "--packages",
      ".packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_signature_cold",
    kind: CaseKind::PackageSignatureCold,
    args: &[
      "restore",
      "PackageSignatures.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_signature_warm",
    kind: CaseKind::PackageSignatureWarm,
    args: &[
      "restore",
      "PackageSignatures.csproj",
      "--locked-mode",
      "--packages",
      ".packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuspec_framework_metadata",
    kind: CaseKind::NuspecFrameworkMetadata,
    args: &[
      "restore",
      "FrameworkMetadata.csproj",
      "--packages",
      ".packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_rid_content_cold",
    kind: CaseKind::PackageRidContentCold,
    args: &[
      "restore",
      "WindowsFallback.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_rid_content_warm",
    kind: CaseKind::PackageRidContentWarm,
    args: &[
      "restore",
      "WindowsFallback.csproj",
      "--locked-mode",
      "--packages",
      ".packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "central_package_management",
    kind: CaseKind::CentralPackageManagement,
    args: &[
      "restore",
      "CentralPackages.csproj",
      "--locked-mode",
      "--packages",
      ".packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_conflict_resolution",
    kind: CaseKind::PackageConflictResolution,
    args: &[
      "restore",
      "ConflictResolution.csproj",
      "--packages",
      ".packages",
      "-p:NoWarn=NU1605",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_batch_resolution",
    kind: CaseKind::PackageBatchResolution,
    args: &["restore", "PackageBatch.csproj", "--packages", ".packages", "--nologo", "--verbosity", "quiet"],
    implemented: true,
  },
  Case {
    name: "package_diagnostics",
    kind: CaseKind::PackageDiagnostics,
    args: &[
      "restore",
      "ConflictFailure.csproj",
      "--packages",
      ".packages",
      "--nologo",
      "--verbosity",
      "minimal",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_config_hierarchy",
    kind: CaseKind::NugetConfigHierarchy,
    args: &[
      "restore",
      "ConfigHierarchy.csproj",
      "--locked-mode",
      "--no-http-cache",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_config_merge",
    kind: CaseKind::NugetConfigMerge,
    args: &[
      "restore",
      "ConfigMerge.csproj",
      "--locked-mode",
      "--no-http-cache",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_source_sections",
    kind: CaseKind::NugetSourceSections,
    args: &[
      "restore",
      "SourceSections.csproj",
      "--locked-mode",
      "--no-http-cache",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_source_mapping",
    kind: CaseKind::NugetSourceMapping,
    args: &[
      "restore",
      "SourceMapping.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_request_budget",
    kind: CaseKind::NugetRequestBudget,
    args: &[
      "restore",
      "RequestBudget.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_source_telemetry",
    kind: CaseKind::NugetSourceTelemetry,
    args: &[
      "restore",
      "RequestBudget.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_storage_policy",
    kind: CaseKind::NugetStoragePolicy,
    args: &[
      "restore",
      "StoragePolicy.csproj",
      "--locked-mode",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_cli_overrides",
    kind: CaseKind::NugetCliOverrides,
    args: &[
      "restore",
      "CliOverrides.csproj",
      "--locked-mode",
      "--source",
      "https://api.nuget.org/v3/index.json",
      "--configfile",
      "config/selected.config",
      "--packages",
      "policy/cli-global",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_local_sources",
    kind: CaseKind::NugetLocalSources,
    args: &[
      "restore",
      "LocalSources.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_floating_version",
    kind: CaseKind::NugetFloatingVersion,
    args: &[
      "restore",
      "FloatingVersion.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_service_index",
    kind: CaseKind::NugetServiceIndex,
    args: &["oracle/bin/Release/ServiceIndexOracle.dll", "https://api.nuget.org/v3/index.json"],
    implemented: true,
  },
  Case {
    name: "nuget_credentials",
    kind: CaseKind::NugetCredentials,
    args: &["oracle/bin/Release/CredentialOracle.dll", "."],
    implemented: true,
  },
  Case {
    name: "nuget_credential_provider",
    kind: CaseKind::NugetCredentialProvider,
    args: &["oracle/bin/Release/CredentialProviderOracle.dll", "https://private.example.test/v3/index.json"],
    implemented: true,
  },
  Case {
    name: "nuget_client_certificates",
    kind: CaseKind::NugetClientCertificates,
    args: &["oracle/bin/Release/ClientCertificateOracle.dll", "query", "."],
    implemented: true,
  },
  Case {
    name: "nuget_http_policy",
    kind: CaseKind::NugetHttpPolicy,
    args: &["oracle/bin/Release/HttpPolicyOracle.dll", "."],
    implemented: true,
  },
  Case {
    name: "nuget_source_security",
    kind: CaseKind::NugetSourceSecurity,
    args: &["oracle/bin/Release/SourceSecurityOracle.dll", "."],
    implemented: true,
  },
  Case {
    name: "package_graph_cold",
    kind: CaseKind::PackageGraphCold,
    args: &[
      "restore",
      "LargePackageGraph.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_graph_massive",
    kind: CaseKind::PackageGraphMassive,
    args: &[
      "restore",
      "MassivePackageGraph.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "package_asset_plan",
    kind: CaseKind::PackageAssetPlan,
    args: &[
      "restore",
      "MassivePackageGraph.csproj",
      "--locked-mode",
      "--packages",
      ".packages",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "build_clean",
    kind: CaseKind::BuildClean,
    args: &["build", "--no-restore", "--nologo", "--verbosity", "quiet"],
    implemented: true,
  },
  Case {
    name: "build_noop",
    kind: CaseKind::BuildNoOp,
    args: &["build", "--no-restore", "--nologo", "--verbosity", "quiet"],
    implemented: true,
  },
  Case {
    name: "run_warm",
    kind: CaseKind::RunWarm,
    args: &["run", "--no-build", "--no-restore"],
    implemented: true,
  },
];

const DV_CASES: &[Case] = &[
  Case {
    name: "sdk_current",
    kind: CaseKind::Startup,
    args: &["sdk", "current"],
    implemented: true,
  },
  Case {
    name: "sdk_current_globals",
    kind: CaseKind::Startup,
    args: &["sdk", "--quiet", "--no-color", "current"],
    implemented: true,
  },
  Case {
    name: "sdk_current_compat",
    kind: CaseKind::Startup,
    args: &["--compat", "dotnet", "--version"],
    implemented: true,
  },
  Case {
    name: "cli_golden_trace",
    kind: CaseKind::CliGoldenTrace,
    args: &[
      "restore",
      "SmallConsole.csproj",
      "--packages",
      ".packages",
      "--source",
      "offline-source",
      "--verbosity",
      "quiet",
    ],
    implemented: true,
  },
  Case {
    name: "cli_compat_help",
    kind: CaseKind::Startup,
    args: &["--compat", "dotnet", "build", "-?"],
    implemented: true,
  },
  Case {
    name: "cli_cancellation",
    kind: CaseKind::Startup,
    args: &["sdk", "current"],
    implemented: true,
  },
  Case {
    name: "cli_unknown_option",
    kind: CaseKind::CliUnknownOption,
    args: &["build", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_option_effects",
    kind: CaseKind::CliOptionEffects,
    args: &["--compat", "dotnet", "test", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "dotnet_runtime_inventory",
    kind: CaseKind::CliDriverInventory,
    args: &["--compat", "dotnet", "--list-runtimes"],
    implemented: true,
  },
  Case {
    name: "cli_command_normalization",
    kind: CaseKind::CliCommandNormalization,
    args: &["sync", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_transform_equivalence",
    kind: CaseKind::CliTransformEquivalence,
    args: &["--compat", "dotnet", "restore", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_mode_classification",
    kind: CaseKind::CliModeClassification,
    args: &["--compat", "dotnet", "build", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_exit_policy",
    kind: CaseKind::CliExitPolicy,
    args: &["--compat", "dotnet", "restore", "DefinitelyMissing.csproj"],
    implemented: true,
  },
  Case {
    name: "cli_lexical_preservation",
    kind: CaseKind::CliLexicalPreservation,
    args: &["--compat", "dotnet", "build", "-c:Release", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_route_precedence",
    kind: CaseKind::CliRoutePrecedence,
    args: &["--compat", "dotnet", "pack", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_environment",
    kind: CaseKind::CliEnvironment,
    args: &["build", "--definitely-unknown"],
    implemented: true,
  },
  Case {
    name: "cli_forwarding",
    kind: CaseKind::CliForwarding,
    args: &["--json", "run", "--", "alpha", "", "--color", "two words"],
    implemented: true,
  },
  Case {
    name: "cli_child_exit",
    kind: CaseKind::CliChildExit,
    args: &["--json", "run", "--", "exit", "23"],
    implemented: true,
  },
  Case {
    name: "rid_graph",
    kind: CaseKind::RidGraph,
    args: &["sdk", "compatible-rids", "linux-musl-x64"],
    implemented: true,
  },
  Case {
    name: "project_evaluate",
    kind: CaseKind::ProjectEvaluate,
    args: &["project", "inspect", "SmallConsole.csproj", "--json"],
    implemented: true,
  },
  Case {
    name: "project_select_named",
    kind: CaseKind::ProjectEvaluate,
    args: &["project", "inspect", "--project", "SmallConsole.csproj", "--json"],
    implemented: true,
  },
  Case {
    name: "package_reference_conditions",
    kind: CaseKind::PackageReferenceConditions,
    args: &["project", "inspect", "ConditionalReferences.csproj", "--configuration", "Release", "--json"],
    implemented: true,
  },
  Case {
    name: "runtime_evaluate",
    kind: CaseKind::RuntimeEvaluate,
    args: &["project", "inspect", "RuntimeProject.csproj", "--json"],
    implemented: true,
  },
  Case {
    name: "runtime_pack_plan",
    kind: CaseKind::RuntimePackPlan,
    args: &["project", "runtime-packs", "RuntimePackProject.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "runtime_pack_inventory_cold",
    kind: CaseKind::RuntimePackInventoryCold,
    args: &["project", "runtime-packs", "RuntimePackProject.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "framework_reference_plan",
    kind: CaseKind::FrameworkReferencePlan,
    args: &["project", "frameworks", "FrameworkReferenceProject.csproj", "--json"],
    implemented: true,
  },
  Case {
    name: "pack_diagnostic",
    kind: CaseKind::PackDiagnostic,
    args: &["project", "runtime-packs", "UnavailablePackProject.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "compiler_plan",
    kind: CaseKind::CompilerPlan,
    args: &["build", "--plan", "SmallConsole.csproj", "--json"],
    implemented: true,
  },
  Case {
    name: "cli_version",
    kind: CaseKind::Startup,
    args: &["--version"],
    implemented: true,
  },
  Case {
    name: "cli_protocol_version",
    kind: CaseKind::Startup,
    args: &["--json", "--version"],
    implemented: true,
  },
  Case {
    name: "cli_compat_manifest",
    kind: CaseKind::Startup,
    args: &["compat", "manifest"],
    implemented: true,
  },
  Case {
    name: "cli_compat_check",
    kind: CaseKind::CliCompatCheck,
    args: &["--json", "compat", "check", "ci.yml", "SmallConsole.csproj"],
    implemented: true,
  },
  Case {
    name: "sync_cold",
    kind: CaseKind::RestoreCold,
    args: &["sync", "--json"],
    implemented: true,
  },
  Case {
    name: "package_sync_cold",
    kind: CaseKind::PackageSyncCold,
    args: &["restore", "PackageConsole.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "package_sync_warm",
    kind: CaseKind::PackageSyncWarm,
    args: &["restore", "PackageConsole.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "package_reference_metadata",
    kind: CaseKind::PackageReferenceMetadata,
    args: &["restore", "MetadataProject.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "package_pruning",
    kind: CaseKind::PackagePruning,
    args: &["restore", "LegacyPruningProject.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "package_signature_cold",
    kind: CaseKind::PackageSignatureCold,
    args: &["restore", "PackageSignatures.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "package_signature_warm",
    kind: CaseKind::PackageSignatureWarm,
    args: &["restore", "PackageSignatures.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuspec_framework_metadata",
    kind: CaseKind::NuspecFrameworkMetadata,
    args: &["restore", "FrameworkMetadata.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "package_rid_content_cold",
    kind: CaseKind::PackageRidContentCold,
    args: &["restore", "WindowsFallback.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "package_rid_content_warm",
    kind: CaseKind::PackageRidContentWarm,
    args: &["restore", "WindowsFallback.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "central_package_management",
    kind: CaseKind::CentralPackageManagement,
    args: &["restore", "CentralPackages.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "package_conflict_resolution",
    kind: CaseKind::PackageConflictResolution,
    args: &["restore", "ConflictResolution.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "package_batch_resolution",
    kind: CaseKind::PackageBatchResolution,
    args: &["restore", "PackageBatch.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "package_diagnostics",
    kind: CaseKind::PackageDiagnostics,
    args: &["restore", "ConflictFailure.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_config_hierarchy",
    kind: CaseKind::NugetConfigHierarchy,
    args: &["restore", "ConfigHierarchy.csproj", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_config_merge",
    kind: CaseKind::NugetConfigMerge,
    args: &["restore", "ConfigMerge.csproj", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_source_sections",
    kind: CaseKind::NugetSourceSections,
    args: &["restore", "SourceSections.csproj", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_source_mapping",
    kind: CaseKind::NugetSourceMapping,
    args: &["restore", "SourceMapping.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_request_budget",
    kind: CaseKind::NugetRequestBudget,
    args: &["restore", "RequestBudget.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_source_telemetry",
    kind: CaseKind::NugetSourceTelemetry,
    args: &["restore", "RequestBudget.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_storage_policy",
    kind: CaseKind::NugetStoragePolicy,
    args: &["restore", "StoragePolicy.csproj", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_cli_overrides",
    kind: CaseKind::NugetCliOverrides,
    args: &[
      "restore",
      "CliOverrides.csproj",
      "--source",
      "https://api.nuget.org/v3/index.json",
      "--configfile",
      "config/selected.config",
      "--packages",
      "policy/cli-global",
      "--offline",
      "--json",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_local_sources",
    kind: CaseKind::NugetLocalSources,
    args: &["restore", "LocalSources.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_floating_version",
    kind: CaseKind::NugetFloatingVersion,
    args: &["restore", "FloatingVersion.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_service_index",
    kind: CaseKind::NugetServiceIndex,
    args: &["project", "package-sources", "ServiceIndex.csproj", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_credentials",
    kind: CaseKind::NugetCredentials,
    args: &["project", "package-sources", "CredentialProject.csproj", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_credential_provider",
    kind: CaseKind::NugetCredentialProvider,
    args: &[
      "project",
      "package-sources",
      "CredentialProviderProject.csproj",
      "--offline",
      "--probe-credentials",
      "--json",
    ],
    implemented: true,
  },
  Case {
    name: "nuget_client_certificates",
    kind: CaseKind::NugetClientCertificates,
    args: &["project", "package-sources", "ClientCertificateProject.csproj", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_http_policy",
    kind: CaseKind::NugetHttpPolicy,
    args: &["project", "package-sources", "HttpPolicyProject.csproj", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "nuget_source_security",
    kind: CaseKind::NugetSourceSecurity,
    args: &["project", "package-sources", "SecurityProject.csproj", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "package_graph_cold",
    kind: CaseKind::PackageGraphCold,
    args: &["restore", "LargePackageGraph.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "package_graph_massive",
    kind: CaseKind::PackageGraphMassive,
    args: &["restore", "MassivePackageGraph.csproj", "--packages", ".packages", "--json"],
    implemented: true,
  },
  Case {
    name: "package_asset_plan",
    kind: CaseKind::PackageAssetPlan,
    args: &["restore", "MassivePackageGraph.csproj", "--packages", ".packages", "--offline", "--json"],
    implemented: true,
  },
  Case {
    name: "build_clean",
    kind: CaseKind::BuildClean,
    args: &["build"],
    implemented: false,
  },
  Case {
    name: "build_noop",
    kind: CaseKind::BuildNoOp,
    args: &["build"],
    implemented: false,
  },
  Case {
    name: "run_warm",
    kind: CaseKind::RunWarm,
    args: &["run"],
    implemented: false,
  },
];

struct Options {
  warmups: usize,
  samples: usize,
  output: Option<PathBuf>,
  dv: Option<PathBuf>,
  case: Option<String>,
}

#[derive(Serialize)]
struct Report {
  schema_version: u16,
  generated_unix_seconds: u64,
  environment: Environment,
  runs: Vec<Run>,
}

#[derive(Serialize)]
struct Environment {
  os: &'static str,
  arch: &'static str,
  logical_cpus: usize,
  repository_commit: Option<String>,
}

#[derive(Serialize)]
struct Run {
  tool: String,
  tool_version: String,
  fixture: Option<String>,
  case: String,
  command: Vec<String>,
  status: RunStatus,
  warmups: usize,
  samples_ns: Vec<u64>,
  statistics_ns: Option<Statistics>,
  #[serde(skip_serializing_if = "Option::is_none")]
  network_requests: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  downloaded_bytes: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  downloaded_packages: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  resolved_packages: Option<u64>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunStatus {
  Measured,
  Tbi,
}

#[derive(Serialize)]
struct Statistics {
  min: u64,
  median: u64,
  p95: u64,
  max: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorkEvidence {
  network_requests: Option<u64>,
  downloaded_bytes: Option<u64>,
  downloaded_packages: Option<u64>,
  resolved_packages: Option<u64>,
}

struct Measurement {
  elapsed_ns: u64,
  work: Option<WorkEvidence>,
}

fn main() {
  if let Err(error) = run() {
    eprintln!("benchmark failed: {error}");
    std::process::exit(1);
  }
}

fn run() -> Result<()> {
  let options = parse_options(env::args_os().skip(1))?;
  let repository = repository_root();
  let fixture = repository.join("benchmarks/fixtures/small-console");
  let argument_forwarding_fixture = repository.join("benchmarks/fixtures/argument-forwarding");
  let rid_graph_fixture = repository.join("benchmarks/fixtures/rid-graph-oracle");
  let runtime_fixture = repository.join("benchmarks/fixtures/runtime-project");
  let runtime_pack_fixture = repository.join("benchmarks/fixtures/runtime-pack-project");
  let framework_reference_fixture = repository.join("benchmarks/fixtures/framework-reference-project");
  let unavailable_pack_fixture = repository.join("benchmarks/fixtures/unavailable-pack-project");
  let package_fixture = repository.join("benchmarks/fixtures/package-console");
  let package_reference_metadata_fixture = repository.join("benchmarks/fixtures/package-reference-metadata");
  let package_signature_fixture = repository.join("benchmarks/fixtures/package-signatures");
  let nuspec_framework_metadata_fixture = repository.join("benchmarks/fixtures/nuspec-framework-metadata");
  let package_rid_content_fixture = repository.join("benchmarks/fixtures/package-rid-content");
  let central_package_management_fixture = repository.join("benchmarks/fixtures/central-package-management");
  let package_conflict_resolution_fixture = repository.join("benchmarks/fixtures/package-conflict-resolution");
  let package_reference_conditions_fixture = repository.join("benchmarks/fixtures/package-reference-conditions");
  let nuget_config_fixture = repository.join("benchmarks/fixtures/nuget-config-hierarchy");
  let nuget_config_merge_fixture = repository.join("benchmarks/fixtures/nuget-config-merge");
  let nuget_source_sections_fixture = repository.join("benchmarks/fixtures/nuget-source-sections");
  let nuget_source_mapping_fixture = repository.join("benchmarks/fixtures/nuget-source-mapping");
  let nuget_request_budget_fixture = repository.join("benchmarks/fixtures/nuget-request-budget");
  let nuget_storage_policy_fixture = repository.join("benchmarks/fixtures/nuget-storage-policy");
  let nuget_cli_overrides_fixture = repository.join("benchmarks/fixtures/nuget-cli-overrides");
  let nuget_local_sources_fixture = repository.join("benchmarks/fixtures/nuget-local-sources");
  let nuget_floating_version_fixture = repository.join("benchmarks/fixtures/nuget-floating-version");
  let nuget_service_index_fixture = repository.join("benchmarks/fixtures/nuget-service-index");
  let nuget_credentials_fixture = repository.join("benchmarks/fixtures/nuget-credentials");
  let nuget_credential_provider_fixture = repository.join("benchmarks/fixtures/nuget-credential-provider");
  let nuget_client_certificates_fixture = repository.join("benchmarks/fixtures/nuget-client-certificates");
  let nuget_http_policy_fixture = repository.join("benchmarks/fixtures/nuget-http-policy");
  let nuget_source_security_fixture = repository.join("benchmarks/fixtures/nuget-source-security");
  let package_graph_fixture = repository.join("benchmarks/fixtures/large-package-graph");
  let massive_package_graph_fixture = repository.join("benchmarks/fixtures/massive-package-graph");
  let fixtures = Fixtures {
    small: &fixture,
    argument_forwarding: &argument_forwarding_fixture,
    rid_graph: &rid_graph_fixture,
    runtime: &runtime_fixture,
    runtime_pack: &runtime_pack_fixture,
    framework_reference: &framework_reference_fixture,
    unavailable_pack: &unavailable_pack_fixture,
    package: &package_fixture,
    package_reference_metadata: &package_reference_metadata_fixture,
    package_signature: &package_signature_fixture,
    nuspec_framework_metadata: &nuspec_framework_metadata_fixture,
    package_rid_content: &package_rid_content_fixture,
    central_package_management: &central_package_management_fixture,
    package_conflict_resolution: &package_conflict_resolution_fixture,
    package_reference_conditions: &package_reference_conditions_fixture,
    nuget_config: &nuget_config_fixture,
    nuget_config_merge: &nuget_config_merge_fixture,
    nuget_source_sections: &nuget_source_sections_fixture,
    nuget_source_mapping: &nuget_source_mapping_fixture,
    nuget_request_budget: &nuget_request_budget_fixture,
    nuget_storage_policy: &nuget_storage_policy_fixture,
    nuget_cli_overrides: &nuget_cli_overrides_fixture,
    nuget_local_sources: &nuget_local_sources_fixture,
    nuget_floating_version: &nuget_floating_version_fixture,
    nuget_service_index: &nuget_service_index_fixture,
    nuget_credentials: &nuget_credentials_fixture,
    nuget_credential_provider: &nuget_credential_provider_fixture,
    nuget_client_certificates: &nuget_client_certificates_fixture,
    nuget_http_policy: &nuget_http_policy_fixture,
    nuget_source_security: &nuget_source_security_fixture,
    package_graph: &package_graph_fixture,
    package_graph_massive: &massive_package_graph_fixture,
  };
  let workspace = repository.join(format!("target/benchmark-work-{}", std::process::id()));
  let dv_executable = prepare_dv_executable(&repository, options.dv.as_deref())?;
  ensure_workspace_is_safe(&repository, &workspace)?;
  if options
    .case
    .as_deref()
    .is_none_or(|case| matches!(case, "sdk_current" | "sdk_current_compat" | "cli_cancellation"))
  {
    verify_sdk_selection(&dv_executable, &fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_golden_trace") {
    verify_golden_trace_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-golden-trace"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_cancellation") {
    verify_cancellation_boundary(&dv_executable, &fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_protocol_version") {
    verify_protocol_version_boundary(&dv_executable, &fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_compat_manifest") {
    verify_compatibility_manifest_boundary(&dv_executable, &fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_compat_check") {
    verify_compatibility_check_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-compat-check"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_compat_help") {
    verify_compatibility_help_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-compat-help"))?;
  }
  if options.case.as_deref() == Some("cli_cancellation") {
    let provider_workspace = repository.join(format!("target/benchmark-cli-cancellation-verification-{}", std::process::id()));
    ensure_workspace_is_safe(&repository, &provider_workspace)?;
    reset_fixture(&nuget_credential_provider_fixture, &provider_workspace)?;
    verify_credential_provider_timeout(&dv_executable, &provider_workspace)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_unknown_option") {
    verify_unknown_option_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-unknown-option"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_option_effects") {
    verify_option_effect_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-option-effects"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "dotnet_runtime_inventory") {
    verify_driver_inventory_boundary(&dv_executable, &fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_command_normalization") {
    verify_command_normalization_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-command-normalization"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_transform_equivalence") {
    verify_transform_equivalence_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-transform-equivalence"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_mode_classification") {
    verify_mode_classification_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-mode-classification"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_exit_policy") {
    verify_exit_policy_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-exit-policy"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_lexical_preservation") {
    verify_lexical_preservation_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-lexical-preservation"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_route_precedence") {
    verify_route_precedence_boundary(&dv_executable, &fixture, &workspace.join("verify-cli-route-precedence"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_environment") {
    verify_environment_boundary(
      &dv_executable,
      &fixture,
      &argument_forwarding_fixture,
      &workspace.join("verify-cli-environment"),
    )?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_forwarding") {
    verify_forwarding_boundary(&dv_executable, &argument_forwarding_fixture, &workspace.join("verify-cli-forwarding"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "cli_child_exit") {
    verify_child_exit_boundary(&dv_executable, &argument_forwarding_fixture, &workspace.join("verify-cli-child-exit"))?;
  }
  if options.case.as_deref().is_none_or(|case| case == "rid_graph") {
    verify_rid_graph(&repository, &dv_executable, &rid_graph_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "project_evaluate") {
    verify_project_evaluation(&dv_executable, &fixture, false)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "project_select_named") {
    verify_project_evaluation(&dv_executable, &fixture, true)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "package_reference_conditions") {
    verify_package_reference_conditions(&dv_executable, &package_reference_conditions_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "runtime_evaluate") {
    verify_runtime_evaluation(&repository, &dv_executable, &runtime_fixture)?;
  }
  if options
    .case
    .as_deref()
    .is_none_or(|case| matches!(case, "runtime_pack_plan" | "runtime_pack_inventory_cold"))
  {
    verify_runtime_pack_plan(&repository, &dv_executable, &runtime_pack_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "framework_reference_plan") {
    verify_framework_reference_plan(&repository, &dv_executable, &framework_reference_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "pack_diagnostic") {
    verify_pack_diagnostic(&repository, &dv_executable, &unavailable_pack_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "compiler_plan") {
    verify_compiler_plan(&repository, &dv_executable, &fixture)?;
  }
  if options
    .case
    .as_deref()
    .is_none_or(|case| matches!(case, "package_sync_cold" | "package_sync_warm"))
  {
    verify_package_sync(&repository, &dv_executable, &package_fixture, "PackageConsole.csproj", 1)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "package_reference_metadata") {
    verify_package_reference_metadata(&repository, &dv_executable, &package_reference_metadata_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "package_pruning") {
    verify_package_pruning(&repository, &dv_executable, &package_reference_metadata_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuspec_framework_metadata") {
    verify_nuspec_framework_metadata(&repository, &dv_executable, &nuspec_framework_metadata_fixture)?;
  }
  if options
    .case
    .as_deref()
    .is_none_or(|case| matches!(case, "package_rid_content_cold" | "package_rid_content_warm"))
  {
    verify_package_rid_content(&repository, &dv_executable, &package_rid_content_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "central_package_management") {
    verify_central_package_management(&repository, &dv_executable, &central_package_management_fixture)?;
  }
  if options
    .case
    .as_deref()
    .is_none_or(|case| matches!(case, "package_conflict_resolution" | "package_batch_resolution" | "package_diagnostics"))
  {
    verify_package_conflict_resolution(&repository, &dv_executable, &package_conflict_resolution_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "package_batch_resolution") {
    verify_package_batch_resolution(&repository, &dv_executable, &package_conflict_resolution_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "package_diagnostics") {
    verify_package_diagnostics(&repository, &dv_executable, &package_conflict_resolution_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_config_hierarchy") {
    verify_nuget_config_hierarchy(&repository, &dv_executable, &nuget_config_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_config_merge") {
    verify_nuget_config_merge(&repository, &dv_executable, &nuget_config_merge_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_source_sections") {
    verify_nuget_source_sections(&repository, &dv_executable, &nuget_source_sections_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_source_mapping") {
    verify_nuget_source_mapping(&repository, &dv_executable, &nuget_source_mapping_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_storage_policy") {
    verify_nuget_storage_policy(&repository, &dv_executable, &nuget_storage_policy_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_cli_overrides") {
    verify_nuget_cli_overrides(&repository, &dv_executable, &nuget_cli_overrides_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_local_sources") {
    verify_nuget_local_sources(&repository, &dv_executable, &nuget_local_sources_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_floating_version") {
    verify_nuget_floating_version(&repository, &dv_executable, &nuget_floating_version_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_service_index") {
    verify_nuget_service_index(&repository, &dv_executable, &nuget_service_index_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_credentials") {
    verify_nuget_credentials(&repository, &dv_executable, &nuget_credentials_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_credential_provider") {
    verify_nuget_credential_provider(&repository, &dv_executable, &nuget_credential_provider_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_client_certificates") {
    verify_nuget_client_certificates(&repository, &dv_executable, &nuget_client_certificates_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_http_policy") {
    verify_nuget_http_policy(&repository, &dv_executable, &nuget_http_policy_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_source_security") {
    verify_nuget_source_security(&repository, &dv_executable, &nuget_source_security_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "package_graph_cold") {
    verify_package_sync(&repository, &dv_executable, &package_graph_fixture, "LargePackageGraph.csproj", 50)?;
  }
  if options
    .case
    .as_deref()
    .is_none_or(|case| matches!(case, "package_graph_massive" | "package_asset_plan"))
  {
    verify_package_sync(&repository, &dv_executable, &massive_package_graph_fixture, "MassivePackageGraph.csproj", 203)?;
  }

  let mut runs = run_tool("dotnet", Path::new("dotnet"), DOTNET_CASES, &options, &fixtures, &workspace.join("dotnet"))?;
  runs.extend(run_tool("dv", &dv_executable, DV_CASES, &options, &fixtures, &workspace.join("dv"))?);
  if runs.is_empty() {
    return Err(format!("no benchmark case named {:?}", options.case.as_deref().unwrap_or_default()).into());
  }

  let generated_unix_seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
  let report = Report {
    schema_version: 22,
    generated_unix_seconds,
    environment: Environment {
      os: env::consts::OS,
      arch: env::consts::ARCH,
      logical_cpus: std::thread::available_parallelism().map(usize::from).unwrap_or(1),
      repository_commit: command_text(Path::new("git"), &["rev-parse", "--short=12", "HEAD"], &repository).ok(),
    },
    runs,
  };

  print_summary(&report);
  let output = options
    .output
    .unwrap_or_else(|| repository.join("benchmarks/results").join(format!("baseline-{generated_unix_seconds}.json")));
  if let Some(parent) = output.parent() {
    fs::create_dir_all(parent)?;
  }
  fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
  println!("\n  Raw samples  {}", output.display());
  Ok(())
}

fn verify_sdk_selection(dv_executable: &Path, fixture: &Path) -> Result<()> {
  let dotnet_version = command_text(Path::new("dotnet"), &["--version"], fixture)?;
  let dv_version = command_text(dv_executable, &["sdk", "current"], fixture)?;
  if dotnet_version != dv_version {
    return Err(format!("SDK selection mismatch: dotnet selected {dotnet_version:?}, dv selected {dv_version:?}").into());
  }
  let compatibility_version = command_text(dv_executable, &["--compat", "dotnet", "--version"], fixture)?;
  if dotnet_version != compatibility_version {
    return Err(format!("compatibility SDK selection mismatch: dotnet selected {dotnet_version:?}, dv selected {compatibility_version:?}").into());
  }
  Ok(())
}

fn verify_golden_trace_boundary(dv_executable: &Path, fixture: &Path, workspace: &Path) -> Result<()> {
  let trace: GoldenTraceFile = serde_json::from_str(PHASE_ONE_CI_TRACE)?;
  validate_golden_trace_contract(&trace)?;
  for case in &trace.cases {
    let reference_workspace = workspace.join(format!("{}-reference", case.id));
    let candidate_workspace = workspace.join(format!("{}-candidate", case.id));
    prepare_golden_trace_workspace(fixture, &reference_workspace, case)?;
    prepare_golden_trace_workspace(fixture, &candidate_workspace, case)?;
    let reference_before = snapshot_tree(&reference_workspace)?;
    let candidate_before = snapshot_tree(&candidate_workspace)?;
    if reference_before != candidate_before {
      return Err(format!("{} did not start both tools from an identical filesystem", case.id).into());
    }

    let reference = run_golden_trace_command(Path::new("dotnet"), &case.reference_argv[1..], &case.stdin_utf8, &reference_workspace)?;
    let selected_sdk = (case.id == "github-actions-sdk-selection")
      .then(|| std::str::from_utf8(&reference.stdout).map(|text| text.trim_end_matches(['\r', '\n']).to_owned()))
      .transpose()?;
    let reference_after = snapshot_tree(&reference_workspace)?;
    validate_golden_observation(
      &reference,
      &case.expected.reference,
      &reference_workspace,
      &reference_before,
      &reference_after,
      selected_sdk.as_deref(),
    )?;

    let candidate = run_golden_trace_command(dv_executable, &case.candidate_argv[1..], &case.stdin_utf8, &candidate_workspace)?;
    let candidate_after = snapshot_tree(&candidate_workspace)?;
    validate_golden_observation(
      &candidate,
      &case.expected.candidate,
      &candidate_workspace,
      &candidate_before,
      &candidate_after,
      selected_sdk.as_deref(),
    )?;
  }
  Ok(())
}

fn validate_golden_trace_contract(trace: &GoldenTraceFile) -> Result<()> {
  if trace.schema_version != 1
    || trace.cases.len() != 2
    || trace.cases[0].id != "github-actions-sdk-selection"
    || trace.cases[1].id != "github-actions-offline-restore"
  {
    return Err("Phase 1 CI trace must contain the ordered schema-version-1 SDK and restore cases".into());
  }
  for case in &trace.cases {
    if case.source != "compatibility/scripts/phase1-github-actions.yml"
      || !case.stdin_utf8.is_empty()
      || case.input_filesystem != "identical"
      || case.reference_argv.first().map(String::as_str) != Some("dotnet")
      || case.candidate_argv.first().map(String::as_str) != Some("dv")
      || case.environment_overrides.len() != GOLDEN_TRACE_ENVIRONMENT.len()
      || !case
        .environment_overrides
        .iter()
        .zip(GOLDEN_TRACE_ENVIRONMENT)
        .all(|(actual, expected)| actual.name == expected.0 && actual.value == expected.1)
      || [&case.expected.reference, &case.expected.candidate]
        .into_iter()
        .any(|expected| expected.process_trace != "TBI" || expected.network_trace != "TBI")
    {
      return Err(format!("{} drifted from the schema-version-1 trace contract", case.id).into());
    }
    let reference_command = case.reference_argv.join(" ");
    let expected_script_line = format!("run: {reference_command}");
    if !PHASE_ONE_CI_SCRIPT
      .lines()
      .any(|line| line.trim().strip_prefix("- ") == Some(expected_script_line.as_str()))
    {
      return Err(format!("{} is not present in the checked-in CI script fixture", case.id).into());
    }
  }
  if trace.cases[0].reference_argv.as_ref() != ["dotnet", "--version"]
    || trace.cases[0].candidate_argv.as_ref() != ["dv", "--compat", "dotnet", "--version"]
    || trace.cases[1].reference_argv[1..] != trace.cases[1].candidate_argv[1..]
  {
    return Err("the restore trace must change only the executable token; the SDK query must use its explicit compatibility form".into());
  }
  Ok(())
}

fn prepare_golden_trace_workspace(fixture: &Path, workspace: &Path, case: &GoldenTraceCase) -> Result<()> {
  reset_fixture(fixture, workspace)?;
  if case.id == "github-actions-offline-restore" {
    fs::create_dir(workspace.join("offline-source"))?;
  }
  Ok(())
}

fn run_golden_trace_command(executable: &Path, arguments: &[String], stdin: &str, cwd: &Path) -> Result<Output> {
  let mut command = Command::new(executable);
  command
    .args(arguments)
    .current_dir(cwd)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  apply_golden_trace_environment(&mut command, cwd);
  let mut child = command.spawn()?;
  child.stdin.take().expect("piped stdin exists").write_all(stdin.as_bytes())?;
  Ok(child.wait_with_output()?)
}

fn apply_golden_trace_environment(command: &mut Command, workspace: &Path) {
  for (name, value) in GOLDEN_TRACE_ENVIRONMENT {
    if let Some(suffix) = value.strip_prefix("$WORKSPACE") {
      command.env(name, format!("{}{suffix}", workspace.display()));
    } else {
      command.env(name, value);
    }
  }
}

fn validate_golden_observation(
  output: &Output,
  expected: &GoldenObservation,
  workspace: &Path,
  before: &TreeSnapshot,
  after: &TreeSnapshot,
  selected_sdk: Option<&str>,
) -> Result<()> {
  let stdout = normalize_golden_text(&output.stdout, workspace)?;
  let stderr = normalize_golden_text(&output.stderr, workspace)?;
  let expected_stdout = expand_golden_text(&expected.stdout_utf8, selected_sdk)?;
  let expected_stderr = expand_golden_text(&expected.stderr_utf8, selected_sdk)?;
  if output.status.code() != Some(expected.exit_code) || stdout != expected_stdout || stderr != expected_stderr {
    return Err(
      format!(
        "golden observation mismatch: status={:?}, stdout={stdout:?}, stderr={stderr:?}",
        output.status.code()
      )
      .into(),
    );
  }
  let delta = golden_tree_delta(before, after);
  if delta.as_slice() != expected.filesystem_delta.as_ref() {
    return Err(format!("golden filesystem delta mismatch: {delta:?}").into());
  }
  Ok(())
}

fn normalize_golden_text(bytes: &[u8], workspace: &Path) -> Result<String> {
  let normalized = std::str::from_utf8(bytes)?.replace('\\', "/");
  let workspace = workspace.to_string_lossy().replace('\\', "/");
  Ok(normalized.replace(&workspace, "<WORKSPACE>"))
}

fn expand_golden_text(template: &str, selected_sdk: Option<&str>) -> Result<String> {
  let mut text = template.replace("$PLATFORM_EOL", if cfg!(windows) { "\r\n" } else { "\n" });
  if text.contains("$SELECTED_SDK") {
    text = text.replace("$SELECTED_SDK", selected_sdk.ok_or("selected-SDK placeholder used outside the SDK trace")?);
  }
  Ok(text)
}

fn golden_tree_delta(before: &TreeSnapshot, after: &TreeSnapshot) -> Vec<GoldenFileChange> {
  let mut delta = Vec::new();
  let mut before_index = 0;
  let mut after_index = 0;
  while before_index < before.len() || after_index < after.len() {
    match (before.get(before_index), after.get(after_index)) {
      (Some((before_path, before_bytes)), Some((after_path, after_bytes))) => match before_path.cmp(after_path) {
        std::cmp::Ordering::Less => {
          delta.push(golden_file_change(before_path, before_bytes, "deleted"));
          before_index += 1;
        },
        std::cmp::Ordering::Greater => {
          delta.push(golden_file_change(after_path, after_bytes, "created"));
          after_index += 1;
        },
        std::cmp::Ordering::Equal => {
          if before_bytes != after_bytes {
            delta.push(golden_file_change(after_path, after_bytes, "modified"));
          }
          before_index += 1;
          after_index += 1;
        },
      },
      (Some((path, bytes)), None) => {
        delta.push(golden_file_change(path, bytes, "deleted"));
        before_index += 1;
      },
      (None, Some((path, bytes))) => {
        delta.push(golden_file_change(path, bytes, "created"));
        after_index += 1;
      },
      (None, None) => break,
    }
  }
  delta
}

fn golden_file_change(path: &Path, bytes: &Option<Vec<u8>>, change: &str) -> GoldenFileChange {
  GoldenFileChange {
    path: path.to_string_lossy().replace('\\', "/"),
    kind: if bytes.is_some() { "file" } else { "directory" }.into(),
    change: change.into(),
  }
}

fn validate_golden_trace_sample(output: &Output, reference: bool) -> Result<()> {
  if !output.status.success() || !output.stderr.is_empty() {
    return Err("golden trace sample did not preserve the successful empty-stderr contract".into());
  }
  let text = std::str::from_utf8(&output.stdout)?;
  if reference {
    if !text.is_empty() {
      return Err(format!("reference golden restore emitted unexpected stdout: {text:?}").into());
    }
  } else if !text.starts_with("Package resolution\n")
    || !text.contains("  Packages       0\n")
    || !text.contains("  HTTP requests  0\n")
    || !text.contains("  Target         net10.0\n")
  {
    return Err(format!("candidate golden restore emitted invalid zero-package evidence: {text:?}").into());
  }
  Ok(())
}

fn verify_compatibility_help_boundary(dv_executable: &Path, fixture: &Path, workspace: &Path) -> Result<()> {
  reset_fixture(fixture, workspace)?;
  let before = snapshot_tree(workspace)?;
  let reference = Command::new("dotnet").args(["build", "-?"]).current_dir(workspace).output()?;
  validate_compatibility_help_output(&reference, true)?;
  let after_reference = snapshot_tree(workspace)?;
  if before != after_reference {
    return Err("dotnet compatibility-help preflight mutated the input workspace".into());
  }

  let candidate = Command::new(dv_executable)
    .args(["--compat", "dotnet", "build", "-?"])
    .current_dir(workspace)
    .output()?;
  validate_compatibility_help_output(&candidate, false)?;
  if before != snapshot_tree(workspace)? {
    return Err("dv compatibility-help preflight mutated the input workspace".into());
  }
  Ok(())
}

fn validate_compatibility_help_output(output: &Output, reference: bool) -> Result<()> {
  if !output.status.success() || !output.stderr.is_empty() {
    return Err(
      format!(
        "compatibility help returned status {:?} with stderr {:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
      )
      .into(),
    );
  }
  let stdout = String::from_utf8_lossy(&output.stdout);
  let valid = if reference {
    stdout.contains("dotnet build")
  } else {
    stdout.contains("dv --compat dotnet build") && stdout.contains("dv build --plan")
  };
  if !valid {
    return Err(format!("compatibility help omitted its expected syntax: {stdout}").into());
  }
  Ok(())
}

fn verify_cancellation_boundary(dv_executable: &Path, fixture: &Path) -> Result<()> {
  let output = Command::new(dv_executable).args(["--json", "run"]).current_dir(fixture).output()?;
  if output.status.code() != Some(2) || !output.stderr.is_empty() {
    return Err(
      format!(
        "dv cancellation preflight returned status {:?} with stderr {:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
      )
      .into(),
    );
  }
  let events = output
    .stdout
    .split(|byte| *byte == b'\n')
    .filter(|line| !line.is_empty())
    .map(serde_json::from_slice::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?;
  let diagnostic = events
    .iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("diagnostic"))
    .and_then(|event| event.get("diagnostic"))
    .ok_or("dv cancellation preflight omitted its diagnostic")?;
  let grace = diagnostic.get("context").and_then(serde_json::Value::as_array).and_then(|context| {
    context.iter().find_map(|field| {
      (field.get("name").and_then(serde_json::Value::as_str) == Some("cancellation_grace_ms"))
        .then(|| field.get("value").and_then(serde_json::Value::as_str))
        .flatten()
    })
  });
  if grace != Some("2000") {
    return Err(format!("dv child boundary reported the wrong cancellation grace: {grace:?}").into());
  }
  Ok(())
}

fn verify_unknown_option_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  const ARGS: &[&str] = &["build", "--definitely-unknown"];
  for (executable, name, reference) in [(Path::new("dotnet"), "dotnet", true), (dv_executable, "dv", false)] {
    let workspace = verification.join(name);
    reset_fixture(fixture, &workspace)?;
    let before = snapshot_tree(&workspace)?;
    let output = Command::new(executable).args(ARGS).current_dir(&workspace).output()?;
    validate_unknown_option_failure(&output, reference)?;
    let after = snapshot_tree(&workspace)?;
    if before != after {
      return Err(format!("{name} unknown-option preflight mutated the input workspace").into());
    }
  }
  Ok(())
}

fn validate_unknown_option_failure(output: &Output, reference: bool) -> Result<()> {
  if output.status.success() {
    return Err("unknown-option command unexpectedly succeeded".into());
  }
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if reference {
    let lower = text.to_ascii_lowercase();
    if output.status.code() != Some(1) || !text.contains("MSB1001") || !lower.contains("unknown switch") || !text.contains("--definitely-unknown") {
      return Err(format!("dotnet unknown-option oracle omitted MSB1001/Unknown switch: {text}").into());
    }
  } else {
    if output.status.code() != Some(2) {
      return Err(format!("dv unknown-option command returned {:?} instead of 2", output.status.code()).into());
    }
    if !output.stdout.is_empty() || !text.contains("error[DV0002]") || !text.contains("unknown build option \"--definitely-unknown\"") {
      return Err(format!("dv unknown-option diagnostic did not match the typed CLI boundary: {text}").into());
    }
    if text.contains("error[DV01") || text.contains("error[DV02") {
      return Err(format!("dv unknown-option command performed discovery before rejection: {text}").into());
    }
  }
  Ok(())
}

fn verify_option_effect_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  for (executable, name, arguments, reference) in [
    (Path::new("dotnet"), "dotnet", &["test", "--definitely-unknown"][..], true),
    (dv_executable, "dv", &["--compat", "dotnet", "test", "--definitely-unknown"][..], false),
  ] {
    let workspace = verification.join(name);
    reset_fixture(fixture, &workspace)?;
    let before = snapshot_tree(&workspace)?;
    let output = Command::new(executable).args(arguments).current_dir(&workspace).output()?;
    validate_option_effect_failure(&output, reference)?;
    if before != snapshot_tree(&workspace)? {
      return Err(format!("{name} option-effect preflight mutated the input workspace").into());
    }
  }
  Ok(())
}

fn validate_option_effect_failure(output: &Output, reference: bool) -> Result<()> {
  if output.status.success() {
    return Err("option-effect command unexpectedly succeeded".into());
  }
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if reference {
    let lower = text.to_ascii_lowercase();
    if output.status.code() != Some(1) || !text.contains("MSB1001") || !lower.contains("unknown switch") || !text.contains("--definitely-unknown") {
      return Err(format!("dotnet test option oracle omitted MSB1001/Unknown switch: {text}").into());
    }
  } else {
    if output.status.code() != Some(1)
      || !output.stdout.is_empty()
      || !text.contains("error[DV0002]")
      || !text.contains("unknown test option \"--definitely-unknown\"")
    {
      return Err(format!("dv child-option diagnostic did not match the typed boundary: {text}").into());
    }
    if text.contains("error[DV0003]") || text.contains("error[DV01") || text.contains("error[DV02") {
      return Err(format!("dv child-option command passed rejection or performed discovery: {text}").into());
    }
  }
  Ok(())
}

fn verify_driver_inventory_boundary(dv_executable: &Path, fixture: &Path) -> Result<()> {
  let before = snapshot_tree(fixture)?;
  for (reference_arguments, candidate_arguments, label) in [
    (&["--list-sdks"][..], &["--compat", "dotnet", "--list-sdks"][..], "SDK inventory"),
    (&["--list-runtimes"][..], &["--compat", "dotnet", "--list-runtimes"][..], "runtime inventory"),
  ] {
    let reference = Command::new("dotnet").args(reference_arguments).current_dir(fixture).output()?;
    let candidate = Command::new(dv_executable).args(candidate_arguments).current_dir(fixture).output()?;
    validate_driver_inventory_output(&reference)?;
    validate_driver_inventory_output(&candidate)?;
    let reference_text = normalized_text(&reference.stdout)?;
    let candidate_text = normalized_text(&candidate.stdout)?;
    if reference_text != candidate_text {
      return Err(format!("{label} differs between Microsoft and dv:\nreference={reference_text:?}\ncandidate={candidate_text:?}").into());
    }
  }
  if before != snapshot_tree(fixture)? {
    return Err("driver inventory preflight mutated the input workspace".into());
  }
  Ok(())
}

fn validate_driver_inventory_output(output: &Output) -> Result<()> {
  if !output.status.success() || !output.stderr.is_empty() {
    return Err(
      format!(
        "driver inventory failed with status {:?}: stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
      )
      .into(),
    );
  }
  let text = normalized_text(&output.stdout)?;
  let lines = text.lines().collect::<Vec<_>>();
  if lines.is_empty() || lines.iter().any(|line| !line.contains(" [") || !line.ends_with(']')) {
    return Err(format!("driver inventory did not contain ordered version/path rows: {text:?}").into());
  }
  Ok(())
}

fn normalized_text(bytes: &[u8]) -> Result<String> {
  Ok(std::str::from_utf8(bytes)?.replace("\r\n", "\n"))
}

fn verify_command_normalization_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  for (executable, name, arguments, reference) in [
    (Path::new("dotnet"), "dotnet", &["restore", "--definitely-unknown"][..], true),
    (dv_executable, "dv", &["sync", "--definitely-unknown"][..], false),
  ] {
    let workspace = verification.join(name);
    reset_fixture(fixture, &workspace)?;
    let before = snapshot_tree(&workspace)?;
    let output = Command::new(executable).args(arguments).current_dir(&workspace).output()?;
    validate_command_normalization_failure(&output, reference)?;
    if before != snapshot_tree(&workspace)? {
      return Err(format!("{name} command-normalization preflight mutated the input workspace").into());
    }
  }
  Ok(())
}

fn validate_command_normalization_failure(output: &Output, reference: bool) -> Result<()> {
  if output.status.success() {
    return Err("command-normalization boundary unexpectedly succeeded".into());
  }
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if reference {
    let lower = text.to_ascii_lowercase();
    if output.status.code() != Some(1) || !text.contains("MSB1001") || !lower.contains("unknown switch") || !text.contains("--definitely-unknown") {
      return Err(format!("dotnet restore normalization oracle omitted MSB1001/Unknown switch: {text}").into());
    }
  } else if output.status.code() != Some(2)
    || !output.stdout.is_empty()
    || !text.contains("error[DV0002]")
    || !text.contains("unknown sync option \"--definitely-unknown\"")
  {
    return Err(format!("dv sync did not retain its raw spelling at the typed restore rejection boundary: {text}").into());
  } else if text.contains("error[DV01") || text.contains("error[DV02") {
    return Err(format!("dv sync performed discovery before normalized rejection: {text}").into());
  }
  Ok(())
}

fn verify_transform_equivalence_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  for (executable, name, arguments, reference) in [
    (Path::new("dotnet"), "dotnet", &["restore", "--definitely-unknown"][..], true),
    (dv_executable, "dv", &["--compat", "dotnet", "restore", "--definitely-unknown"][..], false),
  ] {
    let workspace = verification.join(name);
    reset_fixture(fixture, &workspace)?;
    let before = snapshot_tree(&workspace)?;
    let output = Command::new(executable).args(arguments).current_dir(&workspace).output()?;
    validate_transform_equivalence_failure(&output, reference)?;
    if before != snapshot_tree(&workspace)? {
      return Err(format!("{name} transform-equivalence preflight mutated the input workspace").into());
    }
  }
  Ok(())
}

fn validate_transform_equivalence_failure(output: &Output, reference: bool) -> Result<()> {
  if reference {
    return validate_command_normalization_failure(output, true);
  }
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if output.status.code() != Some(1)
    || !output.stdout.is_empty()
    || !text.contains("error[DV0002]")
    || !text.contains("unknown restore option \"--definitely-unknown\"")
    || !text.contains("compatibility_profile: dotnet")
  {
    return Err(format!("dv compatibility restore did not retain the typed canonical transform boundary: {text}").into());
  }
  if text.contains("error[DV01") || text.contains("error[DV02") {
    return Err(format!("dv compatibility restore performed discovery before rejection: {text}").into());
  }
  Ok(())
}

fn verify_mode_classification_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  for (executable, name, arguments, reference) in [
    (Path::new("dotnet"), "dotnet", &["build", "--definitely-unknown"][..], true),
    (dv_executable, "dv", &["--compat", "dotnet", "build", "--definitely-unknown"][..], false),
  ] {
    let workspace = verification.join(name);
    reset_fixture(fixture, &workspace)?;
    let before = snapshot_tree(&workspace)?;
    let output = Command::new(executable).args(arguments).current_dir(&workspace).output()?;
    validate_mode_classification_failure(&output, reference)?;
    if before != snapshot_tree(&workspace)? {
      return Err(format!("{name} mode-classification preflight mutated the input workspace").into());
    }
  }
  Ok(())
}

fn validate_mode_classification_failure(output: &Output, reference: bool) -> Result<()> {
  if reference {
    return validate_unknown_option_failure(output, true);
  }
  if output.status.success() {
    return Err("mode-classification boundary unexpectedly succeeded".into());
  }
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if output.status.code() != Some(1)
    || !output.stdout.is_empty()
    || !text.contains("error[DV0002]")
    || !text.contains("unknown build option \"--definitely-unknown\"")
    || !text.contains("compatibility_profile: dotnet")
  {
    return Err(format!("dv dotnet mode did not apply its typed pre-I/O failure policy: {text}").into());
  }
  if text.contains("error[DV01") || text.contains("error[DV02") {
    return Err(format!("dv dotnet mode performed discovery before classified rejection: {text}").into());
  }
  Ok(())
}

fn verify_exit_policy_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  for (executable, name, arguments, reference) in [
    (Path::new("dotnet"), "dotnet", &["restore", "DefinitelyMissing.csproj"][..], true),
    (dv_executable, "dv", &["--compat", "dotnet", "restore", "DefinitelyMissing.csproj"][..], false),
  ] {
    let workspace = verification.join(name);
    reset_fixture(fixture, &workspace)?;
    let before = snapshot_tree(&workspace)?;
    let output = Command::new(executable).args(arguments).current_dir(&workspace).output()?;
    validate_exit_policy_failure(&output, reference)?;
    if before != snapshot_tree(&workspace)? {
      return Err(format!("{name} exit-policy preflight mutated the input workspace").into());
    }
  }
  Ok(())
}

fn validate_exit_policy_failure(output: &Output, reference: bool) -> Result<()> {
  if output.status.code() != Some(1) {
    return Err(format!("exit-policy boundary returned status {:?}", output.status.code()).into());
  }
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if reference {
    if !text.contains("MSB1009") || !text.contains("DefinitelyMissing.csproj") {
      return Err(format!("dotnet restore exit-policy oracle omitted its missing-project diagnostic: {text}").into());
    }
  } else if !output.stdout.is_empty()
    || !text.contains("error[DV0200]")
    || !text.contains("DefinitelyMissing.csproj")
    || !text.contains("compatibility_profile: dotnet")
  {
    return Err(format!("dv restore did not apply its typed compatibility exit policy: {text}").into());
  }
  Ok(())
}

fn verify_lexical_preservation_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  for (executable, name, arguments, reference) in [
    (Path::new("dotnet"), "dotnet", &["build", "-c:Release", "--definitely-unknown"][..], true),
    (
      dv_executable,
      "dv",
      &["--compat", "dotnet", "build", "-c:Release", "--definitely-unknown"][..],
      false,
    ),
  ] {
    let workspace = verification.join(name);
    reset_fixture(fixture, &workspace)?;
    let before = snapshot_tree(&workspace)?;
    let output = Command::new(executable).args(arguments).current_dir(&workspace).output()?;
    validate_lexical_preservation_failure(&output, reference)?;
    if before != snapshot_tree(&workspace)? {
      return Err(format!("{name} lexical-preservation preflight mutated the input workspace").into());
    }
  }
  Ok(())
}

fn validate_lexical_preservation_failure(output: &Output, reference: bool) -> Result<()> {
  if output.status.success() {
    return Err("lexical-preservation boundary unexpectedly succeeded".into());
  }
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if reference {
    let lower = text.to_ascii_lowercase();
    if output.status.code() != Some(1) || !text.contains("MSB1001") || !lower.contains("unknown switch") || !text.contains("--definitely-unknown") {
      return Err(format!("dotnet lexical oracle did not accept -c:Release before rejecting the sentinel: {text}").into());
    }
  } else if output.status.code() != Some(1)
    || !output.stdout.is_empty()
    || !text.contains("error[DV0002]")
    || !text.contains("unknown build option \"--definitely-unknown\"")
    || !text.contains("compatibility_profile: dotnet")
    || text.contains("unknown build option \"-c:Release\"")
  {
    return Err(format!("dv did not preserve the combined configuration token before sentinel rejection: {text}").into());
  } else if text.contains("error[DV01") || text.contains("error[DV02") {
    return Err(format!("dv lexical parser performed discovery before rejection: {text}").into());
  }
  Ok(())
}

fn verify_route_precedence_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  for (executable, name, arguments, reference) in [
    (Path::new("dotnet"), "dotnet", &["pack", "--definitely-unknown"][..], true),
    (dv_executable, "dv", &["--compat", "dotnet", "pack", "--definitely-unknown"][..], false),
  ] {
    let workspace = verification.join(name);
    reset_fixture(fixture, &workspace)?;
    let before = snapshot_tree(&workspace)?;
    let output = Command::new(executable).args(arguments).current_dir(&workspace).output()?;
    validate_route_precedence_failure(&output, reference)?;
    if before != snapshot_tree(&workspace)? {
      return Err(format!("{name} route-precedence preflight mutated the input workspace").into());
    }
  }
  Ok(())
}

fn validate_route_precedence_failure(output: &Output, reference: bool) -> Result<()> {
  if reference {
    return validate_unknown_option_failure(output, true);
  }
  if output.status.success() {
    return Err("route-precedence boundary unexpectedly succeeded".into());
  }
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if output.status.code() != Some(1)
    || !output.stdout.is_empty()
    || !text.contains("error[DV0002]")
    || !text.contains("unknown pack option \"--definitely-unknown\"")
  {
    return Err(format!("dv dotnet pack route did not reject at its typed pre-I/O boundary: {text}").into());
  }
  if text.contains("error[DV01") || text.contains("error[DV02") || text.contains("error[DV04") {
    return Err(format!("dv dotnet pack route performed discovery before classified rejection: {text}").into());
  }
  Ok(())
}

const ENVIRONMENT_BENCHMARK_SECRET: &str = "environment-benchmark-secret";
const CHILD_ENVIRONMENT_SECRET: &str = "child-environment-benchmark-secret";

fn apply_invocation_environment(command: &mut Command) {
  command
    .env("NO_COLOR", ENVIRONMENT_BENCHMARK_SECRET)
    .env("DV_COLOR", "never")
    .env("DV_VERBOSITY", "normal");
}

fn verify_environment_boundary(dv_executable: &Path, fixture: &Path, child_fixture: &Path, verification: &Path) -> Result<()> {
  const ARGS: &[&str] = &["build", "--definitely-unknown"];
  for (executable, name, reference) in [(Path::new("dotnet"), "dotnet", true), (dv_executable, "dv", false)] {
    let workspace = verification.join(name);
    reset_fixture(fixture, &workspace)?;
    let before = snapshot_tree(&workspace)?;
    let mut command = Command::new(executable);
    command.args(ARGS).current_dir(&workspace);
    apply_invocation_environment(&mut command);
    let output = command.output()?;
    validate_environment_failure(&output, reference)?;
    if before != snapshot_tree(&workspace)? {
      return Err(format!("{name} environment preflight mutated the input workspace").into());
    }
  }

  let child_workspace = verification.join("child");
  reset_fixture(child_fixture, &child_workspace)?;
  run_checked(
    Path::new("dotnet"),
    &["build", "ArgumentForwarding.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
    &child_workspace,
    "environment precedence oracle build",
  )?;
  let reference = Command::new("dotnet")
    .args([
      "[env:DV_CLI013_ORACLE=directive]",
      "run",
      "--project",
      "ArgumentForwarding.csproj",
      "--configuration",
      "Release",
      "--no-build",
      "--no-restore",
      "--environment",
      "DV_CLI013_ORACLE=command-line",
      "--environment",
      "DV_CLI013_TOKEN=child-environment-benchmark-secret",
      "--",
      "environment",
    ])
    .env("DV_CLI013_ORACLE", "ambient")
    .current_dir(&child_workspace)
    .output()?;
  validate_child_environment_reference(&reference)?;

  let actual = Command::new(dv_executable)
    .args([
      "--json",
      "[env:DV_CLI013_ORACLE=directive]",
      "run",
      "--environment",
      "DV_CLI013_ORACLE=command-line",
      "--environment",
      "DV_CLI013_TOKEN=child-environment-benchmark-secret",
      "--",
      "environment",
    ])
    .env("DV_CLI013_ORACLE", "ambient")
    .current_dir(&child_workspace)
    .output()?;
  validate_child_environment_dv(&actual)
}

fn validate_environment_failure(output: &Output, reference: bool) -> Result<()> {
  validate_unknown_option_failure(output, reference)?;
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if text.contains(ENVIRONMENT_BENCHMARK_SECRET) {
    return Err("environment precedence output exposed the environment sentinel".into());
  }
  if text.contains('\u{1b}') {
    return Err("NO_COLOR environment precedence emitted an ANSI escape sequence".into());
  }
  Ok(())
}

fn validate_child_environment_reference(output: &Output) -> Result<()> {
  if !output.status.success() {
    return Err(
      format!(
        "Microsoft child-environment oracle failed with status {:?}: stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
      )
      .into(),
    );
  }
  let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
  if value.get("selected").and_then(serde_json::Value::as_str) != Some("command-line")
    || value.get("secretPresent").and_then(serde_json::Value::as_bool) != Some(true)
  {
    return Err(format!("Microsoft child-environment precedence differs: {value}").into());
  }
  if String::from_utf8_lossy(&output.stdout).contains(CHILD_ENVIRONMENT_SECRET) {
    return Err("Microsoft child-environment oracle printed its secret sentinel".into());
  }
  Ok(())
}

fn validate_child_environment_dv(output: &Output) -> Result<()> {
  if output.status.code() != Some(2) || !output.stderr.is_empty() {
    return Err(format!("dv child-environment boundary returned status {:?}", output.status.code()).into());
  }
  let text = String::from_utf8_lossy(&output.stdout);
  if text.contains(CHILD_ENVIRONMENT_SECRET) || !text.contains("DV_CLI013_TOKEN=<redacted>") {
    return Err(format!("dv child-environment reporter did not contain its secret sentinel: {text}").into());
  }
  let events = output
    .stdout
    .split(|byte| *byte == b'\n')
    .filter(|line| !line.is_empty())
    .map(serde_json::from_slice::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?;
  let diagnostic = events
    .iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("diagnostic"))
    .and_then(|event| event.get("diagnostic"))
    .ok_or("dv child-environment boundary omitted its diagnostic")?;
  if diagnostic.get("code").and_then(serde_json::Value::as_str) != Some("DV0003") {
    return Err("dv child-environment boundary did not reach the typed run request".into());
  }
  for (name, expected) in [("environment_edit_count", "3"), ("sensitive_environment_edit_count", "1")] {
    let actual = diagnostic.get("context").and_then(serde_json::Value::as_array).and_then(|context| {
      context.iter().find_map(|field| {
        (field.get("name").and_then(serde_json::Value::as_str) == Some(name))
          .then(|| field.get("value").and_then(serde_json::Value::as_str))
          .flatten()
      })
    });
    if actual != Some(expected) {
      return Err(format!("dv child-environment boundary reported {name}={actual:?}, expected {expected}").into());
    }
  }
  Ok(())
}

const FORWARDED_ARGUMENTS: &[&str] = &["alpha", "", "--color", "two words"];
const DOTNET_FORWARDING_ORACLE_ARGS: &[&str] = &[
  "run",
  "--project",
  "ArgumentForwarding.csproj",
  "--configuration",
  "Release",
  "--no-build",
  "--no-restore",
  "--",
  "alpha",
  "",
  "--color",
  "two words",
];

fn verify_forwarding_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  let dotnet_workspace = verification.join("dotnet");
  let dv_workspace = verification.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;
  run_checked(
    Path::new("dotnet"),
    &["build", "ArgumentForwarding.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
    &dotnet_workspace,
    "argument-forwarding oracle build",
  )?;

  let reference = Command::new("dotnet")
    .args(DOTNET_FORWARDING_ORACLE_ARGS)
    .current_dir(&dotnet_workspace)
    .output()?;
  validate_forwarding_output(&reference, true)?;

  let actual = Command::new(dv_executable)
    .args(DV_CASES.iter().find(|case| case.name == "cli_forwarding").expect("forwarding case exists").args)
    .current_dir(&dv_workspace)
    .output()?;
  validate_forwarding_output(&actual, false)
}

fn validate_forwarding_output(output: &Output, reference: bool) -> Result<()> {
  if reference {
    if !output.status.success() {
      return Err(
        format!(
          "Microsoft forwarding oracle failed with status {:?}: stdout={} stderr={}",
          output.status.code(),
          String::from_utf8_lossy(&output.stdout),
          String::from_utf8_lossy(&output.stderr)
        )
        .into(),
      );
    }
    let actual: Vec<String> = serde_json::from_slice(&output.stdout)?;
    if actual != FORWARDED_ARGUMENTS {
      return Err(format!("dotnet forwarded argument batch differs: {actual:?}").into());
    }
    return Ok(());
  }

  if output.status.code() != Some(2) || !output.stderr.is_empty() {
    return Err(
      format!(
        "dv forwarding boundary returned status {:?} with stderr {:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
      )
      .into(),
    );
  }
  let events = output
    .stdout
    .split(|byte| *byte == b'\n')
    .filter(|line| !line.is_empty())
    .map(serde_json::from_slice::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?;
  let start = events
    .iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("command_started"))
    .ok_or("dv forwarding event stream omitted command_started")?;
  let raw = start
    .get("args")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv forwarding event stream omitted raw args")?;
  let delimiter = raw
    .iter()
    .position(|value| value.as_str() == Some("--"))
    .ok_or("dv forwarding event stream omitted delimiter")?;
  let forwarded = raw[delimiter + 1..]
    .iter()
    .map(|value| value.as_str().ok_or("dv forwarded argument was not text"))
    .collect::<std::result::Result<Vec<_>, _>>()?;
  if forwarded != FORWARDED_ARGUMENTS {
    return Err(format!("dv forwarded argument batch differs: {forwarded:?}").into());
  }
  let diagnostic = events
    .iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("diagnostic"))
    .and_then(|event| event.get("diagnostic"))
    .ok_or("dv forwarding boundary omitted its diagnostic")?;
  if diagnostic.get("code").and_then(serde_json::Value::as_str) != Some("DV0003") {
    return Err("dv forwarding boundary did not reach the typed run request".into());
  }
  let forwarded_count = diagnostic.get("context").and_then(serde_json::Value::as_array).and_then(|context| {
    context.iter().find_map(|field| {
      (field.get("name").and_then(serde_json::Value::as_str) == Some("forwarded_argument_count"))
        .then(|| field.get("value").and_then(serde_json::Value::as_str))
        .flatten()
    })
  });
  if forwarded_count != Some("4") {
    return Err(format!("dv forwarding boundary reported the wrong typed batch count: {forwarded_count:?}").into());
  }
  Ok(())
}

const CHILD_EXIT_CODE: i32 = 23;
const DOTNET_CHILD_EXIT_ORACLE_ARGS: &[&str] = &[
  "run",
  "--project",
  "ArgumentForwarding.csproj",
  "--configuration",
  "Release",
  "--no-build",
  "--no-restore",
  "--",
  "exit",
  "23",
];

fn verify_child_exit_boundary(dv_executable: &Path, fixture: &Path, verification: &Path) -> Result<()> {
  let dotnet_workspace = verification.join("dotnet");
  let dv_workspace = verification.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;
  run_checked(
    Path::new("dotnet"),
    &["build", "ArgumentForwarding.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
    &dotnet_workspace,
    "child-exit oracle build",
  )?;

  let reference = Command::new("dotnet")
    .args(DOTNET_CHILD_EXIT_ORACLE_ARGS)
    .current_dir(&dotnet_workspace)
    .output()?;
  validate_child_exit_output(&reference, true)?;

  let actual = Command::new(dv_executable)
    .args(DV_CASES.iter().find(|case| case.name == "cli_child_exit").expect("child-exit case exists").args)
    .current_dir(&dv_workspace)
    .output()?;
  validate_child_exit_output(&actual, false)
}

fn validate_child_exit_output(output: &Output, reference: bool) -> Result<()> {
  if reference {
    if output.status.code() != Some(CHILD_EXIT_CODE) || !output.stdout.is_empty() || !output.stderr.is_empty() {
      return Err(
        format!(
          "Microsoft child-exit oracle returned status {:?}: stdout={} stderr={}",
          output.status.code(),
          String::from_utf8_lossy(&output.stdout),
          String::from_utf8_lossy(&output.stderr)
        )
        .into(),
      );
    }
    return Ok(());
  }

  if output.status.code() != Some(2) || !output.stderr.is_empty() {
    return Err(format!("dv child-exit boundary returned status {:?}", output.status.code()).into());
  }
  let events = output
    .stdout
    .split(|byte| *byte == b'\n')
    .filter(|line| !line.is_empty())
    .map(serde_json::from_slice::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?;
  let diagnostic = events
    .iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("diagnostic"))
    .and_then(|event| event.get("diagnostic"))
    .ok_or("dv child-exit boundary omitted its diagnostic")?;
  if diagnostic.get("code").and_then(serde_json::Value::as_str) != Some("DV0003") {
    return Err("dv child-exit boundary did not reach the typed run request".into());
  }
  for (name, expected) in [("forwarded_argument_count", "2"), ("child_exit_policy", "preserve")] {
    let actual = diagnostic.get("context").and_then(serde_json::Value::as_array).and_then(|context| {
      context.iter().find_map(|field| {
        (field.get("name").and_then(serde_json::Value::as_str) == Some(name))
          .then(|| field.get("value").and_then(serde_json::Value::as_str))
          .flatten()
      })
    });
    if actual != Some(expected) {
      return Err(format!("dv child-exit boundary reported {name}={actual:?}, expected {expected}").into());
    }
  }
  Ok(())
}

const EVENT_SCHEMA_VERSION: u64 = 21;
const COMMAND_SYNTAX_VERSION: u64 = 3;
const PROTOCOL_VERSION_ALIASES: &[&[&str]] = &[&["--json", "version"], &["--json", "--version"], &["-V", "--json"]];

fn verify_protocol_version_boundary(dv_executable: &Path, fixture: &Path) -> Result<()> {
  for arguments in PROTOCOL_VERSION_ALIASES {
    let output = Command::new(dv_executable).args(*arguments).current_dir(fixture).output()?;
    validate_protocol_version_output(&output, arguments)?;
  }
  Ok(())
}

fn verify_compatibility_manifest_boundary(dv_executable: &Path, fixture: &Path) -> Result<()> {
  let plain = Command::new(dv_executable).args(["compat", "manifest"]).current_dir(fixture).output()?;
  let json_global = Command::new(dv_executable)
    .args(["--json", "compat", "manifest"])
    .current_dir(fixture)
    .output()?;
  check_output(plain.clone(), dv_executable, &["compat", "manifest"], "compatibility manifest preflight")?;
  check_output(
    json_global.clone(),
    dv_executable,
    &["--json", "compat", "manifest"],
    "compatibility manifest JSON-global preflight",
  )?;
  if plain.stdout != json_global.stdout {
    return Err("compatibility manifest bytes changed under the global JSON selector".into());
  }
  validate_compatibility_manifest_output(&plain)?;
  Ok(())
}

fn validate_compatibility_manifest_output(output: &Output) -> Result<()> {
  if !output.status.success() || !output.stderr.is_empty() {
    return Err("compatibility manifest query did not produce a clean success".into());
  }
  let manifest: serde_json::Value = serde_json::from_slice(&output.stdout)?;
  if manifest.get("schema_version").and_then(serde_json::Value::as_u64) != Some(1)
    || manifest.get("manifest_version").and_then(serde_json::Value::as_u64) != Some(1)
    || manifest.get("command_syntax_version").and_then(serde_json::Value::as_u64) != Some(COMMAND_SYNTAX_VERSION)
    || manifest.get("event_schema_version").and_then(serde_json::Value::as_u64) != Some(EVENT_SCHEMA_VERSION)
    || manifest
      .pointer("/reference/dotnet_sdk")
      .and_then(serde_json::Value::as_str)
      .is_none_or(str::is_empty)
    || manifest
      .get("commands")
      .and_then(serde_json::Value::as_array)
      .is_none_or(|commands| commands.len() < 100)
    || manifest
      .get("parity_rows")
      .and_then(serde_json::Value::as_array)
      .is_none_or(|rows| rows.len() != 468)
  {
    return Err("compatibility manifest query returned an incomplete or incompatible artifact".into());
  }
  Ok(())
}

fn verify_compatibility_check_boundary(dv_executable: &Path, fixture: &Path, workspace: &Path) -> Result<()> {
  reset_fixture(fixture, workspace)?;
  let output = Command::new(dv_executable)
    .args(["--json", "compat", "check", "ci.yml", "SmallConsole.csproj"])
    .current_dir(workspace)
    .output()?;
  validate_compatibility_check_output(&output)?;
  if workspace.join("obj").exists() {
    return Err("compatibility check wrote project artifacts".into());
  }
  Ok(())
}

fn validate_compatibility_check_output(output: &Output) -> Result<()> {
  if output.status.code() != Some(2) || !output.stderr.is_empty() {
    return Err(format!("compatibility check returned status {:?}", output.status.code()).into());
  }
  let events = output
    .stdout
    .split(|byte| *byte == b'\n')
    .filter(|line| !line.is_empty())
    .map(serde_json::from_slice::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?;
  if events.len() != 3
    || events
      .iter()
      .any(|event| event.get("schema_version").and_then(serde_json::Value::as_u64) != Some(EVENT_SCHEMA_VERSION))
  {
    return Err("compatibility check returned an invalid event batch".into());
  }
  let report = events
    .iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("compatibility_checked"))
    .ok_or("compatibility check omitted its report event")?;
  if report.get("manifest_version").and_then(serde_json::Value::as_u64) != Some(1)
    || report
      .get("unsupported_count")
      .and_then(serde_json::Value::as_u64)
      .is_none_or(|count| count == 0)
    || report
      .get("invocations")
      .and_then(serde_json::Value::as_array)
      .is_none_or(|rows| rows.len() != 2)
  {
    return Err("compatibility check returned an incomplete report".into());
  }
  Ok(())
}

fn validate_protocol_version_output(output: &Output, expected_arguments: &[&str]) -> Result<()> {
  if !output.status.success() || !output.stderr.is_empty() {
    return Err(format!("dv protocol-version query failed with status {:?}", output.status.code()).into());
  }
  let events = output
    .stdout
    .split(|byte| *byte == b'\n')
    .filter(|line| !line.is_empty())
    .map(serde_json::from_slice::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?;
  if events.len() != 3 {
    return Err(format!("dv protocol-version query emitted {} events; expected 3", events.len()).into());
  }
  for (sequence, event) in events.iter().enumerate() {
    if event.get("schema_version").and_then(serde_json::Value::as_u64) != Some(EVENT_SCHEMA_VERSION)
      || event.get("sequence").and_then(serde_json::Value::as_u64) != Some(sequence as u64)
    {
      return Err(format!("dv protocol-version event {sequence} has the wrong schema or sequence").into());
    }
  }
  let started = &events[0];
  let arguments = started
    .get("args")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv protocol-version start omitted args")?;
  if started.get("type").and_then(serde_json::Value::as_str) != Some("command_started")
    || started.get("command").and_then(serde_json::Value::as_str) != Some("version")
    || started.get("command_syntax_version").and_then(serde_json::Value::as_u64) != Some(COMMAND_SYNTAX_VERSION)
    || arguments.len() != expected_arguments.len()
    || !arguments
      .iter()
      .zip(expected_arguments)
      .all(|(actual, expected)| actual.as_str() == Some(*expected))
  {
    return Err("dv protocol-version start did not preserve the alias under one syntax version".into());
  }
  let version = &events[1];
  if version.get("type").and_then(serde_json::Value::as_str) != Some("tool_version")
    || version.get("version").and_then(serde_json::Value::as_str).is_none_or(str::is_empty)
    || version.get("command_syntax_version").and_then(serde_json::Value::as_u64) != Some(COMMAND_SYNTAX_VERSION)
    || version.get("event_schema_version").and_then(serde_json::Value::as_u64) != Some(EVENT_SCHEMA_VERSION)
  {
    return Err("dv tool_version did not report both independent protocol versions".into());
  }
  let finished = &events[2];
  if finished.get("type").and_then(serde_json::Value::as_str) != Some("command_finished")
    || finished.get("command").and_then(serde_json::Value::as_str) != Some("version")
    || finished.get("outcome").and_then(serde_json::Value::as_str) != Some("succeeded")
  {
    return Err("dv protocol-version query omitted its successful terminal event".into());
  }
  Ok(())
}

fn verify_rid_graph(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let verification = repository.join("target/benchmark-rid-graph-verification");
  ensure_workspace_is_safe(repository, &verification)?;
  reset_fixture(fixture, &verification)?;
  prepare_rid_oracle(Path::new("dotnet"), &verification)?;
  verify_sdk_selection(dv_executable, &verification)?;

  let runtime_identifier = "linux-musl-x64";
  let reference = command_text(Path::new("dotnet"), &["bin/Release/RidGraphOracle.dll", runtime_identifier], &verification)?;
  let reference = reference.lines().map(str::to_owned).collect::<Vec<_>>();
  let actual = command_text(dv_executable, &["sdk", "compatible-rids", runtime_identifier], &verification)?;
  let actual = actual.lines().map(str::to_owned).collect::<Vec<_>>();
  if reference != actual {
    return Err(format!("portable RID expansion mismatch: NuGet={reference:?}, dv={actual:?}").into());
  }
  Ok(())
}

fn verify_project_evaluation(dv_executable: &Path, fixture: &Path, named_selection: bool) -> Result<()> {
  let dotnet_text = command_text(
    Path::new("dotnet"),
    &[
      "msbuild",
      "SmallConsole.csproj",
      "--nologo",
      "-getProperty:TargetFramework,OutputType,Nullable,ImplicitUsings,AssemblyName,RootNamespace,Configuration,Deterministic",
      "-getItem:Compile,ProjectReference,PackageReference",
    ],
    fixture,
  )?;
  let dotnet: serde_json::Value = serde_json::from_str(&dotnet_text)?;
  let dv_arguments = if named_selection {
    &["project", "inspect", "--project", "SmallConsole.csproj", "--json"][..]
  } else {
    &["project", "inspect", "SmallConsole.csproj", "--json"][..]
  };
  let dv_text = command_text(dv_executable, dv_arguments, fixture)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("project_evaluated"))
    .ok_or("dv project inspection did not emit project_evaluated")?;

  for (dotnet_name, dv_name) in [
    ("TargetFramework", "target_framework"),
    ("OutputType", "output_type"),
    ("Nullable", "nullable"),
    ("ImplicitUsings", "implicit_usings"),
    ("AssemblyName", "assembly_name"),
    ("RootNamespace", "root_namespace"),
    ("Configuration", "configuration"),
  ] {
    let reference = dotnet.pointer(&format!("/Properties/{dotnet_name}")).and_then(serde_json::Value::as_str);
    let actual = dv.get(dv_name).and_then(serde_json::Value::as_str);
    if reference != actual {
      return Err(format!("project evaluation mismatch for {dotnet_name}: dotnet={reference:?}, dv={actual:?}").into());
    }
  }
  let reference_deterministic = dotnet.pointer("/Properties/Deterministic").and_then(serde_json::Value::as_str);
  let actual_deterministic = dv.get("deterministic").and_then(serde_json::Value::as_bool);
  if reference_deterministic != actual_deterministic.map(|value| if value { "true" } else { "false" }) {
    return Err(format!("project evaluation mismatch for Deterministic: dotnet={reference_deterministic:?}, dv={actual_deterministic:?}").into());
  }

  let dotnet_sources = item_identities(&dotnet, "Compile")?;
  let dv_sources = string_array(&dv, "sources")?;
  if dotnet_sources != dv_sources {
    return Err(format!("project source evaluation mismatch: dotnet={dotnet_sources:?}, dv={dv_sources:?}").into());
  }
  if !item_identities(&dotnet, "ProjectReference")?.is_empty() || !string_array(&dv, "project_references")?.is_empty() {
    return Err("small-console project unexpectedly contains project references".into());
  }
  if !item_identities(&dotnet, "PackageReference")?.is_empty() {
    return Err("small-console project unexpectedly contains package references".into());
  }
  if !dv.get("package_references").and_then(serde_json::Value::as_array).is_some_and(Vec::is_empty) {
    return Err("dv small-console evaluation unexpectedly contains package references".into());
  }
  Ok(())
}

fn verify_package_reference_conditions(dv_executable: &Path, fixture: &Path) -> Result<()> {
  let dotnet_text = command_text(
    Path::new("dotnet"),
    &[
      "msbuild",
      "ConditionalReferences.csproj",
      "--nologo",
      "-p:Configuration=Release",
      "-getProperty:TargetFramework,RuntimeIdentifier,Configuration",
      "-getItem:PackageReference,ProjectReference,FrameworkReference",
    ],
    fixture,
  )?;
  let dotnet: serde_json::Value = serde_json::from_str(&dotnet_text)?;
  let dv_text = command_text(
    dv_executable,
    &["project", "inspect", "ConditionalReferences.csproj", "--configuration", "Release", "--json"],
    fixture,
  )?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("project_evaluated"))
    .ok_or("dv conditional-reference inspection did not emit project_evaluated")?;

  for (dotnet_name, dv_name) in [
    ("TargetFramework", "target_framework"),
    ("RuntimeIdentifier", "runtime_identifier"),
    ("Configuration", "configuration"),
  ] {
    let reference = dotnet.pointer(&format!("/Properties/{dotnet_name}")).and_then(serde_json::Value::as_str);
    let actual = dv.get(dv_name).and_then(serde_json::Value::as_str);
    if reference != actual {
      return Err(format!("conditional-reference property mismatch for {dotnet_name}: dotnet={reference:?}, dv={actual:?}").into());
    }
  }

  let expected_packages = [("Newtonsoft.Json", "13.0.3"), ("Humanizer.Core", "2.14.1"), ("Serilog", "4.3.0")];
  let dotnet_packages = dotnet
    .pointer("/Items/PackageReference")
    .and_then(serde_json::Value::as_array)
    .ok_or("Microsoft conditional-reference oracle omitted PackageReference items")?
    .iter()
    .map(|item| {
      (
        item.get("Identity").and_then(serde_json::Value::as_str).unwrap_or_default(),
        item.get("Version").and_then(serde_json::Value::as_str).unwrap_or_default(),
      )
    })
    .collect::<Vec<_>>();
  let dv_packages = dv
    .get("package_references")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv conditional-reference event omitted package_references")?
    .iter()
    .map(|item| {
      (
        item.get("id").and_then(serde_json::Value::as_str).unwrap_or_default(),
        item.get("version").and_then(serde_json::Value::as_str).unwrap_or_default(),
      )
    })
    .collect::<Vec<_>>();
  if dotnet_packages != expected_packages || dv_packages != expected_packages {
    return Err(format!("conditional package batch mismatch: expected={expected_packages:?}, dotnet={dotnet_packages:?}, dv={dv_packages:?}").into());
  }

  let dotnet_projects = item_identities(&dotnet, "ProjectReference")?
    .into_iter()
    .map(|path| path.replace('\\', "/"))
    .collect::<Vec<_>>();
  let dv_projects = string_array(&dv, "project_references")?;
  if dotnet_projects != ["Library/Library.csproj"] || dv_projects != dotnet_projects {
    return Err(format!("conditional project-reference batch mismatch: dotnet={dotnet_projects:?}, dv={dv_projects:?}").into());
  }

  let dotnet_frameworks = item_identities(&dotnet, "FrameworkReference")?;
  let dv_frameworks = dv
    .get("framework_references")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv conditional-reference event omitted framework_references")?
    .iter()
    .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
    .collect::<Vec<_>>();
  if !dotnet_frameworks.iter().any(|id| id == "Microsoft.AspNetCore.App")
    || dotnet_frameworks.iter().any(|id| id == "Excluded.Framework")
    || dv_frameworks != ["Microsoft.AspNetCore.App"]
  {
    return Err(format!("conditional framework-reference batch mismatch: dotnet={dotnet_frameworks:?}, dv={dv_frameworks:?}").into());
  }
  Ok(())
}

fn verify_runtime_evaluation(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let verification = repository.join("target/benchmark-runtime-evaluation-verification");
  ensure_workspace_is_safe(repository, &verification)?;
  reset_fixture(fixture, &verification)?;
  let dotnet_text = command_text(
    Path::new("dotnet"),
    &[
      "msbuild",
      "RuntimeProject.csproj",
      "--nologo",
      "-getProperty:TargetFramework,RuntimeIdentifier,RuntimeIdentifiers",
    ],
    &verification,
  )?;
  let dotnet: serde_json::Value = serde_json::from_str(&dotnet_text)?;
  let dv_text = command_text(dv_executable, &["project", "inspect", "RuntimeProject.csproj", "--json"], &verification)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("project_evaluated"))
    .ok_or("dv runtime project inspection did not emit project_evaluated")?;

  for (dotnet_name, dv_name) in [("TargetFramework", "target_framework"), ("RuntimeIdentifier", "runtime_identifier")] {
    let reference = dotnet.pointer(&format!("/Properties/{dotnet_name}")).and_then(serde_json::Value::as_str);
    let actual = dv.get(dv_name).and_then(serde_json::Value::as_str);
    if reference != actual {
      return Err(format!("runtime evaluation mismatch for {dotnet_name}: dotnet={reference:?}, dv={actual:?}").into());
    }
  }

  let reference_identifiers = dotnet
    .pointer("/Properties/RuntimeIdentifiers")
    .and_then(serde_json::Value::as_str)
    .unwrap_or_default()
    .split(';')
    .map(str::trim)
    .filter(|identifier| !identifier.is_empty())
    .map(str::to_owned)
    .collect::<Vec<_>>();
  let actual_identifiers = string_array(&dv, "runtime_identifiers")?;
  if reference_identifiers != actual_identifiers {
    return Err(format!("RuntimeIdentifiers mismatch: dotnet={reference_identifiers:?}, dv={actual_identifiers:?}").into());
  }

  let mut reference_dimensions = reference_identifiers;
  if let Some(selected) = dotnet.pointer("/Properties/RuntimeIdentifier").and_then(serde_json::Value::as_str)
    && !selected.is_empty()
    && !reference_dimensions.iter().any(|identifier| identifier == selected)
  {
    reference_dimensions.push(selected.to_owned());
  }
  let actual_dimensions = string_array(&dv, "runtime_dimensions")?;
  if reference_dimensions != actual_dimensions {
    return Err(format!("runtime target dimensions mismatch: expected={reference_dimensions:?}, dv={actual_dimensions:?}").into());
  }
  Ok(())
}

fn verify_runtime_pack_plan(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let verification = repository.join("target/benchmark-runtime-pack-verification");
  ensure_workspace_is_safe(repository, &verification)?;
  reset_fixture(fixture, &verification)?;
  run_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "RuntimePackProject.csproj",
      "--packages",
      ".packages",
      "--nologo",
      "-r",
      "win-x64",
      "-p:SelfContained=true",
      "-p:UseAppHost=true",
      "-p:NuGetAudit=false",
      "--verbosity",
      "quiet",
    ],
    &verification,
    "runtime-pack verification restore",
  )?;
  let dotnet_text = command_text(
    Path::new("dotnet"),
    &[
      "msbuild",
      "RuntimePackProject.csproj",
      "--nologo",
      "-p:SelfContained=true",
      "-p:UseAppHost=true",
      "-t:ProcessFrameworkReferences;ResolveFrameworkReferences;ResolveRuntimePackAssets;_GetAppHostPaths",
      "-getProperty:RuntimeIdentifier,AppHostSourcePath",
      "-getItem:ResolvedRuntimePack,RuntimePackAsset,ResolvedAppHostPack",
    ],
    &verification,
  )?;
  let dotnet: serde_json::Value = serde_json::from_str(&dotnet_text)?;
  let dv_text = command_text(
    dv_executable,
    &["project", "runtime-packs", "RuntimePackProject.csproj", "--packages", ".packages", "--json"],
    &verification,
  )?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("runtime_pack_plan_created"))
    .ok_or("dv runtime-pack planning did not emit runtime_pack_plan_created")?;

  let requested = dotnet.pointer("/Properties/RuntimeIdentifier").and_then(serde_json::Value::as_str);
  if requested != dv.get("requested_runtime_identifier").and_then(serde_json::Value::as_str) {
    return Err(
      format!(
        "requested runtime identifier mismatch: dotnet={requested:?} dv={:?}",
        dv.get("requested_runtime_identifier")
      )
      .into(),
    );
  }
  let runtime_pack_id = required_string(&dv, "runtime_pack_id")?;
  let runtime_pack = dotnet
    .pointer("/Items/ResolvedRuntimePack")
    .and_then(serde_json::Value::as_array)
    .ok_or("MSBuild omitted ResolvedRuntimePack")?
    .iter()
    .find(|item| item.get("NuGetPackageId").and_then(serde_json::Value::as_str) == Some(runtime_pack_id))
    .ok_or_else(|| format!("MSBuild did not resolve runtime pack {runtime_pack_id}"))?;
  compare_text_field(runtime_pack, "NuGetPackageVersion", &dv, "runtime_pack_version")?;
  compare_text_field(runtime_pack, "RuntimeIdentifier", &dv, "runtime_identifier")?;
  compare_path_field(runtime_pack, "PackageDirectory", &dv, "runtime_pack_root", &verification)?;

  let runtime_assets = dotnet
    .pointer("/Items/RuntimePackAsset")
    .and_then(serde_json::Value::as_array)
    .ok_or("MSBuild omitted RuntimePackAsset")?;
  let reference_managed = runtime_assets
    .iter()
    .filter(|asset| asset.get("NuGetPackageId").and_then(serde_json::Value::as_str) == Some(runtime_pack_id))
    .filter(|asset| asset.get("AssetType").and_then(serde_json::Value::as_str) == Some("runtime"))
    .map(item_identity)
    .collect::<Result<Vec<_>>>()?;
  let reference_native = runtime_assets
    .iter()
    .filter(|asset| asset.get("NuGetPackageId").and_then(serde_json::Value::as_str) == Some(runtime_pack_id))
    .filter(|asset| asset.get("AssetType").and_then(serde_json::Value::as_str) == Some("native"))
    .map(item_identity)
    .collect::<Result<Vec<_>>>()?;
  compare_normalized_paths(
    &reference_managed,
    &string_array(&dv, "managed_assets")?,
    "managed runtime assets",
    &verification,
  )?;
  compare_normalized_paths(&reference_native, &string_array(&dv, "native_assets")?, "native runtime assets", &verification)?;

  let apphost = dotnet.pointer("/Items/ResolvedAppHostPack/0").ok_or("MSBuild omitted ResolvedAppHostPack")?;
  compare_text_field(apphost, "RuntimeIdentifier", &dv, "host_runtime_identifier")?;
  compare_path_field(apphost, "PackageDirectory", &dv, "host_pack_root", &verification)?;
  compare_path_field(apphost, "Path", &dv, "apphost_template", &verification)?;
  let property_apphost = dotnet
    .pointer("/Properties/AppHostSourcePath")
    .and_then(serde_json::Value::as_str)
    .ok_or("MSBuild omitted AppHostSourcePath")?;
  if normalize_path(property_apphost, &verification) != normalize_path(required_string(&dv, "apphost_template")?, &verification) {
    return Err("MSBuild AppHostSourcePath differs from the dv apphost template".into());
  }
  let expected_host_suffix = format!("\\{}\\{}", required_string(&dv, "host_pack_id")?, required_string(&dv, "host_pack_version")?);
  if !normalize_windows_path(required_string(&dv, "host_pack_root")?).ends_with(&normalize_windows_path(&expected_host_suffix)) {
    return Err("dv host pack identity/version do not match its MSBuild-selected directory".into());
  }
  Ok(())
}

fn verify_framework_reference_plan(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let verification = repository.join("target/benchmark-framework-reference-verification");
  ensure_workspace_is_safe(repository, &verification)?;
  reset_fixture(fixture, &verification)?;
  run_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "FrameworkReferenceProject.csproj",
      "--nologo",
      "-p:NuGetAudit=false",
      "--verbosity",
      "quiet",
    ],
    &verification,
    "framework-reference verification restore",
  )?;
  let dotnet_text = command_text(
    Path::new("dotnet"),
    &[
      "msbuild",
      "FrameworkReferenceProject.csproj",
      "--nologo",
      "-t:ResolveTargetingPackAssets",
      "-getProperty:TargetFramework,RollForward,SelfContained",
      "-getItem:RuntimeFramework,ResolvedFrameworkReference",
    ],
    &verification,
  )?;
  let dotnet: serde_json::Value = serde_json::from_str(&dotnet_text)?;
  let dv_text = command_text(
    dv_executable,
    &["project", "frameworks", "FrameworkReferenceProject.csproj", "--json"],
    &verification,
  )?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("framework_reference_plan_created"))
    .ok_or("dv framework planning did not emit framework_reference_plan_created")?;

  for (dotnet_name, dv_name) in [("TargetFramework", "target_framework"), ("RollForward", "roll_forward")] {
    let reference = dotnet.pointer(&format!("/Properties/{dotnet_name}")).and_then(serde_json::Value::as_str);
    let actual = dv.get(dv_name).and_then(serde_json::Value::as_str);
    if reference != actual {
      return Err(format!("framework plan mismatch for {dotnet_name}: dotnet={reference:?}, dv={actual:?}").into());
    }
  }
  let reference_self_contained = dotnet.pointer("/Properties/SelfContained").and_then(serde_json::Value::as_str) == Some("true");
  if dv.get("self_contained").and_then(serde_json::Value::as_bool) != Some(reference_self_contained) {
    return Err("framework plan SelfContained mismatch".into());
  }

  let frameworks = dv
    .get("frameworks")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv framework plan omitted frameworks")?;
  let resolved = dotnet
    .pointer("/Items/ResolvedFrameworkReference")
    .and_then(serde_json::Value::as_array)
    .ok_or("MSBuild omitted ResolvedFrameworkReference")?;
  let runtimes = dotnet
    .pointer("/Items/RuntimeFramework")
    .and_then(serde_json::Value::as_array)
    .ok_or("MSBuild omitted RuntimeFramework")?;
  if frameworks.len() != resolved.len() || frameworks.len() != runtimes.len() {
    return Err(
      format!(
        "framework count mismatch: MSBuild resolved={} runtime={} dv={}",
        resolved.len(),
        runtimes.len(),
        frameworks.len()
      )
      .into(),
    );
  }
  for framework in frameworks {
    let reference_name = required_string(framework, "reference")?;
    let resolved_framework = resolved
      .iter()
      .find(|item| item.get("Identity").and_then(serde_json::Value::as_str) == Some(reference_name))
      .ok_or_else(|| format!("MSBuild did not resolve framework reference {reference_name}"))?;
    compare_text_field(resolved_framework, "TargetingPackName", framework, "targeting_pack_id")?;
    compare_text_field(resolved_framework, "TargetingPackVersion", framework, "targeting_pack_version")?;
    compare_path_field(resolved_framework, "TargetingPackPath", framework, "targeting_pack_root", &verification)?;

    let runtime = runtimes
      .iter()
      .find(|item| item.get("FrameworkName").and_then(serde_json::Value::as_str) == Some(reference_name))
      .ok_or_else(|| format!("MSBuild did not emit runtime framework for {reference_name}"))?;
    compare_text_field(runtime, "Identity", framework, "runtime_name")?;
    compare_text_field(runtime, "Version", framework, "requested_version")?;
    let expected_profile = runtime.get("Profile").and_then(serde_json::Value::as_str).filter(|profile| !profile.is_empty());
    let actual_profile = framework.get("profile").and_then(serde_json::Value::as_str);
    if expected_profile != actual_profile {
      return Err(format!("framework profile mismatch for {reference_name}: MSBuild={expected_profile:?} dv={actual_profile:?}").into());
    }
  }

  run_checked(
    Path::new("dotnet"),
    &[
      "build",
      "FrameworkReferenceProject.csproj",
      "-c",
      "Release",
      "--no-restore",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &verification,
    "framework-reference host oracle build",
  )?;
  let host_output = command_text(Path::new("dotnet"), &["bin/Release/net10.0/FrameworkReferenceProject.dll"], &verification)?;
  let selected = host_output
    .lines()
    .filter_map(|assembly| {
      let version = Path::new(assembly).parent()?.file_name()?.to_str()?;
      let runtime_name = Path::new(assembly).parent()?.parent()?.file_name()?.to_str()?;
      Some((runtime_name, version))
    })
    .collect::<Vec<_>>();
  for framework in frameworks {
    let runtime_name = required_string(framework, "runtime_name")?;
    let actual = framework.get("selected_version").and_then(serde_json::Value::as_str);
    let reference = selected.iter().find_map(|(name, version)| (*name == runtime_name).then_some(*version));
    if reference != actual {
      return Err(format!("installed shared-framework selection mismatch for {runtime_name}: host={reference:?} dv={actual:?}").into());
    }
  }
  Ok(())
}

fn verify_pack_diagnostic(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let verification = repository.join("target/benchmark-pack-diagnostic-verification");
  ensure_workspace_is_safe(repository, &verification)?;
  let dotnet_workspace = verification.join("dotnet");
  let dv_workspace = verification.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;

  let reference = Command::new("dotnet")
    .args([
      "restore",
      "UnavailablePackProject.csproj",
      "--source",
      "offline-source",
      "--packages",
      ".packages",
      "--no-cache",
      "--disable-build-servers",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "minimal",
    ])
    .current_dir(&dotnet_workspace)
    .output()?;
  validate_pack_failure(&reference, true)?;

  let actual = Command::new(dv_executable)
    .args(["project", "runtime-packs", "UnavailablePackProject.csproj", "--packages", ".packages", "--json"])
    .current_dir(&dv_workspace)
    .output()?;
  validate_pack_failure(&actual, false)
}

fn validate_pack_failure(output: &Output, reference: bool) -> Result<()> {
  const IDENTITY: &str = "Microsoft.NETCore.App.Runtime.linux-arm";
  if output.status.success() {
    return Err("unavailable-pack oracle unexpectedly succeeded".into());
  }
  if reference {
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if !text.contains("NU1101") || !text.contains(IDENTITY) {
      return Err(format!("dotnet unavailable-pack diagnostic omitted NU1101 or {IDENTITY}: {text}").into());
    }
    return Ok(());
  }
  if !output.stderr.is_empty() {
    return Err(format!("dv JSON diagnostic wrote stderr: {}", String::from_utf8_lossy(&output.stderr)).into());
  }
  let diagnostic = std::str::from_utf8(&output.stdout)?
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("diagnostic"))
    .and_then(|event| event.get("diagnostic").cloned())
    .ok_or("dv unavailable-pack command omitted its diagnostic event")?;
  if diagnostic.get("code").and_then(serde_json::Value::as_str) != Some("DV0124") {
    return Err("dv unavailable-pack diagnostic code is not DV0124".into());
  }
  let context = diagnostic
    .get("context")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv unavailable-pack diagnostic omitted context")?;
  for (name, expected) in [
    ("pack_kind", "runtime_pack"),
    ("pack_identity", IDENTITY),
    ("pack_version", "10.0.0"),
    ("target_framework", "net10.0"),
    ("runtime_identifier", "linux-arm"),
    ("acquisition", "restore_package"),
  ] {
    let actual = context.iter().find_map(|field| {
      if field.get("name").and_then(serde_json::Value::as_str) == Some(name) {
        field.get("value").and_then(serde_json::Value::as_str)
      } else {
        None
      }
    });
    if actual != Some(expected) {
      return Err(format!("dv unavailable-pack context {name} mismatch: expected={expected:?} actual={actual:?}").into());
    }
  }
  if diagnostic.get("help").and_then(serde_json::Value::as_str) != Some("Restore the required pack from a configured package source.") {
    return Err("dv unavailable-pack diagnostic omitted acquisition guidance".into());
  }
  Ok(())
}

fn validate_source_mapping_failure(output: &Output, reference: bool) -> Result<()> {
  if output.status.success() {
    return Err("unmapped package-source benchmark unexpectedly succeeded".into());
  }
  if reference {
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if !text.contains("NU1100") || !text.contains("Unmapped.Package") || !text.contains("PackageSourceMapping is enabled") {
      return Err(format!("dotnet unmapped-package diagnostic omitted source-mapping evidence: {text}").into());
    }
    if text.contains("NU1301") {
      return Err(format!("dotnet contacted the unreachable source before applying package-source mapping: {text}").into());
    }
    return Ok(());
  }
  if !output.stderr.is_empty() {
    return Err(format!("dv JSON source-mapping diagnostic wrote stderr: {}", String::from_utf8_lossy(&output.stderr)).into());
  }
  let diagnostic = std::str::from_utf8(&output.stdout)?
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("diagnostic"))
    .and_then(|event| event.get("diagnostic").cloned())
    .ok_or("dv unmapped-package command omitted its diagnostic event")?;
  if diagnostic.get("code").and_then(serde_json::Value::as_str) != Some("DV0412") {
    return Err(format!("dv unmapped-package diagnostic is not DV0412: {diagnostic}").into());
  }
  let identity = diagnostic.get("context").and_then(serde_json::Value::as_array).and_then(|context| {
    context.iter().find_map(|field| {
      (field.get("name").and_then(serde_json::Value::as_str) == Some("package_id"))
        .then(|| field.get("value").and_then(serde_json::Value::as_str))
        .flatten()
    })
  });
  if identity != Some("unmapped.package") {
    return Err(format!("dv unmapped-package diagnostic has the wrong package_id: {diagnostic}").into());
  }
  Ok(())
}

fn validate_package_diagnostic(output: &Output, reference: bool) -> Result<()> {
  if output.status.success() {
    return Err("package constraint-conflict benchmark unexpectedly succeeded".into());
  }
  if reference {
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if !text.contains("NU1107") || !text.contains("Diagnostic.Leaf") {
      return Err(format!("Microsoft restore omitted the expected NU1107 constraint conflict: {text}").into());
    }
    return Ok(());
  }
  if !output.stderr.is_empty() {
    return Err(format!("dv JSON package diagnostic wrote stderr: {}", String::from_utf8_lossy(&output.stderr)).into());
  }
  let diagnostic = std::str::from_utf8(&output.stdout)?
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("diagnostic"))
    .and_then(|event| event.get("diagnostic").cloned())
    .ok_or("dv restore omitted its constraint-conflict diagnostic")?;
  if diagnostic.get("code").and_then(serde_json::Value::as_str) != Some("DV0414")
    || diagnostic.get("severity").and_then(serde_json::Value::as_str) != Some("error")
  {
    return Err(format!("dv emitted an unstable constraint-conflict diagnostic: {diagnostic}").into());
  }
  let context = diagnostic
    .get("context")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv constraint-conflict diagnostic omitted context")?;
  for (name, expected) in [("package_id", "Diagnostic.Leaf"), ("required_ranges", "[1.0.0]; [2.0.0]")] {
    let actual = context.iter().find_map(|field| {
      (field.get("name").and_then(serde_json::Value::as_str) == Some(name))
        .then(|| field.get("value").and_then(serde_json::Value::as_str))
        .flatten()
    });
    if actual != Some(expected) {
      return Err(format!("dv constraint-conflict context {name} mismatch: expected={expected:?} actual={actual:?}").into());
    }
  }
  if diagnostic.get("help").and_then(serde_json::Value::as_str).is_none() {
    return Err("dv constraint-conflict diagnostic omitted its action".into());
  }
  Ok(())
}

fn validate_reference_package_error(output: &Output, code: &str, identity: &str) -> Result<()> {
  if output.status.success() {
    return Err(format!("Microsoft package-error oracle {code} unexpectedly succeeded").into());
  }
  let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
  if !text.contains(code) || !text.contains(identity) {
    return Err(format!("Microsoft package-error oracle omitted {code} or {identity}: {text}").into());
  }
  Ok(())
}

fn validate_dv_package_error(output: &Output, code: &str, identity: &str) -> Result<()> {
  if output.status.success() {
    return Err(format!("dv package-error oracle {code} unexpectedly succeeded").into());
  }
  if !output.stderr.is_empty() {
    return Err(format!("dv JSON package-error oracle wrote stderr: {}", String::from_utf8_lossy(&output.stderr)).into());
  }
  let diagnostic = std::str::from_utf8(&output.stdout)?
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("diagnostic"))
    .and_then(|event| event.get("diagnostic").cloned())
    .ok_or_else(|| format!("dv package-error oracle omitted {code}"))?;
  if diagnostic.get("code").and_then(serde_json::Value::as_str) != Some(code)
    || diagnostic.get("severity").and_then(serde_json::Value::as_str) != Some("error")
    || !diagnostic.to_string().to_ascii_lowercase().contains(&identity.to_ascii_lowercase())
  {
    return Err(format!("dv package-error oracle expected {code} and {identity}: {diagnostic}").into());
  }
  Ok(())
}

fn required_string<'a>(value: &'a serde_json::Value, field: &str) -> Result<&'a str> {
  value
    .get(field)
    .and_then(serde_json::Value::as_str)
    .ok_or_else(|| format!("JSON value omitted {field}").into())
}

fn item_identity(value: &serde_json::Value) -> Result<String> {
  value
    .get("Identity")
    .and_then(serde_json::Value::as_str)
    .map(str::to_owned)
    .ok_or_else(|| "MSBuild item omitted Identity".into())
}

fn compare_text_field(reference: &serde_json::Value, reference_field: &str, actual: &serde_json::Value, actual_field: &str) -> Result<()> {
  let reference = required_string(reference, reference_field)?;
  let actual = required_string(actual, actual_field)?;
  if reference != actual {
    return Err(format!("{actual_field} mismatch: dotnet={reference:?} dv={actual:?}").into());
  }
  Ok(())
}

fn compare_path_field(reference: &serde_json::Value, reference_field: &str, actual: &serde_json::Value, actual_field: &str, base: &Path) -> Result<()> {
  let reference = required_string(reference, reference_field)?;
  let actual = required_string(actual, actual_field)?;
  if normalize_path(reference, base) != normalize_path(actual, base) {
    return Err(format!("{actual_field} mismatch: dotnet={reference:?} dv={actual:?}").into());
  }
  Ok(())
}

fn compare_normalized_paths(reference: &[String], actual: &[String], meaning: &str, base: &Path) -> Result<()> {
  let reference = reference.iter().map(|path| normalize_path(path, base)).collect::<Vec<_>>();
  let actual = actual.iter().map(|path| normalize_path(path, base)).collect::<Vec<_>>();
  if reference != actual {
    return Err(format!("{meaning} differ: dotnet={} dv={}", reference.len(), actual.len()).into());
  }
  Ok(())
}

fn normalize_path(path: &str, base: &Path) -> String {
  let path = Path::new(path);
  let absolute = if path.is_absolute() { path.to_owned() } else { base.join(path) };
  normalize_windows_path(&absolute.to_string_lossy())
}

fn normalize_windows_path(path: &str) -> String {
  path.strip_prefix(r"\\?\").unwrap_or(path).replace('/', r"\").to_ascii_lowercase()
}

fn verify_compiler_plan(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let verification = repository.join("target/benchmark-compiler-plan-verification");
  ensure_workspace_is_safe(repository, &verification)?;
  reset_fixture(fixture, &verification)?;
  run_checked(
    Path::new("dotnet"),
    &["restore", "--nologo", "--verbosity", "quiet"],
    &verification,
    "compiler-plan verification restore",
  )?;
  let dotnet_text = command_text(
    Path::new("dotnet"),
    &[
      "msbuild",
      "SmallConsole.csproj",
      "--nologo",
      "-t:ResolveReferences",
      "-getProperty:LangVersion,DefineConstants",
      "-getItem:ReferencePath,Analyzer,Compile",
    ],
    &verification,
  )?;
  let dotnet: serde_json::Value = serde_json::from_str(&dotnet_text)?;
  let dv_text = command_text(dv_executable, &["build", "--plan", "SmallConsole.csproj", "--json"], &verification)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("compiler_plan_created"))
    .ok_or("dv build plan did not emit compiler_plan_created")?;

  let reference_language = dotnet.pointer("/Properties/LangVersion").and_then(serde_json::Value::as_str);
  let actual_language = dv.get("language_version").and_then(serde_json::Value::as_str);
  if reference_language != actual_language {
    return Err(format!("compiler language mismatch: dotnet={reference_language:?}, dv={actual_language:?}").into());
  }
  let reference_defines = dotnet
    .pointer("/Properties/DefineConstants")
    .and_then(serde_json::Value::as_str)
    .unwrap_or_default()
    .split(';')
    .map(str::to_owned)
    .collect::<Vec<_>>();
  if reference_defines != string_array(&dv, "defines")? {
    return Err("compiler define batch does not match MSBuild".into());
  }
  compare_canonical_item_paths(&dotnet, "ReferencePath", &dv, "references")?;
  compare_canonical_item_paths(&dotnet, "Analyzer", &dv, "analyzers")?;

  let reference_sources = item_identities(&dotnet, "Compile")?
    .into_iter()
    .filter(|path| !path.replace('\\', "/").starts_with("obj/"))
    .collect::<Vec<_>>();
  let actual_sources = string_array(&dv, "sources")?
    .into_iter()
    .map(|path| {
      Path::new(&path)
        .strip_prefix(&verification)
        .unwrap_or(Path::new(&path))
        .to_string_lossy()
        .replace('\\', "/")
    })
    .collect::<Vec<_>>();
  if reference_sources != actual_sources {
    return Err(format!("compiler source batch mismatch: dotnet={reference_sources:?}, dv={actual_sources:?}").into());
  }
  Ok(())
}

fn verify_package_sync(repository: &Path, dv_executable: &Path, fixture: &Path, project_file: &str, expected_packages: usize) -> Result<()> {
  let verification_name = project_file.trim_end_matches(".csproj").to_ascii_lowercase();
  let root = repository.join(format!("target/benchmark-{verification_name}-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &root)?;
  let dotnet_workspace = root.join("dotnet");
  let dv_workspace = root.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;
  run_checked(
    Path::new("dotnet"),
    &[
      "restore",
      project_file,
      "--packages",
      ".packages",
      "--no-http-cache",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &dotnet_workspace,
    "package-sync verification restore",
  )?;
  let dv_text = command_text(dv_executable, &["restore", project_file, "--packages", ".packages", "--json"], &dv_workspace)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv restore did not emit package_resolution_created")?;
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join("obj/project.assets.json"))?)?;
  let framework = assets
    .pointer("/project/frameworks")
    .and_then(serde_json::Value::as_object)
    .and_then(|frameworks| frameworks.keys().next())
    .ok_or("dotnet assets omitted the project framework")?;
  if dv.get("target_framework").and_then(serde_json::Value::as_str) != Some(framework) {
    return Err("package-sync target framework does not match dotnet restore".into());
  }

  let reference_libraries = assets
    .get("libraries")
    .and_then(serde_json::Value::as_object)
    .ok_or("dotnet assets omitted libraries")?;
  let mut reference_packages = Vec::with_capacity(expected_packages);
  for (identity, library) in reference_libraries {
    if library.get("type").and_then(serde_json::Value::as_str) != Some("package") {
      continue;
    }
    let (id, version) = identity
      .split_once('/')
      .ok_or_else(|| format!("dotnet package identity {identity:?} omitted its version"))?;
    let lower_id = id.to_ascii_lowercase();
    let lower_version = version.to_ascii_lowercase();
    let hash_path = dotnet_workspace
      .join(".packages")
      .join(&lower_id)
      .join(&lower_version)
      .join(format!("{lower_id}.{lower_version}.nupkg.sha512"));
    let hash = fs::read_to_string(&hash_path)?;
    reference_packages.push((lower_id, lower_version, hash.trim().to_owned()));
  }
  reference_packages.sort_unstable();
  if reference_packages.len() != expected_packages {
    return Err(
      format!(
        "{project_file} resolved {} reference packages; expected {expected_packages}",
        reference_packages.len()
      )
      .into(),
    );
  }
  let mut actual_packages = dv
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv sync omitted packages")?
    .iter()
    .map(|package| {
      let id = package
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("dv package omitted id")?
        .to_ascii_lowercase();
      let version = package
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or("dv package omitted version")?
        .to_ascii_lowercase();
      let hash = package
        .get("sha512")
        .and_then(serde_json::Value::as_str)
        .ok_or("dv package omitted sha512")?
        .to_owned();
      Ok((id, version, hash))
    })
    .collect::<Result<Vec<_>>>()?;
  actual_packages.sort_unstable();
  if reference_packages != actual_packages {
    return Err(
      format!(
        "dv package identity, version, or hash batch differs from dotnet restore: dotnet={} dv={}",
        reference_packages.len(),
        actual_packages.len()
      )
      .into(),
    );
  }

  let target = assets
    .get("targets")
    .and_then(serde_json::Value::as_object)
    .and_then(|targets| targets.get(framework))
    .and_then(serde_json::Value::as_object)
    .ok_or("dotnet assets omitted the package target")?;
  let mut reference_compile = Vec::new();
  for (identity, package) in target {
    let Some(compile) = package.get("compile").and_then(serde_json::Value::as_object) else {
      continue;
    };
    let (id, version) = identity
      .split_once('/')
      .ok_or_else(|| format!("dotnet target identity {identity:?} omitted its version"))?;
    for asset in compile.keys().filter(|asset| !asset.ends_with("/_._")) {
      reference_compile.push(format!("{}/{}/{}", id.to_ascii_lowercase(), version.to_ascii_lowercase(), asset));
    }
  }
  reference_compile.sort_unstable();
  let mut actual_compile = string_array(&dv, "compile_assets")?
    .into_iter()
    .map(|path| package_relative_path(&path))
    .collect::<Result<Vec<_>>>()?;
  actual_compile.sort_unstable();
  if reference_compile != actual_compile {
    return Err(
      format!(
        "package compile asset batch differs: dotnet={} dv={}",
        reference_compile.len(),
        actual_compile.len()
      )
      .into(),
    );
  }
  let mut reference_analyzers = Vec::new();
  for (identity, library) in reference_libraries {
    if library.get("type").and_then(serde_json::Value::as_str) != Some("package") {
      continue;
    }
    let (id, version) = identity
      .split_once('/')
      .ok_or_else(|| format!("dotnet package identity {identity:?} omitted its version"))?;
    for asset in library
      .get("files")
      .and_then(serde_json::Value::as_array)
      .into_iter()
      .flatten()
      .filter_map(serde_json::Value::as_str)
    {
      let lower = asset.to_ascii_lowercase();
      if lower.starts_with("analyzers/") && lower.ends_with(".dll") && !lower.ends_with(".resources.dll") && !lower.contains("/dotnet/vb/") {
        reference_analyzers.push(format!("{}/{}/{}", id.to_ascii_lowercase(), version.to_ascii_lowercase(), asset));
      }
    }
  }
  reference_analyzers.sort_unstable();
  let mut actual_analyzers = string_array(&dv, "analyzers")?
    .into_iter()
    .map(|path| package_relative_path(&path))
    .collect::<Result<Vec<_>>>()?;
  actual_analyzers.sort_unstable();
  if reference_analyzers != actual_analyzers {
    let reference_only = reference_analyzers.iter().find(|asset| actual_analyzers.binary_search(asset).is_err());
    let actual_only = actual_analyzers.iter().find(|asset| reference_analyzers.binary_search(asset).is_err());
    return Err(
      format!(
        "package analyzer batch differs: dotnet={} dv={} first_dotnet_only={reference_only:?} first_dv_only={actual_only:?}",
        reference_analyzers.len(),
        actual_analyzers.len()
      )
      .into(),
    );
  }
  compare_package_asset_family(target, &dv, "runtime", &["runtime_assets"], false)?;
  compare_package_asset_family(target, &dv, "resource", &["resource_assets"], false)?;
  compare_package_asset_family(target, &dv, "contentFiles", &["content_files"], true)?;
  compare_package_asset_family(target, &dv, "build", &["build_assets", "build_transitive_assets"], true)?;
  compare_package_asset_family(target, &dv, "buildMultiTargeting", &["build_multi_targeting_assets"], true)?;
  compare_package_asset_family(target, &dv, "native", &["native_assets"], true)?;
  let mut actual_runtime_targets = dv
    .get("runtime_targets")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv sync omitted runtime_targets")?
    .iter()
    .map(|asset| {
      let path = asset.get("path").and_then(serde_json::Value::as_str).ok_or("dv runtime target omitted path")?;
      let rid = asset
        .get("runtime_identifier")
        .and_then(serde_json::Value::as_str)
        .ok_or("dv runtime target omitted runtime_identifier")?;
      let kind = asset.get("kind").and_then(serde_json::Value::as_str).ok_or("dv runtime target omitted kind")?;
      Ok((package_relative_path(path)?, rid.to_owned(), kind.to_owned()))
    })
    .collect::<Result<Vec<_>>>()?;
  actual_runtime_targets.sort_unstable();
  let mut reference_runtime_targets = Vec::new();
  for (identity, package) in target {
    let Some(assets) = package.get("runtimeTargets").and_then(serde_json::Value::as_object) else {
      continue;
    };
    let (id, version) = identity
      .split_once('/')
      .ok_or_else(|| format!("dotnet target identity {identity:?} omitted its version"))?;
    for (path, metadata) in assets {
      let rid = metadata.get("rid").and_then(serde_json::Value::as_str).unwrap_or_default();
      let kind = metadata.get("assetType").and_then(serde_json::Value::as_str).unwrap_or_default();
      reference_runtime_targets.push((
        format!("{}/{}/{}", id.to_ascii_lowercase(), version.to_ascii_lowercase(), path),
        rid.to_owned(),
        kind.to_owned(),
      ));
    }
  }
  reference_runtime_targets.sort_unstable();
  if reference_runtime_targets != actual_runtime_targets {
    return Err(
      format!(
        "package runtimeTargets differ: dotnet={} dv={}",
        reference_runtime_targets.len(),
        actual_runtime_targets.len()
      )
      .into(),
    );
  }
  Ok(())
}

fn verify_package_reference_metadata(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  verify_package_sync(repository, dv_executable, fixture, "MetadataProject.csproj", 1)?;
  let root = repository.join(format!("target/benchmark-package-reference-metadata-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &root)?;
  let dotnet_workspace = root.join("dotnet-policy");
  let dv_workspace = root.join("dv-policy");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;
  run_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "MetadataProject.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &dotnet_workspace,
    "PackageReference metadata oracle restore",
  )?;
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join("obj/project.assets.json"))?)?;
  let dependency = assets
    .pointer("/project/frameworks/net10.0/dependencies/Newtonsoft.Json")
    .ok_or("Microsoft assets omitted direct PackageReference policy")?;
  if dependency.get("include").and_then(serde_json::Value::as_str) != Some("Compile")
    || dependency.get("suppressParent").and_then(serde_json::Value::as_str) != Some("All")
    || dependency.get("aliases").and_then(serde_json::Value::as_str) != Some("JsonAlias")
    || dependency.get("generatePathProperty").and_then(serde_json::Value::as_bool) != Some(true)
  {
    return Err("Microsoft PackageReference policy oracle changed".into());
  }
  let reference_no_warn = dependency
    .get("noWarn")
    .and_then(serde_json::Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(serde_json::Value::as_str)
    .collect::<Vec<_>>();
  if reference_no_warn != ["NU1603", "NU1701"] {
    return Err(format!("Microsoft PackageReference NoWarn oracle changed: {reference_no_warn:?}").into());
  }
  let target = assets
    .pointer("/targets/net10.0/Newtonsoft.Json~113.0.3")
    .ok_or("Microsoft assets omitted the metadata fixture package target")?;
  let compile = target
    .get("compile")
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft assets omitted the included compile family")?;
  if compile
    .values()
    .any(|metadata| metadata.get("aliases").and_then(serde_json::Value::as_str) != Some("JsonAlias"))
  {
    return Err("Microsoft did not apply JsonAlias to every direct compile asset".into());
  }
  if target
    .get("runtime")
    .and_then(serde_json::Value::as_object)
    .is_none_or(|runtime| runtime.keys().any(|asset| !asset.ends_with("/_._")))
  {
    return Err("Microsoft did not exclude the runtime family".into());
  }
  let property_value = command_text(
    Path::new("dotnet"),
    &["msbuild", "MetadataProject.csproj", "-nologo", "-getProperty:PkgNewtonsoft_Json"],
    &dotnet_workspace,
  )?;
  let property_relative = relative_policy_path(&property_value, &dotnet_workspace)?;
  if property_relative != ".packages/newtonsoft.json/13.0.3" {
    return Err(format!("Microsoft generated a different package path property: {property_relative:?}").into());
  }

  let dv_text = command_text(
    dv_executable,
    &["restore", "MetadataProject.csproj", "--packages", ".packages", "--json"],
    &dv_workspace,
  )?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv restore omitted package_resolution_created")?;
  let policy = dv
    .get("direct_policies")
    .and_then(serde_json::Value::as_array)
    .and_then(|policies| policies.first())
    .ok_or("dv restore omitted direct package policy")?;
  if string_array(policy, "include_assets")? != ["compile"]
    || string_array(policy, "no_warn")? != ["NU1603", "NU1701"]
    || policy.get("aliases").and_then(serde_json::Value::as_str) != Some("JsonAlias")
  {
    return Err("dv direct PackageReference policy differs from Microsoft assets".into());
  }
  let path_property = policy.get("path_property").ok_or("dv direct policy omitted its path property")?;
  if path_property.get("name").and_then(serde_json::Value::as_str) != Some("PkgNewtonsoft_Json") {
    return Err("dv generated a different package path property name".into());
  }
  assert_relative_policy_path(path_property, "value", &dv_workspace, ".packages/newtonsoft.json/13.0.3")?;

  let plan_text = command_text(dv_executable, &["build", "--plan", "MetadataProject.csproj", "--json"], &dv_workspace)?;
  let plan = plan_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("compiler_plan_created"))
    .ok_or("dv build plan omitted compiler_plan_created")?;
  let reference_alias = plan
    .get("reference_aliases")
    .and_then(serde_json::Value::as_array)
    .and_then(|aliases| aliases.first())
    .ok_or("dv compiler plan omitted the direct package alias")?;
  if reference_alias.get("aliases").and_then(serde_json::Value::as_str) != Some("JsonAlias")
    || reference_alias
      .get("reference")
      .and_then(serde_json::Value::as_str)
      .is_none_or(|reference| !reference.ends_with("Newtonsoft.Json.dll"))
  {
    return Err("dv compiler plan attached JsonAlias to the wrong reference".into());
  }
  let plan_property = plan
    .get("package_path_properties")
    .and_then(serde_json::Value::as_array)
    .and_then(|properties| properties.first())
    .ok_or("dv compiler plan omitted PkgNewtonsoft_Json")?;
  if plan_property.get("name").and_then(serde_json::Value::as_str) != Some("PkgNewtonsoft_Json") {
    return Err("dv compiler plan generated a different package property name".into());
  }
  assert_relative_policy_path(plan_property, "value", &dv_workspace, ".packages/newtonsoft.json/13.0.3")?;
  Ok(())
}

fn verify_package_pruning(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  verify_package_sync(repository, dv_executable, fixture, "LegacyPruningProject.csproj", 1)?;
  let oracle_text = command_text(
    Path::new("dotnet"),
    &[
      "msbuild",
      "LegacyPruningProject.csproj",
      "--nologo",
      "-target:AddPrunePackageReferences",
      "-getItem:PrunePackageReference",
    ],
    fixture,
  )?;
  let oracle: serde_json::Value = serde_json::from_str(&oracle_text)?;
  let packages = oracle
    .pointer("/Items/PrunePackageReference")
    .and_then(serde_json::Value::as_array)
    .ok_or("package-pruning oracle omitted PrunePackageReference items")?;
  if packages.len() != 420 {
    return Err(format!("package-pruning oracle expected 420 merged .NET 9 identities, found {}", packages.len()).into());
  }
  for (id, version) in [("System.IO.Pipelines", "9.0.32767"), ("System.Text.Json", "9.0.32767")] {
    let matched = packages.iter().any(|package| {
      package.get("Identity").and_then(serde_json::Value::as_str) == Some(id) && package.get("Version").and_then(serde_json::Value::as_str) == Some(version)
    });
    if !matched {
      return Err(format!("package-pruning oracle omitted {id} {version}").into());
    }
  }
  Ok(())
}

fn verify_nuspec_framework_metadata(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join(format!("target/benchmark-nuspec-framework-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &root)?;
  reset_fixture(fixture, &root)?;
  write_nuspec_framework_packages(&root.join("feed"))?;

  run_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "FrameworkMetadata.csproj",
      "--packages",
      "dotnet-packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &root,
    "Microsoft nuspec framework metadata oracle",
  )?;
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(root.join("obj/project.assets.json"))?)?;
  let microsoft = assets
    .pointer("/targets/net10.0/Framework.Metadata~11.0.0")
    .ok_or("Microsoft assets omitted Framework.Metadata")?;
  let dependencies = microsoft
    .get("dependencies")
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft framework package omitted dependencies")?;
  if dependencies.len() != 1 || dependencies.get("Framework.Child").and_then(serde_json::Value::as_str) != Some("[1.0.0]") {
    return Err(format!("Microsoft selected the wrong nuspec dependency group: {dependencies:?}").into());
  }
  let microsoft_references = json_string_array(microsoft.get("frameworkReferences"), "Microsoft frameworkReferences")?;
  let microsoft_assemblies = microsoft
    .get("frameworkAssemblies")
    .map(|value| json_string_array(Some(value), "Microsoft frameworkAssemblies"))
    .transpose()?
    .unwrap_or_default();
  if microsoft_references != ["Microsoft.AspNetCore.App"] || !microsoft_assemblies.is_empty() {
    return Err(format!("Microsoft selected unexpected framework metadata: references={microsoft_references:?} assemblies={microsoft_assemblies:?}").into());
  }

  remove_generated_path(&root.join("obj"))?;
  run_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "LegacyFrameworkMetadata.csproj",
      "--packages",
      "dotnet-legacy-packages",
      "-p:AutomaticallyUseReferenceAssemblyPackages=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &root,
    "Microsoft legacy nuspec framework assembly oracle",
  )?;
  let legacy_assets: serde_json::Value = serde_json::from_slice(&fs::read(root.join("obj/project.assets.json"))?)?;
  let microsoft_legacy = legacy_assets
    .pointer("/targets/.NETFramework,Version=v4.8/Framework.Metadata~11.0.0")
    .ok_or("Microsoft legacy assets omitted Framework.Metadata")?;
  let microsoft_legacy_assemblies = json_string_array(microsoft_legacy.get("frameworkAssemblies"), "Microsoft legacy frameworkAssemblies")?;
  if microsoft_legacy_assemblies != ["System.Data", "System.Xml"] || microsoft_legacy.get("frameworkReferences").is_some() {
    return Err(format!("Microsoft selected unexpected legacy framework metadata: {microsoft_legacy}").into());
  }

  remove_generated_path(&root.join("obj"))?;
  let cold = command_text(
    dv_executable,
    &["restore", "FrameworkMetadata.csproj", "--packages", "dv-packages", "--offline", "--json"],
    &root,
  )?;
  let warm = command_text(
    dv_executable,
    &["restore", "FrameworkMetadata.csproj", "--packages", "dv-packages", "--offline", "--json"],
    &root,
  )?;
  for (state, output) in [("cold", &cold), ("warm", &warm)] {
    let resolution = json_event(output, "package_resolution_created").ok_or("dv framework restore omitted package_resolution_created")?;
    let packages = resolution
      .get("packages")
      .and_then(serde_json::Value::as_array)
      .ok_or("dv framework restore omitted packages")?;
    if packages.len() != 2 {
      return Err(format!("dv {state} framework restore selected {} packages instead of two", packages.len()).into());
    }
    let package = packages
      .iter()
      .find(|package| package.get("id").and_then(serde_json::Value::as_str) == Some("Framework.Metadata"))
      .ok_or("dv framework restore omitted Framework.Metadata")?;
    let references = json_string_array(package.get("framework_references"), "dv framework_references")?;
    let assemblies = json_string_array(package.get("framework_assemblies"), "dv framework_assemblies")?;
    if references != microsoft_references || assemblies != microsoft_assemblies {
      return Err(format!("dv {state} framework metadata differs: references={references:?} assemblies={assemblies:?}").into());
    }
    if output.contains("Unselected.Dependency") || output.contains("Microsoft.WindowsDesktop.App") || output.contains("System.Net.Http") {
      return Err(format!("dv {state} framework restore leaked an incompatible nuspec group").into());
    }
  }
  let cold_event = json_event(&cold, "package_resolution_created").expect("cold resolution was checked");
  let warm_event = json_event(&warm, "package_resolution_created").expect("warm resolution was checked");
  if cold_event.get("downloaded_packages").and_then(serde_json::Value::as_u64) != Some(2)
    || warm_event.get("cache_hits").and_then(serde_json::Value::as_u64) != Some(2)
  {
    return Err("dv framework metadata preflight did not exercise cold publication and warm-lock reuse".into());
  }
  let lock: serde_json::Value = serde_json::from_slice(&fs::read(root.join("dv.lock.json"))?)?;
  if lock.get("schema_version").and_then(serde_json::Value::as_u64) != Some(8) {
    return Err("dv framework metadata lock did not use schema 8".into());
  }
  remove_generated_path(&root.join("obj"))?;
  remove_generated_path(&root.join("dv.lock.json"))?;
  let legacy_cold = command_text(
    dv_executable,
    &[
      "restore",
      "LegacyFrameworkMetadata.csproj",
      "--packages",
      "dv-legacy-packages",
      "--offline",
      "--json",
    ],
    &root,
  )?;
  let legacy_warm = command_text(
    dv_executable,
    &[
      "restore",
      "LegacyFrameworkMetadata.csproj",
      "--packages",
      "dv-legacy-packages",
      "--offline",
      "--json",
    ],
    &root,
  )?;
  for (state, output) in [("cold", &legacy_cold), ("warm", &legacy_warm)] {
    let legacy_resolution = json_event(output, "package_resolution_created").ok_or("dv legacy framework restore omitted its resolution")?;
    let legacy_package = legacy_resolution
      .get("packages")
      .and_then(serde_json::Value::as_array)
      .and_then(|packages| {
        packages
          .iter()
          .find(|package| package.get("id").and_then(serde_json::Value::as_str) == Some("Framework.Metadata"))
      })
      .ok_or("dv legacy framework restore omitted Framework.Metadata")?;
    let legacy_references = json_string_array(legacy_package.get("framework_references"), "dv legacy framework_references")?;
    let legacy_assemblies = json_string_array(legacy_package.get("framework_assemblies"), "dv legacy framework_assemblies")?;
    if !legacy_references.is_empty() || legacy_assemblies != microsoft_legacy_assemblies {
      return Err(format!("dv legacy {state} framework metadata differs: references={legacy_references:?} assemblies={legacy_assemblies:?}").into());
    }
  }
  let legacy_cold_event = json_event(&legacy_cold, "package_resolution_created").expect("legacy cold resolution was checked");
  let legacy_warm_event = json_event(&legacy_warm, "package_resolution_created").expect("legacy warm resolution was checked");
  if legacy_cold_event.get("downloaded_packages").and_then(serde_json::Value::as_u64) != Some(1)
    || legacy_warm_event.get("cache_hits").and_then(serde_json::Value::as_u64) != Some(1)
  {
    return Err("dv legacy framework metadata preflight did not exercise cold publication and warm-lock reuse".into());
  }
  Ok(())
}

fn verify_package_rid_content(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join(format!("target/benchmark-rid-content-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &root)?;
  reset_fixture(fixture, &root)?;
  write_rid_content_package(&root.join("feed"))?;

  for (project, target, project_rid, asset_rid) in [
    ("Portable.csproj", "net10.0", None, None),
    ("WindowsExact.csproj", "net10.0/win-x64", Some("win-x64"), Some("win-x64")),
    ("LinuxExact.csproj", "net10.0/linux-x64", Some("linux-x64"), Some("linux-x64")),
    ("WindowsFallback.csproj", "net10.0/win-arm64", Some("win-arm64"), Some("win")),
  ] {
    remove_generated_path(&root.join("obj"))?;
    run_checked(
      Path::new("dotnet"),
      &[
        "restore",
        project,
        "--packages",
        "dotnet-packages",
        "--no-http-cache",
        "--nologo",
        "--verbosity",
        "quiet",
      ],
      &root,
      "Microsoft RID/content asset oracle",
    )?;
    let assets: serde_json::Value = serde_json::from_slice(&fs::read(root.join("obj/project.assets.json"))?)?;
    let microsoft = assets
      .get("targets")
      .and_then(serde_json::Value::as_object)
      .and_then(|targets| targets.get(target))
      .and_then(serde_json::Value::as_object)
      .and_then(|target| target.get("Rid.Content.Metadata/1.0.0"))
      .ok_or_else(|| format!("Microsoft assets omitted Rid.Content.Metadata from {target}"))?;

    remove_generated_path(&root.join("dv.lock.json"))?;
    remove_generated_path(&root.join("dv-packages"))?;
    remove_generated_path(&root.join("dv-packages"))?;
    let cold = command_text(dv_executable, &["restore", project, "--packages", "dv-packages", "--offline", "--json"], &root)?;
    let warm = command_text(dv_executable, &["restore", project, "--packages", "dv-packages", "--offline", "--json"], &root)?;
    let cold_event = json_event(&cold, "package_resolution_created").ok_or("dv RID/content cold restore omitted its resolution")?;
    let warm_event = json_event(&warm, "package_resolution_created").ok_or("dv RID/content warm restore omitted its resolution")?;
    if cold_event.get("downloaded_packages").and_then(serde_json::Value::as_u64) != Some(1)
      || warm_event.get("cache_hits").and_then(serde_json::Value::as_u64) != Some(1)
    {
      return Err(format!("{project} did not exercise one cold publication followed by one warm lock hit").into());
    }
    let actual_project_rid = cold_event.get("runtime_identifier").and_then(serde_json::Value::as_str);
    if actual_project_rid != project_rid || (project_rid.is_none() && !cold_event.get("runtime_identifier").is_some_and(serde_json::Value::is_null)) {
      return Err(format!("{project} reported the wrong runtime identifier").into());
    }

    for (microsoft_family, dv_family) in [
      ("compile", "compile_assets"),
      ("runtime", "runtime_assets"),
      ("resource", "resource_assets"),
      ("native", "native_assets"),
    ] {
      let expected = object_keys(microsoft.get(microsoft_family));
      let actual = dv_package_paths(&cold_event, dv_family)?;
      if expected != actual {
        return Err(format!("{project} {microsoft_family} assets differ: Microsoft={expected:?} dv={actual:?}").into());
      }
      if actual != dv_package_paths(&warm_event, dv_family)? {
        return Err(format!("{project} {microsoft_family} assets changed on the warm lock path").into());
      }
    }

    let expected_runtime_targets = microsoft_runtime_targets(microsoft.get("runtimeTargets"))?;
    let actual_runtime_targets = dv_runtime_targets(&cold_event)?;
    if expected_runtime_targets != actual_runtime_targets || actual_runtime_targets != dv_runtime_targets(&warm_event)? {
      return Err(format!("{project} runtimeTargets differ: Microsoft={expected_runtime_targets:?} dv={actual_runtime_targets:?}").into());
    }
    if let Some(expected_rid) = asset_rid {
      if !expected_runtime_targets.is_empty() {
        return Err(format!("concrete project {project} unexpectedly retained portable runtimeTargets").into());
      }
      let runtime = dv_package_paths(&cold_event, "runtime_assets")?;
      if runtime.iter().any(|path| !path.starts_with(&format!("runtimes/{expected_rid}/"))) {
        return Err(format!("{project} did not select the expected {expected_rid} runtime group: {runtime:?}").into());
      }
    } else if expected_runtime_targets.len() < 9 {
      return Err(format!("portable oracle did not exercise all runtime/resource/native RID targets: {expected_runtime_targets:?}").into());
    }

    let expected_content = microsoft_content_files(microsoft.get("contentFiles"))?;
    let actual_content = dv_content_files(&cold_event)?;
    if expected_content != actual_content || actual_content != dv_content_files(&warm_event)? {
      return Err(format!("{project} contentFiles differ: Microsoft={expected_content:?} dv={actual_content:?}").into());
    }
  }

  let lock: serde_json::Value = serde_json::from_slice(&fs::read(root.join("dv.lock.json"))?)?;
  if lock.get("schema_version").and_then(serde_json::Value::as_u64) != Some(8) {
    return Err("RID/content lock did not use schema 8".into());
  }
  Ok(())
}

fn object_keys(value: Option<&serde_json::Value>) -> Vec<String> {
  let mut keys = value
    .and_then(serde_json::Value::as_object)
    .map(|values| values.keys().cloned().collect::<Vec<_>>())
    .unwrap_or_default();
  keys.sort_unstable();
  keys
}

fn dv_package_relative_path(path: &str) -> Result<String> {
  let normalized = path.replace('\\', "/");
  let marker = "/rid.content.metadata/1.0.0/";
  normalized
    .to_ascii_lowercase()
    .find(marker)
    .map(|index| normalized[index + marker.len()..].to_owned())
    .ok_or_else(|| format!("dv asset is outside Rid.Content.Metadata: {path}").into())
}

fn dv_package_paths(event: &serde_json::Value, field: &str) -> Result<Vec<String>> {
  let mut paths = event
    .get(field)
    .and_then(serde_json::Value::as_array)
    .ok_or_else(|| format!("dv RID/content event omitted {field}"))?
    .iter()
    .map(|value| {
      value
        .as_str()
        .ok_or_else(|| format!("dv {field} contains a non-string path").into())
        .and_then(dv_package_relative_path)
    })
    .collect::<Result<Vec<_>>>()?;
  paths.sort_unstable();
  Ok(paths)
}

fn microsoft_runtime_targets(value: Option<&serde_json::Value>) -> Result<Vec<(String, String, String)>> {
  let mut targets = value
    .and_then(serde_json::Value::as_object)
    .map(|targets| {
      targets
        .iter()
        .map(|(path, metadata)| {
          Ok((
            path.clone(),
            metadata
              .get("rid")
              .and_then(serde_json::Value::as_str)
              .ok_or("Microsoft runtime target omitted rid")?
              .to_owned(),
            metadata
              .get("assetType")
              .and_then(serde_json::Value::as_str)
              .ok_or("Microsoft runtime target omitted assetType")?
              .to_owned(),
          ))
        })
        .collect::<Result<Vec<_>>>()
    })
    .transpose()?
    .unwrap_or_default();
  targets.sort_unstable();
  Ok(targets)
}

fn dv_runtime_targets(event: &serde_json::Value) -> Result<Vec<(String, String, String)>> {
  let mut targets = event
    .get("runtime_targets")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv RID/content event omitted runtime_targets")?
    .iter()
    .map(|target| {
      Ok((
        dv_package_relative_path(target.get("path").and_then(serde_json::Value::as_str).ok_or("dv runtime target omitted path")?)?,
        target
          .get("runtime_identifier")
          .and_then(serde_json::Value::as_str)
          .ok_or("dv runtime target omitted runtime_identifier")?
          .to_owned(),
        target
          .get("kind")
          .and_then(serde_json::Value::as_str)
          .ok_or("dv runtime target omitted kind")?
          .to_owned(),
      ))
    })
    .collect::<Result<Vec<_>>>()?;
  targets.sort_unstable();
  Ok(targets)
}

fn microsoft_content_files(value: Option<&serde_json::Value>) -> Result<Vec<(String, String, bool, bool)>> {
  let mut files = value
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft assets omitted contentFiles")?
    .iter()
    .map(|(path, metadata)| {
      let build_action = metadata.get("buildAction").and_then(serde_json::Value::as_str).unwrap_or("Compile").to_owned();
      let copy = metadata.get("copyToOutput").is_some_and(|value| {
        value
          .as_bool()
          .unwrap_or_else(|| value.as_str().is_some_and(|value| value.eq_ignore_ascii_case("true")))
      });
      let flatten = path.ends_with("template.cs.pp");
      (path.clone(), build_action, copy, flatten)
    })
    .collect::<Vec<_>>();
  files.sort_unstable();
  Ok(files)
}

fn dv_content_files(event: &serde_json::Value) -> Result<Vec<(String, String, bool, bool)>> {
  let mut files = event
    .get("content_files")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv RID/content event omitted content_files")?
    .iter()
    .map(|file| {
      Ok((
        dv_package_relative_path(file.get("path").and_then(serde_json::Value::as_str).ok_or("dv content file omitted path")?)?,
        file
          .get("build_action")
          .and_then(serde_json::Value::as_str)
          .ok_or("dv content file omitted build_action")?
          .to_owned(),
        file
          .get("copy_to_output")
          .and_then(serde_json::Value::as_bool)
          .ok_or("dv content file omitted copy_to_output")?,
        file
          .get("flatten")
          .and_then(serde_json::Value::as_bool)
          .ok_or("dv content file omitted flatten")?,
      ))
    })
    .collect::<Result<Vec<_>>>()?;
  files.sort_unstable();
  Ok(files)
}

fn json_string_array<'a>(value: Option<&'a serde_json::Value>, meaning: &str) -> Result<Vec<&'a str>> {
  value
    .and_then(serde_json::Value::as_array)
    .ok_or_else(|| -> Box<dyn Error> { format!("{meaning} is absent or is not an array").into() })?
    .iter()
    .map(|value| value.as_str().ok_or_else(|| format!("{meaning} contains a non-string value").into()))
    .collect()
}

fn verify_central_package_management(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  verify_package_sync(repository, dv_executable, fixture, "CentralPackages.csproj", 54)?;
  let root = repository.join(format!("target/benchmark-centralpackages-verification-{}", std::process::id()));
  let dotnet_workspace = root.join("dotnet");
  let dv_workspace = root.join("dv");
  run_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "CentralPackages.csproj",
      "--use-lock-file",
      "--packages",
      ".packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &dotnet_workspace,
    "central package management lock oracle",
  )?;
  let lock: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join("packages.lock.json"))?)?;
  let central_role = lock.pointer("/dependencies/net10.0/Humanizer.Core/type").and_then(serde_json::Value::as_str);
  if central_role != Some("CentralTransitive") {
    return Err(format!("Microsoft central pin oracle changed: Humanizer.Core role={central_role:?}").into());
  }

  let inspect = command_text(dv_executable, &["project", "inspect", "CentralPackages.csproj", "--json"], &dv_workspace)?;
  let project = json_event(&inspect, "project_evaluated").ok_or("dv central project inspection omitted project_evaluated")?;
  if project.get("central_package_management").and_then(serde_json::Value::as_bool) != Some(true)
    || project.get("central_transitive_pinning").and_then(serde_json::Value::as_bool) != Some(true)
  {
    return Err("dv did not expose the enabled central package policy".into());
  }
  let central_versions = project
    .get("central_package_versions")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv project inspection omitted central package versions")?;
  let expected = [
    ("Humanizer", "2.14.1"),
    ("Humanizer.Core", "2.14.1"),
    ("Microsoft.SourceLink.GitHub", "8.0.0"),
    ("Newtonsoft.Json", "13.0.3"),
  ];
  let actual = central_versions
    .iter()
    .map(|package| {
      Ok((
        package.get("id").and_then(serde_json::Value::as_str).ok_or("central package omitted id")?,
        package
          .get("version")
          .and_then(serde_json::Value::as_str)
          .ok_or("central package omitted version")?,
      ))
    })
    .collect::<Result<Vec<_>>>()?;
  if actual != expected {
    return Err(format!("dv selected a different central version batch: {actual:?}").into());
  }

  let restored = command_text(
    dv_executable,
    &["restore", "CentralPackages.csproj", "--packages", ".packages", "--offline", "--json"],
    &dv_workspace,
  )?;
  let resolution = json_event(&restored, "package_resolution_created").ok_or("dv central restore omitted package_resolution_created")?;
  let pinned = resolution
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .and_then(|packages| {
      packages
        .iter()
        .find(|package| package.get("id").and_then(serde_json::Value::as_str) == Some("Humanizer.Core"))
    })
    .ok_or("dv central restore omitted Humanizer.Core")?;
  if pinned.get("central_transitive").and_then(serde_json::Value::as_bool) != Some(true)
    || pinned.get("direct").and_then(serde_json::Value::as_bool) != Some(false)
  {
    return Err("dv did not preserve the central-transitive package role".into());
  }
  Ok(())
}

fn verify_package_conflict_resolution(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join(format!("target/benchmark-conflict-resolution-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &root)?;
  reset_fixture(fixture, &root)?;
  for package in CONFLICT_PACKAGES {
    write_conflict_package(&root.join("feed"), package)?;
  }
  let reference = Command::new("dotnet")
    .args([
      "restore",
      "ConflictResolution.csproj",
      "--packages",
      "dotnet-packages",
      "-p:WarningsAsErrors=",
      "--nologo",
      "--verbosity",
      "minimal",
    ])
    .current_dir(&root)
    .output()?;
  if !reference.status.success() {
    return Err(
      format!(
        "Microsoft package conflict oracle failed: {}{}",
        String::from_utf8_lossy(&reference.stdout),
        String::from_utf8_lossy(&reference.stderr)
      )
      .into(),
    );
  }
  let reference_text = format!("{}{}", String::from_utf8_lossy(&reference.stdout), String::from_utf8_lossy(&reference.stderr));
  if !reference_text.contains("warning NU1605") || !reference_text.contains("Direct.Leaf") {
    return Err(format!("Microsoft package conflict oracle omitted its NU1605 downgrade warning: {reference_text}").into());
  }
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(root.join("obj/project.assets.json"))?)?;
  let mut microsoft = assets
    .get("libraries")
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft package conflict oracle omitted libraries")?
    .keys()
    .map(|key| {
      key
        .split_once('/')
        .map(|(id, version)| (id.to_owned(), version.to_owned()))
        .ok_or_else(|| format!("Microsoft package conflict library has no version: {key}").into())
    })
    .collect::<Result<Vec<_>>>()?;
  microsoft.sort_unstable();

  remove_generated_path(&root.join("obj"))?;
  let restored = command_text(
    dv_executable,
    &["restore", "ConflictResolution.csproj", "--packages", "dv-packages", "--offline", "--json"],
    &root,
  )?;
  validate_downgrade_warning(&restored)?;
  let warm = command_text(
    dv_executable,
    &["restore", "ConflictResolution.csproj", "--packages", "dv-packages", "--offline", "--json"],
    &root,
  )?;
  validate_downgrade_warning(&warm)?;
  let resolution = json_event(&restored, "package_resolution_created").ok_or("dv conflict restore omitted package_resolution_created")?;
  let mut dv = resolution
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv conflict restore omitted packages")?
    .iter()
    .map(|package| {
      Ok((
        package
          .get("id")
          .and_then(serde_json::Value::as_str)
          .ok_or("dv conflict package omitted id")?
          .to_owned(),
        package
          .get("version")
          .and_then(serde_json::Value::as_str)
          .ok_or("dv conflict package omitted version")?
          .to_owned(),
      ))
    })
    .collect::<Result<Vec<_>>>()?;
  dv.sort_unstable();
  let expected = expected_conflict_packages();
  if microsoft != expected || dv != expected {
    return Err(format!("package conflict selection differs: expected={expected:?} Microsoft={microsoft:?} dv={dv:?}").into());
  }
  Ok(())
}

fn validate_downgrade_warning(output: &str) -> Result<()> {
  let diagnostic = json_event(output, "diagnostic").ok_or("dv conflict restore omitted its downgrade diagnostic")?;
  if diagnostic.pointer("/diagnostic/code").and_then(serde_json::Value::as_str) != Some("DV0413")
    || diagnostic.pointer("/diagnostic/severity").and_then(serde_json::Value::as_str) != Some("warning")
  {
    return Err(format!("dv emitted an unstable downgrade diagnostic: {diagnostic}").into());
  }
  let context = diagnostic
    .pointer("/diagnostic/context")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv downgrade diagnostic omitted context")?;
  for (name, expected) in [
    ("package_id", "Direct.Leaf"),
    ("selected_version", "1.0.0"),
    ("required_range", "[2.0.0]"),
    ("requesting_package", "Direct.Deep"),
  ] {
    let actual = context.iter().find_map(|field| {
      (field.get("name").and_then(serde_json::Value::as_str) == Some(name))
        .then(|| field.get("value").and_then(serde_json::Value::as_str))
        .flatten()
    });
    if actual != Some(expected) {
      return Err(format!("dv downgrade context {name} mismatch: expected={expected:?} actual={actual:?}").into());
    }
  }
  Ok(())
}

fn verify_package_batch_resolution(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join(format!("target/benchmark-package-batch-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &root)?;
  reset_fixture(fixture, &root)?;
  for package in CONFLICT_PACKAGES {
    write_conflict_package(&root.join("feed"), package)?;
  }
  let reference = Command::new("dotnet")
    .args([
      "restore",
      "PackageBatch.csproj",
      "--packages",
      "dotnet-batch-packages",
      "--nologo",
      "--verbosity",
      "quiet",
    ])
    .current_dir(&root)
    .output()?;
  check_output(
    reference,
    Path::new("dotnet"),
    &["restore", "PackageBatch.csproj"],
    "Microsoft package batch oracle",
  )?;

  let expected = expected_batch_packages();
  for relative in ["BatchLeft/obj/project.assets.json", "BatchRight/obj/project.assets.json"] {
    let assets: serde_json::Value = serde_json::from_slice(&fs::read(root.join(relative))?)?;
    let mut actual = assets
      .get("libraries")
      .and_then(serde_json::Value::as_object)
      .ok_or("Microsoft package batch oracle omitted libraries")?
      .keys()
      .map(|key| {
        key
          .split_once('/')
          .map(|(id, version)| (id.to_owned(), version.to_owned()))
          .ok_or_else(|| format!("Microsoft package batch library has no version: {key}").into())
      })
      .collect::<Result<Vec<_>>>()?;
    actual.sort_unstable();
    if actual != expected {
      return Err(format!("Microsoft package batch differs: expected={expected:?} actual={actual:?}").into());
    }
  }

  let actual = Command::new(dv_executable)
    .args(["restore", "PackageBatch.csproj", "--packages", "dv-batch-packages", "--offline", "--json"])
    .current_dir(&root)
    .output()?;
  check_output(actual.clone(), dv_executable, &["restore", "PackageBatch.csproj"], "dv package batch oracle")?;
  let events = String::from_utf8(actual.stdout)?
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?;
  let resolutions = events
    .iter()
    .filter(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .collect::<Vec<_>>();
  if resolutions.len() != 3 {
    return Err(format!("dv package batch emitted {} resolution events instead of three", resolutions.len()).into());
  }
  if resolutions[0]
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .is_none_or(|packages| !packages.is_empty())
  {
    return Err("dv package batch root should have an empty package graph".into());
  }
  let mut downloaded = 0u64;
  for (index, resolution) in resolutions.iter().enumerate().skip(1) {
    let mut packages = resolution
      .get("packages")
      .and_then(serde_json::Value::as_array)
      .ok_or("dv package batch resolution omitted packages")?
      .iter()
      .map(|package| {
        Ok((
          package
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or("dv batch package omitted id")?
            .to_owned(),
          package
            .get("version")
            .and_then(serde_json::Value::as_str)
            .ok_or("dv batch package omitted version")?
            .to_owned(),
        ))
      })
      .collect::<Result<Vec<_>>>()?;
    packages.sort_unstable();
    if packages != expected {
      return Err(format!("dv package batch project {index} differs: expected={expected:?} actual={packages:?}").into());
    }
    if resolution.get("network_requests").and_then(serde_json::Value::as_u64) != Some(0) {
      return Err("dv package batch unexpectedly used HTTP".into());
    }
    downloaded += resolution
      .get("downloaded_packages")
      .and_then(serde_json::Value::as_u64)
      .ok_or("dv package batch omitted downloaded_packages")?;
  }
  if downloaded != expected.len() as u64 || resolutions[2].get("cache_hits").and_then(serde_json::Value::as_u64) != Some(expected.len() as u64) {
    return Err(
      format!(
        "dv package batch did not deduplicate archive publication: downloaded={downloaded} second={}",
        resolutions[2]
      )
      .into(),
    );
  }
  Ok(())
}

fn expected_conflict_packages() -> Vec<(String, String)> {
  let mut expected = [
    ("Cousin.Current", "1.0.0"),
    ("Cousin.Deep", "1.0.0"),
    ("Cousin.Leaf", "2.0.0"),
    ("Cousin.Left", "1.0.0"),
    ("Cousin.Right", "1.0.0"),
    ("Diamond.Leaf", "2.0.0"),
    ("Diamond.Provider", "1.0.0"),
    ("Diamond.Relational", "1.0.0"),
    ("Direct.Deep", "1.0.0"),
    ("Direct.Leaf", "1.0.0"),
    ("Direct.Top", "1.0.0"),
  ]
  .into_iter()
  .map(|(id, version)| (id.to_owned(), version.to_owned()))
  .collect::<Vec<_>>();
  expected.sort_unstable();
  expected
}

fn expected_batch_packages() -> Vec<(String, String)> {
  let mut expected = [
    ("Cousin.Current", "1.0.0"),
    ("Cousin.Deep", "1.0.0"),
    ("Cousin.Leaf", "2.0.0"),
    ("Cousin.Left", "1.0.0"),
    ("Cousin.Right", "1.0.0"),
    ("Diamond.Leaf", "2.0.0"),
    ("Diamond.Provider", "1.0.0"),
    ("Diamond.Relational", "1.0.0"),
  ]
  .into_iter()
  .map(|(id, version)| (id.to_owned(), version.to_owned()))
  .collect::<Vec<_>>();
  expected.sort_unstable();
  expected
}

fn verify_package_diagnostics(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join(format!("target/benchmark-package-diagnostic-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &root)?;
  reset_fixture(fixture, &root)?;
  for package in DIAGNOSTIC_PACKAGES {
    write_conflict_package(&root.join("feed"), package)?;
  }
  write_incompatible_package(&root.join("feed"))?;
  let reference = Command::new("dotnet")
    .args([
      "restore",
      "ConflictFailure.csproj",
      "--packages",
      "dotnet-packages",
      "--nologo",
      "--verbosity",
      "minimal",
    ])
    .current_dir(&root)
    .output()?;
  validate_package_diagnostic(&reference, true)?;

  let actual = Command::new(dv_executable)
    .args(["restore", "ConflictFailure.csproj", "--packages", "dv-packages", "--offline", "--json"])
    .current_dir(&root)
    .output()?;
  validate_package_diagnostic(&actual, false)?;

  for (project, reference_code, dv_code, identity) in [
    ("CycleFailure.csproj", "NU1108", "DV0415", "Diagnostic.Cycle.A"),
    ("MissingPackageFailure.csproj", "NU1101", "DV0416", "Diagnostic.Absent"),
    ("MissingVersionFailure.csproj", "NU1102", "DV0417", "Diagnostic.Known"),
    ("IncompatibleFailure.csproj", "NU1202", "DV0402", "Diagnostic.Future"),
  ] {
    remove_generated_path(&root.join("obj"))?;
    remove_generated_path(&root.join("dv.lock.json"))?;
    let reference = Command::new("dotnet")
      .args([
        "restore",
        project,
        "--packages",
        "dotnet-diagnostic-packages",
        "--nologo",
        "--verbosity",
        "minimal",
      ])
      .current_dir(&root)
      .output()?;
    validate_reference_package_error(&reference, reference_code, identity)?;
    let actual = Command::new(dv_executable)
      .args(["restore", project, "--packages", "dv-diagnostic-packages", "--offline", "--json"])
      .current_dir(&root)
      .output()?;
    validate_dv_package_error(&actual, dv_code, identity)?;
  }
  Ok(())
}

fn verify_nuget_floating_version(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let oracle_root = repository.join(format!("target/benchmark-floating-version-oracle-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &oracle_root)?;
  reset_fixture(fixture, &oracle_root)?;
  run_checked(
    Path::new("dotnet"),
    &[
      "build",
      "oracle/FloatingVersionOracle.csproj",
      "-c",
      "Release",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &oracle_root,
    "NuGet floating-version oracle build",
  )?;
  let patterns = [
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
    "[*]",
    "[1.0,2.*)",
  ];
  let args = std::iter::once("oracle/bin/Release/net10.0/FloatingVersionOracle.dll")
    .chain(patterns)
    .collect::<Vec<_>>();
  let reference = command_text(Path::new("dotnet"), &args, &oracle_root)?;
  let expected = [
    "*|0.0.0||Major|",
    "*-*|0.0.0-0||AbsoluteLatest|",
    "0*|0.0.0||None|",
    "1.*|1.0.0||Minor|",
    "1.2*|1.20.0||Minor|",
    "1.2.*|1.2.0||Patch|",
    "1.2.3.*|1.2.3||Revision|",
    "1.2.3-*|1.2.3-0||Prerelease|",
    "1.2.3-rc.*|1.2.3-rc.0||Prerelease|rc.",
    "1.2.*-*|1.2.0-0||PrereleasePatch|",
    "1.2.*-preview.1.*|1.2.0-preview.1.0||PrereleasePatch|preview.1.",
    "*-rc.*|0.0.0-rc.0||PrereleaseMajor|rc.",
    "[1.*,2.0)|1.0.0|2.0.0|Minor|",
    "[1.2.0-rc.*, )|1.2.0-rc.0||Prerelease|rc.",
    "[*]|invalid",
    "[1.0,2.*)|invalid",
  ];
  if reference.lines().ne(expected) {
    return Err(format!("selected SDK NuGet floating parser changed: {reference:?}").into());
  }
  prepare_nuget_floating_version(&oracle_root)?;
  verify_package_sync(repository, dv_executable, &oracle_root, "FloatingVersion.csproj", 1)
}

fn verify_nuget_config_hierarchy(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join("target/benchmark-nuget-config-verification");
  ensure_workspace_is_safe(repository, &root)?;
  let dotnet_workspace = root.join("dotnet");
  let dv_workspace = root.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;
  let working_directory = dotnet_workspace
    .to_str()
    .ok_or("NuGet configuration verification workspace is not valid UTF-8")?;
  let path_text = nuget_config_command_text(
    Path::new("dotnet"),
    &["nuget", "config", "paths", "--working-directory", working_directory],
    &dotnet_workspace,
  )?;
  let actual_paths = path_text.lines().map(normalize_windows_path).collect::<Vec<_>>();
  let expected_paths = [
    dotnet_workspace.join("NuGet.Config"),
    dotnet_workspace.join("scopes/user/NuGet/NuGet.Config"),
    dotnet_workspace.join("scopes/user/NuGet/config/10.config"),
    dotnet_workspace.join("scopes/user/NuGet/config/20.Config"),
    dotnet_workspace.join("scopes/machine/NuGet/Config/10.config"),
    dotnet_workspace.join("scopes/machine/NuGet/Config/20.config"),
  ]
  .map(|path| normalize_windows_path(&path.to_string_lossy()))
  .into_iter()
  .collect::<Vec<_>>();
  if actual_paths != expected_paths {
    return Err(format!("dotnet NuGet configuration path precedence mismatch: expected={expected_paths:?} actual={actual_paths:?}").into());
  }
  let dotnet_args = [
    "restore",
    "ConfigHierarchy.csproj",
    "--use-lock-file",
    "--no-http-cache",
    "-p:NuGetAudit=false",
    "--nologo",
    "--verbosity",
    "quiet",
  ];
  run_nuget_config_checked(
    Path::new("dotnet"),
    &dotnet_args,
    &dotnet_workspace,
    "NuGet configuration hierarchy verification restore",
  )?;
  let dv_text = nuget_config_command_text(dv_executable, &["restore", "ConfigHierarchy.csproj", "--json"], &dv_workspace)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv configuration hierarchy restore did not emit package_resolution_created")?;
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join("obj/project.assets.json"))?)?;
  let reference_sources = assets
    .pointer("/project/restore/sources")
    .and_then(serde_json::Value::as_object)
    .ok_or("dotnet configuration hierarchy assets omitted restore sources")?;
  if !reference_sources.contains_key("https://api.nuget.org/v3/index.json") {
    return Err("dotnet configuration hierarchy did not retain the repository NuGet source".into());
  }
  let package_folders = assets
    .get("packageFolders")
    .and_then(serde_json::Value::as_object)
    .ok_or("dotnet configuration hierarchy assets omitted packageFolders")?;
  if package_folders.len() != 1 {
    return Err(
      format!(
        "dotnet configuration hierarchy selected {} package folders instead of one",
        package_folders.len()
      )
      .into(),
    );
  }
  let reference_cache = package_folders
    .keys()
    .next()
    .ok_or("dotnet configuration hierarchy omitted its package folder")?;
  let actual_cache = required_string(&dv, "cache_root")?;
  let reference_cache = fs::canonicalize(reference_cache)?;
  let actual_cache = fs::canonicalize(actual_cache)?;
  let reference_relative = reference_cache.strip_prefix(fs::canonicalize(&dotnet_workspace)?)?;
  let actual_relative = actual_cache.strip_prefix(fs::canonicalize(&dv_workspace)?)?;
  if normalize_windows_path(&reference_relative.to_string_lossy()) != normalize_windows_path(&actual_relative.to_string_lossy()) {
    return Err(format!("NuGet configuration cache precedence mismatch: dotnet={reference_cache:?} dv={actual_cache:?}").into());
  }
  if required_string(&dv, "source")? != "nuget.org" || required_string(&dv, "source_protocol")? != "v3" {
    return Err("dv configuration hierarchy did not select the repository NuGet v3 source".into());
  }

  let reference_library = assets
    .get("libraries")
    .and_then(serde_json::Value::as_object)
    .and_then(|libraries| libraries.get("Newtonsoft.Json/13.0.3"))
    .ok_or("dotnet configuration hierarchy did not resolve Newtonsoft.Json 13.0.3")?;
  if reference_library.get("type").and_then(serde_json::Value::as_str) != Some("package") {
    return Err("dotnet configuration hierarchy resolved Newtonsoft.Json as a non-package library".into());
  }
  let package = dv
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .and_then(|packages| {
      packages
        .iter()
        .find(|package| package.get("id").and_then(serde_json::Value::as_str) == Some("Newtonsoft.Json"))
    })
    .ok_or("dv configuration hierarchy did not resolve Newtonsoft.Json")?;
  if package.get("version").and_then(serde_json::Value::as_str) != Some("13.0.3") {
    return Err("dv configuration hierarchy selected a different Newtonsoft.Json version".into());
  }
  let reference_hash = fs::read_to_string(dotnet_workspace.join(".packages/newtonsoft.json/13.0.3/newtonsoft.json.13.0.3.nupkg.sha512"))?;
  if package.get("sha512").and_then(serde_json::Value::as_str) != Some(reference_hash.trim()) {
    return Err("NuGet configuration hierarchy package hash differs between dotnet and dv".into());
  }
  Ok(())
}

fn verify_nuget_config_merge(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join("target/benchmark-nuget-merge-verification");
  ensure_workspace_is_safe(repository, &root)?;
  let dotnet_workspace = root.join("dotnet");
  let dv_workspace = root.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;

  let dotnet_args = [
    "restore",
    "ConfigMerge.csproj",
    "--use-lock-file",
    "--no-http-cache",
    "-p:NuGetAudit=false",
    "--nologo",
    "--verbosity",
    "quiet",
  ];
  run_nuget_config_checked(
    Path::new("dotnet"),
    &dotnet_args,
    &dotnet_workspace,
    "NuGet configuration merge verification restore",
  )?;
  let dv_text = nuget_config_command_text(dv_executable, &["restore", "ConfigMerge.csproj", "--json"], &dv_workspace)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv configuration merge restore did not emit package_resolution_created")?;
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join("obj/project.assets.json"))?)?;
  let reference_sources = assets
    .pointer("/project/restore/sources")
    .and_then(serde_json::Value::as_object)
    .ok_or("dotnet configuration merge assets omitted restore sources")?;
  if !reference_sources.contains_key("https://api.nuget.org/v3/index.json") {
    return Err("dotnet configuration merge did not select the environment-expanded source".into());
  }
  for removed in [
    "https://machine.example.test/v3/index.json",
    "https://transient.example.test/v3/index.json",
    "https://disabled.example.test/v3/index.json",
  ] {
    if reference_sources.contains_key(removed) {
      return Err(format!("dotnet configuration merge retained disabled or cleared source {removed}").into());
    }
  }
  if required_string(&dv, "source")? != "selected" || required_string(&dv, "source_protocol")? != "v3" {
    return Err("dv configuration merge did not select the environment-expanded NuGet v3 source".into());
  }

  let package_folders = assets
    .get("packageFolders")
    .and_then(serde_json::Value::as_object)
    .ok_or("dotnet configuration merge assets omitted packageFolders")?;
  if package_folders.len() != 1 {
    return Err(format!("dotnet configuration merge selected {} package folders instead of one", package_folders.len()).into());
  }
  let reference_cache = package_folders.keys().next().ok_or("dotnet configuration merge omitted its package folder")?;
  let actual_cache = required_string(&dv, "cache_root")?;
  let reference_cache = fs::canonicalize(reference_cache)?;
  let actual_cache = fs::canonicalize(actual_cache)?;
  let reference_relative = reference_cache.strip_prefix(fs::canonicalize(&dotnet_workspace)?)?;
  let actual_relative = actual_cache.strip_prefix(fs::canonicalize(&dv_workspace)?)?;
  if normalize_windows_path(&reference_relative.to_string_lossy()) != normalize_windows_path(&actual_relative.to_string_lossy()) {
    return Err(format!("NuGet configuration merge cache mismatch: dotnet={reference_cache:?} dv={actual_cache:?}").into());
  }

  let reference_library = assets
    .get("libraries")
    .and_then(serde_json::Value::as_object)
    .and_then(|libraries| libraries.get("Newtonsoft.Json/13.0.3"))
    .ok_or("dotnet configuration merge did not resolve Newtonsoft.Json 13.0.3")?;
  if reference_library.get("type").and_then(serde_json::Value::as_str) != Some("package") {
    return Err("dotnet configuration merge resolved Newtonsoft.Json as a non-package library".into());
  }
  let package = dv
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .and_then(|packages| {
      packages
        .iter()
        .find(|package| package.get("id").and_then(serde_json::Value::as_str) == Some("Newtonsoft.Json"))
    })
    .ok_or("dv configuration merge did not resolve Newtonsoft.Json")?;
  if package.get("version").and_then(serde_json::Value::as_str) != Some("13.0.3") {
    return Err("dv configuration merge selected a different Newtonsoft.Json version".into());
  }
  let reference_hash = fs::read_to_string(dotnet_workspace.join(".packages/newtonsoft.json/13.0.3/newtonsoft.json.13.0.3.nupkg.sha512"))?;
  if package.get("sha512").and_then(serde_json::Value::as_str) != Some(reference_hash.trim()) {
    return Err("NuGet configuration merge package hash differs between dotnet and dv".into());
  }
  Ok(())
}

fn verify_nuget_source_sections(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let oracle_fixture = repository.join("benchmarks/fixtures/nuget-source-sections-oracle");
  run_checked(
    Path::new("dotnet"),
    &[
      "build",
      "SourceSectionsOracle.csproj",
      "-c",
      "Release",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &oracle_fixture,
    "NuGet source-section oracle build",
  )?;

  let root = repository.join("target/benchmark-nuget-source-sections-verification");
  ensure_workspace_is_safe(repository, &root)?;
  let dotnet_workspace = root.join("dotnet");
  let dv_workspace = root.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;

  let oracle = oracle_fixture.join("bin/Release/SourceSectionsOracle.dll");
  let oracle_path = oracle.to_str().ok_or("NuGet source-section oracle path is not valid UTF-8")?;
  let oracle_root = dotnet_workspace
    .to_str()
    .ok_or("NuGet source-section verification workspace is not valid UTF-8")?;
  let oracle_text = nuget_config_command_text(Path::new("dotnet"), &[oracle_path, oracle_root], &dotnet_workspace)?;
  let oracle_result: serde_json::Value = serde_json::from_str(&oracle_text)?;
  let expected_oracle = serde_json::json!({
    "packageSources": [
      {"name": "selected", "url": "https://api.nuget.org/v3/index.json", "enabled": true, "protocol": 3},
      {"name": "legacy", "url": "https://www.nuget.org/api/v2", "enabled": false, "protocol": 2},
      {"name": "decoy", "url": "https://www.nuget.org/api/v2", "enabled": true, "protocol": 2}
    ],
    "auditSources": [
      {"name": "security", "url": "https://api.nuget.org/v3/index.json", "protocol": 3}
    ],
    "mappings": {
      "newtonsoft": ["selected"],
      "decoy": ["decoy"],
      "legacy": ["legacy"],
      "cleared": []
    }
  });
  if oracle_result != expected_oracle {
    return Err(format!("Microsoft NuGet source-section oracle mismatch: expected={expected_oracle} actual={oracle_result}").into());
  }

  let dotnet_args = [
    "restore",
    "SourceSections.csproj",
    "--use-lock-file",
    "--no-http-cache",
    "-p:NuGetAudit=false",
    "--nologo",
    "--verbosity",
    "quiet",
  ];
  run_nuget_config_checked(
    Path::new("dotnet"),
    &dotnet_args,
    &dotnet_workspace,
    "NuGet source-section verification restore",
  )?;
  let dv_text = nuget_config_command_text(dv_executable, &["restore", "SourceSections.csproj", "--json"], &dv_workspace)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv source-section restore did not emit package_resolution_created")?;
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join("obj/project.assets.json"))?)?;
  let reference_sources = assets
    .pointer("/project/restore/sources")
    .and_then(serde_json::Value::as_object)
    .ok_or("dotnet source-section assets omitted restore sources")?;
  let http_source_count = reference_sources.keys().filter(|source| source.starts_with("https://")).count();
  if http_source_count != 2
    || !reference_sources.contains_key("https://api.nuget.org/v3/index.json")
    || !reference_sources.contains_key("https://www.nuget.org/api/v2")
  {
    return Err("dotnet source-section restore did not retain the two enabled mapped sources".into());
  }
  if required_string(&dv, "source")? != "selected" || required_string(&dv, "source_protocol")? != "v3" {
    return Err("dv source-section restore did not select the enabled NuGet v3 source".into());
  }

  let package_folders = assets
    .get("packageFolders")
    .and_then(serde_json::Value::as_object)
    .ok_or("dotnet source-section assets omitted packageFolders")?;
  if package_folders.len() != 1 {
    return Err(
      format!(
        "dotnet source-section restore selected {} package folders instead of one",
        package_folders.len()
      )
      .into(),
    );
  }
  let reference_cache = package_folders
    .keys()
    .next()
    .ok_or("dotnet source-section restore omitted its package folder")?;
  let actual_cache = required_string(&dv, "cache_root")?;
  let reference_cache = fs::canonicalize(reference_cache)?;
  let actual_cache = fs::canonicalize(actual_cache)?;
  let reference_relative = reference_cache.strip_prefix(fs::canonicalize(&dotnet_workspace)?)?;
  let actual_relative = actual_cache.strip_prefix(fs::canonicalize(&dv_workspace)?)?;
  if normalize_windows_path(&reference_relative.to_string_lossy()) != normalize_windows_path(&actual_relative.to_string_lossy()) {
    return Err(format!("NuGet source-section cache mismatch: dotnet={reference_cache:?} dv={actual_cache:?}").into());
  }

  let reference_library = assets
    .get("libraries")
    .and_then(serde_json::Value::as_object)
    .and_then(|libraries| libraries.get("Newtonsoft.Json/13.0.3"))
    .ok_or("dotnet source-section restore did not resolve Newtonsoft.Json 13.0.3")?;
  if reference_library.get("type").and_then(serde_json::Value::as_str) != Some("package") {
    return Err("dotnet source-section restore resolved Newtonsoft.Json as a non-package library".into());
  }
  let package = dv
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .and_then(|packages| {
      packages
        .iter()
        .find(|package| package.get("id").and_then(serde_json::Value::as_str) == Some("Newtonsoft.Json"))
    })
    .ok_or("dv source-section restore did not resolve Newtonsoft.Json")?;
  if package.get("version").and_then(serde_json::Value::as_str) != Some("13.0.3") {
    return Err("dv source-section restore selected a different Newtonsoft.Json version".into());
  }
  let reference_metadata: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join(".packages/newtonsoft.json/13.0.3/.nupkg.metadata"))?)?;
  if reference_metadata.get("source").and_then(serde_json::Value::as_str) != Some("https://api.nuget.org/v3/index.json") {
    return Err("dotnet package-source mapping did not select the mapped v3 source ahead of the first configured v2 source".into());
  }
  let reference_hash = fs::read_to_string(dotnet_workspace.join(".packages/newtonsoft.json/13.0.3/newtonsoft.json.13.0.3.nupkg.sha512"))?;
  if package.get("sha512").and_then(serde_json::Value::as_str) != Some(reference_hash.trim()) {
    return Err("NuGet source-section package hash differs between dotnet and dv".into());
  }
  Ok(())
}

fn verify_nuget_source_mapping(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join("target/benchmark-nuget-source-mapping-verification");
  ensure_workspace_is_safe(repository, &root)?;
  let dotnet_workspace = root.join("dotnet");
  let dv_workspace = root.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;

  let dotnet_args = [
    "restore",
    "SourceMapping.csproj",
    "--packages",
    ".packages",
    "--no-http-cache",
    "-p:NuGetAudit=false",
    "--nologo",
    "--verbosity",
    "quiet",
  ];
  let mut dotnet = Command::new("dotnet");
  dotnet.args(dotnet_args).current_dir(&dotnet_workspace);
  apply_nuget_config_environment(&mut dotnet, &dotnet_workspace);
  validate_source_mapping_failure(&dotnet.output()?, true)?;

  let dv_args = ["restore", "SourceMapping.csproj", "--packages", ".packages", "--json"];
  let mut dv = Command::new(dv_executable);
  dv.args(dv_args).current_dir(&dv_workspace);
  apply_nuget_config_environment(&mut dv, &dv_workspace);
  validate_source_mapping_failure(&dv.output()?, false)
}

fn verify_nuget_storage_policy(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let oracle_fixture = repository.join("benchmarks/fixtures/nuget-storage-policy-oracle");
  run_checked(
    Path::new("dotnet"),
    &[
      "build",
      "StoragePolicyOracle.csproj",
      "-c",
      "Release",
      "-p:NuGetAudit=false",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &oracle_fixture,
    "NuGet storage-policy oracle build",
  )?;

  let root = repository.join("target/benchmark-nuget-storage-policy-verification");
  ensure_workspace_is_safe(repository, &root)?;
  let dotnet_workspace = root.join("dotnet");
  let dv_workspace = root.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;

  let oracle = oracle_fixture.join("bin/Release/StoragePolicyOracle.dll");
  let oracle_path = oracle.to_str().ok_or("NuGet storage-policy oracle path is not valid UTF-8")?;
  let oracle_root = dotnet_workspace
    .to_str()
    .ok_or("NuGet storage-policy verification workspace is not valid UTF-8")?;
  let oracle_text = nuget_storage_command_text(Path::new("dotnet"), &[oracle_path, oracle_root], &dotnet_workspace)?;
  let oracle: serde_json::Value = serde_json::from_str(&oracle_text)?;
  assert_relative_policy_path(&oracle, "globalPackages", &dotnet_workspace, "policy/env-global")?;
  assert_relative_policy_path(&oracle, "httpCache", &dotnet_workspace, "policy/http-cache")?;
  assert_relative_policy_path(&oracle, "scratch", &dotnet_workspace, "policy/scratch")?;
  let fallback = oracle
    .get("fallbackPackages")
    .and_then(serde_json::Value::as_array)
    .ok_or("Microsoft NuGet storage-policy oracle omitted fallbackPackages")?;
  if fallback.len() != 1 {
    return Err(
      format!(
        "Microsoft NuGet storage-policy oracle selected {} fallback folders instead of one",
        fallback.len()
      )
      .into(),
    );
  }
  let fallback_path = fallback[0].as_str().ok_or("Microsoft NuGet fallback path is not text")?;
  if relative_policy_path(fallback_path, &dotnet_workspace)? != "policy/fallback-final" {
    return Err(format!("Microsoft NuGet fallback policy differs: {fallback_path:?}").into());
  }
  if oracle.get("signatureValidation").and_then(serde_json::Value::as_str) != Some("accept")
    || oracle.get("proxy").and_then(serde_json::Value::as_str) != Some("http://127.0.0.1:9")
    || oracle.get("noProxy").and_then(serde_json::Value::as_str) != Some("api.nuget.org,localhost")
  {
    return Err(format!("Microsoft NuGet typed storage/proxy policy differs: {oracle}").into());
  }

  let properties_text = nuget_storage_command_text(
    Path::new("dotnet"),
    &[
      "msbuild",
      "StoragePolicy.csproj",
      "--nologo",
      "-getProperty:NuGetAudit,NuGetAuditMode,NuGetAuditLevel",
    ],
    &dotnet_workspace,
  )?;
  let properties: serde_json::Value = serde_json::from_str(&properties_text)?;
  let properties = properties
    .get("Properties")
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft storage-policy query omitted Properties")?;
  if properties.get("NuGetAudit").and_then(serde_json::Value::as_str) != Some("false")
    || properties.get("NuGetAuditMode").and_then(serde_json::Value::as_str) != Some("direct")
    || properties.get("NuGetAuditLevel").and_then(serde_json::Value::as_str) != Some("critical")
  {
    return Err(format!("Microsoft NuGet audit policy differs: {properties:?}").into());
  }

  prepare_nuget_storage_policy(Path::new("dotnet"), &dotnet_workspace)?;
  prepare_nuget_storage_policy(dv_executable, &dv_workspace)?;
  let dv_text = nuget_storage_command_text(dv_executable, &["restore", "StoragePolicy.csproj", "--offline", "--json"], &dv_workspace)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv storage-policy restore did not emit package_resolution_created")?;

  for (field, expected) in [
    ("cache_root", "policy/env-global"),
    ("http_cache_root", "policy/http-cache"),
    ("temp_root", "policy/scratch"),
  ] {
    let actual = required_string(&dv, field)?;
    if relative_policy_path(actual, &dv_workspace)? != expected {
      return Err(format!("dv {field} differs: {actual:?}").into());
    }
  }
  let dv_fallback = string_array(&dv, "fallback_roots")?;
  if dv_fallback.len() != 1 || relative_policy_path(&dv_fallback[0], &dv_workspace)? != "policy/fallback-final" {
    return Err(format!("dv fallback package policy differs: {dv_fallback:?}").into());
  }
  if required_string(&dv, "signature_validation")? != "accept"
    || dv.get("proxy_configured").and_then(serde_json::Value::as_bool) != Some(true)
    || dv.get("audit_enabled").and_then(serde_json::Value::as_bool) != Some(false)
    || required_string(&dv, "audit_mode")? != "direct"
    || required_string(&dv, "audit_level")? != "critical"
  {
    return Err(format!("dv typed storage/audit/proxy policy differs: {dv}").into());
  }
  if dv.get("network_requests").and_then(serde_json::Value::as_u64) != Some(0) || dv.get("downloaded_packages").and_then(serde_json::Value::as_u64) != Some(0) {
    return Err("dv storage-policy verification performed network or package-download work".into());
  }

  let assets: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join("obj/project.assets.json"))?)?;
  let package_folders = assets
    .get("packageFolders")
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft storage-policy assets omitted packageFolders")?;
  let mut reference_folders = package_folders
    .keys()
    .map(|path| relative_policy_path(path, &dotnet_workspace))
    .collect::<Result<Vec<_>>>()?;
  reference_folders.sort_unstable();
  if reference_folders != ["policy/env-global", "policy/fallback-final"] {
    return Err(format!("Microsoft storage-policy package folders differ: {reference_folders:?}").into());
  }
  let package = dv
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .and_then(|packages| packages.first())
    .ok_or("dv storage-policy verification omitted Newtonsoft.Json")?;
  let reference_hash = fs::read_to_string(dotnet_workspace.join("policy/fallback-final/newtonsoft.json/13.0.3/newtonsoft.json.13.0.3.nupkg.sha512"))?;
  if package.get("id").and_then(serde_json::Value::as_str) != Some("Newtonsoft.Json")
    || package.get("version").and_then(serde_json::Value::as_str) != Some("13.0.3")
    || package.get("sha512").and_then(serde_json::Value::as_str) != Some(reference_hash.trim())
  {
    return Err("NuGet storage-policy package identity, version, or hash differs".into());
  }
  let compile = string_array(&dv, "compile_assets")?;
  let compile_path = compile.first().map(|path| path.replace('\\', "/").to_ascii_lowercase());
  if compile_path
    .as_deref()
    .is_none_or(|path| !path.contains("/policy/fallback-final/newtonsoft.json/13.0.3/"))
  {
    return Err(format!("dv did not select the package from its fallback root: {compile:?}").into());
  }
  Ok(())
}

fn verify_nuget_cli_overrides(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join("target/benchmark-nuget-cli-overrides-verification");
  ensure_workspace_is_safe(repository, &root)?;
  let dotnet_workspace = root.join("dotnet");
  let dv_workspace = root.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;
  prepare_nuget_cli_overrides(Path::new("dotnet"), &dotnet_workspace)?;
  prepare_nuget_cli_overrides(dv_executable, &dv_workspace)?;

  run_nuget_cli_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "CliOverrides.csproj",
      "--locked-mode",
      "--source",
      "https://api.nuget.org/v3/index.json",
      "--configfile",
      "config/selected.config",
      "--packages",
      "policy/cli-global",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &dotnet_workspace,
    "NuGet CLI override verification",
  )?;
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join("obj/project.assets.json"))?)?;
  let restore = assets
    .get("project")
    .and_then(|project| project.get("restore"))
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft CLI override assets omitted project.restore")?;
  let packages_path = restore
    .get("packagesPath")
    .and_then(serde_json::Value::as_str)
    .ok_or("Microsoft CLI override assets omitted packagesPath")?;
  if relative_policy_path(packages_path, &dotnet_workspace)? != "policy/cli-global" {
    return Err(format!("Microsoft --packages did not beat environment/config precedence: {packages_path:?}").into());
  }
  let config_paths = restore
    .get("configFilePaths")
    .and_then(serde_json::Value::as_array)
    .ok_or("Microsoft CLI override assets omitted configFilePaths")?;
  if config_paths.len() != 1
    || config_paths[0]
      .as_str()
      .is_none_or(|path| relative_policy_path(path, &dotnet_workspace).ok().as_deref() != Some("config/selected.config"))
  {
    return Err(format!("Microsoft --configfile did not isolate the selected file: {config_paths:?}").into());
  }
  let sources = restore
    .get("sources")
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft CLI override assets omitted sources")?;
  let remote_sources = sources
    .keys()
    .filter(|source| source.starts_with("http://") || source.starts_with("https://"))
    .collect::<Vec<_>>();
  if remote_sources != ["https://api.nuget.org/v3/index.json"] {
    return Err(format!("Microsoft --source did not replace configured remote sources: {remote_sources:?}").into());
  }
  let package_folders = assets
    .get("packageFolders")
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft CLI override assets omitted packageFolders")?;
  let reference_folders = package_folders
    .keys()
    .map(|path| relative_policy_path(path, &dotnet_workspace))
    .collect::<Result<Vec<_>>>()?;
  if reference_folders != ["policy/cli-global"] {
    return Err(format!("Microsoft CLI packages override differs: {reference_folders:?}").into());
  }
  let reference_metadata: serde_json::Value =
    serde_json::from_slice(&fs::read(dotnet_workspace.join("policy/cli-global/newtonsoft.json/13.0.3/.nupkg.metadata"))?)?;
  if reference_metadata.get("source").and_then(serde_json::Value::as_str) != Some("https://api.nuget.org/v3/index.json") {
    return Err("Microsoft CLI source override did not select the requested package source".into());
  }

  let dv_text = nuget_cli_command_text(
    dv_executable,
    &[
      "restore",
      "CliOverrides.csproj",
      "--source",
      "https://api.nuget.org/v3/index.json",
      "--configfile",
      "config/selected.config",
      "--packages",
      "policy/cli-global",
      "--offline",
      "--json",
    ],
    &dv_workspace,
  )?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv CLI override verification omitted package_resolution_created")?;
  if required_string(&dv, "source")? != "https://api.nuget.org/v3/index.json"
    || required_string(&dv, "source_protocol")? != "v3"
    || relative_policy_path(required_string(&dv, "cache_root")?, &dv_workspace)? != "policy/cli-global"
  {
    return Err(format!("dv CLI source/package overrides differ: {dv}").into());
  }
  if dv.get("network_requests").and_then(serde_json::Value::as_u64) != Some(0) || dv.get("downloaded_packages").and_then(serde_json::Value::as_u64) != Some(0) {
    return Err("dv CLI override verification performed network or package-download work".into());
  }
  let package = dv
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .and_then(|packages| packages.first())
    .ok_or("dv CLI override verification omitted Newtonsoft.Json")?;
  let reference_hash = fs::read_to_string(dotnet_workspace.join("policy/cli-global/newtonsoft.json/13.0.3/newtonsoft.json.13.0.3.nupkg.sha512"))?;
  if package.get("id").and_then(serde_json::Value::as_str) != Some("Newtonsoft.Json")
    || package.get("version").and_then(serde_json::Value::as_str) != Some("13.0.3")
    || package.get("sha512").and_then(serde_json::Value::as_str) != Some(reference_hash.trim())
  {
    return Err("NuGet CLI override package identity, version, or hash differs".into());
  }
  for workspace in [&dotnet_workspace, &dv_workspace] {
    for relative in ["policy/env-global", "policy/config-global", "policy/selected-config-global"] {
      if workspace.join(relative).exists() {
        return Err(format!("CLI override unexpectedly used lower-precedence path {relative:?}").into());
      }
    }
  }
  Ok(())
}

fn verify_nuget_local_sources(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let root = repository.join(format!("target/benchmark-nuget-local-sources-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &root)?;
  let dotnet_workspace = root.join("dotnet");
  let dv_workspace = root.join("dv");
  reset_fixture(fixture, &dotnet_workspace)?;
  reset_fixture(fixture, &dv_workspace)?;
  prepare_nuget_local_sources(&dotnet_workspace)?;
  prepare_nuget_local_sources(&dv_workspace)?;

  run_nuget_config_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "LocalSources.csproj",
      "--packages",
      ".packages",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &dotnet_workspace,
    "NuGet local-source verification",
  )?;
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(dotnet_workspace.join("obj/project.assets.json"))?)?;
  let restore = assets
    .get("project")
    .and_then(|project| project.get("restore"))
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft local-source assets omitted project.restore")?;
  let mut sources = restore
    .get("sources")
    .and_then(serde_json::Value::as_object)
    .ok_or("Microsoft local-source assets omitted sources")?
    .keys()
    // .NET 10 injects its installation-wide library-packs source even after
    // packageSources/clear. It is not eligible for either mapped test package.
    .filter(|path| !normalize_windows_path(path).ends_with(r"\dotnet\library-packs"))
    .map(|path| relative_policy_path(path, &dotnet_workspace))
    .collect::<Result<Vec<_>>>()?;
  sources.sort_unstable();
  if sources != ["feeds/flat", "feeds/hierarchical"] {
    return Err(format!("Microsoft local-source paths differ: {sources:?}").into());
  }

  let dv_text = nuget_config_command_text(
    dv_executable,
    &["restore", "LocalSources.csproj", "--packages", ".packages", "--offline", "--json"],
    &dv_workspace,
  )?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv local-source verification omitted package_resolution_created")?;
  if required_string(&dv, "source_protocol")? != "local" || relative_policy_path(required_string(&dv, "cache_root")?, &dv_workspace)? != ".packages" {
    return Err(format!("dv local-source policy differs: {dv}").into());
  }
  if dv.get("network_requests").and_then(serde_json::Value::as_u64) != Some(0) || dv.get("downloaded_packages").and_then(serde_json::Value::as_u64) != Some(2) {
    return Err("dv local-source verification did not publish two packages with zero HTTP requests".into());
  }
  let packages = dv
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv local-source verification omitted packages")?;
  for (id, lower_id, version) in [("Humanizer.Core", "humanizer.core", "2.14.1"), ("Newtonsoft.Json", "newtonsoft.json", "13.0.3")] {
    let package = packages
      .iter()
      .find(|package| {
        package
          .get("id")
          .and_then(serde_json::Value::as_str)
          .is_some_and(|value| value.eq_ignore_ascii_case(id))
      })
      .ok_or_else(|| format!("dv local-source verification omitted {id}"))?;
    let reference_hash = fs::read_to_string(
      dotnet_workspace
        .join(".packages")
        .join(lower_id)
        .join(version)
        .join(format!("{lower_id}.{version}.nupkg.sha512")),
    )?;
    if package.get("version").and_then(serde_json::Value::as_str) != Some(version)
      || package.get("sha512").and_then(serde_json::Value::as_str) != Some(reference_hash.trim())
    {
      return Err(format!("local-source identity, version, or hash differs for {id}").into());
    }
  }
  Ok(())
}

fn verify_nuget_service_index(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let workspace = repository.join(format!("target/benchmark-nuget-service-index-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &workspace)?;
  reset_fixture(fixture, &workspace)?;
  run_checked(
    Path::new("dotnet"),
    &["build", "oracle/ServiceIndexOracle.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
    &workspace,
    "NuGet service-index oracle build",
  )?;
  reset_service_index_iteration(&workspace)?;
  let oracle_text = service_index_command_text(
    Path::new("dotnet"),
    &["oracle/bin/Release/ServiceIndexOracle.dll", "https://api.nuget.org/v3/index.json"],
    &workspace,
  )?;
  let oracle: serde_json::Value = serde_json::from_str(&oracle_text)?;
  reset_service_index_iteration(&workspace)?;
  let dv_text = service_index_command_text(dv_executable, &["project", "package-sources", "ServiceIndex.csproj", "--json"], &workspace)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_sources_inspected"))
    .ok_or("dv service-index verification omitted package_sources_inspected")?;
  let source = dv
    .get("sources")
    .and_then(serde_json::Value::as_array)
    .and_then(|sources| sources.first())
    .ok_or("dv service-index verification omitted its source")?;
  if required_string(source, "name")? != "nuget.org"
    || required_string(source, "location")? != "https://api.nuget.org/v3/index.json"
    || required_string(source, "protocol")? != "v3"
  {
    return Err(format!("dv service-index source differs: {source}").into());
  }
  for (oracle_field, kind) in [
    ("registration", "registration"),
    ("packageContent", "package_content"),
    ("search", "search"),
    ("vulnerability", "vulnerability"),
    ("packagePublish", "package_publish"),
  ] {
    let expected = string_array(&oracle, oracle_field)?;
    let actual = package_service_endpoints(source, kind)?;
    if expected != actual {
      return Err(format!("NuGet {kind} endpoint mismatch: oracle={expected:?} dv={actual:?}").into());
    }
  }
  if dv.get("network_requests").and_then(serde_json::Value::as_u64) != Some(1)
    || dv.get("downloaded_bytes").and_then(serde_json::Value::as_u64).is_none_or(|bytes| bytes == 0)
  {
    return Err("dv service-index verification did not report one nonempty response".into());
  }
  Ok(())
}

fn verify_nuget_credentials(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let workspace = repository.join(format!("target/benchmark-nuget-credentials-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &workspace)?;
  reset_fixture(fixture, &workspace)?;
  run_checked(
    Path::new("dotnet"),
    &["build", "oracle/CredentialOracle.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
    &workspace,
    "NuGet credential oracle build",
  )?;
  let oracle_text = credential_command_text(Path::new("dotnet"), &["oracle/bin/Release/CredentialOracle.dll", "."], &workspace)?;
  reject_credential_output("Microsoft credential oracle", &oracle_text)?;
  let oracle: Vec<serde_json::Value> = serde_json::from_str(&oracle_text)?;
  let dv_text = credential_command_text(
    dv_executable,
    &["project", "package-sources", "CredentialProject.csproj", "--offline", "--json"],
    &workspace,
  )?;
  reject_credential_output("dv credential inspection", &dv_text)?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_sources_inspected"))
    .ok_or("dv credential verification omitted package_sources_inspected")?;
  let actual = dv
    .get("sources")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv credential verification omitted sources")?;
  if oracle.len() != 2 || actual.len() != oracle.len() {
    return Err(format!("credential source count differs: oracle={} dv={}", oracle.len(), actual.len()).into());
  }
  for (expected, actual) in oracle.iter().zip(actual) {
    for field in ["name", "location", "protocol", "authentication"] {
      if required_string(expected, field)? != required_string(actual, field)? {
        return Err(format!("credential source field {field} differs: oracle={expected} dv={actual}").into());
      }
    }
    if expected.get("credentialSelected").and_then(serde_json::Value::as_bool) != Some(true) {
      return Err(format!("Microsoft credential oracle did not select the expected environment/config secret: {expected}").into());
    }
    if actual
      .get("endpoints")
      .and_then(serde_json::Value::as_array)
      .is_none_or(|endpoints| !endpoints.is_empty())
    {
      return Err(format!("offline credential inspection unexpectedly discovered endpoints: {actual}").into());
    }
  }
  if dv.get("network_requests").and_then(serde_json::Value::as_u64) != Some(0) || dv.get("downloaded_bytes").and_then(serde_json::Value::as_u64) != Some(0) {
    return Err("offline credential inspection performed network work".into());
  }
  Ok(())
}

fn verify_nuget_credential_provider(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let workspace = repository.join(format!("target/benchmark-nuget-credential-provider-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &workspace)?;
  reset_fixture(fixture, &workspace)?;
  run_checked(
    Path::new("dotnet"),
    &[
      "build",
      "oracle/CredentialProviderOracle.csproj",
      "-c",
      "Release",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &workspace,
    "NuGet credential-provider oracle build",
  )?;

  let oracle_text = credential_provider_command_text(
    Path::new("dotnet"),
    &["oracle/bin/Release/CredentialProviderOracle.dll", "https://private.example.test/v3/index.json"],
    &workspace,
    Some(&workspace.join("dotnet-provider.trace")),
  )?;
  let oracle: serde_json::Value = serde_json::from_str(&oracle_text)?;
  if oracle.get("authentication").and_then(serde_json::Value::as_str) != Some("basic")
    || oracle.get("selected").and_then(serde_json::Value::as_bool) != Some(true)
    || oracle.get("providerCount").and_then(serde_json::Value::as_u64) != Some(1)
  {
    return Err(format!("Microsoft credential-provider oracle did not acquire the fixture credential: {oracle}").into());
  }

  let dv_text = credential_provider_command_text(
    dv_executable,
    &[
      "project",
      "package-sources",
      "CredentialProviderProject.csproj",
      "--offline",
      "--probe-credentials",
      "--json",
    ],
    &workspace,
    Some(&workspace.join("dv-provider.trace")),
  )?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_sources_inspected"))
    .ok_or("dv credential-provider verification omitted package_sources_inspected")?;
  let sources = dv
    .get("sources")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv credential-provider verification omitted sources")?;
  if sources.len() != 1 || sources[0].get("authentication").and_then(serde_json::Value::as_str) != Some("basic") {
    return Err(format!("dv credential-provider result differs from the Microsoft oracle: {dv}").into());
  }
  if dv.get("network_requests").and_then(serde_json::Value::as_u64) != Some(0) || dv.get("downloaded_bytes").and_then(serde_json::Value::as_u64) != Some(0) {
    return Err("credential-provider probe performed network work".into());
  }
  for trace in [workspace.join("dotnet-provider.trace"), workspace.join("dv-provider.trace")] {
    let trace_text = fs::read_to_string(&trace)?;
    if !trace_text.contains("GetAuthenticationCredentials noninteractive=true dialog=false") {
      return Err(
        format!(
          "credential-provider trace {} did not preserve noninteractive CI policy: {trace_text}",
          trace.display()
        )
        .into(),
      );
    }
  }
  verify_credential_provider_interactive(dv_executable, &workspace)?;
  verify_credential_provider_timeout(dv_executable, &workspace)?;
  Ok(())
}

struct ClientCertificateFixture {
  workspace: PathBuf,
}

impl Drop for ClientCertificateFixture {
  fn drop(&mut self) {
    let _ = Command::new("dotnet")
      .args(["oracle/bin/Release/ClientCertificateOracle.dll", "cleanup", "."])
      .current_dir(&self.workspace)
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status();
  }
}

fn prepare_client_certificate_fixture(workspace: &Path) -> Result<ClientCertificateFixture> {
  let oracle = "oracle/bin/Release/ClientCertificateOracle.dll";
  let _ = Command::new("dotnet")
    .args([oracle, "cleanup", "."])
    .current_dir(workspace)
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status();
  run_checked(Path::new("dotnet"), &[oracle, "setup", "."], workspace, "client-certificate fixture setup")?;
  if !cfg!(windows) {
    return Err("the platform-store client-certificate benchmark currently requires Windows".into());
  }
  let metadata: serde_json::Value = serde_json::from_slice(&fs::read(workspace.join("certs/metadata.json"))?)?;
  let thumbprint = metadata
    .get("client")
    .and_then(serde_json::Value::as_str)
    .ok_or("client-certificate fixture metadata omitted client thumbprint")?;
  let config = fs::read_to_string(workspace.join("NuGet.Config.template"))?.replace("__THUMBPRINT__", thumbprint);
  fs::write(workspace.join("NuGet.Config"), config)?;
  Ok(ClientCertificateFixture {
    workspace: workspace.to_owned(),
  })
}

fn verify_nuget_client_certificates(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let workspace = repository.join(format!("target/benchmark-nuget-client-certificates-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &workspace)?;
  reset_fixture(fixture, &workspace)?;
  run_checked(
    Path::new("dotnet"),
    &[
      "build",
      "oracle/ClientCertificateOracle.csproj",
      "-c",
      "Release",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &workspace,
    "NuGet client-certificate oracle build",
  )?;
  let _fixture = prepare_client_certificate_fixture(&workspace)?;
  let oracle_text = client_certificate_command_text(
    Path::new("dotnet"),
    &["oracle/bin/Release/ClientCertificateOracle.dll", "query", "."],
    &workspace,
  )?;
  let oracle: serde_json::Value = serde_json::from_str(&oracle_text)?;
  let dv_text = client_certificate_command_text(
    dv_executable,
    &["project", "package-sources", "ClientCertificateProject.csproj", "--offline", "--json"],
    &workspace,
  )?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_sources_inspected"))
    .ok_or("dv client-certificate verification omitted package_sources_inspected")?;
  let expected = oracle.as_array().ok_or("client-certificate oracle did not return an array")?;
  let actual = dv
    .get("sources")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv client-certificate verification omitted sources")?;
  if expected.len() != 2 || actual.len() != expected.len() {
    return Err(format!("client-certificate source count differs: oracle={} dv={}", expected.len(), actual.len()).into());
  }
  for (expected, actual) in expected.iter().zip(actual) {
    for field in ["name", "location", "protocol", "authentication"] {
      if required_string(expected, field)? != required_string(actual, field)? {
        return Err(format!("client-certificate source field {field} differs: oracle={expected} dv={actual}").into());
      }
    }
    if expected.get("certificateCount").and_then(serde_json::Value::as_u64) != Some(1) {
      return Err(format!("Microsoft client-certificate oracle did not select exactly one certificate: {expected}").into());
    }
  }
  if dv.get("network_requests").and_then(serde_json::Value::as_u64) != Some(0) {
    return Err("offline client-certificate verification performed network work".into());
  }
  Ok(())
}

fn verify_nuget_http_policy(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let workspace = repository.join(format!("target/benchmark-nuget-http-policy-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &workspace)?;
  reset_fixture(fixture, &workspace)?;
  run_checked(
    Path::new("dotnet"),
    &["build", "oracle/HttpPolicyOracle.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
    &workspace,
    "NuGet HTTP-policy oracle build",
  )?;
  let oracle = http_policy_command_json(
    Path::new("dotnet"),
    &["oracle/bin/Release/HttpPolicyOracle.dll", "."],
    &workspace,
    "NuGet HTTP-policy oracle",
  )?;
  let dv_output = http_policy_command_output(
    dv_executable,
    &["project", "package-sources", "HttpPolicyProject.csproj", "--offline", "--json"],
    &workspace,
    "dv HTTP-policy verification",
  )?;
  let dv = dv_output
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_sources_inspected"))
    .ok_or("dv HTTP-policy verification omitted package_sources_inspected")?;
  let actual = dv.get("http_policy").ok_or("dv HTTP-policy verification omitted http_policy")?;
  for (expected, observed) in [
    ("maxTries", "max_tries"),
    ("retryDelayMs", "retry_delay_ms"),
    ("maxRetryAfterSeconds", "max_retry_after_seconds"),
    ("requestTimeoutSeconds", "request_timeout_seconds"),
    ("downloadTimeoutSeconds", "download_timeout_seconds"),
    ("maxRequestsPerSource", "max_requests_per_source"),
    ("retryHttp429", "retry_http_429"),
    ("observeRetryAfter", "observe_retry_after"),
    ("proxyConfigured", "proxy_configured"),
    ("proxyAuthenticated", "proxy_authenticated"),
    ("noProxyConfigured", "no_proxy_configured"),
  ] {
    if oracle.get(expected) != actual.get(observed) {
      return Err(format!("NuGet HTTP-policy field differs for {observed}: oracle={oracle} dv={actual}").into());
    }
  }
  if actual.get("offline").and_then(serde_json::Value::as_bool) != Some(true)
    || actual.get("tls_validation").and_then(serde_json::Value::as_bool) != Some(true)
    || actual.get("allow_insecure_connections").and_then(serde_json::Value::as_bool) != Some(false)
    || actual.get("max_redirects").and_then(serde_json::Value::as_u64) != Some(10)
  {
    return Err(format!("dv HTTP-policy security/offline fields are invalid: {actual}").into());
  }
  if dv.get("network_requests").and_then(serde_json::Value::as_u64) != Some(0) {
    return Err("offline HTTP-policy verification performed network work".into());
  }
  Ok(())
}

fn verify_nuget_source_security(repository: &Path, dv_executable: &Path, fixture: &Path) -> Result<()> {
  let workspace = repository.join(format!("target/benchmark-nuget-source-security-verification-{}", std::process::id()));
  ensure_workspace_is_safe(repository, &workspace)?;
  reset_fixture(fixture, &workspace)?;
  run_checked(
    Path::new("dotnet"),
    &[
      "build",
      "oracle/SourceSecurityOracle.csproj",
      "-c",
      "Release",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    &workspace,
    "NuGet source-security oracle build",
  )?;
  let oracle: serde_json::Value = serde_json::from_str(&nuget_config_command_text(
    Path::new("dotnet"),
    &["oracle/bin/Release/SourceSecurityOracle.dll", "."],
    &workspace,
  )?)?;
  let dv_text = nuget_config_command_text(
    dv_executable,
    &["project", "package-sources", "SecurityProject.csproj", "--offline", "--json"],
    &workspace,
  )?;
  let dv = dv_text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_sources_inspected"))
    .ok_or("dv source-security verification omitted package_sources_inspected")?;
  let expected = oracle.as_array().ok_or("source-security oracle did not return an array")?;
  let actual = dv
    .get("sources")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv source-security verification omitted sources")?;
  if expected.len() != 3 || actual.len() != expected.len() {
    return Err(format!("source-security source count differs: oracle={} dv={}", expected.len(), actual.len()).into());
  }
  for (expected, actual) in expected.iter().zip(actual) {
    for field in ["name", "location", "protocol"] {
      if required_string(expected, field)? != required_string(actual, field)? {
        return Err(format!("source-security field {field} differs: oracle={expected} dv={actual}").into());
      }
    }
    for (oracle_field, dv_field) in [
      ("allowInsecureConnections", "allow_insecure_connections"),
      ("disableTlsCertificateValidation", "disable_tls_certificate_validation"),
    ] {
      if expected.get(oracle_field) != actual.get(dv_field) {
        return Err(format!("source-security field {dv_field} differs: oracle={expected} dv={actual}").into());
      }
    }
  }
  let policy = dv.get("http_policy").ok_or("dv source-security verification omitted http_policy")?;
  if policy.get("allow_insecure_connections").and_then(serde_json::Value::as_bool) != Some(true)
    || policy.get("tls_validation").and_then(serde_json::Value::as_bool) != Some(false)
    || dv.get("network_requests").and_then(serde_json::Value::as_u64) != Some(0)
  {
    return Err(format!("dv source-security aggregate or offline evidence is invalid: {dv}").into());
  }
  Ok(())
}

fn http_policy_command_json(executable: &Path, args: &[&str], cwd: &Path, purpose: &str) -> Result<serde_json::Value> {
  Ok(serde_json::from_str(&http_policy_command_output(executable, args, cwd, purpose)?)?)
}

fn http_policy_command_output(executable: &Path, args: &[&str], cwd: &Path, purpose: &str) -> Result<String> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_http_policy_environment(&mut command, cwd);
  let output = command.output()?;
  check_output(output.clone(), executable, args, purpose)?;
  Ok(String::from_utf8(output.stdout)?)
}

fn verify_credential_provider_interactive(dv_executable: &Path, workspace: &Path) -> Result<()> {
  const LOGIN_MESSAGE: &str = "fixture device login required";
  let trace = workspace.join("dv-provider-interactive.trace");
  remove_generated_path(&trace)?;
  let mut command = Command::new(dv_executable);
  command
    .args([
      "project",
      "package-sources",
      "CredentialProviderProject.csproj",
      "--offline",
      "--probe-credentials",
      "--interactive",
      "--json",
    ])
    .current_dir(workspace);
  apply_nuget_credential_provider_environment(&mut command, workspace);
  command.env("DV_TEST_PROVIDER_LOG", LOGIN_MESSAGE).env("DV_TEST_PROVIDER_TRACE", &trace);
  let output = command.output()?;
  check_output(
    output.clone(),
    dv_executable,
    &[
      "project",
      "package-sources",
      "CredentialProviderProject.csproj",
      "--offline",
      "--probe-credentials",
      "--interactive",
      "--json",
    ],
    "interactive credential-provider verification",
  )?;
  let stdout = String::from_utf8(output.stdout)?;
  let stderr = String::from_utf8(output.stderr)?;
  reject_credential_output("interactive credential-provider stdout", &stdout)?;
  reject_credential_output("interactive credential-provider stderr", &stderr)?;
  if !stderr.contains(&format!("credential provider: {LOGIN_MESSAGE}")) {
    return Err(format!("interactive credential-provider output omitted login guidance: {stderr:?}").into());
  }
  let trace_text = fs::read_to_string(&trace)?;
  if !trace_text.contains("GetAuthenticationCredentials noninteractive=false dialog=true") || !trace_text.contains("Response Log") {
    return Err(format!("interactive credential-provider trace omitted policy flags or log acknowledgement: {trace_text}").into());
  }
  Ok(())
}

fn verify_credential_provider_timeout(dv_executable: &Path, workspace: &Path) -> Result<()> {
  let trace = workspace.join("dv-provider-timeout.trace");
  remove_generated_path(&trace)?;
  let args = [
    "project",
    "package-sources",
    "CredentialProviderProject.csproj",
    "--offline",
    "--probe-credentials",
    "--json",
  ];
  let mut command = Command::new(dv_executable);
  command.args(args).current_dir(workspace);
  apply_nuget_credential_provider_environment(&mut command, workspace);
  command
    .env("DV_TEST_PROVIDER_MODE", "ignore-cancel")
    .env("DV_TEST_PROVIDER_TRACE", &trace)
    .env("NUGET_PLUGIN_REQUEST_TIMEOUT_IN_SECONDS", "1");
  let started = Instant::now();
  let output = command.output()?;
  let elapsed = started.elapsed();
  if output.status.success() {
    return Err("dv accepted a credential provider which exceeded its request timeout".into());
  }
  if elapsed < Duration::from_secs(2) || elapsed > Duration::from_secs(5) {
    return Err(
      format!("uncooperative credential-provider cleanup took {elapsed:?}; expected forced termination at the two-second cancellation deadline").into(),
    );
  }
  let stdout = String::from_utf8(output.stdout)?;
  let stderr = String::from_utf8(output.stderr)?;
  reject_credential_output("credential-provider timeout stdout", &stdout)?;
  reject_credential_output("credential-provider timeout stderr", &stderr)?;
  if !stdout.contains("DV0410") || !stdout.contains("timed out") {
    return Err(format!("credential-provider timeout omitted stable DV0410 diagnostic: stdout={stdout:?} stderr={stderr:?}").into());
  }
  let trace_text = fs::read_to_string(&trace)?;
  if !trace_text.contains("Cancel GetAuthenticationCredentials") {
    return Err(format!("credential-provider timeout did not send protocol cancellation before stopping the process: {trace_text}").into());
  }
  Ok(())
}

fn reject_credential_output(label: &str, output: &str) -> Result<()> {
  for secret in [
    "config-decoy-user",
    "config-decoy-secret",
    "environment-user",
    "environment-pat",
    "config-only-user",
    "config-only-pat",
    "fixture-user",
    "fixture-secret",
    "provider-benchmark-user",
    "provider-benchmark-secret",
    "fixture-client-password",
    "fixture-server-password",
  ] {
    if output.contains(secret) {
      return Err(format!("{label} exposed credential text {secret:?}").into());
    }
  }
  Ok(())
}

fn package_service_endpoints(source: &serde_json::Value, kind: &str) -> Result<Vec<String>> {
  source
    .get("endpoints")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv package source omitted endpoints")?
    .iter()
    .filter(|endpoint| endpoint.get("kind").and_then(serde_json::Value::as_str) == Some(kind))
    .map(|endpoint| {
      endpoint
        .get("location")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("dv {kind} endpoint omitted location").into())
    })
    .collect()
}

fn assert_relative_policy_path(value: &serde_json::Value, field: &str, workspace: &Path, expected: &str) -> Result<()> {
  let path = value
    .get(field)
    .and_then(serde_json::Value::as_str)
    .ok_or_else(|| format!("Microsoft NuGet storage-policy oracle omitted {field}"))?;
  let actual = relative_policy_path(path, workspace)?;
  if actual != expected {
    return Err(format!("Microsoft NuGet {field} differs: expected={expected:?} actual={actual:?}").into());
  }
  Ok(())
}

fn relative_policy_path(path: &str, workspace: &Path) -> Result<String> {
  let relative = Path::new(path)
    .strip_prefix(workspace)
    .map_err(|_| format!("policy path {path:?} is outside benchmark workspace {}", workspace.display()))?;
  Ok(relative.to_string_lossy().replace('\\', "/").to_ascii_lowercase())
}

fn compare_package_asset_family(
  target: &serde_json::Map<String, serde_json::Value>,
  dv: &serde_json::Value,
  reference_field: &str,
  actual_fields: &[&str],
  include_placeholders: bool,
) -> Result<()> {
  let mut reference = Vec::new();
  for (identity, package) in target {
    let Some(assets) = package.get(reference_field).and_then(serde_json::Value::as_object) else {
      continue;
    };
    let (id, version) = identity
      .split_once('/')
      .ok_or_else(|| format!("dotnet target identity {identity:?} omitted its version"))?;
    for asset in assets.keys().filter(|asset| include_placeholders || !asset.ends_with("/_._")) {
      reference.push(format!("{}/{}/{}", id.to_ascii_lowercase(), version.to_ascii_lowercase(), asset));
    }
  }
  reference.sort_unstable();
  let mut actual = Vec::new();
  for field in actual_fields {
    for path in package_event_asset_paths(dv, field)? {
      actual.push(package_relative_path(&path)?);
    }
  }
  actual.sort_unstable();
  if reference != actual {
    let reference_only = reference.iter().find(|asset| actual.binary_search(asset).is_err());
    let actual_only = actual.iter().find(|asset| reference.binary_search(asset).is_err());
    return Err(
      format!(
        "package {reference_field} asset batch differs: dotnet={} dv={} first_dotnet_only={reference_only:?} first_dv_only={actual_only:?}",
        reference.len(),
        actual.len()
      )
      .into(),
    );
  }
  Ok(())
}

fn package_event_asset_paths(document: &serde_json::Value, field: &str) -> Result<Vec<String>> {
  document
    .get(field)
    .and_then(serde_json::Value::as_array)
    .ok_or_else(|| -> Box<dyn Error> { format!("dv package event omitted {field}").into() })?
    .iter()
    .map(|value| {
      value
        .as_str()
        .or_else(|| value.get("path").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .ok_or_else(|| format!("dv {field} contains an asset without a path").into())
    })
    .collect()
}

fn package_relative_path(path: &str) -> Result<String> {
  let path = path.replace('\\', "/");
  let marker = "/.packages/";
  let relative = if let Some(path) = path.strip_prefix(".packages/") {
    path
  } else {
    path
      .find(marker)
      .map(|index| &path[index + marker.len()..])
      .ok_or_else(|| format!("dv package asset is outside the isolated package cache: {path}"))?
  };
  let mut parts = relative.splitn(3, '/');
  let id = parts.next().ok_or("dv package asset omitted package id")?;
  let version = parts.next().ok_or("dv package asset omitted package version")?;
  let asset = parts.next().ok_or("dv package asset omitted its package-relative path")?;
  Ok(format!("{}/{}/{}", id.to_ascii_lowercase(), version.to_ascii_lowercase(), asset))
}

fn compare_canonical_item_paths(dotnet: &serde_json::Value, dotnet_item: &str, dv: &serde_json::Value, dv_field: &str) -> Result<()> {
  let mut reference = item_identities(dotnet, dotnet_item)?
    .into_iter()
    .map(|path| fs::canonicalize(path).map_err(Into::into))
    .collect::<Result<Vec<_>>>()?;
  let mut actual = string_array(dv, dv_field)?
    .into_iter()
    .map(|path| fs::canonicalize(path).map_err(Into::into))
    .collect::<Result<Vec<_>>>()?;
  reference.sort_unstable();
  actual.sort_unstable();
  if reference != actual {
    return Err(format!("{dv_field} batch does not match MSBuild: dotnet={} dv={}", reference.len(), actual.len()).into());
  }
  Ok(())
}

fn item_identities(document: &serde_json::Value, item: &str) -> Result<Vec<String>> {
  document
    .pointer(&format!("/Items/{item}"))
    .and_then(serde_json::Value::as_array)
    .ok_or_else(|| format!("dotnet project query omitted {item} items"))?
    .iter()
    .map(|value| {
      value
        .get("Identity")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("dotnet {item} item omitted Identity").into())
    })
    .collect()
}

fn json_event(text: &str, kind: &str) -> Option<serde_json::Value> {
  text
    .lines()
    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some(kind))
}

fn string_array(document: &serde_json::Value, field: &str) -> Result<Vec<String>> {
  document
    .get(field)
    .and_then(serde_json::Value::as_array)
    .ok_or_else(|| format!("dv project event omitted {field}"))?
    .iter()
    .map(|value| {
      value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("dv {field} contains non-text data").into())
    })
    .collect()
}

fn prepare_dv_executable(repository: &Path, requested: Option<&Path>) -> Result<PathBuf> {
  let cargo = env::var_os("CARGO").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo"));
  run_checked(
    &cargo,
    &["build", "-p", "nuget-plugin-dv-fixture", "--release", "--quiet"],
    repository,
    "credential-provider fixture release build",
  )?;
  if let Some(path) = requested {
    return Ok(if path.is_absolute() { path.to_owned() } else { repository.join(path) });
  }

  run_checked(&cargo, &["build", "-p", "dv-cli", "--release", "--quiet"], repository, "dv release build")?;
  Ok(repository.join("target/release").join(format!("dv{}", env::consts::EXE_SUFFIX)))
}

fn parse_options(arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<Options> {
  let mut options = Options {
    warmups: 2,
    samples: 10,
    output: None,
    dv: None,
    case: None,
  };
  let mut arguments = arguments.peekable();

  while let Some(argument) = arguments.next() {
    match argument.to_str() {
      Some("--quick") => {
        options.warmups = 1;
        options.samples = 3;
      },
      Some("--warmups") => {
        options.warmups = parse_count("--warmups", arguments.next())?;
      },
      Some("--samples") => {
        options.samples = parse_count("--samples", arguments.next())?;
      },
      Some("--output") => {
        options.output = Some(arguments.next().ok_or("--output requires a path")?.into());
      },
      Some("--dv") => {
        options.dv = Some(arguments.next().ok_or("--dv requires a path")?.into());
      },
      Some("--case") => {
        options.case = Some(
          arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or("--case requires a Unicode case name")?,
        );
      },
      Some("-h" | "--help") => {
        println!(
          "Usage: dv-bench [--quick] [--warmups N] [--samples N] \
                     [--output PATH] [--dv PATH] [--case NAME]"
        );
        std::process::exit(0);
      },
      _ => return Err(format!("unknown or non-Unicode option {argument:?}").into()),
    }
  }

  if options.samples == 0 {
    return Err("--samples must be greater than zero".into());
  }
  Ok(options)
}

fn parse_count(option: &str, value: Option<std::ffi::OsString>) -> Result<usize> {
  value
    .and_then(|value| value.into_string().ok())
    .ok_or_else(|| format!("{option} requires a Unicode integer"))?
    .parse()
    .map_err(|_| format!("{option} requires a non-negative integer").into())
}

fn run_tool(tool_name: &str, executable: &Path, cases: &[Case], options: &Options, fixtures: &Fixtures<'_>, workspace: &Path) -> Result<Vec<Run>> {
  let version = command_text(executable, &["--version"], fixtures.small)?;
  let mut runs = Vec::with_capacity(cases.len());

  for case in cases.iter().filter(|case| options.case.as_deref().is_none_or(|name| name == case.name)) {
    let case_workspace = workspace.join(case.name);
    let case_fixture = case_fixture(case, fixtures);
    let command: Vec<String> = if !case.implemented && case.args.is_empty() {
      vec!["TBI (no equivalent command)".into()]
    } else {
      std::iter::once(executable.display().to_string())
        .chain(case.args.iter().map(|value| (*value).into()))
        .collect()
    };

    if !case.implemented {
      runs.push(Run {
        tool: tool_name.into(),
        tool_version: version.clone(),
        fixture: fixture_name(case).map(str::to_owned),
        case: case.name.into(),
        command,
        status: RunStatus::Tbi,
        warmups: 0,
        samples_ns: Vec::new(),
        statistics_ns: None,
        network_requests: None,
        downloaded_bytes: None,
        downloaded_packages: None,
        resolved_packages: None,
      });
      continue;
    }

    prepare_persistent_case(executable, case, case_fixture, &case_workspace)?;
    let _client_certificate_fixture = if matches!(case.kind, CaseKind::NugetClientCertificates) {
      Some(prepare_client_certificate_fixture(&case_workspace)?)
    } else {
      None
    };
    let request_budget_fixture = if matches!(case.kind, CaseKind::NugetRequestBudget | CaseKind::NugetSourceTelemetry) {
      Some(RequestBudgetFixture::start(&case_workspace)?)
    } else {
      None
    };

    let mut samples_ns = Vec::with_capacity(options.samples);
    let mut work = None;
    let total = options.warmups + options.samples;
    for index in 0..total {
      prepare_iteration(executable, case, case_fixture, &case_workspace)?;
      if let Some(fixture) = request_budget_fixture.as_ref() {
        fixture.reset_metrics();
      }
      let mut measurement = measure(executable, case, case_cwd(case, case_fixture, &case_workspace))?;
      if let Some(fixture) = request_budget_fixture.as_ref() {
        let fixture_work = fixture.validate_metrics()?;
        if matches!(case.kind, CaseKind::NugetSourceTelemetry) && !is_dotnet(executable) {
          fixture.validate_reported_telemetry(measurement.work)?;
        }
        measurement.work = Some(fixture_work);
      }
      if index >= options.warmups {
        samples_ns.push(measurement.elapsed_ns);
        merge_work_evidence(&mut work, measurement.work, tool_name, case.name)?;
      }
    }
    if let Some(fixture) = request_budget_fixture.as_ref() {
      fixture.validate_saturation()?;
    }

    let statistics_ns = statistics(&samples_ns);
    runs.push(Run {
      tool: tool_name.into(),
      tool_version: version.clone(),
      fixture: fixture_name(case).map(str::to_owned),
      case: case.name.into(),
      command,
      status: RunStatus::Measured,
      warmups: options.warmups,
      samples_ns,
      statistics_ns: Some(statistics_ns),
      network_requests: work.and_then(|evidence| evidence.network_requests),
      downloaded_bytes: work.and_then(|evidence| evidence.downloaded_bytes),
      downloaded_packages: work.and_then(|evidence| evidence.downloaded_packages),
      resolved_packages: work.and_then(|evidence| evidence.resolved_packages),
    });
  }

  Ok(runs)
}

fn prepare_persistent_case(executable: &Path, case: &Case, fixture: &Path, workspace: &Path) -> Result<()> {
  if matches!(
    case.kind,
    CaseKind::RidGraph
      | CaseKind::ProjectEvaluate
      | CaseKind::PackageReferenceConditions
      | CaseKind::RuntimeEvaluate
      | CaseKind::RuntimePackPlan
      | CaseKind::RuntimePackInventoryCold
      | CaseKind::FrameworkReferencePlan
      | CaseKind::CompilerPlan
      | CaseKind::PackageAssetPlan
      | CaseKind::PackageSyncWarm
      | CaseKind::PackageReferenceMetadata
      | CaseKind::PackagePruning
      | CaseKind::PackageSignatureCold
      | CaseKind::PackageSignatureWarm
      | CaseKind::NuspecFrameworkMetadata
      | CaseKind::PackageRidContentCold
      | CaseKind::PackageRidContentWarm
      | CaseKind::CentralPackageManagement
      | CaseKind::PackageConflictResolution
      | CaseKind::PackageBatchResolution
      | CaseKind::PackageDiagnostics
      | CaseKind::NugetConfigHierarchy
      | CaseKind::NugetConfigMerge
      | CaseKind::NugetSourceSections
      | CaseKind::NugetSourceMapping
      | CaseKind::NugetRequestBudget
      | CaseKind::NugetSourceTelemetry
      | CaseKind::NugetStoragePolicy
      | CaseKind::NugetCliOverrides
      | CaseKind::NugetLocalSources
      | CaseKind::NugetFloatingVersion
      | CaseKind::NugetServiceIndex
      | CaseKind::NugetCredentials
      | CaseKind::NugetCredentialProvider
      | CaseKind::NugetClientCertificates
      | CaseKind::NugetHttpPolicy
      | CaseKind::NugetSourceSecurity
      | CaseKind::CliGoldenTrace
      | CaseKind::CliCompatCheck
      | CaseKind::CliUnknownOption
      | CaseKind::CliOptionEffects
      | CaseKind::CliDriverInventory
      | CaseKind::CliCommandNormalization
      | CaseKind::CliTransformEquivalence
      | CaseKind::CliModeClassification
      | CaseKind::CliExitPolicy
      | CaseKind::CliLexicalPreservation
      | CaseKind::CliRoutePrecedence
      | CaseKind::CliEnvironment
      | CaseKind::CliForwarding
      | CaseKind::CliChildExit
      | CaseKind::BuildNoOp
      | CaseKind::RunWarm
  ) {
    reset_fixture(fixture, workspace)?;
  }
  if matches!(case.kind, CaseKind::RidGraph) && is_dotnet(executable) {
    prepare_rid_oracle(executable, workspace)?;
  }
  if matches!(case.kind, CaseKind::CliForwarding | CaseKind::CliChildExit) && is_dotnet(executable) {
    run_checked(
      executable,
      &["build", "ArgumentForwarding.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
      workspace,
      "argument-forwarding oracle build",
    )?;
  }
  if matches!(case.kind, CaseKind::NugetServiceIndex) && is_dotnet(executable) {
    run_checked(
      executable,
      &["build", "oracle/ServiceIndexOracle.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
      workspace,
      "NuGet service-index oracle build",
    )?;
  }
  if matches!(case.kind, CaseKind::NugetCredentials) && is_dotnet(executable) {
    run_checked(
      executable,
      &["build", "oracle/CredentialOracle.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
      workspace,
      "NuGet credential oracle build",
    )?;
  }
  if matches!(case.kind, CaseKind::NugetCredentialProvider) && is_dotnet(executable) {
    run_checked(
      executable,
      &[
        "build",
        "oracle/CredentialProviderOracle.csproj",
        "-c",
        "Release",
        "--nologo",
        "--verbosity",
        "quiet",
      ],
      workspace,
      "NuGet credential-provider oracle build",
    )?;
  }
  if matches!(case.kind, CaseKind::NugetClientCertificates) {
    run_checked(
      Path::new("dotnet"),
      &[
        "build",
        "oracle/ClientCertificateOracle.csproj",
        "-c",
        "Release",
        "--nologo",
        "--verbosity",
        "quiet",
      ],
      workspace,
      "NuGet client-certificate oracle build",
    )?;
  }
  if matches!(case.kind, CaseKind::NugetHttpPolicy) && is_dotnet(executable) {
    run_checked(
      executable,
      &["build", "oracle/HttpPolicyOracle.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
      workspace,
      "NuGet HTTP-policy oracle build",
    )?;
  }
  if matches!(case.kind, CaseKind::NugetSourceSecurity) && is_dotnet(executable) {
    run_checked(
      executable,
      &[
        "build",
        "oracle/SourceSecurityOracle.csproj",
        "-c",
        "Release",
        "--nologo",
        "--verbosity",
        "quiet",
      ],
      workspace,
      "NuGet source-security oracle build",
    )?;
  }
  if matches!(case.kind, CaseKind::CompilerPlan) && is_dotnet(executable) {
    run_checked(executable, &["restore", "--nologo", "--verbosity", "quiet"], workspace, "compiler plan restore")?;
  }
  if matches!(case.kind, CaseKind::RuntimePackPlan) {
    run_checked(
      Path::new("dotnet"),
      &[
        "restore",
        "RuntimePackProject.csproj",
        "--packages",
        ".packages",
        "--nologo",
        "-r",
        "win-x64",
        "-p:SelfContained=true",
        "-p:UseAppHost=true",
        "-p:NuGetAudit=false",
        "--verbosity",
        "quiet",
      ],
      workspace,
      "runtime-pack plan restore",
    )?;
  }
  if matches!(case.kind, CaseKind::RuntimePackInventoryCold) {
    run_checked(
      Path::new("dotnet"),
      &[
        "restore",
        "RuntimePackProject.csproj",
        "--packages",
        ".packages",
        "--nologo",
        "-r",
        "win-x64",
        "-p:SelfContained=true",
        "-p:UseAppHost=true",
        "-p:NuGetAudit=false",
        "--verbosity",
        "quiet",
      ],
      workspace,
      "cold runtime-pack inventory setup",
    )?;
  }
  if matches!(case.kind, CaseKind::FrameworkReferencePlan) && is_dotnet(executable) {
    run_checked(
      executable,
      &[
        "restore",
        "FrameworkReferenceProject.csproj",
        "--nologo",
        "-p:NuGetAudit=false",
        "--verbosity",
        "quiet",
      ],
      workspace,
      "framework-reference plan restore",
    )?;
  }
  if matches!(case.kind, CaseKind::PackageSyncWarm) {
    if is_dotnet(executable) {
      run_checked(
        executable,
        &[
          "restore",
          "PackageConsole.csproj",
          "--use-lock-file",
          "--packages",
          ".packages",
          "--no-http-cache",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "warm package restore setup",
      )?;
    } else {
      run_checked(
        executable,
        &["restore", "PackageConsole.csproj", "--packages", ".packages", "--json"],
        workspace,
        "warm package sync setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::NugetConfigHierarchy) {
    if is_dotnet(executable) {
      run_nuget_config_checked(
        executable,
        &[
          "restore",
          "ConfigHierarchy.csproj",
          "--use-lock-file",
          "--no-http-cache",
          "-p:NuGetAudit=false",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "NuGet configuration hierarchy setup",
      )?;
    } else {
      run_nuget_config_checked(
        executable,
        &["restore", "ConfigHierarchy.csproj", "--json"],
        workspace,
        "NuGet configuration hierarchy setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::NugetConfigMerge) {
    if is_dotnet(executable) {
      run_nuget_config_checked(
        executable,
        &[
          "restore",
          "ConfigMerge.csproj",
          "--use-lock-file",
          "--no-http-cache",
          "-p:NuGetAudit=false",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "NuGet configuration merge setup",
      )?;
    } else {
      run_nuget_config_checked(
        executable,
        &["restore", "ConfigMerge.csproj", "--json"],
        workspace,
        "NuGet configuration merge setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::NugetSourceSections) {
    if is_dotnet(executable) {
      run_nuget_config_checked(
        executable,
        &[
          "restore",
          "SourceSections.csproj",
          "--use-lock-file",
          "--no-http-cache",
          "-p:NuGetAudit=false",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "NuGet source-section setup",
      )?;
    } else {
      run_nuget_config_checked(
        executable,
        &["restore", "SourceSections.csproj", "--json"],
        workspace,
        "NuGet source-section setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::NugetStoragePolicy) {
    prepare_nuget_storage_policy(executable, workspace)?;
  }
  if matches!(case.kind, CaseKind::NugetCliOverrides) {
    prepare_nuget_cli_overrides(executable, workspace)?;
  }
  if matches!(case.kind, CaseKind::NugetLocalSources) {
    prepare_nuget_local_sources(workspace)?;
  }
  if matches!(case.kind, CaseKind::NugetFloatingVersion) {
    prepare_nuget_floating_version(workspace)?;
  }
  if matches!(case.kind, CaseKind::NugetRequestBudget | CaseKind::NugetSourceTelemetry) {
    prepare_nuget_request_budget(workspace)?;
  }
  if matches!(case.kind, CaseKind::PackageAssetPlan) {
    if is_dotnet(executable) {
      run_checked(
        executable,
        &[
          "restore",
          "MassivePackageGraph.csproj",
          "--use-lock-file",
          "--packages",
          ".packages",
          "--no-http-cache",
          "-p:NuGetAudit=false",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "package asset plan setup",
      )?;
    } else {
      run_checked(
        executable,
        &["restore", "MassivePackageGraph.csproj", "--packages", ".packages", "--json"],
        workspace,
        "package asset plan setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::PackageReferenceMetadata) {
    if is_dotnet(executable) {
      run_checked(
        executable,
        &[
          "restore",
          "MetadataProject.csproj",
          "--use-lock-file",
          "--packages",
          ".packages",
          "--no-http-cache",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "PackageReference metadata setup",
      )?;
    } else {
      run_checked(
        executable,
        &["restore", "MetadataProject.csproj", "--packages", ".packages", "--json"],
        workspace,
        "PackageReference metadata setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::PackagePruning) {
    if is_dotnet(executable) {
      run_checked(
        executable,
        &[
          "restore",
          "LegacyPruningProject.csproj",
          "--use-lock-file",
          "--packages",
          ".packages",
          "--no-http-cache",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "package-pruning setup",
      )?;
    } else {
      run_checked(
        executable,
        &["restore", "LegacyPruningProject.csproj", "--packages", ".packages", "--json"],
        workspace,
        "package-pruning setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::PackageSignatureWarm) {
    if is_dotnet(executable) {
      run_checked(
        executable,
        &[
          "restore",
          "PackageSignatures.csproj",
          "--use-lock-file",
          "--packages",
          ".packages",
          "--no-http-cache",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "package-signature setup",
      )?;
    } else {
      run_checked(
        executable,
        &["restore", "PackageSignatures.csproj", "--packages", ".packages", "--json"],
        workspace,
        "package-signature setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::NuspecFrameworkMetadata) {
    prepare_nuspec_framework_metadata(workspace)?;
  }
  if matches!(case.kind, CaseKind::PackageRidContentCold | CaseKind::PackageRidContentWarm) {
    prepare_package_rid_content(workspace)?;
  }
  if matches!(case.kind, CaseKind::PackageRidContentWarm) {
    if is_dotnet(executable) {
      run_checked(
        executable,
        &[
          "restore",
          "WindowsFallback.csproj",
          "--use-lock-file",
          "--packages",
          ".packages",
          "--no-http-cache",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "warm RID/content restore setup",
      )?;
    } else {
      run_checked(
        executable,
        &["restore", "WindowsFallback.csproj", "--packages", ".packages", "--json"],
        workspace,
        "warm RID/content sync setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::CentralPackageManagement) {
    if is_dotnet(executable) {
      run_checked(
        executable,
        &[
          "restore",
          "CentralPackages.csproj",
          "--use-lock-file",
          "--packages",
          ".packages",
          "--no-http-cache",
          "--nologo",
          "--verbosity",
          "quiet",
        ],
        workspace,
        "central package management setup",
      )?;
    } else {
      run_checked(
        executable,
        &["restore", "CentralPackages.csproj", "--packages", ".packages", "--json"],
        workspace,
        "central package management setup",
      )?;
    }
  }
  if matches!(case.kind, CaseKind::PackageConflictResolution | CaseKind::PackageDiagnostics) {
    prepare_package_conflict_resolution(executable, workspace, matches!(case.kind, CaseKind::PackageDiagnostics))?;
  }
  if matches!(case.kind, CaseKind::PackageBatchResolution) {
    prepare_package_batch_resolution(workspace)?;
  }
  if matches!(case.kind, CaseKind::BuildNoOp | CaseKind::RunWarm) {
    run_checked(executable, build_args(executable), workspace, "persistent case setup")?;
  }
  Ok(())
}

fn prepare_rid_oracle(executable: &Path, workspace: &Path) -> Result<()> {
  run_checked(
    executable,
    &["build", "RidGraphOracle.csproj", "-c", "Release", "--nologo", "--verbosity", "quiet"],
    workspace,
    "RID graph oracle build",
  )
}

fn prepare_nuget_storage_policy(executable: &Path, workspace: &Path) -> Result<()> {
  let fallback = workspace.join("policy/fallback-final");
  if fallback.exists() {
    fs::remove_dir_all(&fallback)?;
  }
  fs::create_dir_all(&fallback)?;
  let seed = workspace.join("policy/seed");
  if seed.exists() {
    fs::remove_dir_all(&seed)?;
  }
  run_nuget_storage_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "StoragePolicy.csproj",
      "--use-lock-file",
      "--packages",
      "policy/seed",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    workspace,
    "NuGet storage-policy package seed",
  )?;
  copy_directory(&seed.join("newtonsoft.json"), &fallback.join("newtonsoft.json"))?;
  fs::remove_dir_all(seed)?;
  let global = workspace.join("policy/env-global");
  if global.exists() {
    fs::remove_dir_all(&global)?;
  }
  if is_dotnet(executable) {
    run_nuget_storage_checked(
      executable,
      &[
        "restore",
        "StoragePolicy.csproj",
        "--use-lock-file",
        "--no-http-cache",
        "--nologo",
        "--verbosity",
        "quiet",
      ],
      workspace,
      "NuGet storage-policy setup",
    )
  } else {
    run_nuget_storage_checked(
      executable,
      &["restore", "StoragePolicy.csproj", "--offline", "--json"],
      workspace,
      "NuGet storage-policy setup",
    )
  }
}

fn prepare_nuget_cli_overrides(executable: &Path, workspace: &Path) -> Result<()> {
  let packages = workspace.join("policy/cli-global");
  if packages.exists() {
    fs::remove_dir_all(&packages)?;
  }
  run_nuget_cli_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "CliOverrides.csproj",
      "--use-lock-file",
      "--source",
      "https://api.nuget.org/v3/index.json",
      "--configfile",
      "config/selected.config",
      "--packages",
      "policy/cli-global",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    workspace,
    "NuGet CLI override package seed",
  )?;
  if is_dotnet(executable) {
    Ok(())
  } else {
    run_nuget_cli_checked(
      executable,
      &[
        "restore",
        "CliOverrides.csproj",
        "--source",
        "https://api.nuget.org/v3/index.json",
        "--configfile",
        "config/selected.config",
        "--packages",
        "policy/cli-global",
        "--offline",
        "--json",
      ],
      workspace,
      "NuGet CLI override setup",
    )
  }
}

fn prepare_nuget_local_sources(workspace: &Path) -> Result<()> {
  for relative in ["feeds", ".seed", ".packages", "obj", "dv.lock.json", ".seed.config"] {
    remove_generated_path(&workspace.join(relative))?;
  }
  fs::write(
    workspace.join(".seed.config"),
    r#"<?xml version="1.0" encoding="utf-8"?><configuration><packageSources><clear /><add key="nuget.org" value="https://api.nuget.org/v3/index.json" protocolVersion="3" /></packageSources></configuration>"#,
  )?;
  run_nuget_config_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "LocalSources.csproj",
      "--configfile",
      ".seed.config",
      "--packages",
      ".seed",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    workspace,
    "NuGet local-source package seed",
  )?;

  copy_with_parent(
    &workspace.join(".seed/newtonsoft.json/13.0.3/newtonsoft.json.13.0.3.nupkg"),
    &workspace.join("feeds/flat/Newtonsoft.Json.13.0.3.nupkg"),
  )?;
  let hierarchical = workspace.join("feeds/hierarchical/humanizer.core/2.14.1");
  copy_with_parent(
    &workspace.join(".seed/humanizer.core/2.14.1/humanizer.core.2.14.1.nupkg"),
    &hierarchical.join("humanizer.core.2.14.1.nupkg"),
  )?;
  copy_with_parent(
    &workspace.join(".seed/humanizer.core/2.14.1/humanizer.core.nuspec"),
    &hierarchical.join("humanizer.core.nuspec"),
  )?;
  copy_with_parent(
    &workspace.join(".seed/humanizer.core/2.14.1/humanizer.core.2.14.1.nupkg.sha512"),
    &hierarchical.join("humanizer.core.2.14.1.nupkg.sha512"),
  )?;
  fs::remove_file(workspace.join(".seed.config"))?;
  reset_nuget_local_iteration(workspace)
}

fn prepare_nuget_floating_version(workspace: &Path) -> Result<()> {
  for relative in [
    "feeds",
    ".seed",
    ".seed-project",
    ".packages",
    "obj",
    "dv.lock.json",
    "packages.lock.json",
    ".seed.config",
  ] {
    remove_generated_path(&workspace.join(relative))?;
  }
  fs::write(
    workspace.join(".seed.config"),
    r#"<?xml version="1.0" encoding="utf-8"?><configuration><packageSources><clear /><add key="nuget.org" value="https://api.nuget.org/v3/index.json" protocolVersion="3" /></packageSources></configuration>"#,
  )?;
  for version in ["13.0.3", "13.0.4"] {
    let project = format!(".seed-project/{version}/Seed.csproj");
    let project_path = workspace.join(&project);
    fs::create_dir_all(project_path.parent().expect("a seed project has a parent"))?;
    fs::write(
      &project_path,
      format!(
        r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup><ItemGroup><PackageReference Include="Newtonsoft.Json" Version="{version}" /></ItemGroup></Project>"#
      ),
    )?;
    run_nuget_config_checked(
      Path::new("dotnet"),
      &[
        "restore",
        &project,
        "--configfile",
        ".seed.config",
        "--packages",
        ".seed",
        "--no-http-cache",
        "--disable-build-servers",
        "--nologo",
        "--verbosity",
        "quiet",
      ],
      workspace,
      "NuGet floating-version package seed",
    )?;
    copy_with_parent(
      &workspace.join(format!(".seed/newtonsoft.json/{version}/newtonsoft.json.{version}.nupkg")),
      &workspace.join(format!("feeds/Newtonsoft.Json.{version}.nupkg")),
    )?;
  }
  fs::remove_file(workspace.join(".seed.config"))?;
  reset_nuget_floating_iteration(workspace)
}

struct ConflictPackage {
  id: &'static str,
  version: &'static str,
  dependencies: &'static [(&'static str, &'static str)],
}

const CONFLICT_PACKAGES: &[ConflictPackage] = &[
  ConflictPackage {
    id: "Direct.Leaf",
    version: "1.0.0",
    dependencies: &[],
  },
  ConflictPackage {
    id: "Direct.Leaf",
    version: "2.0.0",
    dependencies: &[],
  },
  ConflictPackage {
    id: "Direct.Deep",
    version: "1.0.0",
    dependencies: &[("Direct.Leaf", "[2.0.0]")],
  },
  ConflictPackage {
    id: "Direct.Top",
    version: "1.0.0",
    dependencies: &[("Direct.Leaf", "[1.0.0]"), ("Direct.Deep", "[1.0.0]")],
  },
  ConflictPackage {
    id: "Cousin.Leaf",
    version: "1.0.0",
    dependencies: &[("Cousin.Stale", "[1.0.0]")],
  },
  ConflictPackage {
    id: "Cousin.Leaf",
    version: "2.0.0",
    dependencies: &[("Cousin.Current", "[1.0.0]")],
  },
  ConflictPackage {
    id: "Cousin.Stale",
    version: "1.0.0",
    dependencies: &[],
  },
  ConflictPackage {
    id: "Cousin.Current",
    version: "1.0.0",
    dependencies: &[],
  },
  ConflictPackage {
    id: "Cousin.Deep",
    version: "1.0.0",
    dependencies: &[("Cousin.Leaf", "[2.0.0]")],
  },
  ConflictPackage {
    id: "Cousin.Left",
    version: "1.0.0",
    dependencies: &[("Cousin.Leaf", "[1.0.0,3.0.0)")],
  },
  ConflictPackage {
    id: "Cousin.Right",
    version: "1.0.0",
    dependencies: &[("Cousin.Deep", "[1.0.0]")],
  },
  ConflictPackage {
    id: "Diamond.Leaf",
    version: "1.0.0",
    dependencies: &[],
  },
  ConflictPackage {
    id: "Diamond.Leaf",
    version: "2.0.0",
    dependencies: &[],
  },
  ConflictPackage {
    id: "Diamond.Provider",
    version: "1.0.0",
    dependencies: &[("Diamond.Leaf", "[1.0.0,3.0.0)"), ("Diamond.Relational", "[1.0.0]")],
  },
  ConflictPackage {
    id: "Diamond.Relational",
    version: "1.0.0",
    dependencies: &[("Diamond.Leaf", "[2.0.0]")],
  },
];

const DIAGNOSTIC_PACKAGES: &[ConflictPackage] = &[
  ConflictPackage {
    id: "Diagnostic.Leaf",
    version: "1.0.0",
    dependencies: &[],
  },
  ConflictPackage {
    id: "Diagnostic.Leaf",
    version: "2.0.0",
    dependencies: &[],
  },
  ConflictPackage {
    id: "Diagnostic.Left",
    version: "1.0.0",
    dependencies: &[("Diagnostic.Leaf", "[1.0.0]")],
  },
  ConflictPackage {
    id: "Diagnostic.Right",
    version: "1.0.0",
    dependencies: &[("Diagnostic.Leaf", "[2.0.0]")],
  },
  ConflictPackage {
    id: "Diagnostic.Cycle.A",
    version: "1.0.0",
    dependencies: &[("Diagnostic.Cycle.B", "[1.0.0]")],
  },
  ConflictPackage {
    id: "Diagnostic.Cycle.B",
    version: "1.0.0",
    dependencies: &[("Diagnostic.Cycle.A", "[1.0.0]")],
  },
  ConflictPackage {
    id: "Diagnostic.Known",
    version: "1.0.0",
    dependencies: &[],
  },
];

fn write_conflict_package(feed: &Path, package: &ConflictPackage) -> Result<()> {
  use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

  fs::create_dir_all(feed)?;
  let path = feed.join(format!("{}.{}.nupkg", package.id, package.version));
  let mut archive = ZipWriter::new(fs::File::create(path)?);
  let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
  let mut nuspec = format!(
    "<?xml version=\"1.0\"?><package><metadata><id>{}</id><version>{}</version><authors>dv</authors><description>Conflict resolution oracle.</description><dependencies><group targetFramework=\"net10.0\">",
    package.id, package.version
  );
  for (id, range) in package.dependencies {
    write!(nuspec, "<dependency id=\"{id}\" version=\"{range}\" />")?;
  }
  nuspec.push_str("</group></dependencies></metadata></package>");
  archive.start_file(format!("{}.nuspec", package.id), options)?;
  archive.write_all(nuspec.as_bytes())?;
  archive.start_file(format!("lib/net10.0/{}.dll", package.id), options)?;
  archive.write_all(b"deterministic managed assembly placeholder")?;
  archive.finish()?;
  Ok(())
}

fn write_nuspec_framework_packages(feed: &Path) -> Result<()> {
  use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

  fs::create_dir_all(feed)?;
  let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
  for (id, manifest) in [
    (
      "Framework.Child",
      r#"<?xml version="1.0"?><package><metadata><id>Framework.Child</id><version>1.0.0</version><authors>dv</authors><description>Framework metadata child.</description></metadata></package>"#,
    ),
    (
      "Framework.Metadata",
      r#"<?xml version="1.0"?><package><metadata><id>Framework.Metadata</id><version>1.0.0</version><authors>dv</authors><description>Framework metadata oracle.</description>
<dependencies>
  <group targetFramework="net8.0"><dependency id="Unselected.Dependency" version="[9.0.0]" /></group>
  <group targetFramework="net10.0"><dependency id="Framework.Child" version="[1.0.0]" /></group>
  <group targetFramework="net48" />
</dependencies>
<frameworkReferences>
  <group targetFramework="net8.0"><frameworkReference name="Microsoft.WindowsDesktop.App" /></group>
  <group targetFramework="net10.0"><frameworkReference name="Microsoft.AspNetCore.App" /></group>
</frameworkReferences>
<frameworkAssemblies>
  <frameworkAssembly assemblyName="System.Net.Http" targetFramework=".NETFramework4.7.2" />
  <frameworkAssembly assemblyName="System.Runtime" />
  <frameworkAssembly assemblyName="System.Xml" targetFramework="net48" />
  <frameworkAssembly assemblyName="System.Data" targetFramework=".NETFramework4.8, net472" />
</frameworkAssemblies>
</metadata></package>"#,
    ),
  ] {
    let mut archive = ZipWriter::new(fs::File::create(feed.join(format!("{id}.1.0.0.nupkg")))?);
    archive.start_file(format!("{id}.nuspec"), options)?;
    archive.write_all(manifest.as_bytes())?;
    archive.start_file(format!("lib/net10.0/{id}.dll"), options)?;
    archive.write_all(b"deterministic managed assembly placeholder")?;
    archive.start_file(format!("lib/net48/{id}.dll"), options)?;
    archive.write_all(b"deterministic legacy assembly placeholder")?;
    archive.finish()?;
  }
  Ok(())
}

fn write_rid_content_package(feed: &Path) -> Result<()> {
  use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

  fs::create_dir_all(feed)?;
  let mut archive = ZipWriter::new(fs::File::create(feed.join("Rid.Content.Metadata.1.0.0.nupkg"))?);
  let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
  let manifest = r#"<?xml version="1.0"?><package><metadata>
<id>Rid.Content.Metadata</id><version>1.0.0</version><authors>dv</authors>
<description>RID and contentFiles selection oracle.</description>
<contentFiles>
  <files include="**/*" buildAction="Content" copyToOutput="false" flatten="false" />
  <files include="cs/**/*.pp" buildAction="Compile" copyToOutput="true" flatten="true" />
  <files include="any/**" copyToOutput="true" />
  <files include="**/*.json" buildAction="None" copyToOutput="true" flatten="false" />
</contentFiles>
</metadata></package>"#;
  archive.start_file("Rid.Content.Metadata.nuspec", options)?;
  archive.write_all(manifest.as_bytes())?;
  for path in [
    "lib/net10.0/Rid.Content.Metadata.dll",
    "lib/net10.0/fr/Rid.Content.Metadata.resources.dll",
    "native/portable.bin",
    "runtimes/win-x64/lib/net10.0/Rid.Content.Metadata.dll",
    "runtimes/win-x64/lib/net10.0/fr/Rid.Content.Metadata.resources.dll",
    "runtimes/win-x64/native/exact-win-x64.bin",
    "runtimes/win/lib/net10.0/Rid.Content.Metadata.dll",
    "runtimes/win/lib/net10.0/fr/Rid.Content.Metadata.resources.dll",
    "runtimes/win/native/fallback-win.bin",
    "runtimes/linux-x64/lib/net10.0/Rid.Content.Metadata.dll",
    "runtimes/linux-x64/lib/net10.0/fr/Rid.Content.Metadata.resources.dll",
    "runtimes/linux-x64/native/exact-linux-x64.so",
    "contentFiles/any/any/readme.txt",
    "contentFiles/cs/net8.0/old.cs",
    "contentFiles/cs/net10.0/generated/template.cs.pp",
    "contentFiles/cs/net10.0/config/settings.json",
    "contentFiles/vb/any/sample.vb",
  ] {
    archive.start_file(path, options)?;
    archive.write_all(b"deterministic RID/content fixture payload")?;
  }
  archive.finish()?;
  Ok(())
}

fn prepare_package_rid_content(workspace: &Path) -> Result<()> {
  let feed = workspace.join("feed");
  remove_generated_path(&feed)?;
  write_rid_content_package(&feed)
}

fn prepare_nuspec_framework_metadata(workspace: &Path) -> Result<()> {
  let feed = workspace.join("feed");
  remove_generated_path(&feed)?;
  write_nuspec_framework_packages(&feed)?;
  reset_nuspec_framework_metadata_iteration(workspace)
}

fn write_incompatible_package(feed: &Path) -> Result<()> {
  use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

  fs::create_dir_all(feed)?;
  let mut archive = ZipWriter::new(fs::File::create(feed.join("Diagnostic.Future.1.0.0.nupkg"))?);
  let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
  archive.start_file("Diagnostic.Future.nuspec", options)?;
  archive.write_all(
    br#"<?xml version="1.0"?><package><metadata><id>Diagnostic.Future</id><version>1.0.0</version><authors>dv</authors><description>Incompatible framework oracle.</description></metadata></package>"#,
  )?;
  archive.start_file("lib/net11.0/Diagnostic.Future.dll", options)?;
  archive.write_all(b"deterministic managed assembly placeholder")?;
  archive.finish()?;
  Ok(())
}

fn prepare_package_conflict_resolution(executable: &Path, workspace: &Path, diagnostic: bool) -> Result<()> {
  let feed = workspace.join("feed");
  remove_generated_path(&feed)?;
  for package in if diagnostic { DIAGNOSTIC_PACKAGES } else { CONFLICT_PACKAGES } {
    write_conflict_package(&feed, package)?;
  }
  if diagnostic {
    write_incompatible_package(&feed)?;
    return reset_package_diagnostic_iteration(workspace);
  }
  if is_dotnet(executable) {
    run_checked(
      executable,
      &[
        "restore",
        "ConflictResolution.csproj",
        "--packages",
        ".packages",
        "-p:NoWarn=NU1605",
        "--nologo",
        "--verbosity",
        "quiet",
      ],
      workspace,
      "package conflict resolution setup",
    )?;
  } else {
    run_checked(
      executable,
      &["restore", "ConflictResolution.csproj", "--packages", ".packages", "--offline", "--json"],
      workspace,
      "package conflict resolution setup",
    )?;
  }
  reset_package_conflict_iteration(workspace)
}

fn prepare_package_batch_resolution(workspace: &Path) -> Result<()> {
  let feed = workspace.join("feed");
  remove_generated_path(&feed)?;
  for package in CONFLICT_PACKAGES {
    write_conflict_package(&feed, package)?;
  }
  reset_package_batch_iteration(workspace)
}

fn prepare_nuget_request_budget(workspace: &Path) -> Result<()> {
  for relative in [
    ".seed",
    ".packages",
    ".http-cache",
    "obj",
    "dv.lock.json",
    "packages.lock.json",
    "NuGet.Config",
    ".seed.config",
  ] {
    remove_generated_path(&workspace.join(relative))?;
  }
  fs::write(
    workspace.join(".seed.config"),
    r#"<?xml version="1.0" encoding="utf-8"?><configuration><packageSources><clear /><add key="nuget.org" value="https://api.nuget.org/v3/index.json" protocolVersion="3" /></packageSources></configuration>"#,
  )?;
  run_checked(
    Path::new("dotnet"),
    &[
      "restore",
      "RequestBudget.csproj",
      "--configfile",
      ".seed.config",
      "--packages",
      ".seed",
      "--no-http-cache",
      "--nologo",
      "--verbosity",
      "quiet",
    ],
    workspace,
    "NuGet request-budget package seed",
  )?;
  remove_generated_path(&workspace.join("obj"))?;
  fs::remove_file(workspace.join(".seed.config"))?;
  Ok(())
}

struct ServedPackage {
  id: String,
  version: String,
  archive: Arc<[u8]>,
}

struct RequestMetrics {
  active: AtomicUsize,
  peak: AtomicUsize,
  requests: AtomicUsize,
  bytes: AtomicUsize,
}

impl RequestMetrics {
  fn new() -> Self {
    Self {
      active: AtomicUsize::new(0),
      peak: AtomicUsize::new(0),
      requests: AtomicUsize::new(0),
      bytes: AtomicUsize::new(0),
    }
  }

  fn enter(&self) -> ActiveRequest<'_> {
    let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
    self.peak.fetch_max(active, Ordering::SeqCst);
    self.requests.fetch_add(1, Ordering::Relaxed);
    ActiveRequest { metrics: self }
  }

  fn reset(&self) {
    debug_assert_eq!(self.active.load(Ordering::SeqCst), 0);
    self.peak.store(0, Ordering::SeqCst);
    self.requests.store(0, Ordering::Relaxed);
    self.bytes.store(0, Ordering::Relaxed);
  }
}

struct ActiveRequest<'a> {
  metrics: &'a RequestMetrics,
}

impl Drop for ActiveRequest<'_> {
  fn drop(&mut self) {
    self.metrics.active.fetch_sub(1, Ordering::SeqCst);
  }
}

struct RequestBudgetFixture {
  stop: Arc<AtomicBool>,
  workers: Vec<JoinHandle<()>>,
  global: Arc<RequestMetrics>,
  sources: [Arc<RequestMetrics>; 2],
  package_root: PathBuf,
  packages: Arc<Vec<ServedPackage>>,
  observed_global_peak: AtomicUsize,
  observed_source_peaks: [AtomicUsize; 2],
}

impl RequestBudgetFixture {
  fn start(workspace: &Path) -> Result<Self> {
    let packages = Arc::new(read_seed_packages(&workspace.join(".seed"))?);
    if packages.len() < 6 {
      return Err(format!("request-budget seed produced only {} packages", packages.len()).into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let global = Arc::new(RequestMetrics::new());
    let sources = [Arc::new(RequestMetrics::new()), Arc::new(RequestMetrics::new())];
    let first = start_delayed_feed(0, packages.clone(), stop.clone(), global.clone(), sources[0].clone())?;
    let second = start_delayed_feed(1, packages.clone(), stop.clone(), global.clone(), sources[1].clone())?;
    write_request_budget_config(workspace, &packages, [first.0, second.0])?;
    Ok(Self {
      stop,
      workers: vec![first.1, second.1],
      global,
      sources,
      package_root: workspace.join(".packages"),
      packages,
      observed_global_peak: AtomicUsize::new(0),
      observed_source_peaks: [AtomicUsize::new(0), AtomicUsize::new(0)],
    })
  }

  fn reset_metrics(&self) {
    self.global.reset();
    for source in &self.sources {
      source.reset();
    }
  }

  fn validate_metrics(&self) -> Result<WorkEvidence> {
    let deadline = Instant::now() + Duration::from_secs(1);
    while self.global.active.load(Ordering::SeqCst) != 0 {
      if Instant::now() >= deadline {
        return Err("request-budget server did not become idle after the measured process exited".into());
      }
      thread::sleep(Duration::from_millis(1));
    }
    let global_peak = self.global.peak.load(Ordering::SeqCst);
    let source_peaks = self.sources.each_ref().map(|source| source.peak.load(Ordering::SeqCst));
    let requests = self.global.requests.load(Ordering::Relaxed);
    if requests == 0 {
      return Err("request-budget benchmark performed no HTTP requests".into());
    }
    if !(2..=4).contains(&global_peak) {
      return Err(format!("global request budget mismatch: observed {global_peak}, expected 2..=4").into());
    }
    for (index, peak) in source_peaks.into_iter().enumerate() {
      if !(1..=2).contains(&peak) {
        return Err(format!("source {} request budget mismatch: observed {peak}, expected 1..=2", index + 1).into());
      }
      self.observed_source_peaks[index].fetch_max(peak, Ordering::Relaxed);
    }
    self.observed_global_peak.fetch_max(global_peak, Ordering::Relaxed);
    let (downloaded_packages, downloaded_bytes) = validate_published_packages(&self.package_root, &self.packages)?;
    Ok(WorkEvidence {
      network_requests: Some(u64::try_from(requests)?),
      downloaded_bytes: Some(downloaded_bytes),
      downloaded_packages: Some(downloaded_packages),
      resolved_packages: Some(u64::try_from(self.packages.len())?),
    })
  }

  fn validate_saturation(&self) -> Result<()> {
    let global = self.observed_global_peak.load(Ordering::Relaxed);
    let sources = self.observed_source_peaks.each_ref().map(|peak| peak.load(Ordering::Relaxed));
    if global != 4 || sources != [2, 2] {
      return Err(format!("request budgets never reached their safe limits: global={global}, sources={sources:?}").into());
    }
    Ok(())
  }

  fn validate_reported_telemetry(&self, reported: Option<WorkEvidence>) -> Result<()> {
    let reported = reported.ok_or("dv telemetry benchmark emitted no package work evidence")?;
    let expected_requests = u64::try_from(self.global.requests.load(Ordering::Relaxed))?;
    let expected_bytes = u64::try_from(self.global.bytes.load(Ordering::Relaxed))?;
    if reported.network_requests != Some(expected_requests) || reported.downloaded_bytes != Some(expected_bytes) {
      return Err(
        format!(
          "dv source telemetry differs from server observation: reported requests={:?} bytes={:?}, observed requests={expected_requests} bytes={expected_bytes}",
          reported.network_requests, reported.downloaded_bytes
        )
        .into(),
      );
    }
    Ok(())
  }
}

fn validate_published_packages(root: &Path, packages: &[ServedPackage]) -> Result<(u64, u64)> {
  let (archive_count, archive_bytes) = package_archive_work(root)?;
  if archive_count != u64::try_from(packages.len())? {
    return Err(format!("request-budget restore published {archive_count} packages instead of {}", packages.len()).into());
  }
  for package in packages {
    let path = root
      .join(&package.id)
      .join(&package.version)
      .join(format!("{}.{}.nupkg", package.id, package.version));
    let published = fs::read(&path).map_err(|error| format!("read published request-budget package {}: {error}", path.display()))?;
    if published.as_slice() != package.archive.as_ref() {
      return Err(format!("request-budget package bytes differ for {} {}", package.id, package.version).into());
    }
  }
  Ok((archive_count, archive_bytes))
}

impl Drop for RequestBudgetFixture {
  fn drop(&mut self) {
    self.stop.store(true, Ordering::Release);
    for worker in self.workers.drain(..) {
      let _ = worker.join();
    }
  }
}

fn read_seed_packages(root: &Path) -> Result<Vec<ServedPackage>> {
  let mut packages = Vec::new();
  for id in fs::read_dir(root)? {
    let id = id?;
    if !id.file_type()?.is_dir() || id.file_name() == ".dv" {
      continue;
    }
    let lower_id = id.file_name().to_string_lossy().into_owned();
    for version in fs::read_dir(id.path())? {
      let version = version?;
      if !version.file_type()?.is_dir() {
        continue;
      }
      let normalized = version.file_name().to_string_lossy().into_owned();
      let archive_path = version.path().join(format!("{lower_id}.{normalized}.nupkg"));
      if archive_path.is_file() {
        packages.push(ServedPackage {
          id: lower_id.clone(),
          version: normalized,
          archive: Arc::from(fs::read(archive_path)?),
        });
      }
    }
  }
  packages.sort_unstable_by(|left, right| left.id.cmp(&right.id).then_with(|| left.version.cmp(&right.version)));
  Ok(packages)
}

fn start_delayed_feed(
  source_index: usize,
  packages: Arc<Vec<ServedPackage>>,
  stop: Arc<AtomicBool>,
  global: Arc<RequestMetrics>,
  source: Arc<RequestMetrics>,
) -> Result<(SocketAddr, JoinHandle<()>)> {
  let listener = TcpListener::bind("127.0.0.1:0")?;
  let address = listener.local_addr()?;
  listener.set_nonblocking(true)?;
  let worker = thread::spawn(move || {
    while !stop.load(Ordering::Acquire) {
      match listener.accept() {
        Ok((stream, _)) => {
          let packages = packages.clone();
          let global = global.clone();
          let source = source.clone();
          thread::spawn(move || serve_delayed_request(stream, address, source_index, &packages, &global, &source));
        },
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(1)),
        Err(_) => break,
      }
    }
  });
  Ok((address, worker))
}

fn serve_delayed_request(
  mut stream: TcpStream,
  address: SocketAddr,
  source_index: usize,
  packages: &[ServedPackage],
  global: &RequestMetrics,
  source: &RequestMetrics,
) {
  let _global_active = global.enter();
  let _source_active = source.enter();
  let mut request = [0u8; 8192];
  let Ok(read) = stream.read(&mut request) else {
    return;
  };
  let request = String::from_utf8_lossy(&request[..read]);
  let path = request
    .lines()
    .next()
    .and_then(|line| line.split_ascii_whitespace().nth(1))
    .and_then(|path| path.split('?').next())
    .unwrap_or("/");
  thread::sleep(Duration::from_millis(25));
  let response = feed_response(path, address, source_index, packages);
  global.bytes.fetch_add(response.1.len(), Ordering::Relaxed);
  source.bytes.fetch_add(response.1.len(), Ordering::Relaxed);
  let status = if response.0 { "200 OK" } else { "404 Not Found" };
  let content_type = if path.ends_with(".nupkg") {
    "application/octet-stream"
  } else {
    "application/json"
  };
  let header = format!(
    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
    response.1.len()
  );
  let _ = stream.write_all(header.as_bytes());
  let _ = stream.write_all(&response.1);
}

fn feed_response(path: &str, address: SocketAddr, source_index: usize, packages: &[ServedPackage]) -> (bool, Vec<u8>) {
  if path == "/index.json" {
    return (
      true,
      format!(
        r#"{{"version":"3.0.0","resources":[{{"@id":"http://{address}/flat/","@type":"PackageBaseAddress/3.0.0","comment":"delayed source {}"}}]}}"#,
        source_index + 1
      )
      .into_bytes(),
    );
  }
  let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
  if parts.len() == 3 && parts[0] == "flat" && parts[2] == "index.json" {
    let versions = packages
      .iter()
      .filter(|package| package.id == parts[1])
      .map(|package| format!("\"{}\"", package.version))
      .collect::<Vec<_>>();
    if !versions.is_empty() {
      return (true, format!(r#"{{"versions":[{}]}}"#, versions.join(",")).into_bytes());
    }
  }
  if parts.len() == 4
    && parts[0] == "flat"
    && parts[3].ends_with(".nupkg")
    && let Some(package) = packages.iter().find(|package| package.id == parts[1] && package.version == parts[2])
  {
    return (true, package.archive.to_vec());
  }
  (false, b"{}".to_vec())
}

fn write_request_budget_config(workspace: &Path, packages: &[ServedPackage], addresses: [SocketAddr; 2]) -> Result<()> {
  let mut mappings = [String::new(), String::new()];
  for (index, package) in packages.iter().enumerate() {
    writeln!(mappings[index % 2], "      <package pattern=\"{}\" />", package.id)?;
  }
  let config = format!(
    r#"<?xml version="1.0" encoding="utf-8"?>
<configuration>
  <packageSources>
    <clear />
    <add key="source-a" value="http://{}/index.json" protocolVersion="3" allowInsecureConnections="true" />
    <add key="source-b" value="http://{}/index.json" protocolVersion="3" allowInsecureConnections="true" />
  </packageSources>
  <packageSourceMapping>
    <packageSource key="source-a">
{}    </packageSource>
    <packageSource key="source-b">
{}    </packageSource>
  </packageSourceMapping>
  <config>
    <add key="maxHttpRequestsPerSource" value="2" />
  </config>
</configuration>
"#,
    addresses[0], addresses[1], mappings[0], mappings[1]
  );
  fs::write(workspace.join("NuGet.Config"), config)?;
  Ok(())
}

fn copy_with_parent(source: &Path, destination: &Path) -> Result<()> {
  let parent = destination
    .parent()
    .ok_or_else(|| format!("generated package path {} has no parent", destination.display()))?;
  fs::create_dir_all(parent)?;
  fs::copy(source, destination)?;
  Ok(())
}

fn reset_nuget_local_iteration(workspace: &Path) -> Result<()> {
  for relative in [".packages", "obj", "dv.lock.json"] {
    remove_generated_path(&workspace.join(relative))?;
  }
  Ok(())
}

fn reset_nuget_floating_iteration(workspace: &Path) -> Result<()> {
  for relative in [".packages", "obj", "dv.lock.json", "packages.lock.json"] {
    remove_generated_path(&workspace.join(relative))?;
  }
  Ok(())
}

fn reset_nuget_request_budget_iteration(workspace: &Path) -> Result<()> {
  for relative in [".packages", ".http-cache", "obj", "dv.lock.json", "packages.lock.json"] {
    remove_generated_path(&workspace.join(relative))?;
  }
  Ok(())
}

fn reset_package_conflict_iteration(workspace: &Path) -> Result<()> {
  for relative in ["obj", "dv.lock.json", "packages.lock.json"] {
    remove_generated_path(&workspace.join(relative))?;
  }
  Ok(())
}

fn reset_package_diagnostic_iteration(workspace: &Path) -> Result<()> {
  for relative in [".packages", "obj", "dv.lock.json", "packages.lock.json"] {
    remove_generated_path(&workspace.join(relative))?;
  }
  Ok(())
}

fn reset_package_batch_iteration(workspace: &Path) -> Result<()> {
  for relative in [
    ".packages",
    "obj",
    "BatchLeft/obj",
    "BatchLeft/dv.lock.json",
    "BatchLeft/packages.lock.json",
    "BatchRight/obj",
    "BatchRight/dv.lock.json",
    "BatchRight/packages.lock.json",
  ] {
    remove_generated_path(&workspace.join(relative))?;
  }
  Ok(())
}

fn reset_nuspec_framework_metadata_iteration(workspace: &Path) -> Result<()> {
  for relative in [".packages", "obj", "dv.lock.json", "packages.lock.json"] {
    remove_generated_path(&workspace.join(relative))?;
  }
  Ok(())
}

fn reset_package_rid_content_iteration(workspace: &Path) -> Result<()> {
  for relative in [".packages", "obj", "dv.lock.json", "packages.lock.json"] {
    remove_generated_path(&workspace.join(relative))?;
  }
  Ok(())
}

fn remove_generated_path(path: &Path) -> Result<()> {
  const RETRIES: [Duration; 4] = [
    Duration::from_millis(10),
    Duration::from_millis(50),
    Duration::from_millis(200),
    Duration::from_secs(1),
  ];
  for delay in RETRIES.into_iter().map(Some).chain([None]) {
    let result = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
    match result {
      Ok(()) => return Ok(()),
      Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
      Err(_) if delay.is_some() => thread::sleep(delay.expect("retry has a delay")),
      Err(error) => return Err(format!("remove generated benchmark path {}: {error}", path.display()).into()),
    }
  }
  unreachable!("the final generated-path removal attempt returns")
}

fn prepare_iteration(executable: &Path, case: &Case, fixture: &Path, workspace: &Path) -> Result<()> {
  match case.kind {
    CaseKind::CliGoldenTrace => {
      reset_fixture(fixture, workspace)?;
      fs::create_dir(workspace.join("offline-source"))?;
      Ok(())
    },
    CaseKind::CliCompatCheck
    | CaseKind::CliUnknownOption
    | CaseKind::CliOptionEffects
    | CaseKind::CliDriverInventory
    | CaseKind::CliCommandNormalization
    | CaseKind::CliTransformEquivalence
    | CaseKind::CliModeClassification
    | CaseKind::CliExitPolicy
    | CaseKind::CliLexicalPreservation
    | CaseKind::CliRoutePrecedence
    | CaseKind::CliEnvironment
    | CaseKind::CliForwarding
    | CaseKind::CliChildExit
    | CaseKind::RidGraph
    | CaseKind::ProjectEvaluate
    | CaseKind::PackageReferenceConditions
    | CaseKind::RuntimeEvaluate
    | CaseKind::RuntimePackPlan
    | CaseKind::FrameworkReferencePlan
    | CaseKind::CompilerPlan
    | CaseKind::PackageAssetPlan
    | CaseKind::PackageReferenceMetadata
    | CaseKind::PackagePruning
    | CaseKind::PackageSignatureWarm
    | CaseKind::PackageRidContentWarm
    | CaseKind::CentralPackageManagement
    | CaseKind::NugetConfigHierarchy
    | CaseKind::NugetConfigMerge
    | CaseKind::NugetSourceSections
    | CaseKind::NugetStoragePolicy
    | CaseKind::NugetCliOverrides => Ok(()),
    CaseKind::NuspecFrameworkMetadata => reset_nuspec_framework_metadata_iteration(workspace),
    CaseKind::PackageRidContentCold => reset_package_rid_content_iteration(workspace),
    CaseKind::NugetCredentials
    | CaseKind::NugetCredentialProvider
    | CaseKind::NugetClientCertificates
    | CaseKind::NugetHttpPolicy
    | CaseKind::NugetSourceSecurity => Ok(()),
    CaseKind::PackageConflictResolution => reset_package_conflict_iteration(workspace),
    CaseKind::PackageBatchResolution => reset_package_batch_iteration(workspace),
    CaseKind::PackageDiagnostics => reset_package_diagnostic_iteration(workspace),
    CaseKind::NugetLocalSources => reset_nuget_local_iteration(workspace),
    CaseKind::NugetFloatingVersion => reset_nuget_floating_iteration(workspace),
    CaseKind::NugetRequestBudget | CaseKind::NugetSourceTelemetry => reset_nuget_request_budget_iteration(workspace),
    CaseKind::NugetServiceIndex => reset_service_index_iteration(workspace),
    CaseKind::RuntimePackInventoryCold => reset_pack_inventory_cache(workspace),
    CaseKind::RestoreCold
    | CaseKind::PackageSyncCold
    | CaseKind::PackageSignatureCold
    | CaseKind::PackageGraphCold
    | CaseKind::PackageGraphMassive
    | CaseKind::PackDiagnostic
    | CaseKind::NugetSourceMapping => reset_fixture(fixture, workspace),
    CaseKind::BuildClean => {
      reset_fixture(fixture, workspace)?;
      run_checked(executable, restore_args(executable), workspace, "clean build restore")
    },
    CaseKind::Startup | CaseKind::PackageSyncWarm | CaseKind::BuildNoOp | CaseKind::RunWarm => Ok(()),
  }
}

fn reset_service_index_iteration(workspace: &Path) -> Result<()> {
  remove_generated_path(&workspace.join(".http-cache"))
}

fn reset_pack_inventory_cache(workspace: &Path) -> Result<()> {
  let cache = workspace.join(".packages/.dv/sdk-pack-inventories/v2");
  if cache.exists() {
    fs::remove_dir_all(cache)?;
  }
  Ok(())
}

fn build_args(executable: &Path) -> &'static [&'static str] {
  if is_dotnet(executable) {
    &["build", "--nologo", "--verbosity", "quiet"]
  } else {
    &["build"]
  }
}

fn restore_args(executable: &Path) -> &'static [&'static str] {
  if is_dotnet(executable) {
    &["restore", "--nologo", "--verbosity", "quiet"]
  } else {
    &["sync"]
  }
}

fn is_dotnet(executable: &Path) -> bool {
  executable
    .file_stem()
    .and_then(OsStr::to_str)
    .is_some_and(|name| name.eq_ignore_ascii_case("dotnet"))
}

fn case_cwd<'a>(case: &Case, fixture: &'a Path, workspace: &'a Path) -> &'a Path {
  if matches!(case.kind, CaseKind::Startup) { fixture } else { workspace }
}

fn case_fixture<'a>(case: &Case, fixtures: &Fixtures<'a>) -> &'a Path {
  match case.kind {
    CaseKind::CliForwarding | CaseKind::CliChildExit => fixtures.argument_forwarding,
    CaseKind::RidGraph => fixtures.rid_graph,
    CaseKind::RuntimeEvaluate => fixtures.runtime,
    CaseKind::RuntimePackPlan | CaseKind::RuntimePackInventoryCold => fixtures.runtime_pack,
    CaseKind::FrameworkReferencePlan => fixtures.framework_reference,
    CaseKind::PackDiagnostic => fixtures.unavailable_pack,
    CaseKind::PackageSyncCold | CaseKind::PackageSyncWarm => fixtures.package,
    CaseKind::PackageReferenceMetadata | CaseKind::PackagePruning => fixtures.package_reference_metadata,
    CaseKind::PackageSignatureCold | CaseKind::PackageSignatureWarm => fixtures.package_signature,
    CaseKind::NuspecFrameworkMetadata => fixtures.nuspec_framework_metadata,
    CaseKind::PackageRidContentCold | CaseKind::PackageRidContentWarm => fixtures.package_rid_content,
    CaseKind::CentralPackageManagement => fixtures.central_package_management,
    CaseKind::PackageConflictResolution | CaseKind::PackageBatchResolution | CaseKind::PackageDiagnostics => fixtures.package_conflict_resolution,
    CaseKind::PackageReferenceConditions => fixtures.package_reference_conditions,
    CaseKind::NugetConfigHierarchy => fixtures.nuget_config,
    CaseKind::NugetConfigMerge => fixtures.nuget_config_merge,
    CaseKind::NugetSourceSections => fixtures.nuget_source_sections,
    CaseKind::NugetSourceMapping => fixtures.nuget_source_mapping,
    CaseKind::NugetRequestBudget | CaseKind::NugetSourceTelemetry => fixtures.nuget_request_budget,
    CaseKind::NugetStoragePolicy => fixtures.nuget_storage_policy,
    CaseKind::NugetCliOverrides => fixtures.nuget_cli_overrides,
    CaseKind::NugetLocalSources => fixtures.nuget_local_sources,
    CaseKind::NugetFloatingVersion => fixtures.nuget_floating_version,
    CaseKind::NugetServiceIndex => fixtures.nuget_service_index,
    CaseKind::NugetCredentials => fixtures.nuget_credentials,
    CaseKind::NugetCredentialProvider => fixtures.nuget_credential_provider,
    CaseKind::NugetClientCertificates => fixtures.nuget_client_certificates,
    CaseKind::NugetHttpPolicy => fixtures.nuget_http_policy,
    CaseKind::NugetSourceSecurity => fixtures.nuget_source_security,
    CaseKind::PackageGraphCold => fixtures.package_graph,
    CaseKind::PackageGraphMassive | CaseKind::PackageAssetPlan => fixtures.package_graph_massive,
    _ => fixtures.small,
  }
}

fn fixture_name(case: &Case) -> Option<&'static str> {
  match case.kind {
    CaseKind::Startup => None,
    CaseKind::CliForwarding | CaseKind::CliChildExit => Some("argument-forwarding"),
    CaseKind::RidGraph => Some("rid-graph-oracle"),
    CaseKind::RuntimeEvaluate => Some("runtime-project"),
    CaseKind::RuntimePackPlan | CaseKind::RuntimePackInventoryCold => Some("runtime-pack-project"),
    CaseKind::FrameworkReferencePlan => Some("framework-reference-project"),
    CaseKind::PackDiagnostic => Some("unavailable-pack-project"),
    CaseKind::PackageSyncCold | CaseKind::PackageSyncWarm => Some("package-console"),
    CaseKind::PackageReferenceMetadata | CaseKind::PackagePruning => Some("package-reference-metadata"),
    CaseKind::PackageSignatureCold | CaseKind::PackageSignatureWarm => Some("package-signatures"),
    CaseKind::NuspecFrameworkMetadata => Some("nuspec-framework-metadata"),
    CaseKind::PackageRidContentCold | CaseKind::PackageRidContentWarm => Some("package-rid-content"),
    CaseKind::CentralPackageManagement => Some("central-package-management"),
    CaseKind::PackageConflictResolution | CaseKind::PackageBatchResolution | CaseKind::PackageDiagnostics => Some("package-conflict-resolution"),
    CaseKind::PackageReferenceConditions => Some("package-reference-conditions"),
    CaseKind::NugetConfigHierarchy => Some("nuget-config-hierarchy"),
    CaseKind::NugetConfigMerge => Some("nuget-config-merge"),
    CaseKind::NugetSourceSections => Some("nuget-source-sections"),
    CaseKind::NugetSourceMapping => Some("nuget-source-mapping"),
    CaseKind::NugetRequestBudget | CaseKind::NugetSourceTelemetry => Some("nuget-request-budget"),
    CaseKind::NugetStoragePolicy => Some("nuget-storage-policy"),
    CaseKind::NugetCliOverrides => Some("nuget-cli-overrides"),
    CaseKind::NugetLocalSources => Some("nuget-local-sources"),
    CaseKind::NugetFloatingVersion => Some("nuget-floating-version"),
    CaseKind::NugetServiceIndex => Some("nuget-service-index"),
    CaseKind::NugetCredentials => Some("nuget-credentials"),
    CaseKind::NugetCredentialProvider => Some("nuget-credential-provider"),
    CaseKind::NugetClientCertificates => Some("nuget-client-certificates"),
    CaseKind::NugetHttpPolicy => Some("nuget-http-policy"),
    CaseKind::NugetSourceSecurity => Some("nuget-source-security"),
    CaseKind::PackageGraphCold => Some("large-package-graph"),
    CaseKind::PackageGraphMassive | CaseKind::PackageAssetPlan => Some("massive-package-graph"),
    _ => Some("small-console"),
  }
}

fn measure(executable: &Path, case: &Case, cwd: &Path) -> Result<Measurement> {
  let started = Instant::now();
  let mut command = Command::new(executable);
  command.args(case.args).current_dir(cwd);
  if case.name == "cli_golden_trace" {
    apply_golden_trace_environment(&mut command, cwd);
  }
  if matches!(case.kind, CaseKind::CliEnvironment) {
    apply_invocation_environment(&mut command);
  }
  if matches!(
    case.kind,
    CaseKind::NugetConfigHierarchy
      | CaseKind::NugetConfigMerge
      | CaseKind::NugetSourceSections
      | CaseKind::NugetSourceMapping
      | CaseKind::NugetRequestBudget
      | CaseKind::NugetSourceTelemetry
      | CaseKind::NugetStoragePolicy
      | CaseKind::NugetCliOverrides
      | CaseKind::NugetLocalSources
      | CaseKind::NugetServiceIndex
      | CaseKind::NugetCredentials
      | CaseKind::NugetCredentialProvider
      | CaseKind::NugetClientCertificates
      | CaseKind::NugetHttpPolicy
      | CaseKind::NugetSourceSecurity
  ) {
    apply_case_nuget_environment(&mut command, case.kind, cwd)?;
  }
  let output = command.output()?;
  let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
  if case.name == "cli_protocol_version" {
    validate_protocol_version_output(&output, case.args)?;
  } else if case.name == "cli_compat_manifest" {
    validate_compatibility_manifest_output(&output)?;
  } else if matches!(case.kind, CaseKind::CliCompatCheck) {
    validate_compatibility_check_output(&output)?;
  } else if case.name == "cli_golden_trace" {
    validate_golden_trace_sample(&output, is_dotnet(executable))?;
  } else if case.name == "cli_compat_help" {
    validate_compatibility_help_output(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::PackDiagnostic) {
    validate_pack_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliUnknownOption) {
    validate_unknown_option_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliOptionEffects) {
    validate_option_effect_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliDriverInventory) {
    validate_driver_inventory_output(&output)?;
  } else if matches!(case.kind, CaseKind::CliCommandNormalization) {
    validate_command_normalization_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliTransformEquivalence) {
    validate_transform_equivalence_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliModeClassification) {
    validate_mode_classification_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliExitPolicy) {
    validate_exit_policy_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliLexicalPreservation) {
    validate_lexical_preservation_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliRoutePrecedence) {
    validate_route_precedence_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliEnvironment) {
    validate_environment_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliForwarding) {
    validate_forwarding_output(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::CliChildExit) {
    validate_child_exit_output(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::NugetSourceMapping) {
    validate_source_mapping_failure(&output, is_dotnet(executable))?;
  } else if matches!(case.kind, CaseKind::PackageDiagnostics) {
    validate_package_diagnostic(&output, is_dotnet(executable))?;
  } else {
    check_output(output.clone(), executable, case.args, "measured command")?;
  }
  if !is_dotnet(executable) && matches!(case.kind, CaseKind::RuntimePackPlan | CaseKind::RuntimePackInventoryCold) {
    validate_pack_inventory_cache(cwd)?;
  }
  if !is_dotnet(executable) && matches!(case.kind, CaseKind::NugetSourceTelemetry) {
    validate_source_telemetry(&output.stdout)?;
  }
  let work = if !is_dotnet(executable) && matches!(case.kind, CaseKind::PackageBatchResolution) {
    let evidence = parse_package_batch_work_evidence(&output.stdout)?;
    if evidence.network_requests != Some(0) || evidence.downloaded_packages != Some(8) || evidence.resolved_packages != Some(16) {
      return Err(format!("package-batch sample did not resolve two graphs through eight unique local archives: {evidence:?}").into());
    }
    Some(evidence)
  } else if matches!(case.kind, CaseKind::NugetSourceMapping) {
    Some(WorkEvidence {
      network_requests: Some(0),
      downloaded_bytes: Some(0),
      downloaded_packages: None,
      resolved_packages: None,
    })
  } else if !is_dotnet(executable)
    && matches!(
      case.kind,
      CaseKind::PackageSyncCold
        | CaseKind::NugetFloatingVersion
        | CaseKind::PackageGraphCold
        | CaseKind::PackageGraphMassive
        | CaseKind::PackageAssetPlan
        | CaseKind::PackageReferenceMetadata
        | CaseKind::PackagePruning
        | CaseKind::PackageSignatureCold
        | CaseKind::PackageSignatureWarm
        | CaseKind::NuspecFrameworkMetadata
        | CaseKind::PackageRidContentCold
        | CaseKind::PackageRidContentWarm
        | CaseKind::CentralPackageManagement
        | CaseKind::PackageConflictResolution
        | CaseKind::PackageSyncWarm
        | CaseKind::NugetConfigHierarchy
        | CaseKind::NugetConfigMerge
        | CaseKind::NugetSourceSections
        | CaseKind::NugetStoragePolicy
        | CaseKind::NugetCliOverrides
        | CaseKind::NugetLocalSources
        | CaseKind::NugetRequestBudget
        | CaseKind::NugetSourceTelemetry
    )
  {
    let evidence = parse_work_evidence(&output.stdout)?;
    if matches!(case.kind, CaseKind::NugetFloatingVersion)
      && (evidence.network_requests != Some(0) || evidence.downloaded_packages != Some(1) || evidence.resolved_packages != Some(1))
    {
      return Err(format!("floating-version sample did not perform one zero-network package acquisition: {evidence:?}").into());
    }
    if matches!(case.kind, CaseKind::PackageConflictResolution)
      && (evidence.network_requests != Some(0) || evidence.downloaded_packages != Some(0) || evidence.resolved_packages != Some(11))
    {
      return Err(format!("conflict-resolution sample did not resolve eleven cached packages with zero network work: {evidence:?}").into());
    }
    if matches!(case.kind, CaseKind::NuspecFrameworkMetadata)
      && (evidence.network_requests != Some(0) || evidence.downloaded_packages != Some(2) || evidence.resolved_packages != Some(2))
    {
      return Err(format!("nuspec-framework sample did not acquire two local packages with zero network work: {evidence:?}").into());
    }
    if matches!(case.kind, CaseKind::PackageRidContentCold)
      && (evidence.network_requests != Some(0) || evidence.downloaded_packages != Some(1) || evidence.resolved_packages != Some(1))
    {
      return Err(format!("cold RID/content sample did not acquire one local package with zero network work: {evidence:?}").into());
    }
    if matches!(case.kind, CaseKind::PackageRidContentWarm)
      && (evidence.network_requests != Some(0) || evidence.downloaded_packages != Some(0) || evidence.resolved_packages != Some(1))
    {
      return Err(format!("warm RID/content sample did not reuse one locked package with zero network work: {evidence:?}").into());
    }
    Some(evidence)
  } else if !is_dotnet(executable)
    && matches!(
      case.kind,
      CaseKind::NugetServiceIndex
        | CaseKind::NugetCredentials
        | CaseKind::NugetCredentialProvider
        | CaseKind::NugetClientCertificates
        | CaseKind::NugetHttpPolicy
        | CaseKind::NugetSourceSecurity
    )
  {
    Some(parse_source_work_evidence(&output.stdout)?)
  } else if is_dotnet(executable) && matches!(case.kind, CaseKind::PackageGraphMassive) {
    Some(reference_package_work(cwd)?)
  } else {
    None
  };
  Ok(Measurement { elapsed_ns: elapsed, work })
}

fn validate_pack_inventory_cache(workspace: &Path) -> Result<()> {
  let directory = workspace.join(".packages/.dv/sdk-pack-inventories/v2");
  let entries = fs::read_dir(&directory).map_err(|error| format!("dv did not publish its runtime-pack inventory under {}: {error}", directory.display()))?;
  let mut cache_files = 0usize;
  for entry in entries {
    let entry = entry?;
    if entry.path().extension() == Some(OsStr::new("bin")) {
      cache_files += 1;
    } else {
      return Err(format!("runtime-pack inventory cache retained unexpected entry {}", entry.path().display()).into());
    }
  }
  if cache_files != 1 {
    return Err(format!("runtime-pack inventory cache contains {cache_files} immutable entries instead of one").into());
  }
  Ok(())
}

fn parse_work_evidence(stdout: &[u8]) -> Result<WorkEvidence> {
  let text = std::str::from_utf8(stdout)?;
  let event = text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv restore did not emit package_resolution_created")?;
  Ok(WorkEvidence {
    network_requests: Some(
      event
        .get("network_requests")
        .and_then(serde_json::Value::as_u64)
        .ok_or("dv package event omitted network_requests")?,
    ),
    downloaded_bytes: Some(
      event
        .get("downloaded_bytes")
        .and_then(serde_json::Value::as_u64)
        .ok_or("dv package event omitted downloaded_bytes")?,
    ),
    downloaded_packages: Some(
      event
        .get("downloaded_packages")
        .and_then(serde_json::Value::as_u64)
        .ok_or("dv package event omitted downloaded_packages")?,
    ),
    resolved_packages: Some(
      event
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .map(u64::try_from)
        .transpose()?
        .ok_or("dv package event omitted packages")?,
    ),
  })
}

fn parse_package_batch_work_evidence(stdout: &[u8]) -> Result<WorkEvidence> {
  let mut network_requests = 0u64;
  let mut downloaded_bytes = 0u64;
  let mut downloaded_packages = 0u64;
  let mut resolved_packages = 0u64;
  let mut resolutions = 0usize;
  for event in std::str::from_utf8(stdout)?.lines().map(serde_json::from_str::<serde_json::Value>) {
    let event = event?;
    if event.get("type").and_then(serde_json::Value::as_str) != Some("package_resolution_created") {
      continue;
    }
    resolutions += 1;
    network_requests = network_requests
      .checked_add(
        event
          .get("network_requests")
          .and_then(serde_json::Value::as_u64)
          .ok_or("dv package batch omitted network_requests")?,
      )
      .ok_or("dv package batch request count overflow")?;
    downloaded_bytes = downloaded_bytes
      .checked_add(
        event
          .get("downloaded_bytes")
          .and_then(serde_json::Value::as_u64)
          .ok_or("dv package batch omitted downloaded_bytes")?,
      )
      .ok_or("dv package batch byte count overflow")?;
    downloaded_packages = downloaded_packages
      .checked_add(
        event
          .get("downloaded_packages")
          .and_then(serde_json::Value::as_u64)
          .ok_or("dv package batch omitted downloaded_packages")?,
      )
      .ok_or("dv package batch download count overflow")?;
    resolved_packages = resolved_packages
      .checked_add(
        event
          .get("packages")
          .and_then(serde_json::Value::as_array)
          .map_or(0, |packages| packages.len() as u64),
      )
      .ok_or("dv package batch resolved count overflow")?;
  }
  if resolutions != 3 {
    return Err(format!("dv package batch emitted {resolutions} resolution events instead of three").into());
  }
  Ok(WorkEvidence {
    network_requests: Some(network_requests),
    downloaded_bytes: Some(downloaded_bytes),
    downloaded_packages: Some(downloaded_packages),
    resolved_packages: Some(resolved_packages),
  })
}

fn validate_source_telemetry(stdout: &[u8]) -> Result<()> {
  let text = std::str::from_utf8(stdout)?;
  if text.contains("benchmark-secret") || text.contains("http://127.0.0.1") {
    return Err("dv source telemetry exposed a source location or query credential".into());
  }
  let event = text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_resolution_created"))
    .ok_or("dv telemetry restore did not emit package_resolution_created")?;
  let sources = event
    .get("source_work")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv package event omitted source_work")?;
  if sources.len() != 2 {
    return Err(format!("dv source telemetry reported {} sources instead of two", sources.len()).into());
  }
  let mut requests = 0u64;
  let mut bytes = 0u64;
  for (index, (source, expected_name)) in sources.iter().zip(["source-a", "source-b"]).enumerate() {
    if source.get("name").and_then(serde_json::Value::as_str) != Some(expected_name) || source.get("protocol").and_then(serde_json::Value::as_str) != Some("v3")
    {
      return Err(format!("dv source telemetry row {index} lost configured source identity or order").into());
    }
    let source_requests = source
      .get("requests")
      .and_then(serde_json::Value::as_u64)
      .ok_or("dv source telemetry omitted requests")?;
    let source_bytes = source
      .get("downloaded_bytes")
      .and_then(serde_json::Value::as_u64)
      .ok_or("dv source telemetry omitted downloaded_bytes")?;
    let duration = source
      .get("duration_us")
      .and_then(serde_json::Value::as_u64)
      .ok_or("dv source telemetry omitted duration_us")?;
    if source_requests == 0 || source_bytes == 0 || duration == 0 {
      return Err(format!("dv source telemetry row {index} did not record its cold network work").into());
    }
    requests = requests.checked_add(source_requests).ok_or("source request sum overflowed u64")?;
    bytes = bytes.checked_add(source_bytes).ok_or("source byte sum overflowed u64")?;
  }
  if event.get("network_requests").and_then(serde_json::Value::as_u64) != Some(requests)
    || event.get("downloaded_bytes").and_then(serde_json::Value::as_u64) != Some(bytes)
  {
    return Err("dv aggregate package work differs from its source-work batch".into());
  }
  let packages = event
    .get("packages")
    .and_then(serde_json::Value::as_array)
    .ok_or("dv package event omitted packages")?;
  if packages.len() != 6
    || packages
      .iter()
      .any(|package| package.get("cache_outcome").and_then(serde_json::Value::as_str) != Some("miss"))
  {
    return Err("dv cold telemetry restore did not classify every resolved package as a cache miss".into());
  }
  Ok(())
}

fn parse_source_work_evidence(stdout: &[u8]) -> Result<WorkEvidence> {
  let text = std::str::from_utf8(stdout)?;
  let event = text
    .lines()
    .map(serde_json::from_str::<serde_json::Value>)
    .collect::<std::result::Result<Vec<_>, _>>()?
    .into_iter()
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_sources_inspected"))
    .ok_or("dv source inspection did not emit package_sources_inspected")?;
  Ok(WorkEvidence {
    network_requests: event.get("network_requests").and_then(serde_json::Value::as_u64),
    downloaded_bytes: event.get("downloaded_bytes").and_then(serde_json::Value::as_u64),
    downloaded_packages: None,
    resolved_packages: None,
  })
}

fn reference_package_work(cwd: &Path) -> Result<WorkEvidence> {
  let assets: serde_json::Value = serde_json::from_slice(&fs::read(cwd.join("obj/project.assets.json"))?)?;
  let package_count = u64::try_from(
    assets
      .get("libraries")
      .and_then(serde_json::Value::as_object)
      .ok_or("dotnet project.assets.json omitted libraries")?
      .len(),
  )?;
  let (archive_count, downloaded_bytes) = package_archive_work(&cwd.join(".packages"))?;
  Ok(WorkEvidence {
    network_requests: None,
    downloaded_bytes: Some(downloaded_bytes),
    downloaded_packages: Some(archive_count),
    resolved_packages: Some(package_count),
  })
}

fn package_archive_work(root: &Path) -> Result<(u64, u64)> {
  let mut archive_count = 0u64;
  let mut downloaded_bytes = 0u64;
  let mut directories = vec![root.to_path_buf()];
  while let Some(directory) = directories.pop() {
    for entry in fs::read_dir(&directory)? {
      let entry = entry?;
      let file_type = entry.file_type()?;
      if file_type.is_dir() {
        directories.push(entry.path());
      } else if file_type.is_file() && entry.path().extension().is_some_and(|extension| extension.eq_ignore_ascii_case("nupkg")) {
        archive_count = archive_count.checked_add(1).ok_or("dotnet package archive count overflowed u64")?;
        downloaded_bytes = downloaded_bytes
          .checked_add(entry.metadata()?.len())
          .ok_or("dotnet package payload bytes overflowed u64")?;
      }
    }
  }
  Ok((archive_count, downloaded_bytes))
}

fn merge_work_evidence(current: &mut Option<WorkEvidence>, observed: Option<WorkEvidence>, tool: &str, case: &str) -> Result<()> {
  let Some(observed) = observed else {
    return Ok(());
  };
  if let Some(previous) = current {
    if previous.resolved_packages != observed.resolved_packages || previous.downloaded_packages != observed.downloaded_packages {
      return Err(format!("{tool} {case} reported inconsistent package counts across retained samples: previous={previous:?} observed={observed:?}").into());
    }
    previous.network_requests = max_optional(previous.network_requests, observed.network_requests);
    previous.downloaded_bytes = max_optional(previous.downloaded_bytes, observed.downloaded_bytes);
  } else {
    *current = Some(observed);
  }
  Ok(())
}

fn max_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
  match (left, right) {
    (Some(left), Some(right)) => Some(left.max(right)),
    _ => None,
  }
}

fn run_checked(executable: &Path, args: &[&str], cwd: &Path, purpose: &str) -> Result<()> {
  let output = Command::new(executable).args(args).current_dir(cwd).output()?;
  check_output(output, executable, args, purpose)
}

fn apply_nuget_config_environment(command: &mut Command, cwd: &Path) {
  command
    .env("PROGRAMFILES(X86)", cwd.join("scopes/machine"))
    .env("PROGRAMFILES", cwd.join("scopes/machine"))
    .env("APPDATA", cwd.join("scopes/user"))
    .env("DV_CONFIG_SOURCE", "https://api.nuget.org/v3/index.json")
    .env("DV_CONFIG_PACKAGES", cwd.join(".packages"))
    .env_remove("NUGET_PACKAGES");
}

fn apply_nuget_storage_environment(command: &mut Command, cwd: &Path) {
  apply_nuget_config_environment(command, cwd);
  command
    .env("NUGET_PACKAGES", cwd.join("policy/env-global"))
    .env("NUGET_HTTP_CACHE_PATH", cwd.join("policy/http-cache"))
    .env("NUGET_SCRATCH", cwd.join("policy/scratch"))
    .env_remove("NUGET_FALLBACK_PACKAGES")
    .env_remove("http_proxy")
    .env_remove("no_proxy");
}

fn apply_nuget_cli_environment(command: &mut Command, cwd: &Path) {
  apply_nuget_config_environment(command, cwd);
  command
    .env("NUGET_PACKAGES", cwd.join("policy/env-global"))
    .env("NUGET_HTTP_CACHE_PATH", cwd.join("policy/http-cache"))
    .env("NUGET_SCRATCH", cwd.join("policy/scratch"))
    .env_remove("NUGET_FALLBACK_PACKAGES")
    .env_remove("http_proxy")
    .env_remove("no_proxy");
}

fn apply_nuget_credential_environment(command: &mut Command, cwd: &Path) {
  apply_nuget_config_environment(command, cwd);
  command
    .env(
      "NuGetPackageSourceCredentials_private",
      "Username=environment-user;Password=environment-pat;ValidAuthenticationTypes=Basic",
    )
    .env("DV_BENCH_CONFIG_PAT", "config-only-pat");
}

fn apply_nuget_credential_provider_environment(command: &mut Command, cwd: &Path) {
  apply_nuget_config_environment(command, cwd);
  let executable = repository_root()
    .join("target/release")
    .join(format!("nuget-plugin-dv-fixture{}", env::consts::EXE_SUFFIX));
  command
    .env_remove("NUGET_NETCORE_PLUGIN_PATHS")
    .env("NUGET_PLUGIN_PATHS", executable)
    .env("NUGET_PLUGINS_CACHE_PATH", cwd.join(".plugin-cache"))
    .env("NUGET_PLUGIN_HANDSHAKE_TIMEOUT_IN_SECONDS", "10")
    .env("NUGET_PLUGIN_REQUEST_TIMEOUT_IN_SECONDS", "10")
    .env("DV_TEST_PROVIDER_USERNAME", "provider-benchmark-user")
    .env("DV_TEST_PROVIDER_PASSWORD", "provider-benchmark-secret")
    .env_remove("DV_TEST_PROVIDER_MODE")
    .env_remove("DV_TEST_PROVIDER_LOG")
    .env_remove("DV_TEST_PROVIDER_TRACE");
}

fn apply_nuget_client_certificate_environment(command: &mut Command, cwd: &Path) -> Result<()> {
  apply_nuget_config_environment(command, cwd);
  let metadata: serde_json::Value = serde_json::from_slice(&fs::read(cwd.join("certs/metadata.json"))?)?;
  let thumbprint = metadata
    .get("client")
    .and_then(serde_json::Value::as_str)
    .ok_or("client-certificate fixture metadata omitted client thumbprint")?;
  command
    .env("DV_CERT_THUMBPRINT", thumbprint)
    .env("NUGET_HTTP_CACHE_PATH", cwd.join(".http-cache"));
  Ok(())
}

fn apply_nuget_http_policy_environment(command: &mut Command, cwd: &Path) {
  apply_nuget_config_environment(command, cwd);
  command
    .env("NUGET_HTTP_CACHE_PATH", cwd.join(".http-cache"))
    .env("NUGET_ENHANCED_MAX_NETWORK_TRY_COUNT", "9")
    .env("NUGET_ENHANCED_NETWORK_RETRY_DELAY_MILLISECONDS", "250")
    .env("NUGET_MAX_RETRY_AFTER_DELAY_SECONDS", "12")
    .env("NUGET_RETRY_HTTP_429", "false")
    .env("NUGET_OBSERVE_RETRY_AFTER", "false")
    .env_remove("http_proxy")
    .env_remove("HTTP_PROXY")
    .env_remove("no_proxy")
    .env_remove("NO_PROXY");
}

fn apply_case_nuget_environment(command: &mut Command, kind: CaseKind, cwd: &Path) -> Result<()> {
  if matches!(kind, CaseKind::NugetStoragePolicy) {
    apply_nuget_storage_environment(command, cwd);
  } else if matches!(kind, CaseKind::NugetCliOverrides) {
    apply_nuget_cli_environment(command, cwd);
  } else if matches!(kind, CaseKind::NugetServiceIndex) {
    apply_nuget_config_environment(command, cwd);
    command.env("NUGET_HTTP_CACHE_PATH", cwd.join(".http-cache"));
  } else if matches!(kind, CaseKind::NugetCredentials) {
    apply_nuget_credential_environment(command, cwd);
  } else if matches!(kind, CaseKind::NugetCredentialProvider) {
    apply_nuget_credential_provider_environment(command, cwd);
  } else if matches!(kind, CaseKind::NugetClientCertificates) {
    apply_nuget_client_certificate_environment(command, cwd)?;
  } else if matches!(kind, CaseKind::NugetHttpPolicy) {
    apply_nuget_http_policy_environment(command, cwd);
  } else if matches!(kind, CaseKind::NugetRequestBudget | CaseKind::NugetSourceTelemetry) {
    apply_nuget_config_environment(command, cwd);
    command
      .env("NUGET_CONCURRENCY_LIMIT", "4")
      .env("NUGET_HTTP_CACHE_PATH", cwd.join(".http-cache"))
      .env_remove("http_proxy")
      .env_remove("HTTP_PROXY")
      .env_remove("https_proxy")
      .env_remove("HTTPS_PROXY")
      .env_remove("no_proxy")
      .env_remove("NO_PROXY");
  } else {
    apply_nuget_config_environment(command, cwd);
  }
  Ok(())
}

fn run_nuget_config_checked(executable: &Path, args: &[&str], cwd: &Path, purpose: &str) -> Result<()> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_config_environment(&mut command, cwd);
  check_output(command.output()?, executable, args, purpose)
}

fn run_nuget_storage_checked(executable: &Path, args: &[&str], cwd: &Path, purpose: &str) -> Result<()> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_storage_environment(&mut command, cwd);
  check_output(command.output()?, executable, args, purpose)
}

fn run_nuget_cli_checked(executable: &Path, args: &[&str], cwd: &Path, purpose: &str) -> Result<()> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_cli_environment(&mut command, cwd);
  check_output(command.output()?, executable, args, purpose)
}

fn check_output(output: Output, executable: &Path, args: &[&str], purpose: &str) -> Result<()> {
  if output.status.success() {
    return Ok(());
  }

  Err(
    format!(
      "{purpose} failed: {} {}\nstdout:\n{}\nstderr:\n{}",
      executable.display(),
      args.join(" "),
      String::from_utf8_lossy(&output.stdout),
      String::from_utf8_lossy(&output.stderr)
    )
    .into(),
  )
}

fn command_text(executable: &Path, args: &[&str], cwd: &Path) -> Result<String> {
  let output = Command::new(executable).args(args).current_dir(cwd).output()?;
  check_output(output.clone(), executable, args, "metadata command")?;
  Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn nuget_config_command_text(executable: &Path, args: &[&str], cwd: &Path) -> Result<String> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_config_environment(&mut command, cwd);
  let output = command.output()?;
  check_output(output.clone(), executable, args, "NuGet configuration command")?;
  Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn service_index_command_text(executable: &Path, args: &[&str], cwd: &Path) -> Result<String> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_case_nuget_environment(&mut command, CaseKind::NugetServiceIndex, cwd)?;
  let output = command.output()?;
  check_output(output.clone(), executable, args, "NuGet service-index command")?;
  Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn credential_command_text(executable: &Path, args: &[&str], cwd: &Path) -> Result<String> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_credential_environment(&mut command, cwd);
  let output = command.output()?;
  check_output(output.clone(), executable, args, "NuGet credential command")?;
  let stdout = String::from_utf8(output.stdout)?;
  let stderr = String::from_utf8(output.stderr)?;
  reject_credential_output("NuGet credential stdout", &stdout)?;
  reject_credential_output("NuGet credential stderr", &stderr)?;
  Ok(stdout.trim().to_owned())
}

fn credential_provider_command_text(executable: &Path, args: &[&str], cwd: &Path, trace: Option<&Path>) -> Result<String> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_credential_provider_environment(&mut command, cwd);
  if let Some(trace) = trace {
    remove_generated_path(trace)?;
    command.env("DV_TEST_PROVIDER_TRACE", trace);
  }
  let output = command.output()?;
  check_output(output.clone(), executable, args, "NuGet credential-provider command")?;
  let stdout = String::from_utf8(output.stdout)?;
  let stderr = String::from_utf8(output.stderr)?;
  reject_credential_output("NuGet credential-provider stdout", &stdout)?;
  reject_credential_output("NuGet credential-provider stderr", &stderr)?;
  Ok(stdout.trim().to_owned())
}

fn client_certificate_command_text(executable: &Path, args: &[&str], cwd: &Path) -> Result<String> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_client_certificate_environment(&mut command, cwd)?;
  let output = command.output()?;
  check_output(output.clone(), executable, args, "NuGet client-certificate command")?;
  let stdout = String::from_utf8(output.stdout)?;
  let stderr = String::from_utf8(output.stderr)?;
  reject_credential_output("NuGet client-certificate stdout", &stdout)?;
  reject_credential_output("NuGet client-certificate stderr", &stderr)?;
  Ok(stdout.trim().to_owned())
}

fn nuget_storage_command_text(executable: &Path, args: &[&str], cwd: &Path) -> Result<String> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_storage_environment(&mut command, cwd);
  let output = command.output()?;
  check_output(output.clone(), executable, args, "NuGet storage-policy command")?;
  Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn nuget_cli_command_text(executable: &Path, args: &[&str], cwd: &Path) -> Result<String> {
  let mut command = Command::new(executable);
  command.args(args).current_dir(cwd);
  apply_nuget_cli_environment(&mut command, cwd);
  let output = command.output()?;
  check_output(output.clone(), executable, args, "NuGet CLI override command")?;
  Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn reset_fixture(source: &Path, destination: &Path) -> Result<()> {
  remove_generated_path(destination)?;
  copy_directory(source, destination)
}

type TreeSnapshot = Vec<(PathBuf, Option<Vec<u8>>)>;

fn snapshot_tree(root: &Path) -> Result<TreeSnapshot> {
  fn visit(root: &Path, directory: &Path, entries: &mut TreeSnapshot) -> Result<()> {
    for entry in fs::read_dir(directory)? {
      let entry = entry?;
      let path = entry.path();
      let relative = path.strip_prefix(root)?.to_owned();
      if entry.file_type()?.is_dir() {
        entries.push((relative, None));
        visit(root, &path, entries)?;
      } else {
        entries.push((relative, Some(fs::read(path)?)));
      }
    }
    Ok(())
  }

  let mut entries = Vec::new();
  visit(root, root, &mut entries)?;
  entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
  Ok(entries)
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
  fs::create_dir_all(destination)?;
  let storage_policy_root = source.join("StoragePolicy.csproj").is_file();
  let generated_policy_root = storage_policy_root || source.join("CliOverrides.csproj").is_file();
  let local_sources_root = source.join("LocalSources.csproj").is_file();
  for entry in fs::read_dir(source)? {
    let entry = entry?;
    let source_path = entry.path();
    let destination_path = destination.join(entry.file_name());
    if entry.file_type()?.is_dir() {
      if matches!(
        entry.file_name().to_str(),
        Some("obj" | "bin" | ".packages" | ".dv-packages" | ".oracle-packages" | ".seed" | ".seed-project")
      ) || (generated_policy_root && entry.file_name() == OsStr::new("policy"))
        || (local_sources_root && entry.file_name() == OsStr::new("feeds"))
      {
        continue;
      }
      copy_directory(&source_path, &destination_path)?;
    } else {
      if matches!(entry.file_name().to_str(), Some("dv.lock.json" | "packages.lock.json")) {
        continue;
      }
      fs::copy(source_path, destination_path)?;
    }
  }
  Ok(())
}

fn statistics(samples: &[u64]) -> Statistics {
  assert!(!samples.is_empty());
  let mut sorted = samples.to_vec();
  sorted.sort_unstable();
  let p95_index = (sorted.len() * 95).div_ceil(100).saturating_sub(1);

  Statistics {
    min: sorted[0],
    median: sorted[sorted.len() / 2],
    p95: sorted[p95_index],
    max: sorted[sorted.len() - 1],
  }
}

fn print_summary(report: &Report) {
  print!("{}", render_summary(report, io::stdout().is_terminal()));
}

fn render_summary(report: &Report, color: bool) -> String {
  let tool_width = report.runs.iter().map(|run| run.tool.len()).max().unwrap_or(4).max(4);
  let case_width = report.runs.iter().map(|run| case_label(&run.case).len()).max().unwrap_or(9).max(9);
  let metric_width = report
    .runs
    .iter()
    .filter_map(|run| run.statistics_ns.as_ref())
    .flat_map(|statistics| [statistics.median, statistics.p95, statistics.min, statistics.max])
    .map(format_milliseconds)
    .map(|value| value.len())
    .max()
    .unwrap_or(6)
    .max(6);
  let widths = [tool_width, case_width, metric_width, metric_width, metric_width, metric_width];
  let measured_run = report.runs.iter().find(|run| run.statistics_ns.is_some());
  let sample_count = measured_run.map(|run| run.samples_ns.len()).unwrap_or(0);
  let warmup_count = measured_run.map(|run| run.warmups).unwrap_or(0);
  let sample_label = if sample_count == 1 { "sample" } else { "samples" };
  let warmup_label = if warmup_count == 1 { "warm-up" } else { "warm-ups" };
  let mut output = String::new();

  output.push('\n');
  if color {
    output.push_str("\x1b[1;36m");
  }
  output.push_str("  dv benchmark results");
  if color {
    output.push_str("\x1b[0m");
  }
  output.push('\n');
  writeln!(
    output,
    "  {} {}  •  {} logical CPUs  •  {} {} + {} {}",
    report.environment.os, report.environment.arch, report.environment.logical_cpus, sample_count, sample_label, warmup_count, warmup_label
  )
  .expect("writing a String succeeds");
  output.push('\n');

  write_border(&mut output, '╭', '┬', '╮', &widths);
  write_row(&mut output, &widths, ["TOOL", "BENCHMARK", "MEDIAN", "P95", "MIN", "MAX"], color);
  write_border(&mut output, '├', '┼', '┤', &widths);

  for run in &report.runs {
    let metrics = match &run.statistics_ns {
      Some(statistics) => [
        format_milliseconds(statistics.median),
        format_milliseconds(statistics.p95),
        format_milliseconds(statistics.min),
        format_milliseconds(statistics.max),
      ],
      None => ["TBI".into(), "—".into(), "—".into(), "—".into()],
    };
    let values = [
      run.tool.clone(),
      case_label(&run.case).to_owned(),
      metrics[0].clone(),
      metrics[1].clone(),
      metrics[2].clone(),
      metrics[3].clone(),
    ];
    write_row(&mut output, &widths, values.each_ref().map(String::as_str), false);
  }

  write_border(&mut output, '╰', '┴', '╯', &widths);

  let package_runs = report
    .runs
    .iter()
    .filter(|run| {
      matches!(
        run.case.as_str(),
        "package_sync_cold"
          | "package_graph_cold"
          | "package_graph_massive"
          | "package_asset_plan"
          | "package_reference_metadata"
          | "package_pruning"
          | "package_signature_cold"
          | "package_signature_warm"
          | "package_rid_content_cold"
          | "package_rid_content_warm"
          | "package_sync_warm"
          | "nuget_config_hierarchy"
          | "nuget_config_merge"
          | "nuget_source_sections"
          | "nuget_source_mapping"
          | "nuget_request_budget"
          | "nuget_source_telemetry"
          | "nuget_storage_policy"
          | "nuget_cli_overrides"
          | "nuget_local_sources"
          | "nuget_service_index"
          | "nuget_credentials"
          | "nuget_credential_provider"
          | "nuget_client_certificates"
          | "nuget_http_policy"
          | "nuget_source_security"
      )
    })
    .collect::<Vec<_>>();
  if !package_runs.is_empty() {
    output.push('\n');
    if color {
      output.push_str("\x1b[1m");
    }
    output.push_str("  Observed work");
    if color {
      output.push_str("\x1b[0m");
    }
    output.push('\n');
    let evidence_label_width = package_runs
      .iter()
      .map(|run| run.tool.len() + case_label(&run.case).len() + 3)
      .max()
      .unwrap_or(0);
    for run in package_runs {
      let label = format!("{} · {}", run.tool, case_label(&run.case));
      let evidence = match (
        run.status,
        run.resolved_packages,
        run.downloaded_packages,
        run.network_requests,
        run.downloaded_bytes,
      ) {
        (RunStatus::Tbi, _, _, _, _) => "TBI".to_owned(),
        (_, Some(resolved), Some(downloaded), Some(requests), Some(bytes)) => {
          let package_label = if resolved == 1 { "package" } else { "packages" };
          format!(
            "{} resolved {package_label} · {} downloaded · max {requests} HTTP requests · max {} payload bytes",
            format_integer(resolved),
            format_integer(downloaded),
            format_integer(bytes)
          )
        },
        (_, Some(resolved), Some(downloaded), None, Some(bytes)) => {
          let package_label = if resolved == 1 { "package" } else { "packages" };
          format!(
            "{} resolved {package_label} · {} downloaded · HTTP requests not exposed · max {} payload bytes",
            format_integer(resolved),
            format_integer(downloaded),
            format_integer(bytes)
          )
        },
        (_, None, None, Some(requests), Some(bytes)) => {
          let request_label = if requests == 1 { "request" } else { "requests" };
          format!("max {requests} HTTP {request_label} · max {} response bytes", format_integer(bytes))
        },
        _ => "not exposed by command".to_owned(),
      };
      writeln!(output, "  {label:<evidence_label_width$}  {evidence}").expect("writing a String succeeds");
    }
  }

  output.push('\n');
  if color {
    output.push_str("\x1b[1m");
  }
  output.push_str("  Commands");
  if color {
    output.push_str("\x1b[0m");
  }
  output.push('\n');

  let command_label_width = report
    .runs
    .iter()
    .map(|run| run.tool.len() + case_label(&run.case).len() + 3)
    .max()
    .unwrap_or(0);
  for run in &report.runs {
    let label = format!("{} · {}", run.tool, case_label(&run.case));
    write!(output, "  {:<command_label_width$}  ", label).expect("writing a String succeeds");
    write_command(&mut output, &run.command);
    output.push('\n');
  }
  output
}

fn write_border(output: &mut String, left: char, junction: char, right: char, widths: &[usize; 6]) {
  output.push_str("  ");
  output.push(left);
  for (index, width) in widths.iter().enumerate() {
    output.extend(std::iter::repeat_n('─', width + 2));
    output.push(if index + 1 == widths.len() { right } else { junction });
  }
  output.push('\n');
}

fn write_row(output: &mut String, widths: &[usize; 6], values: [&str; 6], bold: bool) {
  output.push_str("  │");
  if bold {
    output.push_str("\x1b[1m");
  }
  for (index, value) in values.iter().enumerate() {
    if index >= 2 {
      write!(output, " {:>width$} │", value, width = widths[index]).expect("writing a String succeeds");
    } else {
      write!(output, " {:<width$} │", value, width = widths[index]).expect("writing a String succeeds");
    }
  }
  if bold {
    output.push_str("\x1b[0m");
  }
  output.push('\n');
}

fn case_label(case: &str) -> &str {
  match case {
    "sdk_current" => "SDK selection",
    "sdk_current_globals" => "SDK selection + globals",
    "sdk_current_compat" => "SDK selection + compatibility",
    "cli_golden_trace" => "Golden CI substitution trace",
    "cli_compat_help" => "Compatibility help",
    "cli_unknown_option" => "Unknown-option rejection",
    "cli_option_effects" => "Child option effect boundary",
    "dotnet_runtime_inventory" => "Installed runtime inventory",
    "cli_command_normalization" => "Command spelling normalization",
    "cli_transform_equivalence" => "Typed transform equivalence",
    "cli_mode_classification" => "Invocation mode classification",
    "cli_exit_policy" => "Restore failure exit policy",
    "cli_lexical_preservation" => "Lexical argument preservation",
    "cli_route_precedence" => "Ambiguous command routing",
    "cli_environment" => "Environment precedence + redaction",
    "cli_forwarding" => "Forwarded argument capture (run TBI)",
    "cli_child_exit" => "Child exit policy (run TBI)",
    "cli_cancellation" => "Cancellation-ready SDK selection",
    "cli_version" => "CLI self-version",
    "cli_protocol_version" => "Syntax + JSON protocol versions",
    "cli_compat_manifest" => "Compatibility manifest query",
    "cli_compat_check" => "Static compatibility check",
    "project_evaluate" => "Project evaluation",
    "project_select_named" => "Named project selection",
    "package_reference_conditions" => "Conditional references",
    "runtime_pack_plan" => "Runtime pack plan",
    "runtime_pack_inventory_cold" => "Cold runtime pack inventory",
    "framework_reference_plan" => "Framework reference plan",
    "pack_diagnostic" => "Unavailable pack diagnostic",
    "compiler_plan" => "Compiler input plan",
    "restore_cold" => "Cold restore",
    "sync_cold" => "Cold sync",
    "package_sync_cold" => "Cold dependency readiness",
    "package_graph_cold" => "Cold large dependency graph",
    "package_graph_massive" => "Cold massive solution graph",
    "package_asset_plan" => "Warm package asset plan",
    "package_reference_metadata" => "PackageReference metadata",
    "package_pruning" => "Legacy package pruning",
    "package_signature_cold" => "Cold package signature validation",
    "package_signature_warm" => "Warm package signature validation",
    "nuspec_framework_metadata" => "NuGet framework metadata",
    "package_rid_content_cold" => "Cold RID/content asset plan",
    "package_rid_content_warm" => "Warm RID/content asset plan",
    "central_package_management" => "Central package management",
    "package_conflict_resolution" => "Package conflict resolution",
    "package_batch_resolution" => "Two-project package batch",
    "package_diagnostics" => "Package constraint diagnostic",
    "package_sync_warm" => "Warm locked restore",
    "nuget_config_hierarchy" => "NuGet.Config hierarchy",
    "nuget_config_merge" => "NuGet.Config keyed merge",
    "nuget_source_sections" => "NuGet source sections",
    "nuget_source_mapping" => "NuGet source mapping",
    "nuget_request_budget" => "NuGet request budget",
    "nuget_source_telemetry" => "NuGet source telemetry",
    "nuget_storage_policy" => "NuGet storage policy",
    "nuget_cli_overrides" => "NuGet CLI overrides",
    "nuget_local_sources" => "NuGet local sources",
    "nuget_service_index" => "NuGet service index",
    "nuget_credentials" => "NuGet credentials",
    "nuget_credential_provider" => "NuGet credential provider",
    "nuget_client_certificates" => "NuGet client certificates",
    "nuget_http_policy" => "NuGet HTTP policy",
    "nuget_source_security" => "NuGet source security",
    "build_clean" => "Clean build",
    "build_noop" => "No-op build",
    "run_warm" => "Warm run",
    other => other,
  }
}

fn write_command(output: &mut String, command: &[String]) {
  for (index, argument) in command.iter().enumerate() {
    if index > 0 {
      output.push(' ');
    }

    if argument.is_empty() || argument.chars().any(|character| character.is_whitespace() || matches!(character, '"' | ';')) {
      output.push('"');
      for character in argument.chars() {
        if character == '"' {
          output.push('\\');
        }
        output.push(character);
      }
      output.push('"');
    } else {
      output.push_str(argument);
    }
  }
}

fn format_milliseconds(nanoseconds: u64) -> String {
  format!("{:.3} ms", millis(nanoseconds))
}

fn format_integer(value: u64) -> String {
  let digits = value.to_string();
  let first_group = digits.len() % 3;
  let mut output = String::with_capacity(digits.len() + digits.len() / 3);
  for (index, digit) in digits.char_indices() {
    if index > 0 && index % 3 == first_group {
      output.push(',');
    }
    output.push(digit);
  }
  output
}

fn millis(nanoseconds: u64) -> f64 {
  nanoseconds as f64 / 1_000_000.0
}

fn repository_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR"))
    .ancestors()
    .nth(2)
    .expect("benchmark crate is two levels below the repository")
    .to_owned()
}

fn ensure_workspace_is_safe(repository: &Path, workspace: &Path) -> Result<()> {
  let expected_parent = repository.join("target");
  if !workspace.starts_with(&expected_parent) || workspace == expected_parent {
    return Err(format!("benchmark workspace {} must be a child of {}", workspace.display(), expected_parent.display()).into());
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn phase_one_ci_golden_trace_is_typed_and_explicit_about_missing_observers() {
    let trace: GoldenTraceFile = serde_json::from_str(PHASE_ONE_CI_TRACE).unwrap();

    assert_eq!(trace.schema_version, 1);
    assert_eq!(trace.cases.len(), 2);
    validate_golden_trace_contract(&trace).unwrap();
    for case in &trace.cases {
      assert_eq!(case.expected.reference.process_trace, "TBI");
      assert_eq!(case.expected.reference.network_trace, "TBI");
      assert_eq!(case.expected.candidate.process_trace, "TBI");
      assert_eq!(case.expected.candidate.network_trace, "TBI");
    }
  }

  #[test]
  fn percentile_statistics_sort_raw_samples() {
    let values: Vec<u64> = (1..=20).rev().collect();
    let result = statistics(&values);

    assert_eq!(result.min, 1);
    assert_eq!(result.median, 11);
    assert_eq!(result.p95, 19);
    assert_eq!(result.max, 20);
  }

  #[test]
  fn summary_is_aligned_and_readable_without_terminal_escape_codes() {
    let report = Report {
      schema_version: 18,
      generated_unix_seconds: 0,
      environment: Environment {
        os: "windows",
        arch: "x86_64",
        logical_cpus: 24,
        repository_commit: None,
      },
      runs: vec![
        Run {
          tool: "dv".into(),
          tool_version: "dv 0.1.0".into(),
          fixture: None,
          case: "build_noop".into(),
          command: vec!["dv".into(), "build".into()],
          status: RunStatus::Measured,
          warmups: 2,
          samples_ns: vec![12_346_000],
          statistics_ns: Some(Statistics {
            min: 12_346_000,
            median: 12_346_000,
            p95: 12_346_000,
            max: 12_346_000,
          }),
          network_requests: None,
          downloaded_bytes: None,
          downloaded_packages: None,
          resolved_packages: None,
        },
        Run {
          tool: "dv".into(),
          tool_version: "dv 0.1.0".into(),
          fixture: Some("small-console".into()),
          case: "sync_cold".into(),
          command: vec!["dv".into(), "sync".into()],
          status: RunStatus::Tbi,
          warmups: 0,
          samples_ns: Vec::new(),
          statistics_ns: None,
          network_requests: None,
          downloaded_bytes: None,
          downloaded_packages: None,
          resolved_packages: None,
        },
      ],
    };

    let output = render_summary(&report, false);

    assert!(output.contains("dv benchmark results"));
    assert!(output.contains("windows x86_64  •  24 logical CPUs"));
    assert!(output.contains("No-op build"));
    assert!(output.contains("12.346 ms"));
    assert!(output.contains("Cold sync"));
    assert!(output.contains("TBI"));
    assert!(output.contains("dv · No-op build  dv build"));
    assert!(output.contains('╭'));
    assert!(output.contains('╯'));
    assert!(!output.contains("\x1b["));
  }

  #[test]
  fn summary_reports_package_work_evidence() {
    let report = Report {
      schema_version: 18,
      generated_unix_seconds: 0,
      environment: Environment {
        os: "windows",
        arch: "x86_64",
        logical_cpus: 24,
        repository_commit: None,
      },
      runs: vec![Run {
        tool: "dv".into(),
        tool_version: "dv 0.1.0".into(),
        fixture: Some("package-console".into()),
        case: "package_sync_cold".into(),
        command: vec!["dv".into(), "restore".into()],
        status: RunStatus::Measured,
        warmups: 1,
        samples_ns: vec![1],
        statistics_ns: Some(Statistics {
          min: 1,
          median: 1,
          p95: 1,
          max: 1,
        }),
        network_requests: Some(2),
        downloaded_bytes: Some(2_441_966),
        downloaded_packages: Some(1),
        resolved_packages: Some(1),
      }],
    };

    let output = render_summary(&report, false);

    assert!(output.contains("Cold dependency readiness"));
    assert!(output.contains("1 resolved package · 1 downloaded · max 2 HTTP requests · max 2,441,966 payload bytes"));
    assert!(output.find("Observed work").unwrap() < output.find("Commands").unwrap());
  }

  #[test]
  fn integer_counts_have_thousands_separators() {
    assert_eq!(format_integer(0), "0");
    assert_eq!(format_integer(999), "999");
    assert_eq!(format_integer(1_000), "1,000");
    assert_eq!(format_integer(2_441_966), "2,441,966");
  }

  #[test]
  fn package_evidence_counts_the_resolved_package_batch() {
    let stdout =
      br#"{"type":"package_resolution_created","packages":[{"id":"A"},{"id":"B"}],"downloaded_packages":2,"network_requests":3,"downloaded_bytes":42}
"#;

    let evidence = parse_work_evidence(stdout).unwrap();

    assert_eq!(evidence.resolved_packages, Some(2));
    assert_eq!(evidence.downloaded_packages, Some(2));
  }

  #[test]
  fn retained_network_evidence_keeps_the_largest_observed_work() {
    let mut evidence = Some(WorkEvidence {
      network_requests: Some(208),
      downloaded_bytes: Some(165_000_000),
      downloaded_packages: Some(203),
      resolved_packages: Some(203),
    });

    merge_work_evidence(
      &mut evidence,
      Some(WorkEvidence {
        network_requests: Some(210),
        downloaded_bytes: Some(168_000_000),
        downloaded_packages: Some(203),
        resolved_packages: Some(203),
      }),
      "dv",
      "package_graph_massive",
    )
    .unwrap();

    assert_eq!(evidence.unwrap().network_requests, Some(210));
    assert_eq!(evidence.unwrap().downloaded_bytes, Some(168_000_000));
  }

  #[test]
  fn package_asset_paths_normalize_relative_and_absolute_cache_roots() {
    let expected = "humanizer.core/2.14.1/lib/net6.0/Humanizer.dll";

    assert_eq!(
      package_relative_path(".packages/humanizer.core/2.14.1/lib/net6.0/Humanizer.dll").unwrap(),
      expected
    );
    assert_eq!(
      package_relative_path("C:\\work\\.packages\\humanizer.core\\2.14.1\\lib\\net6.0\\Humanizer.dll").unwrap(),
      expected
    );
  }
}
