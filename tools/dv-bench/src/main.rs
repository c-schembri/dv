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
  RestoreCold,
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

const DOTNET_CASES: &[Case] = &[
  Case {
    name: "sdk_current",
    kind: CaseKind::Startup,
    args: &["--version"],
    implemented: true,
  },
  Case {
    name: "restore_cold",
    kind: CaseKind::RestoreCold,
    args: &["restore", "--nologo", "--verbosity", "quiet"],
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
    name: "cli_version",
    kind: CaseKind::Startup,
    args: &["--version"],
    implemented: true,
  },
  Case {
    name: "sync_cold",
    kind: CaseKind::RestoreCold,
    args: &["sync"],
    implemented: false,
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
  let workspace = repository.join("target/benchmark-work");
  let dv_executable = prepare_dv_executable(&repository, options.dv.as_deref())?;
  ensure_workspace_is_safe(&repository, &workspace)?;
  verify_sdk_selection(&dv_executable, &fixture)?;

  let mut runs = run_tool("dotnet", Path::new("dotnet"), DOTNET_CASES, &options, &fixture, &workspace.join("dotnet"))?;
  runs.extend(run_tool("dv", &dv_executable, DV_CASES, &options, &fixture, &workspace.join("dv"))?);
  if runs.is_empty() {
    return Err(format!("no benchmark case named {:?}", options.case.as_deref().unwrap_or_default()).into());
  }

  let generated_unix_seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
  let report = Report {
    schema_version: 2,
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

fn run_tool(tool_name: &str, executable: &Path, cases: &[Case], options: &Options, fixture: &Path, workspace: &Path) -> Result<Vec<Run>> {
  let version = command_text(executable, &["--version"], fixture)?;
  let mut runs = Vec::with_capacity(cases.len());

  for case in cases.iter().filter(|case| options.case.as_deref().is_none_or(|name| name == case.name)) {
    let case_workspace = workspace.join(case.name);
    let command: Vec<String> = std::iter::once(executable.display().to_string())
      .chain(case.args.iter().map(|value| (*value).into()))
      .collect();

    if !case.implemented {
      runs.push(Run {
        tool: tool_name.into(),
        tool_version: version.clone(),
        fixture: (!matches!(case.kind, CaseKind::Startup)).then(|| "small-console".into()),
        case: case.name.into(),
        command,
        status: RunStatus::Tbi,
        warmups: 0,
        samples_ns: Vec::new(),
        statistics_ns: None,
      });
      continue;
    }

    prepare_persistent_case(executable, case, fixture, &case_workspace)?;

    let mut samples_ns = Vec::with_capacity(options.samples);
    let total = options.warmups + options.samples;
    for index in 0..total {
      prepare_iteration(executable, case, fixture, &case_workspace)?;
      let elapsed_ns = measure(executable, case.args, case_cwd(case, fixture, &case_workspace))?;
      if index >= options.warmups {
        samples_ns.push(elapsed_ns);
      }
    }

    let statistics_ns = statistics(&samples_ns);
    runs.push(Run {
      tool: tool_name.into(),
      tool_version: version.clone(),
      fixture: (!matches!(case.kind, CaseKind::Startup)).then(|| "small-console".into()),
      case: case.name.into(),
      command,
      status: RunStatus::Measured,
      warmups: options.warmups,
      samples_ns,
      statistics_ns: Some(statistics_ns),
    });
  }

  Ok(runs)
}

fn prepare_persistent_case(executable: &Path, case: &Case, fixture: &Path, workspace: &Path) -> Result<()> {
  if matches!(case.kind, CaseKind::BuildNoOp | CaseKind::RunWarm) {
    reset_fixture(fixture, workspace)?;
    run_checked(executable, build_args(executable), workspace, "persistent case setup")?;
  }
  Ok(())
}

fn prepare_iteration(executable: &Path, case: &Case, fixture: &Path, workspace: &Path) -> Result<()> {
  match case.kind {
    CaseKind::RestoreCold => reset_fixture(fixture, workspace),
    CaseKind::BuildClean => {
      reset_fixture(fixture, workspace)?;
      run_checked(executable, restore_args(executable), workspace, "clean build restore")
    },
    CaseKind::Startup | CaseKind::BuildNoOp | CaseKind::RunWarm => Ok(()),
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

fn measure(executable: &Path, args: &[&str], cwd: &Path) -> Result<u64> {
  let started = Instant::now();
  let output = Command::new(executable).args(args).current_dir(cwd).output()?;
  let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
  check_output(output, executable, args, "measured command")?;
  Ok(elapsed)
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
    "restore_cold" => "Cold restore",
    "sync_cold" => "Cold sync",
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
      schema_version: 2,
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
}
