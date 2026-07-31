use std::{
  env,
  ffi::OsString,
  io::{self, Write},
  path::{Path, PathBuf},
  process::ExitCode,
  time::Instant,
};

use dv_core::{
  ContextField, Diagnostic, DiagnosticCode, Event, EventPayload, Outcome, ProjectConfiguration, ProjectError, ProjectErrorKind, ProjectPackageEvent,
  ProjectSpec, SdkError, SdkErrorKind, SdkInstallationEvent, Severity, discover_sdks, evaluate_project, evaluate_project_path, write_json_lines,
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
  sync       Resolve and cache dependencies
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
";

const PROJECT_HELP: &str = "\
Usage:
  dv project inspect [PROJECT] [--configuration Debug|Release]
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

fn decode_args(raw_args: &[OsString]) -> Result<Vec<String>, &OsString> {
  raw_args.iter().map(|argument| argument.to_str().map(str::to_owned).ok_or(argument)).collect()
}

fn is_known_command(command: &str) -> bool {
  matches!(
    command,
    "init" | "add" | "remove" | "sync" | "build" | "run" | "test" | "pack" | "publish" | "sdk" | "project"
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

fn run_project(started: Instant, json: bool, args: Vec<String>, project_args: &[String]) -> ExitCode {
  if project_args.is_empty()
    || matches!(project_args, [argument] if matches!(argument.as_str(), "help" | "--help" | "-h"))
    || matches!(project_args, [inspect, argument] if inspect == "inspect" && matches!(argument.as_str(), "help" | "--help" | "-h"))
  {
    print!("{PROJECT_HELP}");
    return ExitCode::SUCCESS;
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

  let (requested_path, configuration) = match parse_project_inspect_args(&project_args[1..]) {
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
  let payload = EventPayload::ProjectEvaluated {
    project: project.project_path().display().to_string(),
    sdk: project.sdk().into(),
    target_framework: project.target_framework().into(),
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
  };
  succeed(started, "project inspect", args, payload)
}

fn parse_project_inspect_args(arguments: &[String]) -> Result<(Option<PathBuf>, ProjectConfiguration), String> {
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
