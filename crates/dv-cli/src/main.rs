use std::{
  env,
  ffi::OsStr,
  io::{self, IsTerminal, Write},
  path::{Path, PathBuf},
  process,
  time::Instant,
};

mod cancellation;
mod compatibility;
mod environment;
mod invocation;
mod output;

use environment::{ChildEnvironmentPlan, EnvironmentError};
use invocation::{COMMAND_SYNTAX_VERSION, ColorChoice, CommandArguments, CommandKind, DiagnosticVerbosity, ExitClass, InvocationBatch, InvocationOptions};
use output::redact_argument_text;

use dv_core::{
  CancellationToken, CentralPackageVersionEvent, ChildExitPolicy, ChildTermination, CompilerPlan, CompilerPlanError, CompilerPlanErrorKind,
  CompilerReferenceAliasEvent, ContentFileEvent, ContextField, Diagnostic, DiagnosticCode, DirectPackagePolicyEvent, Event, EventPayload,
  FrameworkReferenceError, FrameworkReferenceErrorKind, FrameworkReferencePlan, Outcome, PackRequirement, PackageAssetFlags, PackageError, PackageErrorKind,
  PackageHttpPolicyEvent, PackagePathPropertyEvent, PackageResolution, PackageResolveOptions, PackageServiceEndpointEvent, PackageSourceCapabilityEvent,
  PackageSourceInventory, PackageSourceWorkEvent, ProjectConfiguration, ProjectError, ProjectErrorKind, ProjectFrameworkReferenceEvent, ProjectPackageEvent,
  ProjectSpec, ResolvedFrameworkReferenceEvent, ResolvedPackageEvent, RuntimeGraphError, RuntimeGraphErrorKind, RuntimePackError, RuntimePackErrorKind,
  RuntimePackPlan, RuntimeTargetEvent, SdkError, SdkErrorKind, SdkInstallationEvent, Severity, discover_sdks, evaluate_project, evaluate_project_closure,
  evaluate_project_path, inspect_package_sources, load_portable_runtime_graph, plan_compiler_inputs_with_packages, plan_framework_references,
  plan_runtime_packs, resolve_package_inputs, resolve_package_inputs_with_runtime_graph, write_json_lines,
};

const HELP: &str = "\
dv - a fast .NET development toolchain

Usage:
  dv <command> [options]
  dv --compat dotnet|msbuild|nuget|vstest <command> [options]
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
  compat     Inspect the versioned compatibility manifest

Output:
  --json                  Emit the versioned JSON event protocol
  --verbose               Show detailed diagnostics
  --quiet                 Show errors only
  --verbosity LEVEL       quiet|minimal|normal|detailed|diagnostic
  --color | --no-color    Always or never color human diagnostics

Environment:
  DV_COLOR                auto|always|never (overrides NO_COLOR)
  DV_VERBOSITY            quiet|minimal|normal|detailed|diagnostic
  NO_COLOR                Non-empty disables color by default
  Command-line output options override environment defaults
  [env:NAME=VALUE]        Add a child process environment overlay
  -e, --environment NAME=VALUE
                          Add a highest-precedence run/test environment value
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
  dv project inspect [PROJECT|SOLUTION] [--project PATH]
                     [--configuration Debug|Release]
  dv project frameworks [PROJECT|SOLUTION] [--project PATH] [--packages PATH]
                        [--configuration Debug|Release]
  dv project runtime-packs [PROJECT|SOLUTION] [--project PATH] [--packages PATH]
                           [--configuration Debug|Release]
  dv project package-sources [PROJECT|SOLUTION] [--project PATH]
                             [-s|--source SOURCE]...
                             [--configfile PATH] [--offline] [--interactive]
                             [--configuration Debug|Release]
                             [--probe-credentials]
";

const BUILD_HELP: &str = "\
Usage:
  dv build --plan [PROJECT|SOLUTION] [--project PATH]
                  [--configuration Debug|Release]
";

const PACKAGE_HELP: &str = "\
Usage:
  dv restore [PROJECT|SOLUTION]... [--project PATH]
             [-s|--source SOURCE]... [--packages PATH]
             [--configfile PATH] [--offline] [--interactive]
             [--configuration Debug|Release]
  dv sync [PROJECT|SOLUTION]... [--project PATH]
          [-s|--source SOURCE]... [--packages PATH]
          [--configfile PATH] [--offline] [--interactive]
          [--configuration Debug|Release]
";

const COMPAT_HELP: &str = "\
Usage:
  dv compat manifest    Write the release compatibility manifest as JSON
";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
struct ExitCode(i32);

const _: () = assert!(std::mem::size_of::<ExitCode>() == 4);
const _: () = assert!(std::mem::align_of::<ExitCode>() == 4);

impl ExitCode {
  const SUCCESS: Self = Self(ExitClass::Success as i32);

  fn get(self) -> i32 {
    self.0
  }
}

impl From<u8> for ExitCode {
  fn from(code: u8) -> Self {
    Self(i32::from(code))
  }
}

impl From<i32> for ExitCode {
  fn from(code: i32) -> Self {
    Self(code)
  }
}

impl TryFrom<ChildTermination> for ExitCode {
  type Error = ChildTermination;

  fn try_from(termination: ChildTermination) -> Result<Self, Self::Error> {
    termination.exit_code().map(Self).ok_or(termination)
  }
}

fn main() {
  process::exit(run().get());
}

fn run() -> ExitCode {
  let started = Instant::now();
  let invocation = InvocationBatch::capture_process(env::args_os().skip(1));
  let request = invocation.request();
  let globals = invocation.options();
  let json = globals.json();
  let command_args = invocation.command_arguments();
  let cancellation = if command_requires_cancellation(request.command(), command_args) {
    match cancellation::install() {
      Ok(cancellation) => Some(cancellation),
      Err(problem) => {
        return reject(
          started,
          globals,
          "<initialization>",
          invocation.event_arguments(json),
          diagnostic("DV0004", problem, None, Some("Run dv in a process where it can own the Ctrl+C/SIGINT handler.")),
        );
      },
    }
  } else {
    None
  };
  if cancellation.as_ref().is_some_and(CancellationToken::is_cancelled) {
    return cancelled(started, globals, "<initialization>", invocation.event_arguments(json));
  }
  match request.command() {
    CommandKind::Help => {
      if let Some(problem) = unexpected_leaf_argument(globals, "help", command_args) {
        return reject(
          started,
          globals,
          "help",
          invocation.event_arguments(json),
          diagnostic("DV0002", problem, None, Some("Use `dv --help` without command operands.")),
        );
      }
      print!("{HELP}");
      ExitCode::SUCCESS
    },
    CommandKind::Version => {
      if let Some(problem) = unexpected_leaf_argument(globals, "version", command_args) {
        return reject(
          started,
          globals,
          "version",
          invocation.event_arguments(json),
          diagnostic("DV0002", problem, None, Some("Use `dv --version` without command operands.")),
        );
      }
      if json {
        succeed(
          started,
          "version",
          invocation.event_arguments(true),
          EventPayload::ToolVersion {
            version: env!("CARGO_PKG_VERSION").into(),
            command_syntax_version: request.syntax_version().get(),
            event_schema_version: dv_core::EVENT_SCHEMA_VERSION,
          },
        )
      } else {
        println!("dv {}", env!("CARGO_PKG_VERSION"));
        ExitCode::SUCCESS
      }
    },
    CommandKind::Sdk => run_sdk(started, globals, invocation.event_arguments(json), command_args, cancellation.as_ref()),
    CommandKind::Project => run_project(started, globals, invocation.event_arguments(json), command_args, cancellation.as_ref()),
    CommandKind::Build => run_build(started, globals, invocation.event_arguments(json), command_args, cancellation.as_ref()),
    CommandKind::Restore => {
      let command = invocation.command_text().expect("classified native commands are Unicode");
      run_package_command(started, globals, command, invocation.event_arguments(json), command_args, cancellation.as_ref())
    },
    CommandKind::Compat => run_compat(started, globals, invocation.event_arguments(json), command_args),
    CommandKind::Run | CommandKind::Test => {
      let command = invocation.command_text().expect("classified native commands are Unicode");
      unsupported_child_command(
        started,
        globals,
        command,
        invocation.event_arguments(json),
        invocation.forwarded_arguments(),
        ChildEnvironmentPlan::capture(invocation.environment_directives(), command_args),
        cancellation.as_ref().expect("run/test commands install cancellation"),
      )
    },
    CommandKind::Init
    | CommandKind::Add
    | CommandKind::Remove
    | CommandKind::Pack
    | CommandKind::Publish
    | CommandKind::DotnetList
    | CommandKind::NugetRestore
    | CommandKind::NugetPack
    | CommandKind::NugetPush
    | CommandKind::NugetList
    | CommandKind::NugetAdd
    | CommandKind::NugetRemove
    | CommandKind::NugetUpdate
    | CommandKind::MsbuildInput
    | CommandKind::VstestInput => {
      let command = invocation.command_text().expect("classified native commands are Unicode");
      if let Some(problem) = first_unsupported_option(globals, command, command_args) {
        return reject(
          started,
          globals,
          command,
          invocation.event_arguments(json),
          diagnostic(
            "DV0002",
            problem,
            None,
            Some("Remove the unsupported option or inspect the compatibility manifest."),
          ),
        );
      }
      unsupported(
        started,
        globals,
        command,
        invocation.event_arguments(json),
        diagnostic(
          "DV0003",
          format!("command {command:?} is not implemented yet"),
          Some(ContextField {
            name: "command".into(),
            value: command.into(),
          }),
          Some("Use --help to inspect the Phase 0 command surface."),
        ),
      )
    },
    CommandKind::Unknown => {
      let command = invocation.command_text().expect("classified native commands are Unicode");
      let redacted = redact_argument_text(OsStr::new(command));
      reject(
        started,
        globals,
        command,
        invocation.event_arguments(json),
        diagnostic(
          "DV0001",
          format!("unknown command {redacted:?}"),
          Some(ContextField {
            name: "command".into(),
            value: redacted.into_owned(),
          }),
          Some("Use --help to list available commands."),
        ),
      )
    },
    CommandKind::InvalidText => reject(
      started,
      globals,
      "<invalid>",
      invocation.event_arguments(json),
      diagnostic(
        "DV0002",
        "command text is not valid Unicode",
        None,
        Some("Pass the command name as valid Unicode text."),
      ),
    ),
    CommandKind::InvalidOptions => reject(
      started,
      globals,
      "<invalid>",
      invocation.event_arguments(json),
      diagnostic(
        "DV0002",
        invocation.option_error().unwrap_or("invalid global option combination"),
        None,
        Some("Use `dv --help` to inspect global output options."),
      ),
    ),
  }
}

