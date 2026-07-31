use std::{
  env,
  error::Error,
  ffi::OsStr,
  fmt::Write as _,
  fs,
  io::{self, IsTerminal},
  path::{Path, PathBuf},
  process::{Command, Output},
  time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy)]
enum CaseKind {
  Startup,
  ProjectEvaluate,
  CompilerPlan,
  RestoreCold,
  PackageSyncCold,
  PackageGraphCold,
  PackageSyncWarm,
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
  package: &'a Path,
  package_graph: &'a Path,
}

const DOTNET_CASES: &[Case] = &[
  Case {
    name: "sdk_current",
    kind: CaseKind::Startup,
    args: &["--version"],
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
    name: "project_evaluate",
    kind: CaseKind::ProjectEvaluate,
    args: &["project", "inspect", "SmallConsole.csproj", "--json"],
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
    name: "package_graph_cold",
    kind: CaseKind::PackageGraphCold,
    args: &["restore", "LargePackageGraph.csproj", "--packages", ".packages", "--json"],
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct WorkEvidence {
  network_requests: u64,
  downloaded_bytes: u64,
  downloaded_packages: u64,
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
  let package_fixture = repository.join("benchmarks/fixtures/package-console");
  let package_graph_fixture = repository.join("benchmarks/fixtures/large-package-graph");
  let fixtures = Fixtures {
    small: &fixture,
    package: &package_fixture,
    package_graph: &package_graph_fixture,
  };
  let workspace = repository.join("target/benchmark-work");
  let dv_executable = prepare_dv_executable(&repository, options.dv.as_deref())?;
  ensure_workspace_is_safe(&repository, &workspace)?;
  if options.case.as_deref().is_none_or(|case| case == "sdk_current") {
    verify_sdk_selection(&dv_executable, &fixture)?;
  }
  if options.case.as_deref().is_none_or(|case| case == "project_evaluate") {
    verify_project_evaluation(&dv_executable, &fixture)?;
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
  if options.case.as_deref().is_none_or(|case| case == "package_graph_cold") {
    verify_package_sync(&repository, &dv_executable, &package_graph_fixture, "LargePackageGraph.csproj", 50)?;
  }

  let mut runs = run_tool("dotnet", Path::new("dotnet"), DOTNET_CASES, &options, &fixtures, &workspace.join("dotnet"))?;
  runs.extend(run_tool("dv", &dv_executable, DV_CASES, &options, &fixtures, &workspace.join("dv"))?);
  if runs.is_empty() {
    return Err(format!("no benchmark case named {:?}", options.case.as_deref().unwrap_or_default()).into());
  }

  let generated_unix_seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
  let report = Report {
    schema_version: 4,
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
      network_requests: work.map(|evidence| evidence.network_requests),
      downloaded_bytes: work.map(|evidence| evidence.downloaded_bytes),
      downloaded_packages: work.map(|evidence| evidence.downloaded_packages),
    });
  }

  Ok(runs)
}

fn prepare_persistent_case(executable: &Path, case: &Case, fixture: &Path, workspace: &Path) -> Result<()> {
  if matches!(
    case.kind,
    CaseKind::ProjectEvaluate | CaseKind::CompilerPlan | CaseKind::PackageSyncWarm | CaseKind::BuildNoOp | CaseKind::RunWarm
  ) {
    reset_fixture(fixture, workspace)?;
  }
  if matches!(case.kind, CaseKind::CompilerPlan) && is_dotnet(executable) {
    run_checked(executable, &["restore", "--nologo", "--verbosity", "quiet"], workspace, "compiler plan restore")?;
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
  if matches!(case.kind, CaseKind::BuildNoOp | CaseKind::RunWarm) {
    run_checked(executable, build_args(executable), workspace, "persistent case setup")?;
  }
  Ok(())
}

fn prepare_iteration(executable: &Path, case: &Case, fixture: &Path, workspace: &Path) -> Result<()> {
  match case.kind {
    CaseKind::ProjectEvaluate | CaseKind::CompilerPlan => Ok(()),
    CaseKind::RestoreCold | CaseKind::PackageSyncCold | CaseKind::PackageGraphCold => reset_fixture(fixture, workspace),
    CaseKind::BuildClean => {
      reset_fixture(fixture, workspace)?;
      run_checked(executable, restore_args(executable), workspace, "clean build restore")
    },
    CaseKind::Startup | CaseKind::PackageSyncWarm | CaseKind::BuildNoOp | CaseKind::RunWarm => Ok(()),
  }
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
    CaseKind::PackageSyncCold | CaseKind::PackageSyncWarm => fixtures.package,
    CaseKind::PackageGraphCold => fixtures.package_graph,
    _ => fixtures.small,
  }
}

fn fixture_name(case: &Case) -> Option<&'static str> {
  match case.kind {
    CaseKind::Startup => None,
    CaseKind::PackageSyncCold | CaseKind::PackageSyncWarm => Some("package-console"),
    CaseKind::PackageGraphCold => Some("large-package-graph"),
    _ => Some("small-console"),
  }
}

