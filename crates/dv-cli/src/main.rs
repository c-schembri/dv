use std::{
  env,
  ffi::OsString,
  io::{self, Write},
  path::{Path, PathBuf},
  process::ExitCode,
  time::Instant,
};

use dv_core::{
  CompilerPlan, CompilerPlanError, CompilerPlanErrorKind, ContextField, Diagnostic, DiagnosticCode, Event, EventPayload, FrameworkReferenceError,
  FrameworkReferenceErrorKind, FrameworkReferencePlan, Outcome, PackRequirement, PackageCancellation, PackageError, PackageErrorKind, PackageHttpPolicyEvent,
  PackageResolution, PackageResolveOptions, PackageServiceEndpointEvent, PackageSourceCapabilityEvent, PackageSourceInventory, ProjectConfiguration,
  ProjectError, ProjectErrorKind, ProjectFrameworkReferenceEvent, ProjectPackageEvent, ProjectSpec, ResolvedFrameworkReferenceEvent, ResolvedPackageEvent,
  RuntimeGraphError, RuntimeGraphErrorKind, RuntimePackError, RuntimePackErrorKind, RuntimePackPlan, RuntimeTargetEvent, SdkError, SdkErrorKind,
  SdkInstallationEvent, Severity, discover_sdks, evaluate_project, evaluate_project_path, inspect_package_sources, load_portable_runtime_graph,
  plan_compiler_inputs_with_packages, plan_framework_references, plan_runtime_packs, resolve_package_inputs, write_json_lines,
};

const HELP: &str = "\
dv - a fast .NET development toolchain

Usage:
  dv <command> [options]
  dv --help
  dv --version

Commands:
  init       Create project files
  add        Add a package reference
  remove     Remove a package reference
  restore    Resolve and cache dependencies
  sync       Alias for restore
  build      Build a project or workspace
  run        Build and run an application
  test       Build and run tests
  pack       Create packages
  publish    Publish deployable output
  sdk        Manage SDKs and runtimes
  project    Inspect project inputs

Output:
  --json     Emit the versioned JSON event protocol
";

const SDK_HELP: &str = "\
Usage:
  dv sdk current    Print the selected .NET SDK version
  dv sdk list       List discovered .NET SDKs
  dv sdk compatible-rids RID
                    Print RID fallbacks from the selected SDK graph
";

const PROJECT_HELP: &str = "\
Usage:
  dv project inspect [PROJECT] [--configuration Debug|Release]
  dv project frameworks [PROJECT] [--packages PATH]
  dv project runtime-packs [PROJECT] [--packages PATH]
  dv project package-sources [PROJECT] [-s|--source SOURCE]...
                             [--configfile PATH] [--offline] [--interactive]
                             [--probe-credentials]
";

const BUILD_HELP: &str = "\
Usage:
  dv build --plan [PROJECT] [--configuration Debug|Release]
";

const PACKAGE_HELP: &str = "\
Usage:
  dv restore [PROJECT] [-s|--source SOURCE]... [--packages PATH]
             [--configfile PATH] [--offline] [--interactive]
  dv sync [PROJECT] [-s|--source SOURCE]... [--packages PATH]
          [--configfile PATH] [--offline] [--interactive]
";