fn command_requires_cancellation(command: CommandKind, command_args: CommandArguments<'_>) -> bool {
  let work_command = matches!(command, CommandKind::Sdk | CommandKind::Project | CommandKind::Build | CommandKind::Restore);
  (work_command && !command_help_only(command, command_args)) || matches!(command, CommandKind::Run | CommandKind::Test)
}

fn run_compat(started: Instant, globals: InvocationOptions, args: Vec<String>, command_args: CommandArguments<'_>) -> ExitCode {
  let mut semantic = command_args.iter();
  match semantic.next() {
    None => {
      print!("{COMPAT_HELP}");
      ExitCode::SUCCESS
    },
    Some(value) if matches!(value.to_str(), Some("help" | "--help" | "-h")) && semantic.next().is_none() => {
      print!("{COMPAT_HELP}");
      ExitCode::SUCCESS
    },
    Some(value) if value == "manifest" && semantic.next().is_none() => {
      let stdout = io::stdout();
      match compatibility::write_manifest(stdout.lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => reject(
          started,
          globals,
          "compat",
          args,
          diagnostic(
            "DV0006",
            format!("could not write the compatibility manifest: {error}"),
            None,
            Some("Redirect output to a writable destination or retry with an open stdout pipe."),
          ),
        ),
      }
    },
    Some(value) => reject(
      started,
      globals,
      "compat",
      args,
      diagnostic(
        "DV0002",
        format!("unknown compat query {:?}", redact_argument_text(value)),
        None,
        Some("Use `dv compat manifest` to emit the current compatibility artifact."),
      ),
    ),
  }
}

fn command_help_only(command: CommandKind, command_args: CommandArguments<'_>) -> bool {
  if command_args.is_empty() {
    return matches!(command, CommandKind::Sdk | CommandKind::Project);
  }
  let mut arguments = command_args.iter();
  if matches!(arguments.next().and_then(OsStr::to_str), Some("help" | "--help" | "-h")) {
    return arguments.next().is_none();
  }
  matches!(command, CommandKind::Project)
    && matches!(
      command_args.first().and_then(OsStr::to_str),
      Some("inspect" | "frameworks" | "runtime-packs" | "package-sources")
    )
    && matches!(command_args.get(1).and_then(OsStr::to_str), Some("help" | "--help" | "-h"))
    && command_args.len() == 2
}

fn unsupported_child_command(
  started: Instant,
  globals: InvocationOptions,
  command: &str,
  args: Vec<String>,
  forwarded_args: Option<invocation::ForwardedArguments<'_>>,
  environment: Result<ChildEnvironmentPlan<'_>, EnvironmentError>,
  cancellation: &CancellationToken,
) -> ExitCode {
  let environment = match environment {
    Ok(environment) => environment,
    Err(error) => {
      return reject(
        started,
        globals,
        command,
        args,
        diagnostic(
          "DV0002",
          error.to_string(),
          None,
          Some("Use NAME=VALUE with [env:NAME=VALUE], -e, or --environment."),
        ),
      );
    },
  };
  let mut problem = diagnostic(
    "DV0003",
    format!("command {command:?} is not implemented yet"),
    Some(ContextField {
      name: "command".into(),
      value: command.into(),
    }),
    Some("Use --help to inspect the Phase 0 command surface."),
  );
  if let Some(forwarded) = forwarded_args {
    problem.context.push(ContextField {
      name: "forwarded_argument_count".into(),
      value: forwarded.as_slice().len().to_string(),
    });
  }
  problem.context.push(ContextField {
    name: "child_exit_policy".into(),
    value: child_exit_policy(command).as_str().into(),
  });
  if environment.edit_count() != 0 {
    problem.context.push(ContextField {
      name: "environment_edit_count".into(),
      value: environment.edit_count().to_string(),
    });
    problem.context.push(ContextField {
      name: "sensitive_environment_edit_count".into(),
      value: environment.sensitive_edit_count().to_string(),
    });
  }
  problem.context.push(ContextField {
    name: "cancellation_grace_ms".into(),
    value: cancellation.child_grace().as_millis().to_string(),
  });
  unsupported(started, globals, command, args, problem)
}

fn child_exit_policy(command: &str) -> ChildExitPolicy {
  match command {
    "run" => ChildExitPolicy::Preserve,
    "test" => ChildExitPolicy::MapToCommandFailure,
    _ => unreachable!("only child commands reach the child-exit boundary"),
  }
}

fn unexpected_leaf_argument(globals: InvocationOptions, command: &str, arguments: CommandArguments<'_>) -> Option<String> {
  let mut unexpected = None;
  for argument in arguments.iter() {
    match argument.to_str() {
      _ if globals.argument_is_option(argument) => return Some(format!("unknown {command} option {:?}", redact_argument_text(argument))),
      Some(value) => {
        unexpected.get_or_insert_with(|| format!("unexpected {command} argument {:?}", redact_argument_text(OsStr::new(value))));
      },
      None => return Some(format!("unexpected non-Unicode {command} argument {:?}", redact_argument_text(argument))),
    }
  }
  unexpected
}

fn first_unsupported_option(globals: InvocationOptions, command: &str, arguments: CommandArguments<'_>) -> Option<String> {
  arguments.iter().find_map(|argument| {
    globals
      .argument_is_option(argument)
      .then(|| format!("unknown {command} option {:?}", redact_argument_text(argument)))
  })
}