fn measure(executable: &Path, case: &Case, cwd: &Path) -> Result<Measurement> {
  let started = Instant::now();
  let output = Command::new(executable).args(case.args).current_dir(cwd).output()?;
  let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
  check_output(output.clone(), executable, case.args, "measured command")?;
  let work = if !is_dotnet(executable) && matches!(case.kind, CaseKind::PackageSyncCold | CaseKind::PackageGraphCold | CaseKind::PackageSyncWarm) {
    Some(parse_work_evidence(&output.stdout)?)
  } else {
    None
  };
  Ok(Measurement { elapsed_ns: elapsed, work })
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
    network_requests: event
      .get("network_requests")
      .and_then(serde_json::Value::as_u64)
      .ok_or("dv package event omitted network_requests")?,
    downloaded_bytes: event
      .get("downloaded_bytes")
      .and_then(serde_json::Value::as_u64)
      .ok_or("dv package event omitted downloaded_bytes")?,
    downloaded_packages: event
      .get("downloaded_packages")
      .and_then(serde_json::Value::as_u64)
      .ok_or("dv package event omitted downloaded_packages")?,
  })
}

fn merge_work_evidence(current: &mut Option<WorkEvidence>, observed: Option<WorkEvidence>, tool: &str, case: &str) -> Result<()> {
  let Some(observed) = observed else {
    return Ok(());
  };
  if current.is_some_and(|current| current != observed) {
    return Err(format!("{tool} {case} reported inconsistent package, request, or byte counts across retained samples").into());
  }
  *current = Some(observed);
  Ok(())
}

fn run_checked(executable: &Path, args: &[&str], cwd: &Path, purpose: &str) -> Result<()> {
  let output = Command::new(executable).args(args).current_dir(cwd).output()?;
  check_output(output, executable, args, purpose)
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

fn reset_fixture(source: &Path, destination: &Path) -> Result<()> {
  if destination.exists() {
    fs::remove_dir_all(destination)?;
  }
  copy_directory(source, destination)
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
  fs::create_dir_all(destination)?;
  for entry in fs::read_dir(source)? {
    let entry = entry?;
    let source_path = entry.path();
    let destination_path = destination.join(entry.file_name());
    if entry.file_type()?.is_dir() {
      copy_directory(&source_path, &destination_path)?;
    } else {
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
    .filter(|run| matches!(run.case.as_str(), "package_sync_cold" | "package_graph_cold" | "package_sync_warm"))
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
      let evidence = match (run.downloaded_packages, run.network_requests, run.downloaded_bytes) {
        (Some(packages), Some(requests), Some(bytes)) => {
          let package_label = if packages == 1 { "package" } else { "packages" };
          format!(
            "{} {package_label} · {requests} HTTP requests · {} payload bytes",
            format_integer(packages),
            format_integer(bytes)
          )
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
    "compiler_plan" => "Compiler input plan",
    "restore_cold" => "Cold restore",
    "sync_cold" => "Cold sync",
    "package_sync_cold" => "Cold dependency readiness",
    "package_graph_cold" => "Cold large dependency graph",
    "package_sync_warm" => "Warm locked restore",
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

    if argument.is_empty() || argument.chars().any(|character| character.is_whitespace() || character == '"') {
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
      schema_version: 4,
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
      schema_version: 4,
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
      }],
    };

    let output = render_summary(&report, false);

    assert!(output.contains("Cold dependency readiness"));
    assert!(output.contains("1 package · 2 HTTP requests · 2,441,966 payload bytes"));
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
