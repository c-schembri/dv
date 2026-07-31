use std::{
  env,
  error::Error,
  ffi::OsStr,
  fmt::Write as _,
  fs,
  io::{self, IsTerminal},
  path::{Path, PathBuf},
  process::{Command, Output},
  thread,
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy)]
enum CaseKind {
  Startup,
  RidGraph,
  ProjectEvaluate,
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
  PackageSyncWarm,
  NugetConfigHierarchy,
  NugetConfigMerge,
  NugetSourceSections,
  NugetStoragePolicy,
  NugetCliOverrides,
  NugetLocalSources,
  NugetServiceIndex,
  NugetCredentials,
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
  rid_graph: &'a Path,
  runtime: &'a Path,
  runtime_pack: &'a Path,
  framework_reference: &'a Path,
  unavailable_pack: &'a Path,
  package: &'a Path,
  nuget_config: &'a Path,
  nuget_config_merge: &'a Path,
  nuget_source_sections: &'a Path,
  nuget_storage_policy: &'a Path,
  nuget_cli_overrides: &'a Path,
  nuget_local_sources: &'a Path,
  nuget_service_index: &'a Path,
  nuget_credentials: &'a Path,
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
  let rid_graph_fixture = repository.join("benchmarks/fixtures/rid-graph-oracle");
  let runtime_fixture = repository.join("benchmarks/fixtures/runtime-project");
  let runtime_pack_fixture = repository.join("benchmarks/fixtures/runtime-pack-project");
  let framework_reference_fixture = repository.join("benchmarks/fixtures/framework-reference-project");
  let unavailable_pack_fixture = repository.join("benchmarks/fixtures/unavailable-pack-project");
  let package_fixture = repository.join("benchmarks/fixtures/package-console");
  let nuget_config_fixture = repository.join("benchmarks/fixtures/nuget-config-hierarchy");
  let nuget_config_merge_fixture = repository.join("benchmarks/fixtures/nuget-config-merge");
  let nuget_source_sections_fixture = repository.join("benchmarks/fixtures/nuget-source-sections");
  let nuget_storage_policy_fixture = repository.join("benchmarks/fixtures/nuget-storage-policy");
  let nuget_cli_overrides_fixture = repository.join("benchmarks/fixtures/nuget-cli-overrides");
  let nuget_local_sources_fixture = repository.join("benchmarks/fixtures/nuget-local-sources");
  let nuget_service_index_fixture = repository.join("benchmarks/fixtures/nuget-service-index");
  let nuget_credentials_fixture = repository.join("benchmarks/fixtures/nuget-credentials");
  let package_graph_fixture = repository.join("benchmarks/fixtures/large-package-graph");
  let massive_package_graph_fixture = repository.join("benchmarks/fixtures/massive-package-graph");
  let fixtures = Fixtures {
    small: &fixture,
    rid_graph: &rid_graph_fixture,
    runtime: &runtime_fixture,
    runtime_pack: &runtime_pack_fixture,
    framework_reference: &framework_reference_fixture,
    unavailable_pack: &unavailable_pack_fixture,
    package: &package_fixture,
    nuget_config: &nuget_config_fixture,
    nuget_config_merge: &nuget_config_merge_fixture,
    nuget_source_sections: &nuget_source_sections_fixture,
    nuget_storage_policy: &nuget_storage_policy_fixture,
    nuget_cli_overrides: &nuget_cli_overrides_fixture,
    nuget_local_sources: &nuget_local_sources_fixture,
    nuget_service_index: &nuget_service_index_fixture,
    nuget_credentials: &nuget_credentials_fixture,
    package_graph: &package_graph_fixture,
    package_graph_massive: &massive_package_graph_fixture,
  };
  let workspace = repository.join(format!("target/benchmark-work-{}", std::process::id()));
  let dv_executable = prepare_dv_executable(&repository, options.dv.as_deref())?;
  ensure_workspace_is_safe(&repository, &workspace)?;
  if options.case.as_deref().is_none_or(|case| case == "sdk_current") {
    verify_sdk_selection(&dv_executable, &fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "rid_graph") {
    verify_rid_graph(&repository, &dv_executable, &rid_graph_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "project_evaluate") {
    verify_project_evaluation(&dv_executable, &fixture)?;
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
  if options.case.as_deref().is_none_or(|case| case == "nuget_config_hierarchy") {
    verify_nuget_config_hierarchy(&repository, &dv_executable, &nuget_config_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_config_merge") {
    verify_nuget_config_merge(&repository, &dv_executable, &nuget_config_merge_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_source_sections") {
    verify_nuget_source_sections(&repository, &dv_executable, &nuget_source_sections_fixture)?;
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
  if options.case.as_deref().is_none_or(|case| case == "nuget_service_index") {
    verify_nuget_service_index(&repository, &dv_executable, &nuget_service_index_fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "nuget_credentials") {
    verify_nuget_credentials(&repository, &dv_executable, &nuget_credentials_fixture)?;
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
    schema_version: 14,
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

fn verify_project_evaluation(dv_executable: &Path, fixture: &Path) -> Result<()> {
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
  let dv_text = command_text(dv_executable, &["project", "inspect", "SmallConsole.csproj", "--json"], fixture)?;
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
  let root = repository.join(format!("target/benchmark-{verification_name}-verification"));
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
  if required_string(&dv, "source")? != "https://api.nuget.org/v3/index.json" || required_string(&dv, "source_protocol")? != "v3" {
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
  if required_string(&dv, "source")? != "https://api.nuget.org/v3/index.json" || required_string(&dv, "source_protocol")? != "v3" {
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
  if required_string(&dv, "source")? != "https://api.nuget.org/v3/index.json" || required_string(&dv, "source_protocol")? != "v3" {
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

fn reject_credential_output(label: &str, output: &str) -> Result<()> {
  for secret in [
    "config-decoy-user",
    "config-decoy-secret",
    "environment-user",
    "environment-pat",
    "config-only-user",
    "config-only-pat",
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
    for path in string_array(dv, field)? {
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
  if let Some(path) = requested {
    return Ok(if path.is_absolute() { path.to_owned() } else { repository.join(path) });
  }

  let cargo = env::var_os("CARGO").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo"));
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
    let command: Vec<String> = std::iter::once(executable.display().to_string())
      .chain(case.args.iter().map(|value| (*value).into()))
      .collect();

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

    let mut samples_ns = Vec::with_capacity(options.samples);
    let mut work = None;
    let total = options.warmups + options.samples;
    for index in 0..total {
      prepare_iteration(executable, case, case_fixture, &case_workspace)?;
      let measurement = measure(executable, case, case_cwd(case, case_fixture, &case_workspace))?;
      if index >= options.warmups {
        samples_ns.push(measurement.elapsed_ns);
        merge_work_evidence(&mut work, measurement.work, tool_name, case.name)?;
      }
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
      | CaseKind::RuntimeEvaluate
      | CaseKind::RuntimePackPlan
      | CaseKind::RuntimePackInventoryCold
      | CaseKind::FrameworkReferencePlan
      | CaseKind::CompilerPlan
      | CaseKind::PackageAssetPlan
      | CaseKind::PackageSyncWarm
      | CaseKind::NugetConfigHierarchy
      | CaseKind::NugetConfigMerge
      | CaseKind::NugetSourceSections
      | CaseKind::NugetStoragePolicy
      | CaseKind::NugetCliOverrides
      | CaseKind::NugetLocalSources
      | CaseKind::NugetServiceIndex
      | CaseKind::NugetCredentials
      | CaseKind::BuildNoOp
      | CaseKind::RunWarm
  ) {
    reset_fixture(fixture, workspace)?;
  }
  if matches!(case.kind, CaseKind::RidGraph) && is_dotnet(executable) {
    prepare_rid_oracle(executable, workspace)?;
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
    CaseKind::RidGraph
    | CaseKind::ProjectEvaluate
    | CaseKind::RuntimeEvaluate
    | CaseKind::RuntimePackPlan
    | CaseKind::FrameworkReferencePlan
    | CaseKind::CompilerPlan
    | CaseKind::PackageAssetPlan
    | CaseKind::NugetConfigHierarchy
    | CaseKind::NugetConfigMerge
    | CaseKind::NugetSourceSections
    | CaseKind::NugetStoragePolicy
    | CaseKind::NugetCliOverrides => Ok(()),
    CaseKind::NugetCredentials => Ok(()),
    CaseKind::NugetLocalSources => reset_nuget_local_iteration(workspace),
    CaseKind::NugetServiceIndex => reset_service_index_iteration(workspace),
    CaseKind::RuntimePackInventoryCold => reset_pack_inventory_cache(workspace),
    CaseKind::RestoreCold | CaseKind::PackageSyncCold | CaseKind::PackageGraphCold | CaseKind::PackageGraphMassive | CaseKind::PackDiagnostic => {
      reset_fixture(fixture, workspace)
    },
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
    CaseKind::RidGraph => fixtures.rid_graph,
    CaseKind::RuntimeEvaluate => fixtures.runtime,
    CaseKind::RuntimePackPlan | CaseKind::RuntimePackInventoryCold => fixtures.runtime_pack,
    CaseKind::FrameworkReferencePlan => fixtures.framework_reference,
    CaseKind::PackDiagnostic => fixtures.unavailable_pack,
    CaseKind::PackageSyncCold | CaseKind::PackageSyncWarm => fixtures.package,
    CaseKind::NugetConfigHierarchy => fixtures.nuget_config,
    CaseKind::NugetConfigMerge => fixtures.nuget_config_merge,
    CaseKind::NugetSourceSections => fixtures.nuget_source_sections,
    CaseKind::NugetStoragePolicy => fixtures.nuget_storage_policy,
    CaseKind::NugetCliOverrides => fixtures.nuget_cli_overrides,
    CaseKind::NugetLocalSources => fixtures.nuget_local_sources,
    CaseKind::NugetServiceIndex => fixtures.nuget_service_index,
    CaseKind::NugetCredentials => fixtures.nuget_credentials,
    CaseKind::PackageGraphCold => fixtures.package_graph,
    CaseKind::PackageGraphMassive | CaseKind::PackageAssetPlan => fixtures.package_graph_massive,
    _ => fixtures.small,
  }
}

fn fixture_name(case: &Case) -> Option<&'static str> {
  match case.kind {
    CaseKind::Startup => None,
    CaseKind::RidGraph => Some("rid-graph-oracle"),
    CaseKind::RuntimeEvaluate => Some("runtime-project"),
    CaseKind::RuntimePackPlan | CaseKind::RuntimePackInventoryCold => Some("runtime-pack-project"),
    CaseKind::FrameworkReferencePlan => Some("framework-reference-project"),
    CaseKind::PackDiagnostic => Some("unavailable-pack-project"),
    CaseKind::PackageSyncCold | CaseKind::PackageSyncWarm => Some("package-console"),
    CaseKind::NugetConfigHierarchy => Some("nuget-config-hierarchy"),
    CaseKind::NugetConfigMerge => Some("nuget-config-merge"),
    CaseKind::NugetSourceSections => Some("nuget-source-sections"),
    CaseKind::NugetStoragePolicy => Some("nuget-storage-policy"),
    CaseKind::NugetCliOverrides => Some("nuget-cli-overrides"),
    CaseKind::NugetLocalSources => Some("nuget-local-sources"),
    CaseKind::NugetServiceIndex => Some("nuget-service-index"),
    CaseKind::NugetCredentials => Some("nuget-credentials"),
    CaseKind::PackageGraphCold => Some("large-package-graph"),
    CaseKind::PackageGraphMassive | CaseKind::PackageAssetPlan => Some("massive-package-graph"),
    _ => Some("small-console"),
  }
}

fn measure(executable: &Path, case: &Case, cwd: &Path) -> Result<Measurement> {
  let started = Instant::now();
  let mut command = Command::new(executable);
  command.args(case.args).current_dir(cwd);
  if matches!(
    case.kind,
    CaseKind::NugetConfigHierarchy
      | CaseKind::NugetConfigMerge
      | CaseKind::NugetSourceSections
      | CaseKind::NugetStoragePolicy
      | CaseKind::NugetCliOverrides
      | CaseKind::NugetLocalSources
      | CaseKind::NugetServiceIndex
      | CaseKind::NugetCredentials
  ) {
    apply_case_nuget_environment(&mut command, case.kind, cwd);
  }
  let output = command.output()?;
  let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
  if matches!(case.kind, CaseKind::PackDiagnostic) {
    validate_pack_failure(&output, is_dotnet(executable))?;
  } else {
    check_output(output.clone(), executable, case.args, "measured command")?;
  }
  if !is_dotnet(executable) && matches!(case.kind, CaseKind::RuntimePackPlan | CaseKind::RuntimePackInventoryCold) {
    validate_pack_inventory_cache(cwd)?;
  }
  let work = if !is_dotnet(executable)
    && matches!(
      case.kind,
      CaseKind::PackageSyncCold
        | CaseKind::PackageGraphCold
        | CaseKind::PackageGraphMassive
        | CaseKind::PackageAssetPlan
        | CaseKind::PackageSyncWarm
        | CaseKind::NugetConfigHierarchy
        | CaseKind::NugetConfigMerge
        | CaseKind::NugetSourceSections
        | CaseKind::NugetStoragePolicy
        | CaseKind::NugetCliOverrides
        | CaseKind::NugetLocalSources
    ) {
    Some(parse_work_evidence(&output.stdout)?)
  } else if !is_dotnet(executable) && matches!(case.kind, CaseKind::NugetServiceIndex | CaseKind::NugetCredentials) {
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
  let mut archive_count = 0u64;
  let mut downloaded_bytes = 0u64;
  let mut directories = vec![cwd.join(".packages")];
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
  Ok(WorkEvidence {
    network_requests: None,
    downloaded_bytes: Some(downloaded_bytes),
    downloaded_packages: Some(archive_count),
    resolved_packages: Some(package_count),
  })
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

fn apply_case_nuget_environment(command: &mut Command, kind: CaseKind, cwd: &Path) {
  if matches!(kind, CaseKind::NugetStoragePolicy) {
    apply_nuget_storage_environment(command, cwd);
  } else if matches!(kind, CaseKind::NugetCliOverrides) {
    apply_nuget_cli_environment(command, cwd);
  } else if matches!(kind, CaseKind::NugetServiceIndex) {
    apply_nuget_config_environment(command, cwd);
    command.env("NUGET_HTTP_CACHE_PATH", cwd.join(".http-cache"));
  } else if matches!(kind, CaseKind::NugetCredentials) {
    apply_nuget_credential_environment(command, cwd);
  } else {
    apply_nuget_config_environment(command, cwd);
  }
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
  apply_case_nuget_environment(&mut command, CaseKind::NugetServiceIndex, cwd);
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
      if matches!(entry.file_name().to_str(), Some("obj" | "bin" | ".packages" | ".seed"))
        || (generated_policy_root && entry.file_name() == OsStr::new("policy"))
        || (local_sources_root && entry.file_name() == OsStr::new("feeds"))
      {
        continue;
      }
      copy_directory(&source_path, &destination_path)?;
    } else {
      if (generated_policy_root || local_sources_root) && matches!(entry.file_name().to_str(), Some("dv.lock.json" | "packages.lock.json")) {
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
  let sample_count = report.runs.first().map(|run| run.samples_ns.len()).unwrap_or(0);
  let warmup_count = report.runs.first().map(|run| run.warmups).unwrap_or(0);
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
          | "package_sync_warm"
          | "nuget_config_hierarchy"
          | "nuget_config_merge"
          | "nuget_source_sections"
          | "nuget_storage_policy"
          | "nuget_cli_overrides"
          | "nuget_local_sources"
          | "nuget_service_index"
          | "nuget_credentials"
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
    "cli_version" => "CLI self-version",
    "project_evaluate" => "Project evaluation",
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
    "package_sync_warm" => "Warm locked restore",
    "nuget_config_hierarchy" => "NuGet.Config hierarchy",
    "nuget_config_merge" => "NuGet.Config keyed merge",
    "nuget_source_sections" => "NuGet source sections",
    "nuget_storage_policy" => "NuGet storage policy",
    "nuget_cli_overrides" => "NuGet CLI overrides",
    "nuget_local_sources" => "NuGet local sources",
    "nuget_service_index" => "NuGet service index",
    "nuget_credentials" => "NuGet credentials",
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
      schema_version: 14,
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
      schema_version: 14,
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