fn run_package_command(
  started: Instant,
  globals: InvocationOptions,
  command: &str,
  args: Vec<String>,
  command_args: CommandArguments<'_>,
  cancellation: Option<&CancellationToken>,
) -> ExitCode {
  let json = globals.json();
  let mut semantic = command_args.iter();
  if matches!(semantic.next().and_then(|argument| argument.to_str()), Some("help" | "--help" | "-h")) && semantic.next().is_none() {
    print!("{PACKAGE_HELP}");
    return ExitCode::SUCCESS;
  }
  let cancellation = cancellation.expect("non-help package commands install cancellation");
  let options = match parse_package_args(globals, command, command_args) {
    Ok(options) => options,
    Err(problem) => {
      return reject(
        started,
        globals,
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
      return restore_fail(
        started,
        globals,
        command,
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let configuration = options.configuration.unwrap_or(ProjectConfiguration::Debug);
  let mut projects = Vec::with_capacity(options.additional_projects.len() + 1);
  let mut seen = Vec::<PathBuf>::new();
  let roots = std::iter::once(options.project.path()).chain(options.additional_projects.iter().copied().map(Some));
  for requested in roots {
    let root = match load_project(&current_directory, requested, configuration) {
      Ok(project) => project,
      Err(error) => return restore_fail(started, globals, command, args, project_diagnostic(error)),
    };
    let closure = match evaluate_project_closure(root) {
      Ok(projects) => projects,
      Err(error) => return restore_fail(started, globals, command, args, project_diagnostic(error)),
    };
    for project in closure {
      match seen.binary_search_by(|path| path.as_path().cmp(project.project_path())) {
        Ok(_) => {},
        Err(index) => {
          seen.insert(index, project.project_path().to_owned());
          projects.push(project);
        },
      }
    }
  }
  let options = match normalize_package_options(options, &current_directory, true, cancellation) {
    Ok(options) => options,
    Err(problem) => return reject(started, globals, command, args, diagnostic("DV0002", problem, None, None)),
  };
  let project_refs = projects.iter().collect::<Vec<_>>();
  let resolutions = match resolve_package_inputs(&project_refs, &options) {
    Ok(resolutions) => resolutions,
    Err(error) => return package_failure(started, globals, ExitClass::RestoreFailure, command, args, error),
  };
  if cancellation.is_cancelled() {
    return cancelled(started, globals, command, args);
  }
  let diagnostics = resolutions.iter().flat_map(package_downgrade_diagnostics).collect::<Vec<_>>();
  if !json {
    write_human_diagnostics(&diagnostics, globals);
    for (project, resolution) in projects.iter().zip(&resolutions) {
      if projects.len() > 1 {
        println!("Project {}", project.project_path().display());
      }
      let _ = write_package_resolution(resolution);
    }
    return ExitCode::SUCCESS;
  }
  if projects.len() == 1 && diagnostics.is_empty() {
    return succeed(started, command, args, package_resolution_payload(&projects[0], &resolutions[0]));
  }
  let payloads = projects
    .iter()
    .zip(&resolutions)
    .map(|(project, resolution)| package_resolution_payload(project, resolution))
    .collect();
  succeed_batch_with_diagnostics(started, globals, command, args, diagnostics, payloads)
}

fn normalize_package_options(
  options: PackageCommandOptions<'_>,
  current_directory: &Path,
  write_lock: bool,
  cancellation: &CancellationToken,
) -> Result<PackageResolveOptions, String> {
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
    cancellation: Some(cancellation.clone()),
    credential_provider_log_sink: options.interactive.then_some(write_credential_provider_log),
  })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ProjectSelection<'a> {
  #[default]
  CurrentDirectory,
  Positional(&'a Path),
  Named(&'a Path),
}

impl<'a> ProjectSelection<'a> {
  fn path(self) -> Option<&'a Path> {
    match self {
      Self::CurrentDirectory => None,
      Self::Positional(path) | Self::Named(path) => Some(path),
    }
  }

  fn select_positional(&mut self, path: &'a OsStr) -> Result<(), String> {
    if path.is_empty() {
      return Err("project or solution path must not be empty".into());
    }
    match self {
      Self::CurrentDirectory => {
        *self = Self::Positional(Path::new(path));
        Ok(())
      },
      Self::Named(_) => Err("--project cannot be combined with a positional project or solution path".into()),
      Self::Positional(_) => Err("a project or solution path cannot be specified more than once".into()),
    }
  }

  fn select_named(&mut self, path: &'a OsStr) -> Result<(), String> {
    if path.is_empty() {
      return Err("--project requires a project or solution path".into());
    }
    match self {
      Self::CurrentDirectory => {
        *self = Self::Named(Path::new(path));
        Ok(())
      },
      Self::Named(_) => Err("--project cannot be specified more than once".into()),
      Self::Positional(_) => Err("--project cannot be combined with a positional project or solution path".into()),
    }
  }

  fn is_positional(self) -> bool {
    matches!(self, Self::Positional(_))
  }
}

// One borrowed fat path plus a compact source tag; this singleton lives only
// for argument parsing and performs no path allocation in the common case.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<ProjectSelection<'_>>() == 24);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::align_of::<ProjectSelection<'_>>() == std::mem::align_of::<usize>());

struct PackageCommandOptions<'a> {
  project: ProjectSelection<'a>,
  additional_projects: Vec<&'a Path>,
  configuration: Option<ProjectConfiguration>,
  packages_directory: Option<PathBuf>,
  config_file: Option<PathBuf>,
  sources: Vec<String>,
  offline: bool,
  interactive: bool,
  probe_credentials: bool,
}

fn parse_package_args<'a>(globals: InvocationOptions, command: &str, arguments: CommandArguments<'a>) -> Result<PackageCommandOptions<'a>, String> {
  let mut options = PackageCommandOptions {
    project: ProjectSelection::default(),
    additional_projects: Vec::new(),
    configuration: None,
    packages_directory: None,
    config_file: None,
    sources: Vec::new(),
    offline: false,
    interactive: false,
    probe_credentials: false,
  };
  let mut index = 0;
  while index < arguments.len() {
    let argument = arguments.get(index).expect("bounded project option index is valid");
    match argument.to_str() {
      Some("--configuration" | "-c") if options.configuration.is_some() => return Err("--configuration cannot be specified more than once".into()),
      Some("--configuration" | "-c") => {
        let value = take_semantic_value(arguments, &mut index, false, "--configuration requires Debug or Release")?
          .to_str()
          .ok_or("--configuration requires valid Unicode text")?;
        options.configuration = Some(parse_configuration(value)?);
      },
      Some(value) if combined_option_value(value, "--configuration", Some("-c")).is_some() => {
        if options.configuration.is_some() {
          return Err("--configuration cannot be specified more than once".into());
        }
        let value = combined_option_value(value, "--configuration", Some("-c")).expect("guard accepted a combined configuration");
        options.configuration = Some(parse_configuration(value)?);
      },
      Some("--project") => {
        let path = take_project_value(globals, arguments, &mut index, false)?;
        options.project.select_named(path)?;
      },
      Some(value) if combined_option_value(value, "--project", None).is_some() => {
        let value = combined_option_value(value, "--project", None).expect("guard accepted a combined project");
        options.project.select_named(OsStr::new(value))?;
      },
      Some("--packages") if options.packages_directory.is_some() => return Err("--packages cannot be specified more than once".into()),
      Some("--packages") => {
        let path = take_semantic_value(arguments, &mut index, false, "--packages requires a path")?;
        if path.is_empty() {
          return Err("--packages requires a path".into());
        }
        options.packages_directory = Some(PathBuf::from(path));
      },
      Some(value) if combined_option_value(value, "--packages", None).is_some() => {
        if options.packages_directory.is_some() {
          return Err("--packages cannot be specified more than once".into());
        }
        let value = combined_option_value(value, "--packages", None).expect("guard accepted a combined packages path");
        if value.is_empty() {
          return Err("--packages requires a path".into());
        }
        options.packages_directory = Some(PathBuf::from(value));
      },
      Some("--configfile") if options.config_file.is_some() => return Err("--configfile cannot be specified more than once".into()),
      Some("--configfile") => {
        let path = take_semantic_value(arguments, &mut index, false, "--configfile requires a path")?;
        if path.is_empty() {
          return Err("--configfile requires a path".into());
        }
        options.config_file = Some(PathBuf::from(path));
      },
      Some(value) if combined_option_value(value, "--configfile", None).is_some() => {
        if options.config_file.is_some() {
          return Err("--configfile cannot be specified more than once".into());
        }
        let value = combined_option_value(value, "--configfile", None).expect("guard accepted a combined config path");
        if value.is_empty() {
          return Err("--configfile requires a path".into());
        }
        options.config_file = Some(PathBuf::from(value));
      },
      Some("--source" | "-s") => {
        let source = take_semantic_value(arguments, &mut index, false, "--source requires a package source")?
          .to_str()
          .ok_or("--source requires valid Unicode text")?;
        if source.is_empty() {
          return Err("--source requires a package source".into());
        }
        validate_command_source(source)?;
        options.sources.push(source.to_owned());
      },
      Some(value) if combined_option_value(value, "--source", Some("-s")).is_some() => {
        let source = combined_option_value(value, "--source", Some("-s")).expect("guard accepted a combined package source");
        if source.is_empty() {
          return Err("--source requires a package source".into());
        }
        validate_command_source(source)?;
        options.sources.push(source.to_owned());
      },
      Some("--offline") => options.offline = true,
      Some("--interactive") => options.interactive = true,
      Some("--probe-credentials") if command == "project package-sources" => options.probe_credentials = true,
      _ if globals.argument_is_option(argument) => return Err(format!("unknown {command} option {:?}", redact_argument_text(argument))),
      _ if matches!(options.project, ProjectSelection::CurrentDirectory) => options.project.select_positional(argument)?,
      _ if matches!(command, "restore" | "sync") && options.project.is_positional() => {
        if argument.is_empty() {
          return Err("project or solution path must not be empty".into());
        }
        options.additional_projects.push(Path::new(argument));
      },
      _ if matches!(options.project, ProjectSelection::Named(_)) => {
        return Err("--project cannot be combined with a positional project or solution path".into());
      },
      _ => return Err(format!("unexpected {command} argument {:?}", redact_argument_text(argument))),
    }
    index += 1;
  }
  Ok(options)
}

fn take_semantic_value<'a>(arguments: CommandArguments<'a>, index: &mut usize, ignore_plan: bool, missing: &'static str) -> Result<&'a OsStr, &'static str> {
  loop {
    *index += 1;
    let value = arguments.get(*index).ok_or(missing)?;
    if !ignore_plan || value != "--plan" {
      return Ok(value);
    }
  }
}

fn combined_option_value<'a>(argument: &'a str, long: &str, short: Option<&str>) -> Option<&'a str> {
  [Some(long), short].into_iter().flatten().find_map(|name| {
    let suffix = argument.strip_prefix(name)?;
    matches!(suffix.as_bytes().first(), Some(b'=' | b':')).then(|| &suffix[1..])
  })
}

fn parse_configuration(value: &str) -> Result<ProjectConfiguration, String> {
  ProjectConfiguration::parse(value).ok_or_else(|| format!("configuration {:?} is unsupported", redact_argument_text(OsStr::new(value))))
}

