use std::{
  collections::{BTreeSet, VecDeque},
  env,
  error::Error,
  ffi::{OsStr, OsString},
  fs,
  io::{Read, Write},
  path::{Path, PathBuf},
  process::{Command, ExitStatus, Stdio},
  thread,
  time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MANIFEST_SCHEMA_VERSION: u16 = 1;
const COMMAND_SYNTAX_VERSION: u16 = 1;
const MAX_COMMANDS: usize = 512;
const MAX_COMMAND_DEPTH: usize = 4;
const MAX_STREAM_BYTES: usize = 1024 * 1024;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Serialize, Deserialize)]
struct CompatibilityManifest {
  schema_version: u16,
  manifest_version: u16,
  command_syntax_version: u16,
  event_schema_version: u16,
  reference: ReferenceSet,
  capture: CaptureEvidence,
  environment_inputs: Vec<EnvironmentInput>,
  exit_cases: Vec<ExitCase>,
  output_formats: Vec<OutputFormat>,
  commands: Vec<CommandSurface>,
  parity_rows: Vec<ParityRow>,
}

#[derive(Serialize, Deserialize)]
struct ReferenceSet {
  dotnet_sdk: String,
  msbuild: String,
  nuget: String,
  vstest: String,
}

#[derive(Serialize, Deserialize)]
struct CaptureEvidence {
  os: String,
  arch: String,
  executable: String,
  max_command_depth: usize,
  max_commands: usize,
  max_stream_bytes: usize,
  process_timeout_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct EnvironmentInput {
  name: String,
  tool: String,
  role: String,
}

#[derive(Serialize, Deserialize)]
struct ExitCase {
  tool: String,
  argv: Vec<String>,
  exit_code: i32,
}

#[derive(Serialize, Deserialize)]
struct OutputFormat {
  tool: String,
  name: String,
  selectors: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct CommandSurface {
  tool: String,
  path: Vec<String>,
  probe_argv: Vec<String>,
  help_exit_code: i32,
  support: SupportState,
  canonical_path: Option<Vec<String>>,
  dimensions: SupportDimensions,
  parity_rows: Vec<String>,
  usage: Vec<String>,
  arguments: Vec<ArgumentSurface>,
  options: Vec<OptionSurface>,
  children: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SupportState {
  Implemented,
  Partial,
  Missing,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct SupportDimensions {
  options: SupportState,
  arguments: SupportState,
  defaults: SupportState,
  environment: SupportState,
  exits: SupportState,
  outputs: SupportState,
}

#[derive(Deserialize)]
struct SupportSource {
  schema_version: u16,
  manifest_version: u16,
  dv_command_syntax_version: u16,
  dv_event_schema_version: u16,
  commands: Vec<SupportCommand>,
}

#[derive(Deserialize)]
struct SupportCommand {
  tool: String,
  path: Vec<String>,
  canonical_path: Vec<String>,
  status: SupportState,
  dimensions: SupportDimensions,
  parity_rows: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct ArgumentSurface {
  position: usize,
  syntax: String,
  description: String,
}

#[derive(Serialize, Deserialize)]
struct OptionSurface {
  syntax: String,
  spellings: Vec<String>,
  value: Option<String>,
  default: Option<String>,
  description: String,
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ParityRow {
  id: String,
  status: ParityStatus,
  summary: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ParityStatus {
  Implemented,
  Partial,
  Missing,
}

struct CaptureOptions {
  dotnet: PathBuf,
  expected_sdk: String,
  support: PathBuf,
  parity_map: PathBuf,
  output: PathBuf,
}

struct ProcessOutput {
  status: ExitStatus,
  stdout: Vec<u8>,
  stderr: Vec<u8>,
}

struct StreamCapture {
  bytes: Vec<u8>,
  exceeded: bool,
}

#[derive(Default)]
struct ParsedHelp {
  usage: Vec<String>,
  arguments: Vec<ArgumentSurface>,
  options: Vec<OptionSurface>,
  children: Vec<String>,
}

fn main() {
  if let Err(error) = run() {
    eprintln!("compatibility manifest failed: {error}");
    std::process::exit(1);
  }
}

fn run() -> Result<()> {
  let mut arguments = env::args_os().skip(1);
  match arguments.next().as_deref().and_then(OsStr::to_str) {
    Some("capture") => capture(parse_capture_options(arguments)?)?,
    Some("check") => {
      let path = arguments.next().ok_or("check requires a manifest path")?;
      if arguments.next().is_some() {
        return Err("check accepts exactly one manifest path".into());
      }
      let manifest = read_manifest(Path::new(&path))?;
      validate_manifest(&manifest)?;
      println!(
        "manifest v{}: {} commands, {} options, {} parity rows",
        manifest.manifest_version,
        manifest.commands.len(),
        manifest.commands.iter().map(|command| command.options.len()).sum::<usize>(),
        manifest.parity_rows.len()
      );
    },
    _ => {
      return Err(
        "usage: dv-compat-manifest capture --dotnet PATH --expected-sdk VERSION --support PATH --parity-map PATH --output PATH | check MANIFEST".into(),
      );
    },
  }
  Ok(())
}

fn parse_capture_options(arguments: impl Iterator<Item = OsString>) -> Result<CaptureOptions> {
  let mut dotnet = None;
  let mut expected_sdk = None;
  let mut support = None;
  let mut parity_map = None;
  let mut output = None;
  let mut arguments = arguments;
  while let Some(option) = arguments.next() {
    let value = arguments.next().ok_or_else(|| format!("{} requires a value", option.to_string_lossy()))?;
    match option.to_str() {
      Some("--dotnet") if dotnet.is_none() => dotnet = Some(PathBuf::from(value)),
      Some("--expected-sdk") if expected_sdk.is_none() => {
        expected_sdk = Some(value.into_string().map_err(|_| "--expected-sdk must be Unicode")?);
      },
      Some("--support") if support.is_none() => support = Some(PathBuf::from(value)),
      Some("--parity-map") if parity_map.is_none() => parity_map = Some(PathBuf::from(value)),
      Some("--output") if output.is_none() => output = Some(PathBuf::from(value)),
      Some(name) => return Err(format!("unknown or repeated capture option {name}").into()),
      None => return Err("capture option names must be Unicode".into()),
    }
  }
  Ok(CaptureOptions {
    dotnet: dotnet.ok_or("capture requires --dotnet")?,
    expected_sdk: expected_sdk.ok_or("capture requires --expected-sdk")?,
    support: support.ok_or("capture requires --support")?,
    parity_map: parity_map.ok_or("capture requires --parity-map")?,
    output: output.ok_or("capture requires --output")?,
  })
}

fn capture(options: CaptureOptions) -> Result<()> {
  let dotnet_sdk = probe_version(&options.dotnet, &["--version"], VersionShape::Exact)?;
  if dotnet_sdk != options.expected_sdk {
    return Err(format!("selected SDK is {dotnet_sdk}, expected {}", options.expected_sdk).into());
  }
  let reference = ReferenceSet {
    dotnet_sdk,
    msbuild: probe_version(&options.dotnet, &["msbuild", "-version"], VersionShape::NumericLine)?,
    nuget: probe_version(&options.dotnet, &["nuget", "--version"], VersionShape::NumericLine)?,
    vstest: probe_version(&options.dotnet, &["vstest", "--help"], VersionShape::Prefix("VSTest version"))?,
  };
  let support: SupportSource = serde_json::from_slice(&fs::read(&options.support)?)?;
  validate_support_source(&support)?;
  let commands = capture_commands(&options.dotnet, &support)?;
  let exit_cases = capture_exit_cases(&options.dotnet)?;
  let parity_rows = parse_parity_map(&fs::read_to_string(&options.parity_map)?)?;
  let manifest = CompatibilityManifest {
    schema_version: MANIFEST_SCHEMA_VERSION,
    manifest_version: support.manifest_version,
    command_syntax_version: COMMAND_SYNTAX_VERSION,
    event_schema_version: support.dv_event_schema_version,
    reference,
    capture: CaptureEvidence {
      os: env::consts::OS.into(),
      arch: env::consts::ARCH.into(),
      executable: options
        .dotnet
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("reference executable name must be Unicode")?
        .into(),
      max_command_depth: MAX_COMMAND_DEPTH,
      max_commands: MAX_COMMANDS,
      max_stream_bytes: MAX_STREAM_BYTES,
      process_timeout_ms: PROCESS_TIMEOUT.as_millis() as u64,
    },
    environment_inputs: environment_inputs(),
    exit_cases,
    output_formats: output_formats(),
    commands,
    parity_rows,
  };
  validate_manifest(&manifest)?;
  let mut encoded = serde_json::to_vec(&manifest)?;
  encoded.push(b'\n');
  write_new_file(&options.output, &encoded)?;
  Ok(())
}

enum VersionShape {
  Exact,
  NumericLine,
  Prefix(&'static str),
}

fn probe_version(dotnet: &Path, arguments: &[&str], shape: VersionShape) -> Result<String> {
  let output = run_bounded(dotnet, arguments)?;
  if !output.status.success() {
    return Err(format!("{} exited with {:?}", command_text(dotnet, arguments), output.status.code()).into());
  }
  let text = combined_text(&output)?;
  let selected = match shape {
    VersionShape::Exact => text.lines().map(str::trim).find(|line| !line.is_empty()),
    VersionShape::NumericLine => text
      .lines()
      .map(str::trim)
      .find(|line| !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.') && line.bytes().any(|byte| byte == b'.')),
    VersionShape::Prefix(prefix) => text
      .lines()
      .map(str::trim)
      .find_map(|line| line.strip_prefix(prefix).map(str::trim))
      .and_then(|version| version.split_ascii_whitespace().next()),
  };
  selected
    .map(str::to_owned)
    .ok_or_else(|| format!("{} returned no matching version text", command_text(dotnet, arguments)).into())
}

fn capture_commands(dotnet: &Path, support: &SupportSource) -> Result<Vec<CommandSurface>> {
  let mut queued = BTreeSet::new();
  let mut queue = VecDeque::new();
  queued.insert(Vec::<String>::new());
  queue.push_back(Vec::<String>::new());
  let mut commands = Vec::new();
  while let Some(path) = queue.pop_front() {
    if commands.len() >= MAX_COMMANDS {
      return Err(format!("command surface exceeds the {MAX_COMMANDS}-command bound").into());
    }
    let mut probe = path.clone();
    probe.push(if path.first().is_some_and(|command| command == "msbuild") {
      "-help".into()
    } else {
      "--help".into()
    });
    let output = run_bounded(dotnet, &probe)?;
    let help = normalize_capture_working_directory(combined_text(&output)?)?;
    let mut parsed = parse_help(&help);
    if path.first().is_some_and(|command| command == "msbuild") {
      expand_msbuild_spellings(&mut parsed.options);
    }
    if path.len() < MAX_COMMAND_DEPTH {
      for child in &parsed.children {
        let mut child_path = path.clone();
        child_path.push(child.clone());
        if queued.insert(child_path.clone()) {
          queue.push_back(child_path);
        }
      }
    } else if !parsed.children.is_empty() {
      return Err(format!("command path {:?} exceeds the depth-{MAX_COMMAND_DEPTH} capture bound", path).into());
    }
    let declared = support
      .commands
      .iter()
      .find(|command| command.tool == command_tool(&path) && command.path == path);
    commands.push(CommandSurface {
      tool: command_tool(&path).into(),
      support: declared.map_or(SupportState::Missing, |command| command.status),
      canonical_path: declared.map(|command| command.canonical_path.clone()),
      dimensions: declared.map_or_else(missing_dimensions, |command| command.dimensions.clone()),
      parity_rows: declared.map_or_else(Vec::new, |command| command.parity_rows.clone()),
      path,
      probe_argv: probe,
      help_exit_code: output.status.code().unwrap_or(-1),
      usage: parsed.usage,
      arguments: parsed.arguments,
      options: parsed.options,
      children: parsed.children,
    });
  }
  commands.sort_by(|left, right| left.path.cmp(&right.path));
  for declared in &support.commands {
    if !commands.iter().any(|command| command.tool == declared.tool && command.path == declared.path) {
      return Err(
        format!(
          "support source command {:?} was not exposed by the selected {} tool",
          declared.path, declared.tool
        )
        .into(),
      );
    }
  }
  Ok(commands)
}

fn capture_exit_cases(dotnet: &Path) -> Result<Vec<ExitCase>> {
  [
    ("dotnet", vec!["--definitely-invalid"]),
    ("msbuild", vec!["msbuild", "-definitely-invalid"]),
    ("nuget", vec!["nuget", "--definitely-invalid"]),
    ("vstest", vec!["vstest", "/DefinitelyInvalid"]),
  ]
  .into_iter()
  .map(|(tool, arguments)| {
    let output = run_bounded(dotnet, &arguments)?;
    Ok(ExitCase {
      tool: tool.into(),
      argv: arguments.into_iter().map(str::to_owned).collect(),
      exit_code: output.status.code().unwrap_or(-1),
    })
  })
  .collect()
}

fn command_tool(path: &[String]) -> &'static str {
  match path.first().map(String::as_str) {
    Some("msbuild") => "msbuild",
    Some("nuget") => "nuget",
    Some("vstest") => "vstest",
    _ => "dotnet",
  }
}

fn missing_dimensions() -> SupportDimensions {
  SupportDimensions {
    options: SupportState::Missing,
    arguments: SupportState::Missing,
    defaults: SupportState::Missing,
    environment: SupportState::Missing,
    exits: SupportState::Missing,
    outputs: SupportState::Missing,
  }
}

fn normalize_capture_working_directory(mut text: String) -> Result<String> {
  let directory = env::current_dir()?;
  let directory = directory.to_str().ok_or("capture working directory must be Unicode")?;
  let with_separator = format!("{directory}{}", std::path::MAIN_SEPARATOR);
  text = text.replace(&with_separator, "$CWD/");
  Ok(text.replace(directory, "$CWD"))
}

fn expand_msbuild_spellings(options: &mut [OptionSurface]) {
  for option in options {
    let mut expanded = option.spellings.clone();
    for spelling in &option.spellings {
      if let Some(name) = spelling.strip_prefix('-').filter(|name| !name.starts_with('-')) {
        expanded.push(format!("/{name}"));
        expanded.push(format!("--{name}"));
      }
    }
    expanded.sort();
    expanded.dedup();
    option.spellings = expanded;
  }
}

fn validate_support_source(source: &SupportSource) -> Result<()> {
  if source.schema_version != MANIFEST_SCHEMA_VERSION {
    return Err(
      format!(
        "support source schema {} does not match manifest schema {MANIFEST_SCHEMA_VERSION}",
        source.schema_version
      )
      .into(),
    );
  }
  if source.manifest_version == 0 || source.dv_command_syntax_version != COMMAND_SYNTAX_VERSION || source.dv_event_schema_version == 0 {
    return Err("support source versions are missing or incompatible".into());
  }
  let mut keys = BTreeSet::new();
  for command in &source.commands {
    if command.path.is_empty() || command.canonical_path.is_empty() || !keys.insert((command.tool.as_str(), command.path.as_slice())) {
      return Err("support commands require unique nonempty tool/path and canonical paths".into());
    }
  }
  Ok(())
}

fn parse_help(text: &str) -> ParsedHelp {
  let lines = text.lines().map(str::trim_end).collect::<Vec<_>>();
  let mut parsed = ParsedHelp::default();
  let mut section = "";
  let mut option = None::<OptionSurface>;
  let mut index = 0;
  while index < lines.len() {
    let line = lines[index];
    let trimmed = line.trim();
    if trimmed.is_empty() {
      flush_option(&mut parsed.options, &mut option);
      index += 1;
      continue;
    }
    if trimmed == "Usage:" || trimmed.starts_with("Usage: ") {
      flush_option(&mut parsed.options, &mut option);
      let inline = trimmed.strip_prefix("Usage:").unwrap_or_default().trim();
      if !inline.is_empty() {
        parsed.usage.push(inline.into());
      }
      index += 1;
      while index < lines.len() && !lines[index].trim().is_empty() {
        parsed.usage.push(lines[index].trim().into());
        index += 1;
      }
      continue;
    }
    if matches!(trimmed, "Commands:" | "SDK commands:" | "Additional commands from bundled tools:") {
      flush_option(&mut parsed.options, &mut option);
      section = "commands";
      index += 1;
      continue;
    }
    if matches!(trimmed, "Options:" | "runtime-options:" | "sdk-options:") || trimmed.starts_with("Switches:") {
      flush_option(&mut parsed.options, &mut option);
      section = "options";
      index += 1;
      continue;
    }
    if matches!(trimmed, "Arguments:" | "path-to-application:") {
      flush_option(&mut parsed.options, &mut option);
      section = "arguments";
      if trimmed == "path-to-application:" {
        parsed.arguments.push(ArgumentSurface {
          position: parsed.arguments.len(),
          syntax: "<path-to-application>".into(),
          description: String::new(),
        });
      }
      index += 1;
      continue;
    }
    if !line.chars().next().is_some_and(char::is_whitespace) && trimmed.ends_with(':') && !starts_surface_token(trimmed) {
      flush_option(&mut parsed.options, &mut option);
      section = "";
    }
    match section {
      "commands" => {
        if let Some(child) = parse_command_name(line) {
          parsed.children.push(child.into());
        }
      },
      "options" => {
        let indent = line.len() - line.trim_start().len();
        if indent <= 4
          && let Some((syntax, description)) = split_surface_line(line, true)
        {
          flush_option(&mut parsed.options, &mut option);
          option = Some(option_surface(syntax, description));
        } else if let Some(current) = &mut option {
          append_description(&mut current.description, trimmed);
          current.default = parse_default(&current.description);
        }
      },
      "arguments" => {
        if let Some((syntax, description)) = split_surface_line(line, false) {
          parsed.arguments.push(ArgumentSurface {
            position: parsed.arguments.len(),
            syntax: syntax.into(),
            description: description.into(),
          });
        } else if let Some(current) = parsed.arguments.last_mut() {
          append_description(&mut current.description, trimmed);
        }
      },
      _ => {},
    }
    index += 1;
  }
  flush_option(&mut parsed.options, &mut option);
  parsed.children.sort();
  parsed.children.dedup();
  parsed
}

fn starts_surface_token(value: &str) -> bool {
  value.as_bytes().first().is_some_and(|byte| matches!(byte, b'-' | b'/' | b'@'))
}

fn parse_command_name(line: &str) -> Option<&str> {
  if !line.chars().next().is_some_and(char::is_whitespace) {
    return None;
  }
  let trimmed = line.trim_start();
  let end = trimmed.find(char::is_whitespace)?;
  let name = &trimmed[..end];
  (!name.is_empty() && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')).then_some(name)
}

fn split_surface_line(line: &str, option: bool) -> Option<(&str, &str)> {
  let trimmed = line.trim_start();
  if option && !starts_surface_token(trimmed) {
    return None;
  }
  if !option && !trimmed.as_bytes().first().is_some_and(|byte| matches!(byte, b'<' | b'[')) {
    return None;
  }
  let bytes = trimmed.as_bytes();
  for index in 0..bytes.len().saturating_sub(1) {
    if bytes[index].is_ascii_whitespace() && bytes[index + 1].is_ascii_whitespace() {
      let mut description = index + 2;
      while description < bytes.len() && bytes[description].is_ascii_whitespace() {
        description += 1;
      }
      return Some((trimmed[..index].trim_end(), trimmed[description..].trim()));
    }
  }
  if option {
    let mut angle_depth = 0_u8;
    let mut square_depth = 0_u8;
    for (index, character) in trimmed.char_indices() {
      match character {
        '<' => angle_depth = angle_depth.saturating_add(1),
        '>' => angle_depth = angle_depth.saturating_sub(1),
        '[' => square_depth = square_depth.saturating_add(1),
        ']' => square_depth = square_depth.saturating_sub(1),
        _ if character.is_whitespace() && angle_depth == 0 && square_depth == 0 => {
          return Some((trimmed[..index].trim_end(), trimmed[index..].trim_start()));
        },
        _ => {},
      }
    }
  }
  Some((trimmed, ""))
}

fn option_surface(syntax: &str, description: &str) -> OptionSurface {
  let spellings = syntax
    .split([',', '|'])
    .flat_map(str::split_whitespace)
    .filter(|part| starts_surface_token(part))
    .map(normalize_spelling)
    .collect::<Vec<_>>();
  let value = syntax.find(['<', '[']).map(|index| syntax[index..].trim().to_owned()).or_else(|| {
    syntax
      .find([':', '='])
      .map(|index| syntax[index + 1..].trim().to_owned())
      .filter(|value| !value.is_empty())
  });
  OptionSurface {
    syntax: syntax.into(),
    spellings,
    value,
    default: parse_default(description),
    description: description.into(),
  }
}

fn normalize_spelling(value: &str) -> String {
  let end = value.find([':', '=', '[', '<']).unwrap_or(value.len());
  value[..end].trim_end_matches([',', '|']).to_owned()
}

fn parse_default(description: &str) -> Option<String> {
  let marker = "[default:";
  let start = description.to_ascii_lowercase().find(marker)? + marker.len();
  let end = description[start..].find(']')? + start;
  Some(description[start..end].trim().to_owned())
}

fn append_description(description: &mut String, continuation: &str) {
  if continuation.is_empty() {
    return;
  }
  if !description.is_empty() {
    description.push(' ');
  }
  description.push_str(continuation);
}

fn flush_option(options: &mut Vec<OptionSurface>, current: &mut Option<OptionSurface>) {
  if let Some(mut option) = current.take() {
    if let Some(short_form) = short_form(&option.description) {
      let short_form = normalize_spelling(short_form);
      if !option.spellings.iter().any(|spelling| spelling == &short_form) {
        option.spellings.push(short_form);
      }
    }
    options.push(option);
  }
}

fn short_form(description: &str) -> Option<&str> {
  let lower = description.to_ascii_lowercase();
  let marker = "short form";
  let start = lower.find(marker)? + marker.len();
  let tail = description[start..].trim_start_matches([' ', ':']);
  let end = tail
    .find(|character: char| character == ')' || character == ',' || character == ';' || character.is_whitespace())
    .unwrap_or(tail.len());
  let candidate = tail[..end].trim();
  starts_surface_token(candidate).then_some(candidate)
}

fn parse_parity_map(contents: &str) -> Result<Vec<ParityRow>> {
  let mut rows = Vec::new();
  for line in contents.lines() {
    let trimmed = line.trim_start();
    let status = if trimmed.starts_with("- [x] ") {
      ParityStatus::Implemented
    } else if trimmed.starts_with("- [~] ") {
      ParityStatus::Partial
    } else if trimmed.starts_with("- [ ] ") {
      ParityStatus::Missing
    } else {
      continue;
    };
    let Some(open) = trimmed.find('`') else { continue };
    let Some(close_offset) = trimmed[open + 1..].find('`') else { continue };
    let close = open + 1 + close_offset;
    let id = &trimmed[open + 1..close];
    if !id.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-') {
      continue;
    }
    rows.push(ParityRow {
      id: id.into(),
      status,
      summary: trimmed[close + 1..].trim().into(),
    });
  }
  if rows.is_empty() {
    return Err("parity map contained no capability rows".into());
  }
  rows.sort_by(|left, right| left.id.cmp(&right.id));
  if rows.windows(2).any(|pair| pair[0].id == pair[1].id) {
    return Err("parity map contains duplicate capability IDs".into());
  }
  Ok(rows)
}

fn environment_inputs() -> Vec<EnvironmentInput> {
  [
    ("DOTNET_ROOT", "dotnet", "host installation root"),
    ("DOTNET_CLI_HOME", "dotnet", "CLI state home"),
    ("DOTNET_CLI_TELEMETRY_OPTOUT", "dotnet", "telemetry opt-out"),
    ("DOTNET_NOLOGO", "dotnet", "first-run and logo output"),
    ("DOTNET_ROLL_FORWARD", "dotnet", "runtime roll-forward"),
    ("DOTNET_ROLL_FORWARD_TO_PRERELEASE", "dotnet", "prerelease runtime roll-forward"),
    ("DOTNET_HOST_PATH", "dotnet", "host path passed to SDK tools"),
    ("MSBuildSDKsPath", "msbuild", "SDK resolver root"),
    ("MSBUILDNOINPROCNODE", "msbuild", "node hosting policy"),
    ("MSBUILDDISABLENODEREUSE", "msbuild", "node reuse policy"),
    ("NUGET_PACKAGES", "nuget", "global packages folder"),
    ("NUGET_HTTP_CACHE_PATH", "nuget", "HTTP cache folder"),
    ("NUGET_PLUGINS_CACHE_PATH", "nuget", "plugin cache folder"),
    ("NUGET_CREDENTIALPROVIDERS_PATH", "nuget", "credential provider search path"),
    ("NUGET_PLUGIN_PATHS", "nuget", "explicit plugin paths"),
    ("VSTEST_HOST_DEBUG", "vstest", "test-host debugging"),
    ("VSTEST_CONNECTION_TIMEOUT", "vstest", "test-host connection timeout"),
  ]
  .into_iter()
  .map(|(name, tool, role)| EnvironmentInput {
    name: name.into(),
    tool: tool.into(),
    role: role.into(),
  })
  .collect()
}

fn output_formats() -> Vec<OutputFormat> {
  [
    ("dotnet", "human_text", &[][..]),
    ("dotnet", "json", &["--format json"][..]),
    ("msbuild", "human_text", &[][..]),
    ("msbuild", "query_json", &["-getProperty", "-getItem", "-getTargetResult"][..]),
    ("msbuild", "binary_log", &["-binaryLogger", "-bl"][..]),
    ("msbuild", "preprocessed_xml", &["-preprocess", "-pp"][..]),
    ("nuget", "human_text", &[][..]),
    ("vstest", "human_text", &[][..]),
    ("vstest", "trx", &["--logger:trx", "/logger:trx"][..]),
    ("vstest", "diagnostic_log", &["--Diag", "/Diag"][..]),
  ]
  .into_iter()
  .map(|(tool, name, selectors)| OutputFormat {
    tool: tool.into(),
    name: name.into(),
    selectors: selectors.iter().map(|selector| (*selector).into()).collect(),
  })
  .collect()
}

fn run_bounded(executable: &Path, arguments: &[impl AsRef<OsStr>]) -> Result<ProcessOutput> {
  let mut child = Command::new(executable)
    .args(arguments)
    .env("DOTNET_CLI_UI_LANGUAGE", "en-US")
    .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
    .env("DOTNET_NOLOGO", "1")
    .env("DOTNET_SKIP_FIRST_TIME_EXPERIENCE", "1")
    .env("VSLANG", "1033")
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
  let stdout = child.stdout.take().ok_or("could not capture stdout")?;
  let stderr = child.stderr.take().ok_or("could not capture stderr")?;
  let stdout_reader = thread::spawn(move || drain_stream(stdout));
  let stderr_reader = thread::spawn(move || drain_stream(stderr));
  let started = Instant::now();
  let mut timed_out = false;
  let status = loop {
    if let Some(status) = child.try_wait()? {
      break status;
    }
    if started.elapsed() >= PROCESS_TIMEOUT {
      child.kill()?;
      timed_out = true;
      break child.wait()?;
    }
    thread::sleep(Duration::from_millis(5));
  };
  let stdout = stdout_reader.join().map_err(|_| "stdout reader panicked")??;
  let stderr = stderr_reader.join().map_err(|_| "stderr reader panicked")??;
  if timed_out {
    return Err(format!("{} exceeded the {} ms timeout", executable.display(), PROCESS_TIMEOUT.as_millis()).into());
  }
  if stdout.exceeded || stderr.exceeded {
    return Err(format!("{} exceeded the {MAX_STREAM_BYTES}-byte stream bound", executable.display()).into());
  }
  Ok(ProcessOutput {
    status,
    stdout: stdout.bytes,
    stderr: stderr.bytes,
  })
}

fn drain_stream(mut stream: impl Read) -> std::io::Result<StreamCapture> {
  let mut bytes = Vec::with_capacity(16 * 1024);
  let mut buffer = [0_u8; 8192];
  let mut exceeded = false;
  loop {
    let read = stream.read(&mut buffer)?;
    if read == 0 {
      break;
    }
    let remaining = MAX_STREAM_BYTES.saturating_sub(bytes.len());
    let retained = read.min(remaining);
    bytes.extend_from_slice(&buffer[..retained]);
    exceeded |= retained != read;
  }
  Ok(StreamCapture { bytes, exceeded })
}

fn combined_text(output: &ProcessOutput) -> Result<String> {
  let mut text = String::with_capacity(output.stdout.len() + output.stderr.len() + 1);
  text.push_str(std::str::from_utf8(&output.stdout)?);
  if !output.stdout.is_empty() && !output.stderr.is_empty() {
    text.push('\n');
  }
  text.push_str(std::str::from_utf8(&output.stderr)?);
  Ok(text)
}

fn command_text(executable: &Path, arguments: &[&str]) -> String {
  let mut text = executable.display().to_string();
  for argument in arguments {
    text.push(' ');
    text.push_str(argument);
  }
  text
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
  if path.exists() {
    return Err(format!("refusing to replace existing manifest {}; remove it explicitly before capture", path.display()).into());
  }
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)?;
  }
  let mut temporary = path.as_os_str().to_owned();
  temporary.push(format!(".{}.tmp", std::process::id()));
  let temporary = PathBuf::from(temporary);
  let result = (|| -> Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
  })();
  if result.is_err() {
    let _ = fs::remove_file(&temporary);
  }
  result
}

fn read_manifest(path: &Path) -> Result<CompatibilityManifest> {
  Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn validate_manifest(manifest: &CompatibilityManifest) -> Result<()> {
  if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
    return Err(format!("unsupported manifest schema {}", manifest.schema_version).into());
  }
  if manifest.manifest_version == 0 || manifest.command_syntax_version == 0 || manifest.event_schema_version == 0 {
    return Err("manifest, command syntax, and event schema versions must be nonzero".into());
  }
  if manifest.reference.dotnet_sdk.is_empty()
    || manifest.reference.msbuild.is_empty()
    || manifest.reference.nuget.is_empty()
    || manifest.reference.vstest.is_empty()
  {
    return Err("every selected reference tool requires a version".into());
  }
  if manifest.commands.is_empty() || !manifest.commands[0].path.is_empty() {
    return Err("manifest requires one sorted root command record".into());
  }
  if manifest.commands.windows(2).any(|pair| pair[0].path >= pair[1].path) {
    return Err("command paths must be unique and strictly sorted".into());
  }
  let paths = manifest.commands.iter().map(|command| command.path.as_slice()).collect::<BTreeSet<_>>();
  for command in &manifest.commands {
    for child in &command.children {
      let mut child_path = command.path.clone();
      child_path.push(child.clone());
      if !paths.contains(child_path.as_slice()) {
        return Err(format!("captured child path {child_path:?} has no command record").into());
      }
    }
  }
  if manifest.commands.len() > manifest.capture.max_commands {
    return Err("captured command count exceeds its declared bound".into());
  }
  if manifest.commands.iter().any(|command| command.path.len() > manifest.capture.max_command_depth) {
    return Err("captured command path exceeds its declared depth bound".into());
  }
  if manifest
    .commands
    .iter()
    .any(|command| matches!(command.support, SupportState::Implemented | SupportState::Partial) && command.canonical_path.as_ref().is_none_or(Vec::is_empty))
  {
    return Err("implemented and partial commands require a canonical path".into());
  }
  if manifest
    .commands
    .iter()
    .flat_map(|command| &command.options)
    .any(|option| option.syntax.is_empty() || option.spellings.is_empty())
  {
    return Err("every captured option requires syntax and at least one spelling".into());
  }
  if manifest.commands.iter().any(|command| {
    command
      .arguments
      .iter()
      .enumerate()
      .any(|(position, argument)| argument.position != position || argument.syntax.is_empty())
  }) {
    return Err("captured arguments require contiguous zero-based positions and syntax".into());
  }
  if manifest.parity_rows.is_empty() || manifest.parity_rows.windows(2).any(|pair| pair[0].id >= pair[1].id) {
    return Err("parity rows must be nonempty, unique, and strictly sorted".into());
  }
  let parity_ids = manifest.parity_rows.iter().map(|row| row.id.as_str()).collect::<BTreeSet<_>>();
  if manifest
    .commands
    .iter()
    .flat_map(|command| &command.parity_rows)
    .any(|id| !parity_ids.contains(id.as_str()))
  {
    return Err("command support references an unknown parity row".into());
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_system_command_line_help_as_batched_surface_data() {
    let parsed = parse_help(
      "Usage:\n  dotnet build [<PROJECT>...] [options]\n\nArguments:\n  <PROJECT>  Project input.\n\nOptions:\n  -c, --configuration <CONFIGURATION>  Build configuration. [default: Debug]\n  --no-restore                         Do not restore. [default: False]\n\nCommands:\n  child  Child command.\n",
    );

    assert_eq!(parsed.usage, ["dotnet build [<PROJECT>...] [options]"]);
    assert_eq!(parsed.arguments.len(), 1);
    assert_eq!(parsed.arguments[0].position, 0);
    assert_eq!(
      parsed.options.len(),
      2,
      "captured syntaxes: {:?}",
      parsed.options.iter().map(|option| option.syntax.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(parsed.options[0].spellings, ["-c", "--configuration"]);
    assert_eq!(parsed.options[0].value.as_deref(), Some("<CONFIGURATION>"));
    assert_eq!(parsed.options[0].default.as_deref(), Some("Debug"));
    assert_eq!(parsed.children, ["child"]);
  }

  #[test]
  fn captures_the_runtime_application_path_heading_as_a_position() {
    let parsed = parse_help("path-to-application:\n  The path to an application .dll file to execute.\n");

    assert_eq!(parsed.arguments.len(), 1);
    assert_eq!(parsed.arguments[0].position, 0);
    assert_eq!(parsed.arguments[0].syntax, "<path-to-application>");
    assert_eq!(parsed.arguments[0].description, "The path to an application .dll file to execute.");
  }

  #[test]
  fn parses_vstest_pipe_and_colon_spellings() {
    let parsed = parse_help("Options:\n\n--Tests|/Tests:<Test Names>\n      Run selected tests.\n\n-?|--Help|/?|/Help\n      Display help.\n");

    assert_eq!(parsed.options.len(), 2);
    assert_eq!(parsed.options[0].spellings, ["--Tests", "/Tests"]);
    assert_eq!(parsed.options[0].value.as_deref(), Some("<Test Names>"));
    assert_eq!(parsed.options[1].spellings, ["-?", "--Help", "/?", "/Help"]);
  }

  #[test]
  fn parses_msbuild_switch_heading_short_forms_and_prefixes() {
    let mut parsed = parse_help(
      "Switches:            Note that switches accept -, /, and --.\n\n  -target:<targets>  Build targets. (Short form: -t)\n                     Example:\n                       -target:Build\n\n  -property:<n>=<v>  Set properties. (Short form: -p)\n\n  -verbosity:<level> Display verbosity. (Short form: -v)\n",
    );
    expand_msbuild_spellings(&mut parsed.options);

    assert_eq!(
      parsed.options.len(),
      3,
      "captured syntaxes: {:?}",
      parsed.options.iter().map(|option| option.syntax.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(parsed.options[0].spellings, ["--t", "--target", "-t", "-target", "/t", "/target"]);
    assert_eq!(parsed.options[1].spellings, ["--p", "--property", "-p", "-property", "/p", "/property"]);
    assert_eq!(parsed.options[2].value.as_deref(), Some("<level>"));
    assert_eq!(parsed.options[2].description, "Display verbosity. (Short form: -v)");
  }

  #[test]
  fn parses_and_sorts_the_parity_ledger() {
    let rows = parse_parity_map("- [ ] `DROP-002` Missing.\n- [x] `CLI-001` Done.\n- [~] `DROP-001` Partial.\n").unwrap();

    assert_eq!(rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(), ["CLI-001", "DROP-001", "DROP-002"]);
    assert!(matches!(rows[0].status, ParityStatus::Implemented));
    assert!(matches!(rows[1].status, ParityStatus::Partial));
    assert!(matches!(rows[2].status, ParityStatus::Missing));
  }

  #[test]
  fn normalizes_only_the_capture_working_directory() {
    let directory = env::current_dir().unwrap();
    let text = format!(
      "default: {}{}project.csproj; example: C:\\Reference\\example.csproj",
      directory.display(),
      std::path::MAIN_SEPARATOR
    );

    let normalized = normalize_capture_working_directory(text).unwrap();

    assert!(normalized.contains("default: $CWD/project.csproj"));
    assert!(normalized.contains("example: C:\\Reference\\example.csproj"));
  }

  #[test]
  fn checked_in_manifest_is_valid() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = read_manifest(&repository.join("compatibility/manifest.json")).unwrap();
    validate_manifest(&manifest).unwrap();
    assert_eq!(manifest.reference.dotnet_sdk, "10.0.100");
    assert_eq!(manifest.commands.len(), 115);
    assert_eq!(manifest.commands.iter().map(|command| command.options.len()).sum::<usize>(), 769);
    assert_eq!(manifest.commands.iter().map(|command| command.arguments.len()).sum::<usize>(), 74);
    assert_eq!(manifest.environment_inputs.len(), 17);
    assert_eq!(manifest.exit_cases.len(), 4);
    assert_eq!(manifest.output_formats.len(), 10);
    assert_eq!(manifest.parity_rows.len(), 468);

    let current_parity = parse_parity_map(&fs::read_to_string(repository.join("docs/feature-parity-map.md")).unwrap()).unwrap();
    assert_eq!(manifest.parity_rows, current_parity, "regenerate the manifest after changing parity state");

    let support: SupportSource = serde_json::from_slice(&fs::read(repository.join("compatibility/phase1-support.json")).unwrap()).unwrap();
    validate_support_source(&support).unwrap();
    assert_eq!(manifest.manifest_version, support.manifest_version);
    assert_eq!(manifest.command_syntax_version, support.dv_command_syntax_version);
    assert_eq!(manifest.event_schema_version, support.dv_event_schema_version);
    for declared in support.commands {
      let captured = manifest
        .commands
        .iter()
        .find(|command| command.tool == declared.tool && command.path == declared.path)
        .expect("every declared support row is captured");
      assert_eq!(captured.support, declared.status);
      assert_eq!(captured.canonical_path.as_ref(), Some(&declared.canonical_path));
      assert_eq!(captured.dimensions, declared.dimensions);
      assert_eq!(captured.parity_rows, declared.parity_rows);
    }
  }
}