fn main() -> ExitCode {
  let started = Instant::now();
  let mut argument_iter = env::args_os().skip(1);
  let first = argument_iter.next();
  let second = argument_iter.next();

  if second.is_none() {
    match first.as_deref() {
      None => {
        print!("{HELP}");
        return ExitCode::SUCCESS;
      },
      Some(value) if value == "-h" || value == "--help" || value == "help" => {
        print!("{HELP}");
        return ExitCode::SUCCESS;
      },
      Some(value) if value == "-V" || value == "--version" || value == "version" => {
        println!("dv {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
      },
      Some(_) => {},
    }
  }

  let raw_args: Vec<OsString> = first.into_iter().chain(second).chain(argument_iter).collect();
  let json = raw_args.iter().any(|argument| argument == "--json");
  let args = match decode_args(&raw_args) {
    Ok(args) => args,
    Err(argument) => {
      return fail(
        started,
        json,
        "<invalid>",
        Vec::new(),
        diagnostic(
          "DV0002",
          "a command-line argument is not valid Unicode",
          Some(ContextField {
            name: "argument".into(),
            value: argument.to_string_lossy().into_owned(),
          }),
          Some("Pass command names and options as valid Unicode text."),
        ),
      );
    },
  };
  let semantic_args: Vec<String> = args.iter().filter(|argument| argument.as_str() != "--json").cloned().collect();

  match semantic_args.first().map(String::as_str) {
    None | Some("-h" | "--help" | "help") => {
      print!("{HELP}");
      ExitCode::SUCCESS
    },
    Some("-V" | "--version" | "version") => {
      println!("dv {}", env!("CARGO_PKG_VERSION"));
      ExitCode::SUCCESS
    },
    Some("sdk") => run_sdk(started, json, args, &semantic_args[1..]),
    Some("project") => run_project(started, json, args, &semantic_args[1..]),
    Some("build") => run_build(started, json, args, &semantic_args[1..]),
    Some(command @ ("restore" | "sync")) => run_package_command(started, json, command, args, &semantic_args[1..]),
    Some(command) if is_known_command(command) => fail(
      started,
      json,
      command,
      args,
      diagnostic(
        "DV0003",
        format!("command {command:?} is not implemented yet"),
        Some(ContextField {
          name: "command".into(),
          value: command.into(),
        }),
        Some("Use --help to inspect the Phase 0 command surface."),
      ),
    ),
    Some(command) => fail(
      started,
      json,
      command,
      args,
      diagnostic(
        "DV0001",
        format!("unknown command {command:?}"),
        Some(ContextField {
          name: "command".into(),
          value: command.into(),
        }),
        Some("Use --help to list available commands."),
      ),
    ),
  }
}

fn run_package_command(started: Instant, json: bool, command: &str, args: Vec<String>, command_args: &[String]) -> ExitCode {
  if matches!(command_args, [argument] if matches!(argument.as_str(), "help" | "--help" | "-h")) {
    print!("{PACKAGE_HELP}");
    return ExitCode::SUCCESS;
  }
  let options = match parse_package_args(command, command_args) {
    Ok(options) => options,
    Err(problem) => {
      return fail(
        started,
        json,
        command,
        args,
        diagnostic(
          "DV0002",
          problem,
          None,
          Some("Use `dv restore --help` or `dv sync --help` to inspect the accepted arguments."),
        ),
      );
    },
  };
  let current_directory = match env::current_dir() {
    Ok(directory) => directory,
    Err(error) => {
      return fail(
        started,
        json,
        command,
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, options.project.as_deref(), ProjectConfiguration::Debug) {
    Ok(project) => project,
    Err(error) => return fail(started, json, command, args, project_diagnostic(error)),
  };
  let options = match normalize_package_options(options, &current_directory, true) {
    Ok(options) => options,
    Err(problem) => return fail(started, json, command, args, diagnostic("DV0002", problem, None, None)),
  };
  let resolutions = match resolve_package_inputs(&[&project], &options) {
    Ok(resolutions) => resolutions,
    Err(error) => return fail(started, json, command, args, package_diagnostic(error)),
  };
  let resolution = &resolutions[0];
  if !json {
    return write_package_resolution(resolution);
  }
  succeed(started, command, args, package_resolution_payload(&project, resolution))
}

fn normalize_package_options(options: PackageCommandOptions, current_directory: &Path, write_lock: bool) -> Result<PackageResolveOptions, String> {
  let cancellation = install_credential_provider_cancellation()?;
  Ok(PackageResolveOptions {
    packages_directory: options
      .packages_directory
      .map(|path| if path.is_absolute() { path } else { current_directory.join(path) }),
    config_file: options
      .config_file
      .map(|path| if path.is_absolute() { path } else { current_directory.join(path) }),
    sources: options
      .sources
      .into_iter()
      .map(|source| normalize_command_source(source, current_directory))
      .collect(),
    offline: options.offline,
    write_lock,
    interactive: options.interactive,
    probe_credentials: options.probe_credentials,
    cancellation,
    credential_provider_log_sink: options.interactive.then_some(write_credential_provider_log),
  })
}

fn install_credential_provider_cancellation() -> Result<Option<PackageCancellation>, String> {
  let configured = env::var_os("NUGET_NETCORE_PLUGIN_PATHS")
    .or_else(|| env::var_os("NUGET_PLUGIN_PATHS"))
    .is_some_and(|paths| !paths.is_empty());
  if !configured {
    return Ok(None);
  }
  let cancellation = PackageCancellation::new();
  let signal = cancellation.clone();
  ctrlc::set_handler(move || signal.cancel()).map_err(|error| format!("failed to install credential-provider cancellation handler: {error}"))?;
  Ok(Some(cancellation))
}

struct PackageCommandOptions {
  project: Option<PathBuf>,
  packages_directory: Option<PathBuf>,
  config_file: Option<PathBuf>,
  sources: Vec<String>,
  offline: bool,
  interactive: bool,
  probe_credentials: bool,
}

fn parse_package_args(command: &str, arguments: &[String]) -> Result<PackageCommandOptions, String> {
  let mut options = PackageCommandOptions {
    project: None,
    packages_directory: None,
    config_file: None,
    sources: Vec::new(),
    offline: false,
    interactive: false,
    probe_credentials: false,
  };
  let mut index = 0;
  while index < arguments.len() {
    match arguments[index].as_str() {
      "--packages" if options.packages_directory.is_some() => return Err("--packages cannot be specified more than once".into()),
      "--packages" => {
        index += 1;
        let path = arguments.get(index).ok_or("--packages requires a path")?;
        if path.is_empty() {
          return Err("--packages requires a path".into());
        }
        options.packages_directory = Some(PathBuf::from(path));
      },
      value if value.starts_with("--packages=") => {
        if options.packages_directory.is_some() {
          return Err("--packages cannot be specified more than once".into());
        }
        let value = &value["--packages=".len()..];
        if value.is_empty() {
          return Err("--packages requires a path".into());
        }
        options.packages_directory = Some(PathBuf::from(value));
      },
      "--configfile" if options.config_file.is_some() => return Err("--configfile cannot be specified more than once".into()),
      "--configfile" => {
        index += 1;
        let path = arguments.get(index).ok_or("--configfile requires a path")?;
        if path.is_empty() {
          return Err("--configfile requires a path".into());
        }
        options.config_file = Some(PathBuf::from(path));
      },
      value if value.starts_with("--configfile=") => {
        if options.config_file.is_some() {
          return Err("--configfile cannot be specified more than once".into());
        }
        let value = &value["--configfile=".len()..];
        if value.is_empty() {
          return Err("--configfile requires a path".into());
        }
        options.config_file = Some(PathBuf::from(value));
      },
      "--source" | "-s" => {
        index += 1;
        let source = arguments.get(index).ok_or("--source requires a package source")?;
        if source.is_empty() {
          return Err("--source requires a package source".into());
        }
        validate_command_source(source)?;
        options.sources.push(source.clone());
      },
      value if value.starts_with("--source=") || value.starts_with("-s=") => {
        let source = value.strip_prefix("--source=").or_else(|| value.strip_prefix("-s=")).unwrap_or_default();
        if source.is_empty() {
          return Err("--source requires a package source".into());
        }
        validate_command_source(source)?;
        options.sources.push(source.to_owned());
      },
      "--offline" => options.offline = true,
      "--interactive" => options.interactive = true,
      "--probe-credentials" if command == "project package-sources" => options.probe_credentials = true,
      value if value.starts_with('-') => return Err(format!("unknown {command} option {value:?}")),
      value if options.project.is_none() => options.project = Some(PathBuf::from(value)),
      value => return Err(format!("unexpected {command} argument {value:?}")),
    }
    index += 1;
  }
  Ok(options)
}

fn validate_command_source(source: &str) -> Result<(), String> {
  if source.contains("://")
    && !source.get(..8).is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    && !source.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
    && !source.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
  {
    return Err("--source requires HTTP, HTTPS, file://, or a local folder path".into());
  }
  Ok(())
}

fn normalize_command_source(source: String, current_directory: &Path) -> String {
  if source.contains("://") {
    return source;
  }
  let path = PathBuf::from(&source);
  if path.is_absolute() {
    source
  } else {
    current_directory.join(path).to_string_lossy().into_owned()
  }
}

fn write_credential_provider_log(message: &str) {
  let _ = writeln!(io::stderr().lock(), "credential provider: {message}");
}

fn run_build(started: Instant, json: bool, args: Vec<String>, build_args: &[String]) -> ExitCode {
  if matches!(build_args, [argument] if matches!(argument.as_str(), "help" | "--help" | "-h")) {
    print!("{BUILD_HELP}");
    return ExitCode::SUCCESS;
  }
  if !build_args.iter().any(|argument| argument == "--plan") {
    return fail(
      started,
      json,
      "build",
      args,
      diagnostic(
        "DV0003",
        "build execution is not implemented yet; only input planning is available",
        None,
        Some("Use `dv build --plan [PROJECT]` to inspect compiler inputs."),
      ),
    );
  }
  let plan_arguments: Vec<String> = build_args.iter().filter(|argument| argument.as_str() != "--plan").cloned().collect();
  let (requested_path, configuration) = match parse_project_args(&plan_arguments) {
    Ok(options) => options,
    Err(problem) => {
      return fail(
        started,
        json,
        "build --plan",
        args,
        diagnostic("DV0002", problem, None, Some("Use `dv build --help` to inspect the accepted arguments.")),
      );
    },
  };
  let current_directory = match env::current_dir() {
    Ok(directory) => directory,
    Err(error) => {
      return fail(
        started,
        json,
        "build --plan",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, requested_path.as_deref(), configuration) {
    Ok(project) => project,
    Err(error) => return fail(started, json, "build --plan", args, project_diagnostic(error)),
  };
  let inventory = match discover_sdks(&current_directory) {
    Ok(inventory) => inventory,
    Err(error) => return fail(started, json, "build --plan", args, sdk_diagnostic(&current_directory, error)),
  };
  let package_options = PackageResolveOptions {
    packages_directory: None,
    config_file: None,
    sources: Vec::new(),
    offline: false,
    write_lock: true,
    ..PackageResolveOptions::default()
  };
  let package_resolutions = match resolve_package_inputs(&[&project], &package_options) {
    Ok(resolutions) => resolutions,
    Err(error) => return fail(started, json, "build --plan", args, package_diagnostic(error)),
  };
  let plans = match plan_compiler_inputs_with_packages(&[&project], &inventory, &package_resolutions) {
    Ok(plans) => plans,
    Err(error) => return fail(started, json, "build --plan", args, compiler_plan_diagnostic(error)),
  };
  let plan = &plans[0];
  let packages = &package_resolutions[0];
  if !json {
    return write_compiler_plan(plan);
  }

  succeed(
    started,
    "build --plan",
    args,
    EventPayload::CompilerPlanCreated {
      project: plan.project().into(),
      sdk_version: plan.sdk_version().into(),
      compiler: plan.compiler().into(),
      framework_pack_version: plan.framework_pack_version().into(),
      framework_pack: plan.framework_pack().into(),
      language_version: plan.language_version().into(),
      warning_level: plan.warning_level(),
      configuration: plan.configuration().as_str().into(),
      output_type: plan.output_type().as_str().into(),
      nullable: plan.nullable_enabled(),
      deterministic: plan.deterministic(),
      output_assembly: plan.output_assembly().into(),
      output_pdb: plan.output_pdb().into(),
      reference_output: plan.reference_output().into(),
      sources: plan.sources().map(str::to_owned).collect(),
      generated_sources: plan.generated_sources().map(str::to_owned).collect(),
      references: plan.references().map(str::to_owned).collect(),
      analyzers: plan.analyzers().map(str::to_owned).collect(),
      analyzer_configs: plan.analyzer_configs().map(str::to_owned).collect(),
      defines: plan.defines().map(str::to_owned).collect(),
      package_count: packages.packages().len() as u32,
      package_compile_assets: packages.compile_assets().len() as u32,
      package_cache_hits: packages.cache_hits(),
      downloaded_packages: packages.downloaded_packages(),
      package_network_requests: packages.network_requests(),
      package_downloaded_bytes: packages.downloaded_bytes(),
    },
  )
}

fn package_resolution_payload(project: &ProjectSpec, resolution: &PackageResolution) -> EventPayload {
  let packages = resolution
    .packages()
    .iter()
    .copied()
    .enumerate()
    .map(|(index, package)| ResolvedPackageEvent {
      id: resolution.package_id(package).into(),
      version: resolution.package_version(package).into(),
      sha512: resolution.package_hash(index).into(),
      direct: resolution.package_is_direct(package),
      dependency_count: resolution.package_dependencies(package).len() as u32,
    })
    .collect();
  EventPayload::PackageResolutionCreated {
    project: project.project_path().display().to_string(),
    cache_root: resolution.cache_root().display().to_string(),
    http_cache_root: resolution.http_cache_root().display().to_string(),
    temp_root: resolution.temp_root().display().to_string(),
    fallback_roots: resolution.fallback_roots().map(|path| path.display().to_string()).collect(),
    signature_validation: resolution.signature_validation().as_str().into(),
    audit_enabled: resolution.audit_enabled(),
    audit_mode: resolution.audit_mode().as_str().into(),
    audit_level: resolution.audit_level().as_str().into(),
    proxy_configured: resolution.proxy_configured(),
    lock_path: resolution.lock_path().display().to_string(),
    target_framework: resolution.target_framework().into(),
    source: resolution.source().into(),
    source_protocol: resolution.source_protocol().into(),
    packages,
    compile_assets: resolution.compile_assets().map(|path| path.display().to_string()).collect(),
    runtime_assets: resolution.runtime_assets().map(|path| path.display().to_string()).collect(),
    analyzers: resolution.analyzers().map(|path| path.display().to_string()).collect(),
    resource_assets: resolution.resource_assets().map(|path| path.display().to_string()).collect(),
    content_files: resolution.content_files().map(|path| path.display().to_string()).collect(),
    build_assets: resolution.build_assets().map(|path| path.display().to_string()).collect(),
    build_multi_targeting_assets: resolution.build_multi_targeting_assets().map(|path| path.display().to_string()).collect(),
    build_transitive_assets: resolution.build_transitive_assets().map(|path| path.display().to_string()).collect(),
    native_assets: resolution.native_assets().map(|path| path.display().to_string()).collect(),
    runtime_targets: resolution
      .runtime_targets()
      .map(|(path, runtime_identifier, kind)| RuntimeTargetEvent {
        path: path.display().to_string(),
        runtime_identifier: runtime_identifier.to_owned(),
        kind,
      })
      .collect(),
    cache_hits: resolution.cache_hits(),
    downloaded_packages: resolution.downloaded_packages(),
    network_requests: resolution.network_requests(),
    downloaded_bytes: resolution.downloaded_bytes(),
  }
}

fn write_package_resolution(resolution: &PackageResolution) -> ExitCode {
  let mut output = String::with_capacity(1024);
  use std::fmt::Write as _;
  writeln!(output, "Package resolution").expect("writing a String succeeds");
  writeln!(output, "  Packages       {}", resolution.packages().len()).expect("writing a String succeeds");
  writeln!(output, "  Cache hits     {}", resolution.cache_hits()).expect("writing a String succeeds");
  writeln!(output, "  Downloaded     {}", resolution.downloaded_packages()).expect("writing a String succeeds");
  writeln!(output, "  HTTP requests  {}", resolution.network_requests()).expect("writing a String succeeds");
  writeln!(output, "  Payload bytes  {}", resolution.downloaded_bytes()).expect("writing a String succeeds");
  writeln!(output, "  Compile assets {}", resolution.compile_assets().len()).expect("writing a String succeeds");
  writeln!(output, "  Runtime assets {}", resolution.runtime_assets().len()).expect("writing a String succeeds");
  writeln!(output, "  Resource assets {}", resolution.resource_assets().len()).expect("writing a String succeeds");
  writeln!(output, "  Content files   {}", resolution.content_files().len()).expect("writing a String succeeds");
  writeln!(
    output,
    "  Build imports   {}",
    resolution.build_assets().len() + resolution.build_transitive_assets().len()
  )
  .expect("writing a String succeeds");
  writeln!(output, "  Runtime targets {}", resolution.runtime_targets().len()).expect("writing a String succeeds");
  writeln!(output, "  Target         {}", resolution.target_framework()).expect("writing a String succeeds");
  writeln!(output, "  Source         {} ({})", resolution.source(), resolution.source_protocol()).expect("writing a String succeeds");
  writeln!(output, "  Cache          {}", resolution.cache_root().display()).expect("writing a String succeeds");
  writeln!(output, "  Lock           {}", resolution.lock_path().display()).expect("writing a String succeeds");
  for package in resolution.packages().iter().copied() {
    writeln!(
      output,
      "  {} {}{}",
      resolution.package_id(package),
      resolution.package_version(package),
      if resolution.package_is_direct(package) { " (direct)" } else { "" }
    )
    .expect("writing a String succeeds");
  }
  io::stdout()
    .lock()
    .write_all(output.as_bytes())
    .expect("writing package resolution to stdout succeeds");
  ExitCode::SUCCESS
}

fn decode_args(raw_args: &[OsString]) -> Result<Vec<String>, &OsString> {
  raw_args.iter().map(|argument| argument.to_str().map(str::to_owned).ok_or(argument)).collect()
}

fn is_known_command(command: &str) -> bool {
  matches!(
    command,
    "init" | "add" | "remove" | "restore" | "sync" | "build" | "run" | "test" | "pack" | "publish" | "sdk" | "project"
  )
}

fn run_sdk(started: Instant, json: bool, args: Vec<String>, sdk_args: &[String]) -> ExitCode {
  match sdk_args {
    [] => {
      print!("{SDK_HELP}");
      ExitCode::SUCCESS
    },
    [sdk] if sdk == "help" || sdk == "--help" || sdk == "-h" => {
      print!("{SDK_HELP}");
      ExitCode::SUCCESS
    },
    [sdk] if sdk == "current" => sdk_current(started, json, args),
    [sdk] if sdk == "list" => sdk_list(started, json, args),
    [sdk, runtime_identifier] if sdk == "compatible-rids" => sdk_compatible_rids(started, json, args, runtime_identifier),
    [sdk, ..] if sdk == "compatible-rids" => fail(
      started,
      json,
      "sdk compatible-rids",
      args,
      diagnostic(
        "DV0002",
        "sdk compatible-rids requires exactly one runtime identifier",
        None,
        Some("Use `dv sdk compatible-rids RID`."),
      ),
    ),
    _ => {
      let subcommand = sdk_args.first().map_or("<missing>", String::as_str);
      fail(
        started,
        json,
        "sdk",
        args,
        diagnostic(
          "DV0001",
          format!("unknown sdk command {subcommand:?}"),
          Some(ContextField {
            name: "command".into(),
            value: format!("sdk {subcommand}"),
          }),
          Some("Use `dv sdk --help` to list SDK commands."),
        ),
      )
    },
  }
}

fn sdk_current(started: Instant, json: bool, args: Vec<String>) -> ExitCode {
  let inventory = match load_sdk_inventory(started, json, &args) {
    Ok(inventory) => inventory,
    Err(exit_code) => return exit_code,
  };
  let selected = inventory.selected();

  if !json {
    println!("{}", selected.version);
    return ExitCode::SUCCESS;
  }

  let path = inventory.installation_path(selected);
  let payload = match (
    path_text(&path, "SDK installation"),
    path_text(inventory.root(selected), "SDK root"),
    optional_path_text(inventory.global_json.as_deref(), "global.json"),
  ) {
    (Ok(path), Ok(root), Ok(global_json)) => EventPayload::SdkSelected {
      version: selected.version.as_str().into(),
      path,
      root,
      global_json,
    },
    (Err(diagnostic), _, _) | (_, Err(diagnostic), _) | (_, _, Err(diagnostic)) => {
      return fail(started, true, "sdk current", args, *diagnostic);
    },
  };
  succeed(started, "sdk current", args, payload)
}

fn sdk_list(started: Instant, json: bool, args: Vec<String>) -> ExitCode {
  let inventory = match load_sdk_inventory(started, json, &args) {
    Ok(inventory) => inventory,
    Err(exit_code) => return exit_code,
  };

  if !json {
    for (index, installation) in inventory.installations.iter().enumerate() {
      let marker = if index == inventory.selected_index { '*' } else { ' ' };
      println!("{marker} {} [{}]", installation.version, inventory.installation_path(installation).display());
    }
    return ExitCode::SUCCESS;
  }

  let installations: Result<Vec<SdkInstallationEvent>, Box<Diagnostic>> = inventory
    .installations
    .iter()
    .enumerate()
    .map(|(index, installation)| {
      let path = inventory.installation_path(installation);
      Ok(SdkInstallationEvent {
        version: installation.version.as_str().into(),
        path: path_text(&path, "SDK installation")?,
        selected: index == inventory.selected_index,
      })
    })
    .collect();
  let installations = match installations {
    Ok(installations) => installations,
    Err(diagnostic) => return fail(started, true, "sdk list", args, *diagnostic),
  };
  let global_json = match optional_path_text(inventory.global_json.as_deref(), "global.json") {
    Ok(path) => path,
    Err(diagnostic) => return fail(started, true, "sdk list", args, *diagnostic),
  };
  succeed(started, "sdk list", args, EventPayload::SdkInventory { installations, global_json })
}

fn sdk_compatible_rids(started: Instant, json: bool, args: Vec<String>, runtime_identifier: &str) -> ExitCode {
  if runtime_identifier.is_empty() {
    return fail(
      started,
      json,
      "sdk compatible-rids",
      args,
      diagnostic(
        "DV0002",
        "runtime identifier must not be empty",
        None,
        Some("Pass one literal runtime identifier."),
      ),
    );
  }
  let inventory = match load_sdk_inventory(started, json, &args) {
    Ok(inventory) => inventory,
    Err(exit_code) => return exit_code,
  };
  let graph = match load_portable_runtime_graph(&inventory) {
    Ok(graph) => graph,
    Err(error) => return fail(started, json, "sdk compatible-rids", args, runtime_graph_diagnostic(error)),
  };

  if !json {
    for compatible in graph.compatible_rids(runtime_identifier) {
      println!("{compatible}");
    }
    return ExitCode::SUCCESS;
  }

  let graph_path = match path_text(graph.source(), "portable RID graph") {
    Ok(path) => path,
    Err(diagnostic) => return fail(started, true, "sdk compatible-rids", args, *diagnostic),
  };
  succeed(
    started,
    "sdk compatible-rids",
    args,
    EventPayload::RuntimeCompatibility {
      sdk_version: inventory.selected().version.as_str().into(),
      graph_path,
      runtime_identifier: runtime_identifier.into(),
      compatible_runtimes: graph.compatible_rids(runtime_identifier).map(str::to_owned).collect(),
      node_count: graph.node_count() as u32,
      edge_count: graph.edge_count() as u32,
      compatibility_count: graph.compatibility_count() as u32,
    },
  )
}

fn run_project(started: Instant, json: bool, args: Vec<String>, project_args: &[String]) -> ExitCode {
  if project_args.is_empty()
    || matches!(project_args, [argument] if matches!(argument.as_str(), "help" | "--help" | "-h"))
    || matches!(project_args, [inspect, argument] if inspect == "inspect" && matches!(argument.as_str(), "help" | "--help" | "-h"))
    || matches!(project_args, [frameworks, argument] if frameworks == "frameworks" && matches!(argument.as_str(), "help" | "--help" | "-h"))
    || matches!(project_args, [packs, argument] if packs == "runtime-packs" && matches!(argument.as_str(), "help" | "--help" | "-h"))
    || matches!(project_args, [sources, argument] if sources == "package-sources" && matches!(argument.as_str(), "help" | "--help" | "-h"))
  {
    print!("{PROJECT_HELP}");
    return ExitCode::SUCCESS;
  }
  if project_args.first().map(String::as_str) == Some("runtime-packs") {
    return project_runtime_packs(started, json, args, &project_args[1..]);
  }
  if project_args.first().map(String::as_str) == Some("frameworks") {
    return project_frameworks(started, json, args, &project_args[1..]);
  }
  if project_args.first().map(String::as_str) == Some("package-sources") {
    return project_package_sources(started, json, args, &project_args[1..]);
  }
  if project_args.first().map(String::as_str) != Some("inspect") {
    let subcommand = project_args.first().map_or("<missing>", String::as_str);
    return fail(
      started,
      json,
      "project",
      args,
      diagnostic(
        "DV0001",
        format!("unknown project command {subcommand:?}"),
        Some(ContextField {
          name: "command".into(),
          value: format!("project {subcommand}"),
        }),
        Some("Use `dv project --help` to list project commands."),
      ),
    );
  }

  let (requested_path, configuration) = match parse_project_args(&project_args[1..]) {
    Ok(options) => options,
    Err(problem) => {
      return fail(
        started,
        json,
        "project inspect",
        args,
        diagnostic(
          "DV0002",
          problem,
          None,
          Some("Use `dv project inspect --help` to inspect the accepted arguments."),
        ),
      );
    },
  };
  let current_directory = match env::current_dir() {
    Ok(directory) => directory,
    Err(error) => {
      return fail(
        started,
        json,
        "project inspect",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, requested_path.as_deref(), configuration) {
    Ok(project) => project,
    Err(error) => return fail(started, json, "project inspect", args, project_diagnostic(error)),
  };

  if !json {
    return write_project(&project);
  }

  let packages = project
    .package_references()
    .iter()
    .map(|package| ProjectPackageEvent {
      id: project.package_id(*package).into(),
      version: project.package_version(*package).into(),
    })
    .collect();
  let frameworks = project
    .framework_references()
    .iter()
    .map(|reference| ProjectFrameworkReferenceEvent {
      id: project.framework_reference_id(*reference).into(),
      runtime_version: project.framework_runtime_version(*reference).map(str::to_owned),
      targeting_pack_version: project.framework_targeting_pack_version(*reference).map(str::to_owned),
      target_latest_runtime_patch: project.framework_target_latest_runtime_patch(*reference),
    })
    .collect();
  let payload = EventPayload::ProjectEvaluated {
    project: project.project_path().display().to_string(),
    sdk: project.sdk().into(),
    target_framework: project.target_framework().into(),
    runtime_identifier: project.runtime_identifier().map(str::to_owned),
    runtime_identifiers: project.runtime_identifiers().map(str::to_owned).collect(),
    runtime_dimensions: project.runtime_dimensions().map(str::to_owned).collect(),
    output_type: project.output_type().as_str().into(),
    configuration: project.configuration().as_str().into(),
    assembly_name: project.assembly_name().into(),
    root_namespace: project.root_namespace().into(),
    nullable: toggle_text(project.nullable_enabled()).into(),
    implicit_usings: toggle_text(project.implicit_usings_enabled()).into(),
    deterministic: project.deterministic(),
    sources: project.sources().map(str::to_owned).collect(),
    project_references: project.project_references().map(str::to_owned).collect(),
    package_references: packages,
    framework_references: frameworks,
    runtime_framework_version: project.runtime_framework_version().map(str::to_owned),
    target_latest_runtime_patch: project.target_latest_runtime_patch(),
    roll_forward: project.roll_forward().as_str().into(),
    self_contained: project.self_contained(),
  };
  succeed(started, "project inspect", args, payload)
}

fn project_package_sources(started: Instant, json: bool, args: Vec<String>, project_args: &[String]) -> ExitCode {
  let parsed = match parse_package_args("project package-sources", project_args) {
    Ok(options) => options,
    Err(problem) => {
      return fail(
        started,
        json,
        "project package-sources",
        args,
        diagnostic(
          "DV0002",
          problem,
          None,
          Some("Use `dv project package-sources --help` to inspect the accepted arguments."),
        ),
      );
    },
  };
  let current_directory = match env::current_dir() {
    Ok(directory) => directory,
    Err(error) => {
      return fail(
        started,
        json,
        "project package-sources",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, parsed.project.as_deref(), ProjectConfiguration::Debug) {
    Ok(project) => project,
    Err(error) => return fail(started, json, "project package-sources", args, project_diagnostic(error)),
  };
  let options = match normalize_package_options(parsed, &current_directory, false) {
    Ok(options) => options,
    Err(problem) => return fail(started, json, "project package-sources", args, diagnostic("DV0002", problem, None, None)),
  };
  let inventories = match inspect_package_sources(&[&project], &options) {
    Ok(inventories) => inventories,
    Err(error) => return fail(started, json, "project package-sources", args, package_diagnostic(error)),
  };
  let inventory = &inventories[0];
  if !json {
    return write_package_sources(inventory);
  }
  let sources = inventory
    .sources()
    .map(|source| PackageSourceCapabilityEvent {
      name: inventory.source_name(source).to_owned(),
      location: inventory.source_location(source).to_owned(),
      protocol: inventory.source_protocol(source).to_owned(),
      authentication: inventory.source_authentication(source).as_str().to_owned(),
      allow_insecure_connections: inventory.source_allows_insecure_connections(source),
      disable_tls_certificate_validation: !inventory.source_tls_validation(source),
      endpoints: inventory
        .source_endpoints(source)
        .map(|endpoint| PackageServiceEndpointEvent {
          kind: inventory.endpoint_kind(endpoint).as_str().to_owned(),
          location: inventory.endpoint_location(endpoint).to_owned(),
        })
        .collect(),
    })
    .collect();
  let policy = inventory.http_policy();
  succeed(
    started,
    "project package-sources",
    args,
    EventPayload::PackageSourcesInspected {
      project: project.project_path().display().to_string(),
      sources,
      http_policy: PackageHttpPolicyEvent {
        max_tries: policy.max_tries(),
        retry_delay_ms: policy.retry_delay_ms(),
        max_retry_after_seconds: policy.max_retry_after_seconds(),
        request_timeout_seconds: policy.request_timeout_seconds(),
        download_timeout_seconds: policy.download_timeout_seconds(),
        max_requests_per_source: policy.max_requests_per_source(),
        retry_http_429: policy.retries_http_429(),
        observe_retry_after: policy.observes_retry_after(),
        proxy_configured: policy.proxy_configured(),
        proxy_authenticated: policy.proxy_authenticated(),
        no_proxy_configured: policy.no_proxy_configured(),
        offline: policy.offline(),
        tls_validation: policy.tls_validation(),
        allow_insecure_connections: policy.allows_insecure_connections(),
        max_redirects: policy.max_redirects(),
      },
      network_requests: inventory.network_requests(),
      downloaded_bytes: inventory.downloaded_bytes(),
    },
  )
}

fn write_package_sources(inventory: &PackageSourceInventory) -> ExitCode {
  let mut output = String::with_capacity(1024);
  use std::fmt::Write as _;
  let policy = inventory.http_policy();
  writeln!(
    output,
    "HTTP: tries={}, delay={}ms, timeout={}s/{}s, per-source={}, proxy={}, proxy-auth={}, no-proxy={}, offline={}, insecure-http={}, tls-validation={}",
    policy.max_tries(),
    policy.retry_delay_ms(),
    policy.request_timeout_seconds(),
    policy.download_timeout_seconds(),
    policy.max_requests_per_source(),
    policy.proxy_configured(),
    policy.proxy_authenticated(),
    policy.no_proxy_configured(),
    policy.offline(),
    policy.allows_insecure_connections(),
    policy.tls_validation()
  )
  .expect("writing a String succeeds");
  for source in inventory.sources() {
    writeln!(
      output,
      "{} ({}, {}, insecure-http={}, tls-validation={})",
      inventory.source_name(source),
      inventory.source_protocol(source),
      inventory.source_authentication(source).as_str(),
      inventory.source_allows_insecure_connections(source),
      inventory.source_tls_validation(source)
    )
    .expect("writing a String succeeds");
    for endpoint in inventory.source_endpoints(source) {
      writeln!(
        output,
        "  {:<15} {}",
        inventory.endpoint_kind(endpoint).as_str(),
        inventory.endpoint_location(endpoint)
      )
      .expect("writing a String succeeds");
    }
  }
  io::stdout()
    .lock()
    .write_all(output.as_bytes())
    .expect("writing package-source output to stdout succeeds");
  ExitCode::SUCCESS
}

fn project_frameworks(started: Instant, json: bool, args: Vec<String>, project_args: &[String]) -> ExitCode {
  let (requested_path, packages_directory) = match parse_pack_plan_args(project_args, "frameworks") {
    Ok(options) => options,
    Err(problem) => {
      return fail(
        started,
        json,
        "project frameworks",
        args,
        diagnostic(
          "DV0002",
          problem,
          None,
          Some("Use `dv project frameworks --help` to inspect the accepted arguments."),
        ),
      );
    },
  };
  let current_directory = match env::current_dir() {
    Ok(directory) => directory,
    Err(error) => {
      return fail(
        started,
        json,
        "project frameworks",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, requested_path.as_deref(), ProjectConfiguration::Debug) {
    Ok(project) => project,
    Err(error) => return fail(started, json, "project frameworks", args, project_diagnostic(error)),
  };
  let inventory = match discover_sdks(project.project_directory()) {
    Ok(inventory) => inventory,
    Err(error) => {
      let directory = project.project_directory();
      return fail(started, json, "project frameworks", args, sdk_diagnostic(directory, error));
    },
  };
  let plans = match plan_framework_references(&[&project], &inventory, packages_directory.as_deref()) {
    Ok(plans) => plans,
    Err(error) => return fail(started, json, "project frameworks", args, framework_reference_diagnostic(error)),
  };
  let plan = &plans[0];

  if !json {
    return write_framework_reference_plan(plan);
  }
  let frameworks = plan
    .frameworks()
    .iter()
    .map(|framework| ResolvedFrameworkReferenceEvent {
      reference: plan.reference(*framework).into(),
      runtime_name: plan.runtime_name(*framework).into(),
      requested_version: plan.requested_version(*framework).into(),
      selected_version: plan.selected_version(*framework).map(str::to_owned),
      shared_root: plan.shared_root(*framework).map(str::to_owned),
      targeting_pack_id: plan.targeting_pack_id(*framework).into(),
      targeting_pack_version: plan.targeting_pack_version(*framework).into(),
      targeting_pack_root: plan.targeting_pack_root(*framework).into(),
      profile: plan.profile(*framework).map(str::to_owned),
    })
    .collect();
  succeed(
    started,
    "project frameworks",
    args,
    EventPayload::FrameworkReferencePlanCreated {
      project: plan.project().into(),
      sdk_version: plan.sdk_version().into(),
      manifest: plan.manifest().into(),
      target_framework: plan.target_framework().into(),
      roll_forward: plan.roll_forward().as_str().into(),
      self_contained: plan.self_contained(),
      frameworks,
    },
  )
}

fn project_runtime_packs(started: Instant, json: bool, args: Vec<String>, project_args: &[String]) -> ExitCode {
  let (requested_path, packages_directory) = match parse_pack_plan_args(project_args, "runtime-packs") {
    Ok(options) => options,
    Err(problem) => {
      return fail(
        started,
        json,
        "project runtime-packs",
        args,
        diagnostic(
          "DV0002",
          problem,
          None,
          Some("Use `dv project runtime-packs --help` to inspect the accepted arguments."),
        ),
      );
    },
  };
  let current_directory = match env::current_dir() {
    Ok(directory) => directory,
    Err(error) => {
      return fail(
        started,
        json,
        "project runtime-packs",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, requested_path.as_deref(), ProjectConfiguration::Debug) {
    Ok(project) => project,
    Err(error) => return fail(started, json, "project runtime-packs", args, project_diagnostic(error)),
  };
  let inventory = match discover_sdks(project.project_directory()) {
    Ok(inventory) => inventory,
    Err(error) => {
      let directory = project.project_directory();
      return fail(started, json, "project runtime-packs", args, sdk_diagnostic(directory, error));
    },
  };
  let plan = match plan_runtime_packs(&project, &inventory, packages_directory.as_deref()) {
    Ok(plan) => plan,
    Err(error) => return fail(started, json, "project runtime-packs", args, runtime_pack_diagnostic(error)),
  };

  if !json {
    return write_runtime_pack_plan(&plan);
  }
  succeed(
    started,
    "project runtime-packs",
    args,
    EventPayload::RuntimePackPlanCreated {
      project: plan.project().into(),
      sdk_version: plan.sdk_version().into(),
      manifest: plan.manifest().into(),
      target_framework: plan.target_framework().into(),
      requested_runtime_identifier: plan.requested_runtime_identifier().into(),
      runtime_identifier: plan.runtime_identifier().into(),
      runtime_pack_id: plan.runtime_pack_id().into(),
      runtime_pack_version: plan.runtime_pack_version().into(),
      runtime_pack_root: plan.runtime_pack_root().into(),
      host_runtime_identifier: plan.host_runtime_identifier().into(),
      host_pack_id: plan.host_pack_id().into(),
      host_pack_version: plan.host_pack_version().into(),
      host_pack_root: plan.host_pack_root().into(),
      apphost_template: plan.apphost_template().into(),
      managed_assets: plan.managed_assets().map(str::to_owned).collect(),
      native_assets: plan.native_assets().map(str::to_owned).collect(),
    },
  )
}

fn parse_pack_plan_args(arguments: &[String], command: &str) -> Result<(Option<PathBuf>, Option<PathBuf>), String> {
  let mut project = None;
  let mut packages = None;
  let mut index = 0;
  while index < arguments.len() {
    match arguments[index].as_str() {
      "--packages" => {
        index += 1;
        packages = Some(PathBuf::from(arguments.get(index).ok_or("--packages requires a path")?));
      },
      value if value.starts_with('-') => return Err(format!("unknown project {command} option {value:?}")),
      value if project.is_none() => project = Some(PathBuf::from(value)),
      value => return Err(format!("unexpected project {command} argument {value:?}")),
    }
    index += 1;
  }
  Ok((project, packages))
}

fn parse_project_args(arguments: &[String]) -> Result<(Option<PathBuf>, ProjectConfiguration), String> {
  let mut project = None;
  let mut configuration = ProjectConfiguration::Debug;
  let mut index = 0;
  while index < arguments.len() {
    match arguments[index].as_str() {
      "-h" | "--help" | "help" => return Err("help must be requested as `dv project --help`".into()),
      "--configuration" => {
        index += 1;
        let value = arguments.get(index).ok_or("--configuration requires Debug or Release")?;
        configuration = ProjectConfiguration::parse(value).ok_or_else(|| format!("configuration {value:?} is unsupported"))?;
      },
      value if value.starts_with('-') => return Err(format!("unknown project option {value:?}")),
      value if project.is_none() => project = Some(PathBuf::from(value)),
      value => return Err(format!("unexpected project argument {value:?}")),
    }
    index += 1;
  }
  Ok((project, configuration))
}

fn write_compiler_plan(plan: &CompilerPlan) -> ExitCode {
  let mut output = String::with_capacity(2048);
  use std::fmt::Write as _;
  writeln!(output, "Compiler input plan").expect("writing a String succeeds");
  writeln!(output, "  Project          {}", plan.project()).expect("writing a String succeeds");
  writeln!(output, "  Configuration    {}", plan.configuration().as_str()).expect("writing a String succeeds");
  writeln!(output, "  SDK              {}", plan.sdk_version()).expect("writing a String succeeds");
  writeln!(output, "  Language         C# {}", plan.language_version()).expect("writing a String succeeds");
  writeln!(output, "  Warnings         level {}", plan.warning_level()).expect("writing a String succeeds");
  writeln!(output, "  Output kind      {}", plan.output_type().as_str()).expect("writing a String succeeds");
  writeln!(output, "  Nullable         {}", toggle_text(plan.nullable_enabled())).expect("writing a String succeeds");
  writeln!(output, "  Deterministic    {}", plan.deterministic()).expect("writing a String succeeds");
  writeln!(output, "  Framework pack   {} ({})", plan.framework_pack_version(), plan.framework_pack()).expect("writing a String succeeds");
  writeln!(output, "  Compiler         {}", plan.compiler()).expect("writing a String succeeds");
  writeln!(output).expect("writing a String succeeds");
  writeln!(output, "Inputs").expect("writing a String succeeds");
  writeln!(output, "  Sources          {:>4}", plan.sources().len()).expect("writing a String succeeds");
  writeln!(output, "  Generated        {:>4}", plan.generated_sources().len()).expect("writing a String succeeds");
  writeln!(output, "  References       {:>4}", plan.references().len()).expect("writing a String succeeds");
  writeln!(output, "  Analyzers        {:>4}", plan.analyzers().len()).expect("writing a String succeeds");
  writeln!(output, "  Analyzer configs {:>4}", plan.analyzer_configs().len()).expect("writing a String succeeds");
  writeln!(output, "  Defines          {:>4}", plan.defines().len()).expect("writing a String succeeds");
  writeln!(output).expect("writing a String succeeds");
  writeln!(output, "Outputs").expect("writing a String succeeds");
  writeln!(output, "  Assembly         {}", plan.output_assembly()).expect("writing a String succeeds");
  writeln!(output, "  Symbols          {}", plan.output_pdb()).expect("writing a String succeeds");
  writeln!(output, "  Reference        {}", plan.reference_output()).expect("writing a String succeeds");
  io::stdout()
    .lock()
    .write_all(output.as_bytes())
    .expect("writing compiler plan to stdout succeeds");
  ExitCode::SUCCESS
}

fn write_runtime_pack_plan(plan: &RuntimePackPlan) -> ExitCode {
  let mut output = String::with_capacity(1024);
  use std::fmt::Write as _;
  writeln!(output, "Runtime pack plan").expect("writing a String succeeds");
  writeln!(output, "  Project          {}", plan.project()).expect("writing a String succeeds");
  writeln!(output, "  SDK              {}", plan.sdk_version()).expect("writing a String succeeds");
  writeln!(output, "  Target           {}", plan.target_framework()).expect("writing a String succeeds");
  writeln!(output, "  Requested RID    {}", plan.requested_runtime_identifier()).expect("writing a String succeeds");
  writeln!(output, "  Runtime RID      {}", plan.runtime_identifier()).expect("writing a String succeeds");
  writeln!(output, "  Runtime pack     {} {}", plan.runtime_pack_id(), plan.runtime_pack_version()).expect("writing a String succeeds");
  writeln!(output, "  Runtime root     {}", plan.runtime_pack_root()).expect("writing a String succeeds");
  writeln!(output, "  Managed assets   {}", plan.managed_assets().len()).expect("writing a String succeeds");
  writeln!(output, "  Native assets    {}", plan.native_assets().len()).expect("writing a String succeeds");
  writeln!(output, "  Host RID         {}", plan.host_runtime_identifier()).expect("writing a String succeeds");
  writeln!(output, "  Host pack        {} {}", plan.host_pack_id(), plan.host_pack_version()).expect("writing a String succeeds");
  writeln!(output, "  Host root        {}", plan.host_pack_root()).expect("writing a String succeeds");
  writeln!(output, "  Apphost template {}", plan.apphost_template()).expect("writing a String succeeds");
  io::stdout()
    .lock()
    .write_all(output.as_bytes())
    .expect("writing runtime-pack plan to stdout succeeds");
  ExitCode::SUCCESS
}

fn write_framework_reference_plan(plan: &FrameworkReferencePlan) -> ExitCode {
  let mut output = String::with_capacity(1536);
  use std::fmt::Write as _;
  writeln!(output, "Framework reference plan").expect("writing a String succeeds");
  writeln!(output, "  Project          {}", plan.project()).expect("writing a String succeeds");
  writeln!(output, "  SDK              {}", plan.sdk_version()).expect("writing a String succeeds");
  writeln!(output, "  Target           {}", plan.target_framework()).expect("writing a String succeeds");
  writeln!(output, "  Roll forward     {}", plan.roll_forward().as_str()).expect("writing a String succeeds");
  writeln!(output, "  Self-contained   {}", plan.self_contained()).expect("writing a String succeeds");
  writeln!(output, "  Frameworks       {}", plan.frameworks().len()).expect("writing a String succeeds");
  for framework in plan.frameworks() {
    writeln!(output).expect("writing a String succeeds");
    writeln!(output, "{}", plan.reference(*framework)).expect("writing a String succeeds");
    writeln!(
      output,
      "  Runtime          {} {}",
      plan.runtime_name(*framework),
      plan.requested_version(*framework)
    )
    .expect("writing a String succeeds");
    if let Some(selected) = plan.selected_version(*framework) {
      writeln!(output, "  Selected         {selected}").expect("writing a String succeeds");
    }
    if let Some(root) = plan.shared_root(*framework) {
      writeln!(output, "  Shared root      {root}").expect("writing a String succeeds");
    }
    writeln!(
      output,
      "  Targeting pack   {} {}",
      plan.targeting_pack_id(*framework),
      plan.targeting_pack_version(*framework)
    )
    .expect("writing a String succeeds");
    writeln!(output, "  Targeting root   {}", plan.targeting_pack_root(*framework)).expect("writing a String succeeds");
    if let Some(profile) = plan.profile(*framework) {
      writeln!(output, "  Profile          {profile}").expect("writing a String succeeds");
    }
  }
  io::stdout()
    .lock()
    .write_all(output.as_bytes())
    .expect("writing framework-reference plan to stdout succeeds");
  ExitCode::SUCCESS
}

fn load_project(directory: &Path, requested: Option<&Path>, configuration: ProjectConfiguration) -> Result<ProjectSpec, ProjectError> {
  let Some(requested) = requested else {
    return evaluate_project(directory, configuration);
  };
  let requested = if requested.is_absolute() {
    requested.to_owned()
  } else {
    directory.join(requested)
  };
  if requested.is_dir() {
    evaluate_project(&requested, configuration)
  } else {
    evaluate_project_path(&requested, configuration)
  }
}

fn write_project(project: &ProjectSpec) -> ExitCode {
  let mut output = String::with_capacity(512);
  use std::fmt::Write as _;
  writeln!(output, "Project             {}", project.project_path().display()).expect("writing a String succeeds");
  writeln!(output, "SDK                 {}", project.sdk()).expect("writing a String succeeds");
  writeln!(output, "Target              {}", project.target_framework()).expect("writing a String succeeds");
  writeln!(output, "Runtime             {}", project.runtime_identifier().unwrap_or("portable")).expect("writing a String succeeds");
  writeln!(output, "Roll forward        {}", project.roll_forward().as_str()).expect("writing a String succeeds");
  writeln!(output, "Self-contained      {}", project.self_contained()).expect("writing a String succeeds");
  writeln!(output, "Runtime dimensions  {}", project.runtime_dimensions().len()).expect("writing a String succeeds");
  for runtime in project.runtime_dimensions() {
    writeln!(output, "  {runtime}").expect("writing a String succeeds");
  }
  writeln!(output, "Output              {}", project.output_type().as_str()).expect("writing a String succeeds");
  writeln!(output, "Configuration       {}", project.configuration().as_str()).expect("writing a String succeeds");
  writeln!(output, "Assembly            {}", project.assembly_name()).expect("writing a String succeeds");
  writeln!(output, "Root namespace      {}", project.root_namespace()).expect("writing a String succeeds");
  writeln!(output, "Nullable            {}", toggle_text(project.nullable_enabled())).expect("writing a String succeeds");
  writeln!(output, "Implicit usings     {}", toggle_text(project.implicit_usings_enabled())).expect("writing a String succeeds");
  writeln!(output, "Deterministic       {}", project.deterministic()).expect("writing a String succeeds");
  writeln!(output, "Sources             {}", project.sources().len()).expect("writing a String succeeds");
  for source in project.sources() {
    writeln!(output, "  {source}").expect("writing a String succeeds");
  }
  writeln!(output, "Project references  {}", project.project_references().len()).expect("writing a String succeeds");
  for reference in project.project_references() {
    writeln!(output, "  {reference}").expect("writing a String succeeds");
  }
  writeln!(output, "Package references  {}", project.package_references().len()).expect("writing a String succeeds");
  for package in project.package_references() {
    writeln!(output, "  {} {}", project.package_id(*package), project.package_version(*package)).expect("writing a String succeeds");
  }
  writeln!(output, "Framework references {}", project.framework_references().len()).expect("writing a String succeeds");
  for reference in project.framework_references() {
    writeln!(output, "  {}", project.framework_reference_id(*reference)).expect("writing a String succeeds");
  }
  io::stdout()
    .lock()
    .write_all(output.as_bytes())
    .expect("writing project output to stdout succeeds");
  ExitCode::SUCCESS
}

fn toggle_text(enabled: bool) -> &'static str {
  if enabled { "enable" } else { "disable" }
}

fn project_diagnostic(error: ProjectError) -> Diagnostic {
  let code = match error.kind() {
    ProjectErrorKind::NotFound => "DV0200",
    ProjectErrorKind::Ambiguous => "DV0201",
    ProjectErrorKind::Io => "DV0202",
    ProjectErrorKind::InvalidXml => "DV0203",
    ProjectErrorKind::Unsupported => "DV0204",
    ProjectErrorKind::InvalidProperty => "DV0205",
    ProjectErrorKind::NonUnicodePath => "DV0206",
  };
  let help = match error.kind() {
    ProjectErrorKind::NotFound => Some("Pass a path to one SDK-style C# project."),
    ProjectErrorKind::Ambiguous => Some("Pass one .csproj path explicitly."),
    ProjectErrorKind::Unsupported | ProjectErrorKind::InvalidProperty => Some("Use the supported single-target Microsoft.NET.Sdk project subset."),
    ProjectErrorKind::InvalidXml => Some("Correct the project XML and try again."),
    ProjectErrorKind::Io | ProjectErrorKind::NonUnicodePath => None,
  };
  diagnostic(
    code,
    error.to_string(),
    Some(ContextField {
      name: "path".into(),
      value: error.path().display().to_string(),
    }),
    help,
  )
}

fn compiler_plan_diagnostic(error: CompilerPlanError) -> Diagnostic {
  let code = match error.kind() {
    CompilerPlanErrorKind::PackNotFound => "DV0300",
    CompilerPlanErrorKind::InvalidManifest => "DV0301",
    CompilerPlanErrorKind::MissingAsset => "DV0302",
    CompilerPlanErrorKind::UnsupportedSdk => "DV0303",
    CompilerPlanErrorKind::Io => "DV0304",
    CompilerPlanErrorKind::NonUnicodePath => "DV0305",
    CompilerPlanErrorKind::TextOverflow => "DV0306",
    CompilerPlanErrorKind::PackageResolution => "DV0307",
  };
  let help = error
    .requirement()
    .map(|requirement| requirement.acquisition().help())
    .or_else(|| match error.kind() {
      CompilerPlanErrorKind::PackNotFound => Some("Install the targeting pack required by the project target framework."),
      CompilerPlanErrorKind::InvalidManifest | CompilerPlanErrorKind::MissingAsset => Some("Repair or reinstall the selected .NET SDK."),
      CompilerPlanErrorKind::UnsupportedSdk => Some("Install and select a stable SDK compatible with the project target framework."),
      CompilerPlanErrorKind::PackageResolution => Some("Run `dv restore` (or `dv sync`) for every package-bearing project before compiler planning."),
      CompilerPlanErrorKind::Io | CompilerPlanErrorKind::NonUnicodePath | CompilerPlanErrorKind::TextOverflow => None,
    });
  let mut diagnostic = diagnostic(
    code,
    error.to_string(),
    Some(ContextField {
      name: "path".into(),
      value: error.path().display().to_string(),
    }),
    help,
  );
  if let Some(requirement) = error.requirement() {
    append_pack_requirement(&mut diagnostic, requirement);
  }
  diagnostic
}

fn package_diagnostic(error: PackageError) -> Diagnostic {
  let code = match error.kind() {
    PackageErrorKind::Configuration => "DV0400",
    PackageErrorKind::Resolution => "DV0401",
    PackageErrorKind::Incompatible => "DV0402",
    PackageErrorKind::OfflineMiss => "DV0403",
    PackageErrorKind::Network => "DV0404",
    PackageErrorKind::Integrity => "DV0405",
    PackageErrorKind::Archive => "DV0406",
    PackageErrorKind::Io => "DV0407",
    PackageErrorKind::NonUnicodePath => "DV0408",
    PackageErrorKind::TextOverflow => "DV0409",
    PackageErrorKind::CredentialProvider => "DV0410",
    PackageErrorKind::Cancelled => "DV0411",
  };
  let help = match error.kind() {
    PackageErrorKind::OfflineMiss => Some("Populate the global package cache or rerun without --offline."),
    PackageErrorKind::Configuration => Some("Use an HTTPS NuGet v2/v3 source, a local folder source, and the supported NuGet.Config subset."),
    PackageErrorKind::Incompatible => Some("Use a package with compatible lib or ref assets and no unsupported build/runtime assets."),
    PackageErrorKind::Network => Some("Check source availability, proxy settings, and package identity/version."),
    PackageErrorKind::CredentialProvider => Some("Install a self-contained NuGet V2 credential provider and check its timeout and login policy."),
    PackageErrorKind::Cancelled => Some("Rerun the command when package authentication can complete."),
    PackageErrorKind::Integrity | PackageErrorKind::Archive => Some("Remove the corrupt cache entry and retry from a trusted source."),
    PackageErrorKind::Resolution | PackageErrorKind::Io | PackageErrorKind::NonUnicodePath | PackageErrorKind::TextOverflow => None,
  };
  diagnostic(
    code,
    error.to_string(),
    Some(ContextField {
      name: "context".into(),
      value: error.context().into(),
    }),
    help,
  )
}

fn load_sdk_inventory(started: Instant, json: bool, args: &[String]) -> Result<dv_core::SdkInventory, ExitCode> {
  let current_directory = env::current_dir().map_err(|error| {
    fail(
      started,
      json,
      "sdk",
      args.to_vec(),
      diagnostic("DV0101", format!("failed to read the current directory: {error}"), None, None),
    )
  })?;
  discover_sdks(&current_directory).map_err(|error| fail(started, json, "sdk", args.to_vec(), sdk_diagnostic(&current_directory, error)))
}

fn sdk_diagnostic(current_directory: &Path, error: SdkError) -> Diagnostic {
  let code = match error.kind() {
    SdkErrorKind::RootNotFound => "DV0100",
    SdkErrorKind::Io => "DV0101",
    SdkErrorKind::GlobalJson => "DV0102",
    SdkErrorKind::InvalidVersion => "DV0103",
    SdkErrorKind::NoCompatibleSdk => "DV0104",
  };
  let help = match error.kind() {
    SdkErrorKind::RootNotFound => Some("Install a .NET SDK or add its dotnet root to PATH."),
    SdkErrorKind::GlobalJson | SdkErrorKind::InvalidVersion => Some("Correct the nearest global.json SDK policy."),
    SdkErrorKind::NoCompatibleSdk => Some("Install a compatible SDK or adjust global.json."),
    SdkErrorKind::Io => None,
  };
  diagnostic(
    code,
    error.to_string(),
    Some(ContextField {
      name: "directory".into(),
      value: current_directory.display().to_string(),
    }),
    help,
  )
}

fn runtime_graph_diagnostic(error: RuntimeGraphError) -> Diagnostic {
  let (code, help) = match error.kind() {
    RuntimeGraphErrorKind::NotFound => ("DV0110", Some("Repair or reinstall the selected .NET SDK.")),
    RuntimeGraphErrorKind::Io => ("DV0111", None),
    RuntimeGraphErrorKind::InvalidJson | RuntimeGraphErrorKind::InvalidGraph => ("DV0112", Some("Repair or reinstall the selected .NET SDK.")),
    RuntimeGraphErrorKind::TextOverflow => ("DV0113", Some("Use an SDK with a valid bounded portable RID graph.")),
  };
  diagnostic(
    code,
    error.to_string(),
    Some(ContextField {
      name: "path".into(),
      value: error.path().display().to_string(),
    }),
    help,
  )
}

fn runtime_pack_diagnostic(error: RuntimePackError) -> Diagnostic {
  let code = match error.kind() {
    RuntimePackErrorKind::Io => "DV0120",
    RuntimePackErrorKind::InvalidManifest => "DV0121",
    RuntimePackErrorKind::RuntimeRequired => "DV0122",
    RuntimePackErrorKind::UnsupportedRuntime => "DV0123",
    RuntimePackErrorKind::PackNotFound => "DV0124",
    RuntimePackErrorKind::MissingAsset => "DV0125",
    RuntimePackErrorKind::Configuration => "DV0126",
    RuntimePackErrorKind::NonUnicodePath => "DV0127",
    RuntimePackErrorKind::TextOverflow => "DV0128",
  };
  let help = error
    .requirement()
    .map(|requirement| requirement.acquisition().help())
    .or_else(|| match error.kind() {
      RuntimePackErrorKind::Io | RuntimePackErrorKind::NonUnicodePath => None,
      RuntimePackErrorKind::InvalidManifest => Some("Repair or reinstall the selected .NET SDK or pack."),
      RuntimePackErrorKind::RuntimeRequired => Some("Set one RuntimeIdentifier in the project."),
      RuntimePackErrorKind::UnsupportedRuntime => Some("Choose a RID supported by the selected SDK's portable RID graph and pack manifest."),
      RuntimePackErrorKind::PackNotFound => Some("Restore the required pack or install the matching SDK workload."),
      RuntimePackErrorKind::MissingAsset => Some("Restore, repair, or reinstall the selected pack."),
      RuntimePackErrorKind::Configuration => Some("Correct NuGet.Config or set NUGET_PACKAGES."),
      RuntimePackErrorKind::TextOverflow => Some("Use a bounded SDK and package installation."),
    });
  let mut diagnostic = diagnostic(
    code,
    error.to_string(),
    Some(ContextField {
      name: "path".into(),
      value: error.path().display().to_string(),
    }),
    help,
  );
  if let Some(requirement) = error.requirement() {
    append_pack_requirement(&mut diagnostic, requirement);
  }
  diagnostic
}

fn framework_reference_diagnostic(error: FrameworkReferenceError) -> Diagnostic {
  let code = match error.kind() {
    FrameworkReferenceErrorKind::Io => "DV0130",
    FrameworkReferenceErrorKind::InvalidManifest => "DV0131",
    FrameworkReferenceErrorKind::UnknownFramework => "DV0132",
    FrameworkReferenceErrorKind::InvalidVersion => "DV0133",
    FrameworkReferenceErrorKind::TargetingPackNotFound => "DV0134",
    FrameworkReferenceErrorKind::SharedFrameworkNotFound => "DV0135",
    FrameworkReferenceErrorKind::Configuration => "DV0136",
    FrameworkReferenceErrorKind::NonUnicodePath => "DV0137",
    FrameworkReferenceErrorKind::TextOverflow => "DV0138",
  };
  let help = error
    .requirement()
    .map(|requirement| requirement.acquisition().help())
    .or_else(|| match error.kind() {
      FrameworkReferenceErrorKind::Io | FrameworkReferenceErrorKind::NonUnicodePath => None,
      FrameworkReferenceErrorKind::InvalidManifest => Some("Repair or reinstall the selected .NET SDK."),
      FrameworkReferenceErrorKind::UnknownFramework => Some("Choose a FrameworkReference supported by the selected SDK and target framework."),
      FrameworkReferenceErrorKind::InvalidVersion => Some("Use a valid three-part .NET runtime or targeting-pack version."),
      FrameworkReferenceErrorKind::TargetingPackNotFound => Some("Restore the required targeting pack or install the matching SDK."),
      FrameworkReferenceErrorKind::SharedFrameworkNotFound => Some("Install a compatible shared framework or adjust the project's RollForward policy."),
      FrameworkReferenceErrorKind::Configuration => Some("Correct NuGet.Config or set NUGET_PACKAGES."),
      FrameworkReferenceErrorKind::TextOverflow => Some("Use a bounded SDK and framework installation."),
    });
  let mut diagnostic = diagnostic(
    code,
    error.to_string(),
    Some(ContextField {
      name: "path".into(),
      value: error.path().display().to_string(),
    }),
    help,
  );
  if let Some(requirement) = error.requirement() {
    append_pack_requirement(&mut diagnostic, requirement);
  }
  diagnostic
}

fn append_pack_requirement(diagnostic: &mut Diagnostic, requirement: &PackRequirement) {
  diagnostic.context.push(ContextField {
    name: "pack_kind".into(),
    value: requirement.kind().as_str().into(),
  });
  diagnostic.context.push(ContextField {
    name: "pack_identity".into(),
    value: requirement.identity().into(),
  });
  if let Some(version) = requirement.version() {
    diagnostic.context.push(ContextField {
      name: "pack_version".into(),
      value: version.into(),
    });
  }
  diagnostic.context.push(ContextField {
    name: "target_framework".into(),
    value: requirement.target_framework().into(),
  });
  if let Some(runtime_identifier) = requirement.runtime_identifier() {
    diagnostic.context.push(ContextField {
      name: "runtime_identifier".into(),
      value: runtime_identifier.into(),
    });
  }
  diagnostic.context.push(ContextField {
    name: "acquisition".into(),
    value: requirement.acquisition().as_str().into(),
  });
}

fn path_text(path: &Path, meaning: &str) -> Result<String, Box<Diagnostic>> {
  path.to_str().map(str::to_owned).ok_or_else(|| {
    Box::new(diagnostic(
      "DV0105",
      format!("{meaning} path is not valid Unicode"),
      Some(ContextField {
        name: "path".into(),
        value: path.display().to_string(),
      }),
      None,
    ))
  })
}

fn optional_path_text(path: Option<&Path>, meaning: &str) -> Result<Option<String>, Box<Diagnostic>> {
  path.map(|path| path_text(path, meaning)).transpose()
}

fn succeed(started: Instant, command: &str, args: Vec<String>, payload: EventPayload) -> ExitCode {
  let elapsed_us = micros(started.elapsed());
  let events = [
    Event::new(0, 0, EventPayload::CommandStarted { command: command.into(), args }),
    Event::new(1, elapsed_us, payload),
    Event::new(
      2,
      elapsed_us,
      EventPayload::CommandFinished {
        command: command.into(),
        duration_us: elapsed_us,
        outcome: Outcome::Succeeded,
      },
    ),
  ];
  write_json_lines(&events, io::stdout().lock()).expect("writing structured output to stdout succeeds");
  ExitCode::SUCCESS
}

fn diagnostic(code: &str, message: impl Into<String>, context: Option<ContextField>, help: Option<&str>) -> Diagnostic {
  let mut diagnostic = Diagnostic::new(DiagnosticCode::parse(code).expect("static diagnostic code is valid"), Severity::Error, message);
  diagnostic.context.extend(context);
  diagnostic.help = help.map(str::to_owned);
  diagnostic
}

fn fail(started: Instant, json: bool, command: &str, args: Vec<String>, diagnostic: Diagnostic) -> ExitCode {
  let elapsed_us = micros(started.elapsed());

  if json {
    let events = [
      Event::new(0, 0, EventPayload::CommandStarted { command: command.into(), args }),
      Event::new(
        1,
        elapsed_us,
        EventPayload::Diagnostic {
          diagnostic: diagnostic.clone(),
        },
      ),
      Event::new(
        2,
        elapsed_us,
        EventPayload::CommandFinished {
          command: command.into(),
          duration_us: elapsed_us,
          outcome: Outcome::Failed,
        },
      ),
    ];
    write_json_lines(&events, io::stdout().lock()).expect("writing structured output to stdout succeeds");
  } else {
    let mut stderr = io::stderr().lock();
    writeln!(stderr, "error[{}]: {}", diagnostic.code, diagnostic.message).expect("writing diagnostics to stderr succeeds");
    for field in diagnostic.context {
      writeln!(stderr, "  {}: {}", field.name, field.value).expect("writing diagnostic context succeeds");
    }
    if let Some(help) = diagnostic.help {
      writeln!(stderr, "  help: {help}").expect("writing diagnostic help succeeds");
    }
  }

  ExitCode::from(2)
}

fn micros(duration: std::time::Duration) -> u64 {
  duration.as_micros().min(u128::from(u64::MAX)) as u64
}