fn take_project_value<'a>(
  globals: InvocationOptions,
  arguments: CommandArguments<'a>,
  index: &mut usize,
  ignore_plan: bool,
) -> Result<&'a OsStr, &'static str> {
  const MISSING: &str = "--project requires a project or solution path";
  let value = take_semantic_value(arguments, index, ignore_plan, MISSING)?;
  if globals.argument_is_option(value) { Err(MISSING) } else { Ok(value) }
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

fn run_build(
  started: Instant,
  globals: InvocationOptions,
  args: Vec<String>,
  build_args: CommandArguments<'_>,
  cancellation: Option<&CancellationToken>,
) -> ExitCode {
  let json = globals.json();
  let mut semantic = build_args.iter();
  if matches!(semantic.next().and_then(|argument| argument.to_str()), Some("help" | "--help" | "-h")) && semantic.next().is_none() {
    print!("{BUILD_HELP}");
    return ExitCode::SUCCESS;
  }
  let cancellation = cancellation.expect("non-help build commands install cancellation");
  let plan_requested = build_args.iter().any(|argument| argument == "--plan");
  let (requested_path, configuration) = match parse_project_args(globals, build_args, true, "build") {
    Ok(options) => options,
    Err(problem) => {
      return reject(
        started,
        globals,
        "build",
        args,
        diagnostic("DV0002", problem, None, Some("Use `dv build --help` to inspect the accepted arguments.")),
      );
    },
  };
  if !plan_requested {
    return unsupported(
      started,
      globals,
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
  let current_directory = match env::current_dir() {
    Ok(directory) => directory,
    Err(error) => {
      return build_fail(
        started,
        globals,
        "build --plan",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, requested_path.path(), configuration) {
    Ok(project) => project,
    Err(error) => return build_fail(started, globals, "build --plan", args, project_diagnostic(error)),
  };
  let inventory = match discover_sdks(&current_directory) {
    Ok(inventory) => inventory,
    Err(error) => return build_fail(started, globals, "build --plan", args, sdk_diagnostic(&current_directory, error)),
  };
  let package_options = PackageResolveOptions {
    packages_directory: None,
    config_file: None,
    sources: Vec::new(),
    offline: false,
    write_lock: true,
    cancellation: Some(cancellation.clone()),
    ..PackageResolveOptions::default()
  };
  let runtime_graph = if !project.package_references().is_empty() && project.runtime_identifier().is_some() {
    match load_portable_runtime_graph(&inventory) {
      Ok(graph) => Some(graph),
      Err(error) => return build_fail(started, globals, "build --plan", args, runtime_graph_diagnostic(error)),
    }
  } else {
    None
  };
  let package_resolutions = match resolve_package_inputs_with_runtime_graph(&[&project], &package_options, runtime_graph.as_ref(), Some(&inventory)) {
    Ok(resolutions) => resolutions,
    Err(error) => return package_failure(started, globals, ExitClass::BuildFailure, "build --plan", args, error),
  };
  let plans = match plan_compiler_inputs_with_packages(&[&project], &inventory, &package_resolutions) {
    Ok(plans) => plans,
    Err(error) => return build_fail(started, globals, "build --plan", args, compiler_plan_diagnostic(error)),
  };
  if cancellation.is_cancelled() {
    return cancelled(started, globals, "build --plan", args);
  }
  let plan = &plans[0];
  let packages = &package_resolutions[0];
  if !json {
    return write_compiler_plan(plan);
  }

  let references = plan.references().map(str::to_owned).collect::<Vec<_>>();
  let reference_aliases = plan
    .reference_aliases()
    .map(|(reference_index, aliases)| CompilerReferenceAliasEvent {
      reference_index,
      reference: references[reference_index as usize].clone(),
      aliases: aliases.to_owned(),
    })
    .collect();
  let package_path_properties = packages
    .direct_policies()
    .filter_map(|policy| packages.direct_policy_path_property(policy))
    .map(|(name, value)| PackagePathPropertyEvent {
      name: name.to_owned(),
      value: value.display().to_string(),
    })
    .collect();

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
      references,
      reference_aliases,
      package_path_properties,
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
  let mut framework_rows = resolution.package_frameworks().peekable();
  let packages = resolution
    .packages()
    .iter()
    .copied()
    .enumerate()
    .map(|(index, package)| {
      let framework = framework_rows.next_if(|framework| resolution.package_framework_package(*framework) as usize == index);
      ResolvedPackageEvent {
        id: resolution.package_id(package).into(),
        version: resolution.package_version(package).into(),
        sha512: resolution.package_hash(index).into(),
        direct: resolution.package_is_direct(package),
        central_transitive: resolution.package_is_central_transitive(package),
        dependency_count: resolution.package_dependencies(package).len() as u32,
        framework_references: framework
          .map(|framework| resolution.package_framework_references(framework).map(str::to_owned).collect())
          .unwrap_or_default(),
        framework_assemblies: framework
          .map(|framework| resolution.package_framework_assemblies(framework).map(str::to_owned).collect())
          .unwrap_or_default(),
        cache_outcome: resolution.package_cache_outcome(package),
      }
    })
    .collect();
  let direct_policies = resolution
    .direct_policies()
    .map(|policy| DirectPackagePolicyEvent {
      package_index: resolution.direct_policy_package(policy),
      include_assets: package_asset_names(resolution.direct_policy_include_assets(policy)),
      private_assets: package_asset_names(resolution.direct_policy_private_assets(policy)),
      no_warn: metadata_values(resolution.direct_policy_no_warn(policy)),
      aliases: resolution.direct_policy_aliases(policy).map(str::to_owned),
      path_property: resolution.direct_policy_path_property(policy).map(|(name, value)| PackagePathPropertyEvent {
        name: name.to_owned(),
        value: value.display().to_string(),
      }),
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
    runtime_identifier: resolution.runtime_identifier().map(str::to_owned),
    source: resolution.source().into(),
    source_protocol: resolution.source_protocol().into(),
    source_work: resolution
      .source_work()
      .map(|source| PackageSourceWorkEvent {
        name: resolution.source_work_name(source).to_owned(),
        protocol: resolution.source_work_protocol(source).to_owned(),
        requests: resolution.source_work_requests(source),
        downloaded_bytes: resolution.source_work_downloaded_bytes(source),
        duration_us: resolution.source_work_duration_us(source),
      })
      .collect(),
    packages,
    direct_policies,
    compile_assets: resolution.compile_assets().map(|path| path.display().to_string()).collect(),
    runtime_assets: resolution.runtime_assets().map(|path| path.display().to_string()).collect(),
    analyzers: resolution.analyzers().map(|path| path.display().to_string()).collect(),
    resource_assets: resolution.resource_assets().map(|path| path.display().to_string()).collect(),
    content_files: resolution
      .content_files_with_metadata()
      .map(|(path, build_action, copy_to_output, flatten)| ContentFileEvent {
        path: path.display().to_string(),
        build_action: build_action.to_owned(),
        copy_to_output,
        flatten,
      })
      .collect(),
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
  let (framework_references, framework_assemblies) = resolution.package_frameworks().fold((0usize, 0usize), |counts, framework| {
    (
      counts.0 + resolution.package_framework_references(framework).len(),
      counts.1 + resolution.package_framework_assemblies(framework).len(),
    )
  });
  writeln!(output, "Package resolution").expect("writing a String succeeds");
  writeln!(output, "  Packages       {}", resolution.packages().len()).expect("writing a String succeeds");
  writeln!(output, "  Cache hits     {}", resolution.cache_hits()).expect("writing a String succeeds");
  writeln!(output, "  Downloaded     {}", resolution.downloaded_packages()).expect("writing a String succeeds");
  writeln!(output, "  HTTP requests  {}", resolution.network_requests()).expect("writing a String succeeds");
  writeln!(output, "  Source bytes   {}", resolution.downloaded_bytes()).expect("writing a String succeeds");
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
  writeln!(output, "  Framework refs  {framework_references}").expect("writing a String succeeds");
  writeln!(output, "  Framework asm   {framework_assemblies}").expect("writing a String succeeds");
  writeln!(output, "  Target         {}", resolution.target_framework()).expect("writing a String succeeds");
  writeln!(output, "  Source         {} ({})", resolution.source(), resolution.source_protocol()).expect("writing a String succeeds");
  for source in resolution.source_work() {
    writeln!(
      output,
      "  Source work    {} ({}) requests={} bytes={} duration={}us",
      resolution.source_work_name(source),
      resolution.source_work_protocol(source),
      resolution.source_work_requests(source),
      resolution.source_work_downloaded_bytes(source),
      resolution.source_work_duration_us(source)
    )
    .expect("writing package source work succeeds");
  }
  writeln!(output, "  Cache          {}", resolution.cache_root().display()).expect("writing a String succeeds");
  writeln!(output, "  Lock           {}", resolution.lock_path().display()).expect("writing a String succeeds");
  for package in resolution.packages().iter().copied() {
    let role = if resolution.package_is_direct(package) {
      " (direct)"
    } else if resolution.package_is_central_transitive(package) {
      " (central transitive)"
    } else {
      ""
    };
    writeln!(output, "  {} {}{}", resolution.package_id(package), resolution.package_version(package), role).expect("writing a String succeeds");
  }
  io::stdout()
    .lock()
    .write_all(output.as_bytes())
    .expect("writing package resolution to stdout succeeds");
  ExitCode::SUCCESS
}

fn run_sdk(
  started: Instant,
  globals: InvocationOptions,
  args: Vec<String>,
  sdk_args: CommandArguments<'_>,
  cancellation: Option<&CancellationToken>,
) -> ExitCode {
  let request = match parse_sdk_request(globals, sdk_args) {
    Ok(request) => request,
    Err(diagnostic) => return reject(started, globals, "sdk", args, *diagnostic),
  };
  match request {
    SdkRequest::Help => {
      print!("{SDK_HELP}");
      ExitCode::SUCCESS
    },
    SdkRequest::Current => sdk_current(started, globals, args, cancellation.expect("SDK current installs cancellation")),
    SdkRequest::List => sdk_list(started, globals, args, cancellation.expect("SDK list installs cancellation")),
    SdkRequest::CompatibleRids(runtime_identifier) => sdk_compatible_rids(
      started,
      globals,
      args,
      runtime_identifier,
      cancellation.expect("SDK RID expansion installs cancellation"),
    ),
  }
}

// A parsed SDK invocation is a true singleton; the RID remains borrowed from
// the process-lifetime argument batch and successful parsing allocates nothing.
#[derive(Clone, Copy)]
enum SdkRequest<'a> {
  Help,
  Current,
  List,
  CompatibleRids(&'a str),
}

fn parse_sdk_request(globals: InvocationOptions, arguments: CommandArguments<'_>) -> Result<SdkRequest<'_>, Box<Diagnostic>> {
  let Some(command_argument) = arguments.first() else {
    return Ok(SdkRequest::Help);
  };
  let command = command_argument
    .to_str()
    .ok_or_else(|| Box::new(non_unicode_argument_diagnostic(command_argument, "SDK command")))?;
  if matches!(command, "help" | "--help" | "-h") {
    return match unexpected_leaf_argument(globals, "sdk help", arguments.slice_from(1)) {
      None => Ok(SdkRequest::Help),
      Some(problem) => Err(Box::new(diagnostic(
        "DV0002",
        problem,
        None,
        Some("Use `dv sdk --help` without additional operands."),
      ))),
    };
  }
  if globals.argument_is_option(command_argument) {
    return Err(unknown_sdk_option_diagnostic("sdk", command_argument));
  }

  match command {
    "current" | "list" => {
      let request = if command == "current" { SdkRequest::Current } else { SdkRequest::List };
      let mut unexpected = None;
      for argument in arguments.slice_from(1).iter() {
        if globals.argument_is_option(argument) {
          return Err(unknown_sdk_option_diagnostic(&format!("sdk {command}"), argument));
        }
        let value = argument
          .to_str()
          .ok_or_else(|| Box::new(non_unicode_argument_diagnostic(argument, "SDK argument")))?;
        unexpected.get_or_insert(value);
      }
      match unexpected {
        None => Ok(request),
        Some(value) => Err(Box::new(diagnostic(
          "DV0002",
          format!("sdk {command} does not accept argument {:?}", redact_argument_text(OsStr::new(value))),
          None,
          Some("Use `dv sdk --help` to inspect the accepted arguments."),
        ))),
      }
    },
    "compatible-rids" => {
      for argument in arguments.slice_from(1).iter() {
        if globals.argument_is_option(argument) {
          return Err(unknown_sdk_option_diagnostic("sdk compatible-rids", argument));
        }
        argument
          .to_str()
          .ok_or_else(|| Box::new(non_unicode_argument_diagnostic(argument, "runtime identifier")))?;
      }
      if arguments.len() != 2 {
        return Err(Box::new(diagnostic(
          "DV0002",
          "sdk compatible-rids requires exactly one runtime identifier",
          None,
          Some("Use `dv sdk compatible-rids RID`."),
        )));
      }
      let runtime_identifier = arguments.get(1).and_then(OsStr::to_str).expect("compatible RID was validated as Unicode");
      Ok(SdkRequest::CompatibleRids(runtime_identifier))
    },
    _ => {
      let command = redact_argument_text(OsStr::new(command));
      Err(Box::new(diagnostic(
        "DV0001",
        format!("unknown sdk command {command:?}"),
        Some(ContextField {
          name: "command".into(),
          value: format!("sdk {command}"),
        }),
        Some("Use `dv sdk --help` to list SDK commands."),
      )))
    },
  }
}

fn unknown_sdk_option_diagnostic(command: &str, option: &OsStr) -> Box<Diagnostic> {
  let option = redact_argument_text(option);
  Box::new(diagnostic(
    "DV0002",
    format!("unknown {command} option {option:?}"),
    Some(ContextField {
      name: "option".into(),
      value: option.into_owned(),
    }),
    Some("Use `dv sdk --help` to inspect the accepted arguments."),
  ))
}

fn sdk_current(started: Instant, globals: InvocationOptions, args: Vec<String>, cancellation: &CancellationToken) -> ExitCode {
  let json = globals.json();
  let inventory = match load_sdk_inventory(started, globals, &args) {
    Ok(inventory) => inventory,
    Err(exit_code) => return exit_code,
  };
  let selected = inventory.selected();
  if cancellation.is_cancelled() {
    return cancelled(started, globals, "sdk current", args);
  }

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
      return fail(started, globals, "sdk current", args, *diagnostic);
    },
  };
  succeed(started, "sdk current", args, payload)
}

fn sdk_list(started: Instant, globals: InvocationOptions, args: Vec<String>, cancellation: &CancellationToken) -> ExitCode {
  let json = globals.json();
  let inventory = match load_sdk_inventory(started, globals, &args) {
    Ok(inventory) => inventory,
    Err(exit_code) => return exit_code,
  };
  if cancellation.is_cancelled() {
    return cancelled(started, globals, "sdk list", args);
  }

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
    Err(diagnostic) => return fail(started, globals, "sdk list", args, *diagnostic),
  };
  let global_json = match optional_path_text(inventory.global_json.as_deref(), "global.json") {
    Ok(path) => path,
    Err(diagnostic) => return fail(started, globals, "sdk list", args, *diagnostic),
  };
  succeed(started, "sdk list", args, EventPayload::SdkInventory { installations, global_json })
}

fn sdk_compatible_rids(
  started: Instant,
  globals: InvocationOptions,
  args: Vec<String>,
  runtime_identifier: &str,
  cancellation: &CancellationToken,
) -> ExitCode {
  let json = globals.json();
  if runtime_identifier.is_empty() {
    return reject(
      started,
      globals,
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
  let inventory = match load_sdk_inventory(started, globals, &args) {
    Ok(inventory) => inventory,
    Err(exit_code) => return exit_code,
  };
  let graph = match load_portable_runtime_graph(&inventory) {
    Ok(graph) => graph,
    Err(error) => return fail(started, globals, "sdk compatible-rids", args, runtime_graph_diagnostic(error)),
  };
  if cancellation.is_cancelled() {
    return cancelled(started, globals, "sdk compatible-rids", args);
  }

  if !json {
    for compatible in graph.compatible_rids(runtime_identifier) {
      println!("{compatible}");
    }
    return ExitCode::SUCCESS;
  }

  let graph_path = match path_text(graph.source(), "portable RID graph") {
    Ok(path) => path,
    Err(diagnostic) => return fail(started, globals, "sdk compatible-rids", args, *diagnostic),
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

fn run_project(
  started: Instant,
  globals: InvocationOptions,
  args: Vec<String>,
  project_args: CommandArguments<'_>,
  cancellation: Option<&CancellationToken>,
) -> ExitCode {
  let json = globals.json();
  if project_args.is_empty() {
    print!("{PROJECT_HELP}");
    return ExitCode::SUCCESS;
  }
  let subcommand_argument = &project_args[0];
  let subcommand = match subcommand_argument.to_str() {
    Some(value) => value,
    None => {
      return reject(
        started,
        globals,
        "project",
        args,
        non_unicode_argument_diagnostic(&project_args[0], "project command"),
      );
    },
  };
  if globals.argument_is_option(subcommand_argument) && !matches!(subcommand, "--help" | "-h") {
    let subcommand = redact_argument_text(subcommand_argument);
    return reject(
      started,
      globals,
      "project",
      args,
      diagnostic(
        "DV0002",
        format!("unknown project option {subcommand:?}"),
        None,
        Some("Use `dv project --help` to inspect the accepted arguments."),
      ),
    );
  }
  let operands = project_args.slice_from(1);
  if matches!(subcommand, "help" | "--help" | "-h") {
    if let Some(problem) = unexpected_leaf_argument(globals, "project help", operands) {
      return reject(
        started,
        globals,
        "project help",
        args,
        diagnostic("DV0002", problem, None, Some("Use `dv project --help` without additional operands.")),
      );
    }
    print!("{PROJECT_HELP}");
    return ExitCode::SUCCESS;
  }
  let mut semantic_operands = operands.iter();
  if matches!(subcommand, "inspect" | "frameworks" | "runtime-packs" | "package-sources")
    && matches!(semantic_operands.next().and_then(|argument| argument.to_str()), Some("help" | "--help" | "-h"))
    && semantic_operands.next().is_none()
  {
    print!("{PROJECT_HELP}");
    return ExitCode::SUCCESS;
  }
  let cancellation = cancellation.expect("non-help project commands install cancellation");
  if subcommand == "runtime-packs" {
    return project_runtime_packs(started, globals, args, operands, cancellation);
  }
  if subcommand == "frameworks" {
    return project_frameworks(started, globals, args, operands, cancellation);
  }
  if subcommand == "package-sources" {
    return project_package_sources(started, globals, args, operands, cancellation);
  }
  if subcommand != "inspect" {
    let subcommand = redact_argument_text(OsStr::new(subcommand));
    return reject(
      started,
      globals,
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

  let (requested_path, configuration) = match parse_project_args(globals, operands, false, "project inspect") {
    Ok(options) => options,
    Err(problem) => {
      return reject(
        started,
        globals,
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
        globals,
        "project inspect",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, requested_path.path(), configuration) {
    Ok(project) => project,
    Err(error) => return fail(started, globals, "project inspect", args, project_diagnostic(error)),
  };
  if cancellation.is_cancelled() {
    return cancelled(started, globals, "project inspect", args);
  }

  if !json {
    return write_project(&project);
  }

  let packages = project
    .package_references()
    .iter()
    .map(|package| ProjectPackageEvent {
      id: project.package_id(*package).into(),
      version: project.package_version(*package).into(),
      include_assets: package_asset_names(project.package_include_assets(*package)),
      exclude_assets: package_asset_names(project.package_exclude_assets(*package)),
      private_assets: package_asset_names(project.package_private_assets(*package)),
      no_warn: metadata_values(project.package_no_warn(*package)),
      aliases: project.package_aliases(*package).map(str::to_owned),
      generate_path_property: project.package_generate_path_property(*package),
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
    central_package_management: project.central_package_management_enabled(),
    central_transitive_pinning: project.central_package_transitive_pinning_enabled(),
    central_package_versions: project
      .central_package_versions()
      .iter()
      .map(|package| CentralPackageVersionEvent {
        id: project.central_package_id(*package).to_owned(),
        version: project.central_package_version(*package).to_owned(),
      })
      .collect(),
    framework_references: frameworks,
    runtime_framework_version: project.runtime_framework_version().map(str::to_owned),
    target_latest_runtime_patch: project.target_latest_runtime_patch(),
    roll_forward: project.roll_forward().as_str().into(),
    self_contained: project.self_contained(),
  };
  succeed(started, "project inspect", args, payload)
}

fn project_package_sources(
  started: Instant,
  globals: InvocationOptions,
  args: Vec<String>,
  project_args: CommandArguments<'_>,
  cancellation: &CancellationToken,
) -> ExitCode {
  let json = globals.json();
  let parsed = match parse_package_args(globals, "project package-sources", project_args) {
    Ok(options) => options,
    Err(problem) => {
      return reject(
        started,
        globals,
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
        globals,
        "project package-sources",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let configuration = parsed.configuration.unwrap_or(ProjectConfiguration::Debug);
  let project = match load_project(&current_directory, parsed.project.path(), configuration) {
    Ok(project) => project,
    Err(error) => return fail(started, globals, "project package-sources", args, project_diagnostic(error)),
  };
  let options = match normalize_package_options(parsed, &current_directory, false, cancellation) {
    Ok(options) => options,
    Err(problem) => return reject(started, globals, "project package-sources", args, diagnostic("DV0002", problem, None, None)),
  };
  let inventories = match inspect_package_sources(&[&project], &options) {
    Ok(inventories) => inventories,
    Err(error) => return package_failure(started, globals, ExitClass::Operation, "project package-sources", args, error),
  };
  if cancellation.is_cancelled() {
    return cancelled(started, globals, "project package-sources", args);
  }
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
      requests: inventory.source_requests(source),
      downloaded_bytes: inventory.source_downloaded_bytes(source),
      duration_us: inventory.source_duration_us(source),
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

fn project_frameworks(
  started: Instant,
  globals: InvocationOptions,
  args: Vec<String>,
  project_args: CommandArguments<'_>,
  cancellation: &CancellationToken,
) -> ExitCode {
  let json = globals.json();
  let (requested_path, packages_directory, configuration) = match parse_pack_plan_args(globals, project_args, "frameworks") {
    Ok(options) => options,
    Err(problem) => {
      return reject(
        started,
        globals,
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
        globals,
        "project frameworks",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, requested_path.path(), configuration) {
    Ok(project) => project,
    Err(error) => return fail(started, globals, "project frameworks", args, project_diagnostic(error)),
  };
  let inventory = match discover_sdks(project.project_directory()) {
    Ok(inventory) => inventory,
    Err(error) => {
      let directory = project.project_directory();
      return fail(started, globals, "project frameworks", args, sdk_diagnostic(directory, error));
    },
  };
  let plans = match plan_framework_references(&[&project], &inventory, packages_directory.as_deref()) {
    Ok(plans) => plans,
    Err(error) => return fail(started, globals, "project frameworks", args, framework_reference_diagnostic(error)),
  };
  if cancellation.is_cancelled() {
    return cancelled(started, globals, "project frameworks", args);
  }
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

fn project_runtime_packs(
  started: Instant,
  globals: InvocationOptions,
  args: Vec<String>,
  project_args: CommandArguments<'_>,
  cancellation: &CancellationToken,
) -> ExitCode {
  let json = globals.json();
  let (requested_path, packages_directory, configuration) = match parse_pack_plan_args(globals, project_args, "runtime-packs") {
    Ok(options) => options,
    Err(problem) => {
      return reject(
        started,
        globals,
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
        globals,
        "project runtime-packs",
        args,
        diagnostic("DV0202", format!("failed to read the current directory: {error}"), None, None),
      );
    },
  };
  let project = match load_project(&current_directory, requested_path.path(), configuration) {
    Ok(project) => project,
    Err(error) => return fail(started, globals, "project runtime-packs", args, project_diagnostic(error)),
  };
  let inventory = match discover_sdks(project.project_directory()) {
    Ok(inventory) => inventory,
    Err(error) => {
      let directory = project.project_directory();
      return fail(started, globals, "project runtime-packs", args, sdk_diagnostic(directory, error));
    },
  };
  let plan = match plan_runtime_packs(&project, &inventory, packages_directory.as_deref()) {
    Ok(plan) => plan,
    Err(error) => return fail(started, globals, "project runtime-packs", args, runtime_pack_diagnostic(error)),
  };
  if cancellation.is_cancelled() {
    return cancelled(started, globals, "project runtime-packs", args);
  }

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

fn parse_pack_plan_args<'a>(
  globals: InvocationOptions,
  arguments: CommandArguments<'a>,
  command: &str,
) -> Result<(ProjectSelection<'a>, Option<PathBuf>, ProjectConfiguration), String> {
  let mut project = ProjectSelection::default();
  let mut packages = None;
  let mut configuration = None;
  let mut index = 0;
  while index < arguments.len() {
    let argument = arguments.get(index).expect("bounded project option index is valid");
    match argument.to_str() {
      Some("--packages") if packages.is_some() => return Err("--packages cannot be specified more than once".into()),
      Some("--packages") => {
        let path = take_semantic_value(arguments, &mut index, false, "--packages requires a path")?;
        if path.is_empty() {
          return Err("--packages requires a path".into());
        }
        packages = Some(PathBuf::from(path));
      },
      Some(value) if combined_option_value(value, "--packages", None).is_some() => {
        if packages.is_some() {
          return Err("--packages cannot be specified more than once".into());
        }
        let value = combined_option_value(value, "--packages", None).expect("guard accepted a combined packages path");
        if value.is_empty() {
          return Err("--packages requires a path".into());
        }
        packages = Some(PathBuf::from(value));
      },
      Some("--project") => {
        let path = take_project_value(globals, arguments, &mut index, false)?;
        project.select_named(path)?;
      },
      Some(value) if combined_option_value(value, "--project", None).is_some() => {
        let value = combined_option_value(value, "--project", None).expect("guard accepted a combined project");
        project.select_named(OsStr::new(value))?;
      },
      Some("--configuration" | "-c") if configuration.is_some() => return Err("--configuration cannot be specified more than once".into()),
      Some("--configuration" | "-c") => {
        let value = take_semantic_value(arguments, &mut index, false, "--configuration requires Debug or Release")?
          .to_str()
          .ok_or("--configuration requires valid Unicode text")?;
        configuration = Some(parse_configuration(value)?);
      },
      Some(value) if combined_option_value(value, "--configuration", Some("-c")).is_some() => {
        if configuration.is_some() {
          return Err("--configuration cannot be specified more than once".into());
        }
        let value = combined_option_value(value, "--configuration", Some("-c")).expect("guard accepted a combined configuration");
        configuration = Some(parse_configuration(value)?);
      },
      _ if globals.argument_is_option(argument) => {
        return Err(format!("unknown project {command} option {:?}", redact_argument_text(argument)));
      },
      _ if matches!(project, ProjectSelection::CurrentDirectory) => project.select_positional(argument)?,
      _ if matches!(project, ProjectSelection::Named(_)) => {
        return Err("--project cannot be combined with a positional project or solution path".into());
      },
      _ => return Err(format!("unexpected project {command} argument {:?}", redact_argument_text(argument))),
    }
    index += 1;
  }
  Ok((project, packages, configuration.unwrap_or(ProjectConfiguration::Debug)))
}

fn parse_project_args<'a>(
  globals: InvocationOptions,
  arguments: CommandArguments<'a>,
  ignore_plan: bool,
  command: &str,
) -> Result<(ProjectSelection<'a>, ProjectConfiguration), String> {
  let mut project = ProjectSelection::default();
  let mut configuration = None;
  let mut index = 0;
  while index < arguments.len() {
    let argument = arguments.get(index).expect("bounded project option index is valid");
    match argument.to_str() {
      Some("--plan") if ignore_plan => {},
      Some("-h" | "--help" | "help") => return Err(format!("help must be requested as `dv {command} --help`")),
      Some("--project") => {
        let path = take_project_value(globals, arguments, &mut index, ignore_plan)?;
        project.select_named(path)?;
      },
      Some(value) if combined_option_value(value, "--project", None).is_some() => {
        let value = combined_option_value(value, "--project", None).expect("guard accepted a combined project");
        project.select_named(OsStr::new(value))?;
      },
      Some("--configuration" | "-c") if configuration.is_some() => return Err("--configuration cannot be specified more than once".into()),
      Some("--configuration" | "-c") => {
        let value = take_semantic_value(arguments, &mut index, ignore_plan, "--configuration requires Debug or Release")?
          .to_str()
          .ok_or("--configuration requires valid Unicode text")?;
        configuration = Some(parse_configuration(value)?);
      },
      Some(value) if combined_option_value(value, "--configuration", Some("-c")).is_some() => {
        if configuration.is_some() {
          return Err("--configuration cannot be specified more than once".into());
        }
        let value = combined_option_value(value, "--configuration", Some("-c")).expect("guard accepted a combined configuration");
        configuration = Some(parse_configuration(value)?);
      },
      _ if globals.argument_is_option(argument) => return Err(format!("unknown {command} option {:?}", redact_argument_text(argument))),
      _ if matches!(project, ProjectSelection::CurrentDirectory) => project.select_positional(argument)?,
      _ if matches!(project, ProjectSelection::Named(_)) => {
        return Err("--project cannot be combined with a positional project or solution path".into());
      },
      _ => return Err(format!("unexpected {command} argument {:?}", redact_argument_text(argument))),
    }
    index += 1;
  }
  Ok((project, configuration.unwrap_or(ProjectConfiguration::Debug)))
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
  writeln!(output, "Central packages     {}", project.central_package_management_enabled()).expect("writing a String succeeds");
  writeln!(output, "Transitive pinning   {}", project.central_package_transitive_pinning_enabled()).expect("writing a String succeeds");
  for package in project.central_package_versions() {
    writeln!(
      output,
      "  {} {}",
      project.central_package_id(*package),
      project.central_package_version(*package)
    )
    .expect("writing a String succeeds");
  }
  writeln!(output, "Package references  {}", project.package_references().len()).expect("writing a String succeeds");
  for package in project.package_references() {
    writeln!(output, "  {} {}", project.package_id(*package), project.package_version(*package)).expect("writing a String succeeds");
    writeln!(
      output,
      "    include={} exclude={} private={}",
      package_asset_names(project.package_include_assets(*package)).join(";"),
      package_asset_names(project.package_exclude_assets(*package)).join(";"),
      package_asset_names(project.package_private_assets(*package)).join(";")
    )
    .expect("writing a String succeeds");
    if let Some(no_warn) = project.package_no_warn(*package) {
      writeln!(output, "    no-warn={no_warn}").expect("writing a String succeeds");
    }
    if let Some(aliases) = project.package_aliases(*package) {
      writeln!(output, "    aliases={aliases}").expect("writing a String succeeds");
    }
    if project.package_generate_path_property(*package) {
      writeln!(output, "    generate-path-property=true").expect("writing a String succeeds");
    }
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

fn package_asset_names(flags: PackageAssetFlags) -> Vec<String> {
  [
    (PackageAssetFlags::RUNTIME, "runtime"),
    (PackageAssetFlags::COMPILE, "compile"),
    (PackageAssetFlags::BUILD, "build"),
    (PackageAssetFlags::BUILD_MULTI_TARGETING, "buildMultitargeting"),
    (PackageAssetFlags::BUILD_TRANSITIVE, "buildTransitive"),
    (PackageAssetFlags::NATIVE, "native"),
    (PackageAssetFlags::CONTENT_FILES, "contentFiles"),
    (PackageAssetFlags::ANALYZERS, "analyzers"),
  ]
  .into_iter()
  .filter(|(flag, _)| flags.contains(*flag))
  .map(|(_, name)| name.to_owned())
  .collect()
}

fn metadata_values(value: Option<&str>) -> Vec<String> {
  value
    .into_iter()
    .flat_map(|value| value.split([',', ';']))
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(str::to_owned)
    .collect()
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
    PackageErrorKind::ConstraintConflict => "DV0414",
    PackageErrorKind::Downgrade => "DV0413",
    PackageErrorKind::DependencyCycle => "DV0415",
    PackageErrorKind::PackageNotFound => "DV0416",
    PackageErrorKind::VersionNotFound => "DV0417",
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
    PackageErrorKind::UnmappedIdentity => "DV0412",
  };
  let help = match error.kind() {
    PackageErrorKind::OfflineMiss => Some("Populate the global package cache or rerun without --offline."),
    PackageErrorKind::Configuration => Some("Use an HTTPS NuGet v2/v3 source, a local folder source, and the supported NuGet.Config subset."),
    PackageErrorKind::Incompatible => Some("Use a package with compatible lib or ref assets and no unsupported build/runtime assets."),
    PackageErrorKind::ConstraintConflict => Some("Reference the package directly with one version that satisfies the dependency graph."),
    PackageErrorKind::Downgrade => Some("Raise the central package version or disable central transitive pinning for this package."),
    PackageErrorKind::DependencyCycle => Some("Correct the circular dependency in the package metadata or contact the package owner."),
    PackageErrorKind::PackageNotFound => Some("Check the package identity and the enabled package sources."),
    PackageErrorKind::VersionNotFound => Some("Choose an available package version or correct the dependency range."),
    PackageErrorKind::Network => Some("Check source availability, proxy settings, and package identity/version."),
    PackageErrorKind::CredentialProvider => Some("Install a self-contained NuGet V2 credential provider and check its timeout and login policy."),
    PackageErrorKind::Cancelled => Some("Rerun the command when package authentication can complete."),
    PackageErrorKind::UnmappedIdentity => Some("Add a matching packageSourceMapping pattern for this package identity and an enabled source."),
    PackageErrorKind::Integrity | PackageErrorKind::Archive => Some("Remove the corrupt cache entry and retry from a trusted source."),
    PackageErrorKind::Resolution | PackageErrorKind::Io | PackageErrorKind::NonUnicodePath | PackageErrorKind::TextOverflow => None,
  };
  let mut typed_context = error.diagnostic_context().peekable();
  let fallback_context = typed_context.peek().is_none().then(|| ContextField {
    name: if error.kind() == PackageErrorKind::UnmappedIdentity {
      "package_id".into()
    } else {
      "context".into()
    },
    value: error.context().into(),
  });
  let mut result = diagnostic(code, error.to_string(), fallback_context, help);
  result.context.extend(typed_context.map(|(name, value)| ContextField {
    name: name.into(),
    value: value.into(),
  }));
  result.causes.extend(error.causes().map(str::to_owned));
  result
}

fn package_downgrade_diagnostics(resolution: &PackageResolution) -> Vec<Diagnostic> {
  resolution
    .downgrades()
    .map(|warning| {
      let package_id = resolution.downgrade_package_id(warning);
      let selected = resolution.downgrade_selected_version(warning);
      let requested = resolution.downgrade_requested_range(warning);
      let requester = resolution.downgrade_requesting_package(warning);
      let mut diagnostic = Diagnostic::new(
        DiagnosticCode::parse("DV0413").expect("static diagnostic code is valid"),
        Severity::Warning,
        format!("detected package downgrade: {package_id} resolved to {selected} instead of {requested}"),
      );
      diagnostic.context.extend([
        ContextField {
          name: "package_id".into(),
          value: package_id.into(),
        },
        ContextField {
          name: "selected_version".into(),
          value: selected.into(),
        },
        ContextField {
          name: "required_range".into(),
          value: requested.into(),
        },
        ContextField {
          name: "requesting_package".into(),
          value: requester.into(),
        },
      ]);
      diagnostic.causes.push(format!("{requester} requires {package_id} {requested}"));
      diagnostic.help = Some(format!("Reference {package_id} directly at a compatible higher version."));
      diagnostic
    })
    .collect()
}

fn load_sdk_inventory(started: Instant, globals: InvocationOptions, args: &[String]) -> Result<dv_core::SdkInventory, ExitCode> {
  let current_directory = env::current_dir().map_err(|error| {
    fail(
      started,
      globals,
      "sdk",
      args.to_vec(),
      diagnostic("DV0101", format!("failed to read the current directory: {error}"), None, None),
    )
  })?;
  discover_sdks(&current_directory).map_err(|error| fail(started, globals, "sdk", args.to_vec(), sdk_diagnostic(&current_directory, error)))
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

fn command_started(command: &str, args: Vec<String>) -> EventPayload {
  EventPayload::CommandStarted {
    command_syntax_version: COMMAND_SYNTAX_VERSION.get(),
    command: command.into(),
    args,
  }
}

fn succeed(started: Instant, command: &str, args: Vec<String>, payload: EventPayload) -> ExitCode {
  let elapsed_us = micros(started.elapsed());
  let events = [
    Event::new(0, 0, command_started(command, args)),
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

fn succeed_batch_with_diagnostics(
  started: Instant,
  globals: InvocationOptions,
  command: &str,
  args: Vec<String>,
  diagnostics: Vec<Diagnostic>,
  payloads: Vec<EventPayload>,
) -> ExitCode {
  let elapsed_us = micros(started.elapsed());
  let mut events = Vec::with_capacity(diagnostics.len() + payloads.len() + 2);
  events.push(Event::new(0, 0, command_started(command, args)));
  for diagnostic in diagnostics
    .into_iter()
    .filter(|diagnostic| diagnostic_visible(diagnostic.severity, globals.verbosity()))
  {
    events.push(Event::new(events.len() as u64, elapsed_us, EventPayload::Diagnostic { diagnostic }));
  }
  for payload in payloads {
    events.push(Event::new(events.len() as u64, elapsed_us, payload));
  }
  events.push(Event::new(
    events.len() as u64,
    elapsed_us,
    EventPayload::CommandFinished {
      command: command.into(),
      duration_us: elapsed_us,
      outcome: Outcome::Succeeded,
    },
  ));
  write_json_lines(&events, io::stdout().lock()).expect("writing structured output to stdout succeeds");
  ExitCode::SUCCESS
}

fn write_human_diagnostics(diagnostics: &[Diagnostic], globals: InvocationOptions) {
  let mut stderr = io::stderr().lock();
  for diagnostic in diagnostics {
    if !diagnostic_visible(diagnostic.severity, globals.verbosity()) {
      continue;
    }
    let severity = match diagnostic.severity {
      Severity::Error => "error",
      Severity::Warning => "warning",
      Severity::Info => "info",
    };
    let color = match diagnostic.severity {
      Severity::Error => 31,
      Severity::Warning => 33,
      Severity::Info => 36,
    };
    let use_color = matches!(globals.color(), ColorChoice::Always) || (matches!(globals.color(), ColorChoice::Auto) && stderr.is_terminal());
    if use_color {
      writeln!(stderr, "\x1b[{color}m{severity}[{}]: {}\x1b[0m", diagnostic.code, diagnostic.message).expect("writing colored diagnostics to stderr succeeds");
    } else {
      writeln!(stderr, "{severity}[{}]: {}", diagnostic.code, diagnostic.message).expect("writing diagnostics to stderr succeeds");
    }
    for field in &diagnostic.context {
      writeln!(stderr, "  {}: {}", field.name, field.value).expect("writing diagnostic context succeeds");
    }
    for cause in &diagnostic.causes {
      writeln!(stderr, "  caused by: {cause}").expect("writing diagnostic cause succeeds");
    }
    if let Some(help) = &diagnostic.help {
      writeln!(stderr, "  help: {help}").expect("writing diagnostic help succeeds");
    }
  }
}

fn diagnostic_visible(severity: Severity, verbosity: DiagnosticVerbosity) -> bool {
  match severity {
    Severity::Error => true,
    Severity::Warning => verbosity >= DiagnosticVerbosity::Minimal,
    Severity::Info => verbosity >= DiagnosticVerbosity::Detailed,
  }
}

fn diagnostic(code: &str, message: impl Into<String>, context: Option<ContextField>, help: Option<&str>) -> Diagnostic {
  let mut diagnostic = Diagnostic::new(DiagnosticCode::parse(code).expect("static diagnostic code is valid"), Severity::Error, message);
  diagnostic.context.extend(context);
  diagnostic.help = help.map(str::to_owned);
  diagnostic
}

fn non_unicode_argument_diagnostic(argument: &OsStr, meaning: &str) -> Diagnostic {
  diagnostic(
    "DV0002",
    format!("{meaning} must be valid Unicode text"),
    Some(ContextField {
      name: "argument".into(),
      value: redact_argument_text(argument).into_owned(),
    }),
    Some("Use lossless OS paths only in command positions documented as paths."),
  )
}

fn reject(started: Instant, globals: InvocationOptions, command: &str, args: Vec<String>, diagnostic: Diagnostic) -> ExitCode {
  fail_with_class(started, globals, ExitClass::Usage, command, args, diagnostic)
}

fn unsupported(started: Instant, globals: InvocationOptions, command: &str, args: Vec<String>, diagnostic: Diagnostic) -> ExitCode {
  fail_with_class(started, globals, ExitClass::Unsupported, command, args, diagnostic)
}

fn fail(started: Instant, globals: InvocationOptions, command: &str, args: Vec<String>, diagnostic: Diagnostic) -> ExitCode {
  fail_with_class(started, globals, ExitClass::Operation, command, args, diagnostic)
}

fn build_fail(started: Instant, globals: InvocationOptions, command: &str, args: Vec<String>, diagnostic: Diagnostic) -> ExitCode {
  fail_with_class(started, globals, ExitClass::BuildFailure, command, args, diagnostic)
}

fn restore_fail(started: Instant, globals: InvocationOptions, command: &str, args: Vec<String>, diagnostic: Diagnostic) -> ExitCode {
  fail_with_class(started, globals, ExitClass::RestoreFailure, command, args, diagnostic)
}

fn package_failure(started: Instant, globals: InvocationOptions, class: ExitClass, command: &str, args: Vec<String>, error: PackageError) -> ExitCode {
  if error.kind() == PackageErrorKind::Cancelled {
    cancelled(started, globals, command, args)
  } else {
    fail_with_class(started, globals, class, command, args, package_diagnostic(error))
  }
}

fn cancelled(started: Instant, globals: InvocationOptions, command: &str, args: Vec<String>) -> ExitCode {
  fail_with_outcome(
    started,
    globals,
    ExitClass::Cancelled,
    command,
    args,
    diagnostic(
      "DV0005",
      "command was cancelled",
      None,
      Some("Rerun the command when the interrupted work can complete."),
    ),
    Outcome::Cancelled,
  )
}

fn fail_with_class(started: Instant, globals: InvocationOptions, class: ExitClass, command: &str, args: Vec<String>, diagnostic: Diagnostic) -> ExitCode {
  fail_with_outcome(started, globals, class, command, args, diagnostic, Outcome::Failed)
}

fn fail_with_outcome(
  started: Instant,
  globals: InvocationOptions,
  class: ExitClass,
  command: &str,
  args: Vec<String>,
  mut diagnostic: Diagnostic,
  outcome: Outcome,
) -> ExitCode {
  let elapsed_us = micros(started.elapsed());
  let json = globals.json();
  if let Some(profile) = globals.compatibility_profile() {
    // Error wire fields own their text; successful dispatch never pays this allocation.
    diagnostic.context.push(ContextField {
      name: "compatibility_profile".into(),
      value: profile.into(),
    });
  }

  if json {
    let events = [
      Event::new(0, 0, command_started(command, args)),
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
          outcome,
        },
      ),
    ];
    write_json_lines(&events, io::stdout().lock()).expect("writing structured output to stdout succeeds");
  } else {
    write_human_diagnostics(std::slice::from_ref(&diagnostic), globals);
  }

  ExitCode::from(globals.exit_code(class).expect("the routed command owns its terminal outcome class"))
}

fn micros(duration: std::time::Duration) -> u64 {
  duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod argument_tests {
  use super::*;
  use std::ffi::OsString;

  #[cfg(windows)]
  fn non_unicode_path() -> OsString {
    use std::os::windows::ffi::OsStringExt;

    OsString::from_wide(&[b'p' as u16, 0xd800])
  }

  #[cfg(unix)]
  fn non_unicode_path() -> OsString {
    use std::os::unix::ffi::OsStringExt;

    OsString::from_vec(vec![b'p', 0x80])
  }

  #[test]
  fn project_path_operands_retain_their_os_encoding() {
    let path = non_unicode_path();
    let batch = InvocationBatch::capture([OsString::from("project"), path.clone()]);
    let (parsed, configuration) = parse_project_args(batch.options(), batch.command_arguments(), false, "project inspect").unwrap();

    assert_eq!(parsed.path().map(Path::as_os_str), Some(path.as_os_str()));
    assert_eq!(configuration, ProjectConfiguration::Debug);
  }

  #[test]
  fn named_project_paths_retain_their_os_encoding() {
    let path = non_unicode_path();
    let batch = InvocationBatch::capture([OsString::from("project"), OsString::from("--project"), path.clone()]);
    let (parsed, _) = parse_project_args(batch.options(), batch.command_arguments(), false, "project inspect").unwrap();

    assert!(matches!(parsed, ProjectSelection::Named(_)));
    assert_eq!(parsed.path().map(Path::as_os_str), Some(path.as_os_str()));

    let batch = InvocationBatch::capture([OsString::from("frameworks"), OsString::from("--project"), path.clone()]);
    let (parsed, _, _) = parse_pack_plan_args(batch.options(), batch.command_arguments(), "frameworks").unwrap();
    assert_eq!(parsed.path().map(Path::as_os_str), Some(path.as_os_str()));

    let batch = InvocationBatch::capture([OsString::from("restore"), OsString::from("--project"), path.clone()]);
    let parsed = parse_package_args(batch.options(), "restore", batch.command_arguments()).unwrap();
    assert_eq!(parsed.project.path().map(Path::as_os_str), Some(path.as_os_str()));
  }

  #[test]
  fn project_selection_rejects_repeated_mixed_and_empty_paths() {
    for arguments in [
      vec!["project", "--project", "App.csproj", "--project", "Other.csproj"],
      vec!["project", "App.csproj", "--project", "Other.csproj"],
      vec!["project", "--project", "App.csproj", "Other.csproj"],
      vec!["project", "--project="],
      vec!["project", "--project", "--configuration", "Release"],
    ] {
      let batch = InvocationBatch::capture(arguments.into_iter().map(OsString::from));
      assert!(parse_project_args(batch.options(), batch.command_arguments(), false, "project inspect").is_err());
    }
  }

  #[test]
  fn restore_keeps_positional_batches_but_named_selection_is_singular() {
    let batch = InvocationBatch::capture(["restore", "App.csproj", "Library.csproj"].map(OsString::from));
    let parsed = parse_package_args(batch.options(), "restore", batch.command_arguments()).unwrap();
    assert_eq!(parsed.project.path(), Some(Path::new("App.csproj")));
    assert_eq!(parsed.additional_projects, [Path::new("Library.csproj")]);

    let batch = InvocationBatch::capture(["restore", "--project", "App.csproj", "Library.csproj"].map(OsString::from));
    assert!(parse_package_args(batch.options(), "restore", batch.command_arguments()).is_err());
  }

  #[test]
  fn option_path_values_retain_their_os_encoding() {
    let path = non_unicode_path();
    let batch = InvocationBatch::capture([
      OsString::from("restore"),
      OsString::from("--packages"),
      path.clone(),
      OsString::from("App.csproj"),
    ]);
    let parsed = parse_package_args(batch.options(), "restore", batch.command_arguments()).unwrap();

    assert_eq!(parsed.packages_directory, Some(PathBuf::from(path)));
  }

  #[test]
  fn global_json_does_not_become_an_option_value() {
    let batch = InvocationBatch::capture(["restore", "--packages", "--json", "packages"].map(OsString::from));
    let parsed = parse_package_args(batch.options(), "restore", batch.command_arguments()).unwrap();

    assert_eq!(parsed.packages_directory, Some(PathBuf::from("packages")));
  }

  #[test]
  fn build_plan_marker_does_not_become_an_option_value() {
    let batch = InvocationBatch::capture(["build", "--configuration", "--plan", "Release"].map(OsString::from));
    let (_, configuration) = parse_project_args(batch.options(), batch.command_arguments(), true, "build").unwrap();

    assert_eq!(configuration, ProjectConfiguration::Release);
  }

  #[test]
  fn configuration_forms_match_dotnet_case_and_separator_rules() {
    for option in ["--configuration=Release", "--configuration:Release", "-c=Release", "-c:Release"] {
      let batch = InvocationBatch::capture(["--compat", "dotnet", "build", option].map(OsString::from));
      let (_, configuration) = parse_project_args(batch.options(), batch.command_arguments(), true, "build").unwrap();
      assert_eq!(configuration, ProjectConfiguration::Release, "{option}");
    }

    for option in ["--Configuration=Release", "-C:Release", "--configurationRelease"] {
      let batch = InvocationBatch::capture(["--compat", "dotnet", "build", option].map(OsString::from));
      assert!(
        parse_project_args(batch.options(), batch.command_arguments(), true, "build").is_err(),
        "{option}"
      );
    }
  }

  #[test]
  fn singleton_options_reject_mixed_repetitions_before_project_io() {
    for arguments in [
      ["build", "--configuration", "Debug", "-c:Release"],
      ["build", "-c=Debug", "--configuration:Release", ""],
    ] {
      let batch = InvocationBatch::capture(arguments.map(OsString::from));
      assert!(parse_project_args(batch.options(), batch.command_arguments(), true, "build").is_err());
    }

    let batch = InvocationBatch::capture(["restore", "--packages:first", "--packages=second"].map(OsString::from));
    assert!(parse_package_args(batch.options(), "restore", batch.command_arguments()).is_err());

    let batch = InvocationBatch::capture(["frameworks", "--packages", ""].map(OsString::from));
    assert!(parse_pack_plan_args(batch.options(), batch.command_arguments(), "frameworks").is_err());
  }

  #[test]
  fn repeatable_sources_preserve_combined_value_order() {
    let batch = InvocationBatch::capture(["restore", "--source:first", "-s=second", "--source", "third", "App.csproj"].map(OsString::from));
    let parsed = parse_package_args(batch.options(), "restore", batch.command_arguments()).unwrap();

    assert_eq!(parsed.sources, ["first", "second", "third"]);
  }

  #[test]
  fn diagnostic_verbosity_has_explicit_severity_boundaries() {
    assert!(diagnostic_visible(Severity::Error, DiagnosticVerbosity::Quiet));
    assert!(!diagnostic_visible(Severity::Warning, DiagnosticVerbosity::Quiet));
    assert!(diagnostic_visible(Severity::Warning, DiagnosticVerbosity::Minimal));
    assert!(!diagnostic_visible(Severity::Info, DiagnosticVerbosity::Normal));
    assert!(diagnostic_visible(Severity::Info, DiagnosticVerbosity::Detailed));
  }

  #[test]
  fn numeric_child_exit_bypasses_failure_code_remapping() {
    for code in [0, 37, 211, -2_147_450_751] {
      assert_eq!(ExitCode::try_from(ChildTermination::Exited(code)).unwrap().get(), code);
    }
    assert_eq!(ExitCode::try_from(ChildTermination::Signalled(15)), Err(ChildTermination::Signalled(15)));
    assert_eq!(ExitCode::try_from(ChildTermination::Unknown), Err(ChildTermination::Unknown));
  }

  #[test]
  fn only_work_bearing_commands_install_cancellation() {
    fn requires(arguments: &[&str]) -> bool {
      let batch = InvocationBatch::capture(arguments.iter().map(OsString::from));
      command_requires_cancellation(batch.request().command(), batch.command_arguments())
    }

    for arguments in [
      &["sdk", "current"][..],
      &["project", "inspect", "App.csproj"],
      &["build", "--plan", "App.csproj"],
      &["restore", "App.csproj"],
      &["sync", "App.csproj"],
      &["run"],
      &["test"],
    ] {
      assert!(requires(arguments), "{arguments:?}");
    }
    for arguments in [
      &["--help"][..],
      &["--version"],
      &["sdk"],
      &["sdk", "--help"],
      &["project"],
      &["project", "inspect", "--help"],
      &["unknown"],
      &["publish"],
    ] {
      assert!(!requires(arguments), "{arguments:?}");
    }
  }
}
