use std::{
  error::Error,
  ffi::OsStr,
  fmt,
  fs::{self, File},
  io::Read,
  mem::{align_of, size_of},
  path::{Path, PathBuf},
  str,
};

use dv_core::{CancellationToken, CompatibilityInputEvent, CompatibilityInvocationEvent, CompatibilitySupport};
use quick_xml::{Reader, XmlVersion, events::Event};

use crate::output::redact_argument_batch;

const MAX_FILES: usize = 4_096;
const MAX_DIRECTORIES: usize = 4_096;
const MAX_FILE_BYTES: usize = 1_048_576;
const MAX_TOTAL_BYTES: u64 = 33_554_432;
const MAX_INVOCATIONS: usize = 4_096;
const MAX_LINE_TOKENS: usize = 64;
const MAX_COMMAND_DEPTH: usize = 4;

struct ManifestCommand {
  path: &'static [&'static str],
  support: CompatibilitySupport,
  parity_rows: &'static [&'static str],
}

const _: () = assert!(size_of::<CompatibilitySupport>() == 1);
const _: () = assert!(align_of::<CompatibilitySupport>() == 1);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<ManifestCommand>() == 40);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<ManifestCommand>() == 8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Tool {
  Dotnet,
  Msbuild,
  Nuget,
  Vstest,
}

impl Tool {
  const fn manifest_name(self) -> &'static str {
    match self {
      Self::Dotnet => "dotnet",
      Self::Msbuild => "msbuild",
      Self::Nuget => "nuget",
      Self::Vstest => "vstest",
    }
  }

  fn from_executable(token: &str) -> Option<Self> {
    let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
    let name = strip_ascii_suffix(name, ".exe");
    if name.eq_ignore_ascii_case("dotnet") {
      Some(Self::Dotnet)
    } else if name.eq_ignore_ascii_case("msbuild") {
      Some(Self::Msbuild)
    } else if name.eq_ignore_ascii_case("nuget") {
      Some(Self::Nuget)
    } else if name.eq_ignore_ascii_case("vstest.console") || name.eq_ignore_ascii_case("vstest") {
      Some(Self::Vstest)
    } else {
      None
    }
  }

  const fn command_is_case_sensitive(self) -> bool {
    matches!(self, Self::Dotnet)
  }
}

const _: () = assert!(size_of::<Tool>() == 1);
const _: () = assert!(align_of::<Tool>() == 1);

fn strip_ascii_suffix<'a>(value: &'a str, suffix: &str) -> &'a str {
  value
    .get(..value.len().saturating_sub(suffix.len()))
    .filter(|prefix| value[prefix.len()..].eq_ignore_ascii_case(suffix))
    .unwrap_or(value)
}

#[derive(Clone, Copy)]
struct CommandRange {
  start: u16,
  len: u16,
}

const _: () = assert!(size_of::<CommandRange>() == 4);
const _: () = assert!(align_of::<CommandRange>() == 2);

include!(concat!(env!("OUT_DIR"), "/compatibility_check_index.rs"));

struct ManifestIndex;

impl ManifestIndex {
  fn find_command(&self, tool: Tool, arguments: &[&str]) -> Result<u16, CheckError> {
    let range = MANIFEST_COMMAND_RANGES[tool as usize];
    let start = usize::from(range.start);
    let end = start + usize::from(range.len);
    let mut best = None;
    let mut best_depth = 0;
    for (offset, command) in MANIFEST_COMMANDS[start..end].iter().enumerate() {
      let command_path = if tool == Tool::Dotnet {
        command.path
      } else {
        let Some((root, tail)) = command.path.split_first() else {
          continue;
        };
        if !root.eq_ignore_ascii_case(tool.manifest_name()) {
          continue;
        }
        tail
      };
      if command_path.len() < best_depth || command_path.len() > arguments.len() {
        continue;
      }
      let matches = command_path.iter().zip(arguments).all(|(expected, actual)| {
        if tool.command_is_case_sensitive() {
          expected == actual
        } else {
          expected.eq_ignore_ascii_case(actual)
        }
      });
      if matches {
        best = Some((start + offset) as u16);
        best_depth = command_path.len();
      }
    }
    best.ok_or_else(|| CheckError::invalid(format!("manifest has no root row for {}", tool.manifest_name())))
  }

  fn command(&self, index: u16) -> &'static ManifestCommand {
    &MANIFEST_COMMANDS[usize::from(index)]
  }
}

struct CompatibilityCheckBatch {
  manifest_version: u16,
  inputs: Vec<CompatibilityInputEvent>,
  invocations: Vec<CompatibilityInvocationEvent>,
}

impl CompatibilityCheckBatch {
  pub(crate) fn unsupported_count(&self) -> usize {
    self
      .inputs
      .iter()
      .map(|input| usize::from(input.support != CompatibilitySupport::Implemented))
      .sum::<usize>()
      + self
        .invocations
        .iter()
        .map(|invocation| usize::from(invocation.support != CompatibilitySupport::Implemented))
        .sum::<usize>()
  }
}

pub(crate) struct CompatibilityCheck {
  batch: CompatibilityCheckBatch,
}

impl CompatibilityCheck {
  pub(crate) fn scan_paths(paths: &[PathBuf], cancellation: &CancellationToken) -> Result<Self, CheckError> {
    scan_paths(paths, cancellation).map(|batch| Self { batch })
  }

  pub(crate) fn manifest_version(&self) -> u16 {
    self.batch.manifest_version
  }

  pub(crate) fn unsupported_count(&self) -> usize {
    self.batch.unsupported_count()
  }

  pub(crate) fn has_unsupported(&self) -> bool {
    self.unsupported_count() != 0
  }

  pub(crate) fn event_data(self) -> (Vec<CompatibilityInputEvent>, Vec<CompatibilityInvocationEvent>) {
    (self.batch.inputs, self.batch.invocations)
  }

  pub(crate) fn write_human(&self, mut writer: impl std::io::Write, color: bool) -> std::io::Result<()> {
    let unsupported = self.unsupported_count();
    writeln!(writer, "Compatibility check")?;
    writeln!(writer, "  Manifest  {}", self.batch.manifest_version)?;
    writeln!(
      writer,
      "  Scanned   {} inputs | {} invocations | {} unresolved",
      self.batch.inputs.len(),
      self.batch.invocations.len(),
      unsupported
    )?;
    if self.batch.inputs.is_empty() {
      return Ok(());
    }

    let location_width = self
      .batch
      .invocations
      .iter()
      .map(|invocation| {
        self
          .batch
          .inputs
          .get(invocation.input_index as usize)
          .map_or(9, |input| input.path.len() + decimal_width(invocation.line) + 1)
      })
      .max()
      .into_iter()
      .chain(self.batch.inputs.iter().map(|input| input.path.len()))
      .max()
      .unwrap_or(9)
      .max("LOCATION".len());
    writeln!(writer, "\n  {:<11}  {:<location_width$}  COMMAND / INPUT", "STATUS", "LOCATION")?;
    for (index, input) in self.batch.inputs.iter().enumerate() {
      write_status(&mut writer, input.support, color)?;
      writeln!(writer, "  {:<location_width$}  input: {}", input.path, input.kind)?;
      if input.support != CompatibilitySupport::Implemented {
        writeln!(writer, "               {}", input.detail)?;
      }
      for invocation in self.batch.invocations.iter().filter(|invocation| invocation.input_index as usize == index) {
        let location = format!("{}:{}", input.path, invocation.line);
        write_status(&mut writer, invocation.support, color)?;
        writeln!(writer, "  {location:<location_width$}  {}", invocation.command)?;
        if !invocation.parity_rows.is_empty() {
          writeln!(writer, "               rows: {}", invocation.parity_rows.join(", "))?;
        }
        if invocation.support != CompatibilitySupport::Implemented {
          writeln!(writer, "               {}", invocation.detail)?;
        }
      }
    }
    Ok(())
  }
}

fn write_status(writer: &mut impl std::io::Write, support: CompatibilitySupport, color: bool) -> std::io::Result<()> {
  let (text, code) = match support {
    CompatibilitySupport::Implemented => ("SUPPORTED", 32),
    CompatibilitySupport::Partial => ("PARTIAL", 33),
    CompatibilitySupport::Missing => ("MISSING", 31),
    CompatibilitySupport::Uncheckable => ("UNCHECKABLE", 31),
  };
  if color {
    write!(writer, "  \x1b[{code}m{text}\x1b[0m{:<width$}", "", width = 11usize.saturating_sub(text.len()))
  } else {
    write!(writer, "  {text:<11}")
  }
}

fn decimal_width(value: u32) -> usize {
  if value == 0 { 1 } else { value.ilog10() as usize + 1 }
}

#[derive(Default)]
struct ScanBatch {
  inputs: Vec<CompatibilityInputEvent>,
  invocations: Vec<CompatibilityInvocationEvent>,
  bytes_scanned: u64,
}

fn scan_paths(paths: &[PathBuf], cancellation: &CancellationToken) -> Result<CompatibilityCheckBatch, CheckError> {
  if paths.is_empty() {
    return Err(CheckError::invalid("compat check requires at least one input path"));
  }
  let manifest = ManifestIndex;
  let discovered = discover_files(paths, cancellation)?;
  let mut scan = ScanBatch {
    inputs: Vec::with_capacity(discovered.len()),
    invocations: Vec::with_capacity(discovered.len().min(64)),
    bytes_scanned: 0,
  };
  let mut buffer = Vec::with_capacity(16 * 1024);
  for path in discovered {
    check_cancelled(cancellation)?;
    let metadata = fs::metadata(&path).map_err(|error| CheckError::at(&path, format!("could not inspect input: {error}")))?;
    if metadata.len() > MAX_FILE_BYTES as u64 {
      return Err(CheckError::at(&path, format!("input exceeds the {MAX_FILE_BYTES}-byte per-file limit")));
    }
    let next_total = scan.bytes_scanned.saturating_add(metadata.len());
    if next_total > MAX_TOTAL_BYTES {
      return Err(CheckError::at(&path, format!("input corpus exceeds the {MAX_TOTAL_BYTES}-byte total limit")));
    }
    buffer.clear();
    File::open(&path)
      .and_then(|file| file.take((MAX_FILE_BYTES + 1) as u64).read_to_end(&mut buffer))
      .map_err(|error| CheckError::at(&path, format!("could not read input: {error}")))?;
    if buffer.len() > MAX_FILE_BYTES {
      return Err(CheckError::at(&path, format!("input exceeds the {MAX_FILE_BYTES}-byte per-file limit")));
    }
    let text = str::from_utf8(&buffer).map_err(|error| CheckError::at(&path, format!("input is not UTF-8: {error}")))?;
    let input_index = u32::try_from(scan.inputs.len()).map_err(|_| CheckError::invalid("input index overflowed"))?;
    let path_text = normalized_path(&path);
    let input = if is_msbuild_xml(&path) {
      scan_msbuild_xml(text, input_index, &manifest, &mut scan.invocations, cancellation, &path)?
    } else {
      let dynamic_command = scan_script(text, input_index, &manifest, &mut scan.invocations, cancellation, &path)?;
      CompatibilityInputEvent {
        path: path_text,
        kind: "script".into(),
        support: if dynamic_command {
          CompatibilitySupport::Uncheckable
        } else {
          CompatibilitySupport::Implemented
        },
        detail: if dynamic_command {
          "dynamic executable selection cannot be classified statically".into()
        } else {
          "literal command positions were scanned without execution".into()
        },
      }
    };
    scan.inputs.push(input);
    scan.bytes_scanned += buffer.len() as u64;
  }
  Ok(CompatibilityCheckBatch {
    manifest_version: MANIFEST_VERSION,
    inputs: scan.inputs,
    invocations: scan.invocations,
  })
}

fn discover_files(paths: &[PathBuf], cancellation: &CancellationToken) -> Result<Vec<PathBuf>, CheckError> {
  let mut files = Vec::with_capacity(paths.len());
  let mut directories = Vec::new();
  let mut visited_directories = 0;
  for input in paths {
    let group_start = files.len();
    classify_input(input, &mut files, &mut directories)?;
    while let Some(directory) = directories.pop() {
      check_cancelled(cancellation)?;
      visited_directories += 1;
      if visited_directories > MAX_DIRECTORIES {
        return Err(CheckError::at(
          &directory,
          format!("directory traversal exceeds the {MAX_DIRECTORIES}-directory limit"),
        ));
      }
      let mut entries = fs::read_dir(&directory)
        .map_err(|error| CheckError::at(&directory, format!("could not enumerate directory: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| CheckError::at(&directory, format!("could not enumerate directory entry: {error}")))?;
      entries.sort_unstable_by_key(|entry| entry.file_name());
      for entry in entries.into_iter().rev() {
        let path = entry.path();
        let file_type = entry
          .file_type()
          .map_err(|error| CheckError::at(&path, format!("could not inspect directory entry: {error}")))?;
        if file_type.is_symlink() {
          continue;
        }
        if file_type.is_dir() {
          if !excluded_directory(&entry.file_name()) {
            directories.push(path);
          }
        } else if file_type.is_file() && is_directory_candidate(&path) {
          files.push(path);
          if files.len() > MAX_FILES {
            return Err(CheckError::at(&directory, format!("input discovery exceeds the {MAX_FILES}-file limit")));
          }
        }
      }
    }
    files[group_start..].sort_unstable();
  }
  Ok(files)
}

fn classify_input(path: &Path, files: &mut Vec<PathBuf>, directories: &mut Vec<PathBuf>) -> Result<(), CheckError> {
  let metadata = fs::symlink_metadata(path).map_err(|error| CheckError::at(path, format!("could not inspect input: {error}")))?;
  if metadata.file_type().is_symlink() {
    return Err(CheckError::at(path, "symbolic-link inputs are not followed"));
  }
  if metadata.is_dir() {
    directories.push(path.to_owned());
  } else if metadata.is_file() {
    files.push(path.to_owned());
    if files.len() > MAX_FILES {
      return Err(CheckError::at(path, format!("input discovery exceeds the {MAX_FILES}-file limit")));
    }
  } else {
    return Err(CheckError::at(path, "input must be a regular file or directory"));
  }
  Ok(())
}

fn excluded_directory(name: &OsStr) -> bool {
  matches!(name.to_str(), Some(".git" | ".hg" | ".svn" | ".vs" | "bin" | "obj" | "target"))
}

fn is_directory_candidate(path: &Path) -> bool {
  let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
  if matches!(name, "Dockerfile" | "Makefile") {
    return true;
  }
  matches!(
    path.extension().and_then(OsStr::to_str).map(str::to_ascii_lowercase).as_deref(),
    Some("yml" | "yaml" | "ps1" | "psm1" | "sh" | "bash" | "zsh" | "cmd" | "bat" | "csproj" | "fsproj" | "vbproj" | "proj" | "props" | "targets")
  )
}

fn is_msbuild_xml(path: &Path) -> bool {
  matches!(
    path.extension().and_then(OsStr::to_str).map(str::to_ascii_lowercase).as_deref(),
    Some("csproj" | "fsproj" | "vbproj" | "proj" | "props" | "targets")
  )
}

fn is_project_file(path: &Path) -> bool {
  matches!(
    path.extension().and_then(OsStr::to_str).map(str::to_ascii_lowercase).as_deref(),
    Some("csproj" | "fsproj" | "vbproj" | "proj")
  )
}

fn normalized_path(path: &Path) -> String {
  let text = path.to_string_lossy().replace('\\', "/");
  text.strip_prefix("./").unwrap_or(&text).to_owned()
}

#[derive(Clone, Copy)]
enum ProjectValue {
  TargetFramework,
  TargetFrameworks,
}

struct ProjectShape {
  root_seen: bool,
  sdk: Option<String>,
  target_framework: Option<String>,
  target_frameworks: Option<String>,
  active_value: Option<ProjectValue>,
  value_text: String,
}

impl ProjectShape {
  fn new() -> Self {
    Self {
      root_seen: false,
      sdk: None,
      target_framework: None,
      target_frameworks: None,
      active_value: None,
      value_text: String::new(),
    }
  }

  fn finish(self, path: String) -> CompatibilityInputEvent {
    let (support, detail) = match self.sdk.as_deref() {
      Some("Microsoft.NET.Sdk") if self.target_frameworks.is_some() => (
        CompatibilitySupport::Missing,
        "TargetFrameworks requires deterministic inner-build expansion, which is not implemented".into(),
      ),
      Some("Microsoft.NET.Sdk") if self.target_framework.as_deref().is_some_and(is_literal_target_framework) => (
        CompatibilitySupport::Implemented,
        "Microsoft.NET.Sdk with one literal target framework is within the current project input contract".into(),
      ),
      Some("Microsoft.NET.Sdk") if self.target_framework.as_deref().is_some_and(contains_expansion) => (
        CompatibilitySupport::Uncheckable,
        "dynamic TargetFramework expansion cannot be classified statically".into(),
      ),
      Some("Microsoft.NET.Sdk") => (
        CompatibilitySupport::Partial,
        "the project does not expose one literal TargetFramework for static classification".into(),
      ),
      Some(value) if contains_expansion(value) => (
        CompatibilitySupport::Uncheckable,
        "dynamic project SDK selection cannot be classified statically".into(),
      ),
      Some(value) => (
        CompatibilitySupport::Missing,
        format!("project SDK {value:?} is not in the current native project contract"),
      ),
      None => (CompatibilitySupport::Missing, "project does not declare an SDK".into()),
    };
    CompatibilityInputEvent {
      path,
      kind: "project".into(),
      support,
      detail,
    }
  }
}

fn is_literal_target_framework(value: &str) -> bool {
  !value.trim().is_empty() && !contains_expansion(value) && !value.contains(';')
}

fn contains_expansion(value: &str) -> bool {
  value.contains("$(") || value.contains("${") || value.contains('%')
}

fn scan_msbuild_xml(
  text: &str,
  input_index: u32,
  manifest: &ManifestIndex,
  invocations: &mut Vec<CompatibilityInvocationEvent>,
  cancellation: &CancellationToken,
  path: &Path,
) -> Result<CompatibilityInputEvent, CheckError> {
  let bytes = text.as_bytes();
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(false);
  let mut cursor = 0;
  let mut line = 1_u32;
  let mut shape = ProjectShape::new();
  let mut dynamic_command = false;
  loop {
    check_cancelled(cancellation)?;
    let event_start = reader.buffer_position() as usize;
    advance_line(bytes, &mut cursor, event_start, &mut line);
    match reader.read_event() {
      Ok(Event::Start(element) | Event::Empty(element)) => {
        let qualified_name = element.name();
        let name = local_name(qualified_name.as_ref());
        if name == b"Project" {
          if shape.root_seen {
            return Err(CheckError::at(path, format!("project declares more than one root at line {line}")));
          }
          shape.root_seen = true;
          for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| CheckError::at(path, format!("invalid XML attribute at line {line}: {error}")))?;
            if local_name(attribute.key.as_ref()) == b"Sdk" {
              if shape.sdk.is_some() {
                return Err(CheckError::at(path, format!("Project declares Sdk more than once at line {line}")));
              }
              shape.sdk = Some(
                attribute
                  .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                  .map_err(|error| CheckError::at(path, format!("invalid Project Sdk at line {line}: {error}")))?
                  .into_owned(),
              );
            }
          }
        } else if name == b"Exec" {
          let mut command_seen = false;
          for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| CheckError::at(path, format!("invalid XML attribute at line {line}: {error}")))?;
            if local_name(attribute.key.as_ref()) != b"Command" {
              continue;
            }
            if command_seen {
              return Err(CheckError::at(path, format!("Exec declares Command more than once at line {line}")));
            }
            command_seen = true;
            let command = attribute
              .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
              .map_err(|error| CheckError::at(path, format!("invalid Exec Command at line {line}: {error}")))?;
            dynamic_command |= scan_script_lines(&command, input_index, line, manifest, invocations, cancellation, path)?;
          }
        } else if name == b"TargetFramework" || name == b"TargetFrameworks" {
          if shape.active_value.is_some() {
            return Err(CheckError::at(path, format!("nested target framework value at line {line}")));
          }
          shape.active_value = Some(if name == b"TargetFramework" {
            ProjectValue::TargetFramework
          } else {
            ProjectValue::TargetFrameworks
          });
          shape.value_text.clear();
        }
      },
      Ok(Event::Text(value)) if shape.active_value.is_some() => {
        let decoded = value
          .xml10_content()
          .map_err(|error| CheckError::at(path, format!("invalid project text at line {line}: {error}")))?;
        let unescaped =
          quick_xml::escape::unescape(&decoded).map_err(|error| CheckError::at(path, format!("invalid project entity at line {line}: {error}")))?;
        shape.value_text.push_str(&unescaped);
      },
      Ok(Event::End(element)) if local_name(element.name().as_ref()) == b"TargetFramework" => {
        if matches!(shape.active_value, Some(ProjectValue::TargetFramework)) {
          if shape.target_framework.replace(shape.value_text.trim().to_owned()).is_some() {
            return Err(CheckError::at(path, "project declares TargetFramework more than once"));
          }
          shape.active_value = None;
        }
      },
      Ok(Event::End(element)) if local_name(element.name().as_ref()) == b"TargetFrameworks" => {
        if matches!(shape.active_value, Some(ProjectValue::TargetFrameworks)) {
          if shape.target_frameworks.replace(shape.value_text.trim().to_owned()).is_some() {
            return Err(CheckError::at(path, "project declares TargetFrameworks more than once"));
          }
          shape.active_value = None;
        }
      },
      Ok(Event::DocType(_)) => return Err(CheckError::at(path, format!("XML document types are not accepted at line {line}"))),
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => {
        return Err(CheckError::at(path, format!("invalid XML at byte {}: {error}", reader.error_position())));
      },
    }
  }
  if !shape.root_seen {
    return Err(CheckError::at(path, "project input does not contain a Project root"));
  }
  if is_project_file(path) {
    let mut input = shape.finish(normalized_path(path));
    if dynamic_command {
      input.support = CompatibilitySupport::Uncheckable;
      input.detail = "dynamic executable selection in Exec cannot be classified statically".into();
    }
    Ok(input)
  } else {
    Ok(CompatibilityInputEvent {
      path: normalized_path(path),
      kind: "msbuild".into(),
      support: if dynamic_command {
        CompatibilitySupport::Uncheckable
      } else {
        CompatibilitySupport::Implemented
      },
      detail: if dynamic_command {
        "dynamic executable selection in Exec cannot be classified statically".into()
      } else {
        "well-formed MSBuild import was scanned for literal Exec commands".into()
      },
    })
  }
}

fn local_name(name: &[u8]) -> &[u8] {
  name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn advance_line(bytes: &[u8], cursor: &mut usize, target: usize, line: &mut u32) {
  let target = target.min(bytes.len());
  *line = line.saturating_add(bytes[*cursor..target].iter().filter(|byte| **byte == b'\n').count() as u32);
  *cursor = target;
}

fn scan_script(
  text: &str,
  input_index: u32,
  manifest: &ManifestIndex,
  invocations: &mut Vec<CompatibilityInvocationEvent>,
  cancellation: &CancellationToken,
  path: &Path,
) -> Result<bool, CheckError> {
  scan_script_lines(text, input_index, 1, manifest, invocations, cancellation, path)
}

fn scan_script_lines(
  text: &str,
  input_index: u32,
  base_line: u32,
  manifest: &ManifestIndex,
  invocations: &mut Vec<CompatibilityInvocationEvent>,
  cancellation: &CancellationToken,
  path: &Path,
) -> Result<bool, CheckError> {
  let mut dynamic_command = false;
  for (offset, raw_line) in text.lines().enumerate() {
    check_cancelled(cancellation)?;
    let line_number = base_line.saturating_add(offset as u32);
    dynamic_command |= scan_line(raw_line.trim_end_matches('\r'), input_index, line_number, manifest, invocations, path)?;
  }
  Ok(dynamic_command)
}

#[derive(Clone, Copy, Default)]
struct Token {
  start: u32,
  end: u32,
  flags: u8,
}

const TOKEN_BOUNDARY: u8 = 1 << 0;
const TOKEN_OBSERVATION: u8 = 1 << 1;
const TOKEN_COMPLEX: u8 = 1 << 2;

const _: () = assert!(size_of::<Token>() == 12);
const _: () = assert!(align_of::<Token>() == 4);

impl Token {
  fn text(self, line: &str) -> &str {
    &line[usize::try_from(self.start).expect("u32 token offset fits usize")..usize::try_from(self.end).expect("u32 token offset fits usize")]
  }

  const fn is_boundary(self) -> bool {
    self.flags & TOKEN_BOUNDARY != 0
  }

  const fn affects_observation(self) -> bool {
    self.flags & TOKEN_OBSERVATION != 0
  }

  const fn is_complex(self) -> bool {
    self.flags & TOKEN_COMPLEX != 0
  }
}

fn scan_line(
  line: &str,
  input_index: u32,
  line_number: u32,
  manifest: &ManifestIndex,
  invocations: &mut Vec<CompatibilityInvocationEvent>,
  path: &Path,
) -> Result<bool, CheckError> {
  let mut tokens = [Token::default(); MAX_LINE_TOKENS];
  let token_count = tokenize_line(line, &mut tokens).map_err(|problem| CheckError::at(path, format!("line {line_number}: {problem}")))?;
  if token_count == 0 {
    return Ok(false);
  }
  let first = tokens[0].text(line);
  if first.eq_ignore_ascii_case("rem") || first.starts_with("::") {
    return Ok(false);
  }

  let mut command_start = true;
  let mut command_observation = false;
  let mut dynamic_command = false;
  for index in 0..token_count {
    let token = tokens[index];
    if token.is_boundary() {
      command_start = true;
      command_observation = token.affects_observation();
      continue;
    }
    if !command_start {
      continue;
    }
    let text = token.text(line);
    if token.is_complex() {
      dynamic_command = true;
      command_start = false;
      command_observation = false;
      continue;
    }
    if command_prefix(text) {
      continue;
    }
    let Some(tool) = Tool::from_executable(text) else {
      dynamic_command |= dynamic_executable(text);
      command_start = false;
      command_observation = false;
      continue;
    };

    let end = tokens[index + 1..token_count]
      .iter()
      .position(|candidate| candidate.is_boundary())
      .map_or(token_count, |offset| index + 1 + offset);
    let invocation_tokens = &tokens[index..end];
    let observation_is_dynamic = command_observation
      || invocation_tokens
        .iter()
        .any(|candidate| candidate.affects_observation() || candidate.is_complex())
      || tokens.get(end).is_some_and(|boundary| boundary.affects_observation());
    let mut argument_storage = [""; MAX_COMMAND_DEPTH];
    let mut argument_count = 0;
    for next in &tokens[index + 1..end] {
      if next.affects_observation() || next.is_complex() || argument_count == argument_storage.len() {
        break;
      }
      argument_storage[argument_count] = next.text(line);
      argument_count += 1;
    }
    let arguments = &argument_storage[..argument_count];
    let command_index = manifest.find_command(tool, arguments)?;
    let command = manifest.command(command_index);
    let command_text = redact_command(line, invocation_tokens);
    let (support, detail) = classify_invocation(tool, arguments, command, observation_is_dynamic);
    let parity_rows = command.parity_rows.iter().copied().map(str::to_owned).collect();
    if invocations.len() == MAX_INVOCATIONS {
      return Err(CheckError::at(path, format!("scan exceeds the {MAX_INVOCATIONS}-invocation limit")));
    }
    invocations.push(CompatibilityInvocationEvent {
      input_index,
      line: line_number,
      column: token.start.saturating_add(1),
      tool: tool.manifest_name().into(),
      command: command_text,
      support,
      parity_rows,
      detail,
    });
    command_start = false;
    command_observation = false;
  }
  Ok(dynamic_command)
}

fn dynamic_executable(token: &str) -> bool {
  token.starts_with('$') || (token.starts_with('%') && token.ends_with('%')) || contains_expansion(token)
}

fn classify_invocation(tool: Tool, arguments: &[&str], command: &ManifestCommand, observation_is_dynamic: bool) -> (CompatibilitySupport, String) {
  if observation_is_dynamic {
    return (
      CompatibilitySupport::Uncheckable,
      "shell quoting, piping, or redirection changes an observable dimension that cannot be classified statically".into(),
    );
  }
  if arguments
    .iter()
    .take(command_path_depth(tool, command))
    .any(|argument| contains_expansion(argument))
  {
    return (
      CompatibilitySupport::Uncheckable,
      "dynamic command selection cannot be classified against a literal manifest row".into(),
    );
  }
  if tool == Tool::Dotnet && matches!(arguments, ["--version"] | ["--info"]) {
    return (
      CompatibilitySupport::Implemented,
      "literal SDK query is implemented by the selected compatibility profile".into(),
    );
  }
  let support = command.support;
  let detail = match support {
    CompatibilitySupport::Implemented => "manifest command row and every referenced parity row are implemented".into(),
    CompatibilitySupport::Partial => "manifest command row retains incomplete compatibility dimensions".into(),
    CompatibilitySupport::Missing => "manifest command row is not implemented".into(),
    CompatibilitySupport::Uncheckable => unreachable!("manifest support does not contain uncheckable"),
  };
  (support, detail)
}

fn command_path_depth(tool: Tool, command: &ManifestCommand) -> usize {
  command.path.len().saturating_sub(usize::from(tool != Tool::Dotnet))
}

fn redact_command(line: &str, tokens: &[Token]) -> String {
  let redacted = redact_argument_batch(
    tokens.iter().filter(|token| !token.is_boundary()).map(|token| OsStr::new(token.text(line))),
    tokens.len(),
  );
  redacted.join(" ")
}

fn command_prefix(token: &str) -> bool {
  matches!(
    token,
    "-" | "run:" | "script:" | "RUN" | "call" | "CALL" | "exec" | "command" | "sudo" | "env" | "if" | "then" | "do"
  ) || environment_assignment(token)
}

fn environment_assignment(token: &str) -> bool {
  let Some((name, _)) = token.split_once('=') else {
    return false;
  };
  !name.is_empty()
    && name
      .bytes()
      .enumerate()
      .all(|(index, byte)| byte == b'_' || byte.is_ascii_alphabetic() || (index != 0 && byte.is_ascii_digit()))
}

fn tokenize_line(line: &str, output: &mut [Token; MAX_LINE_TOKENS]) -> Result<usize, &'static str> {
  let bytes = line.as_bytes();
  let mut index = 0;
  let mut count = 0;
  while index < bytes.len() {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
      index += 1;
    }
    if index == bytes.len() || bytes[index] == b'#' {
      break;
    }
    if count == output.len() {
      return Err("token count exceeds the 64-token line limit");
    }
    if matches!(bytes[index], b';' | b'|' | b'&' | b'<' | b'>') {
      let start = index;
      let operator = bytes[index];
      index += 1;
      if index < bytes.len() && bytes[index] == operator {
        index += 1;
      }
      let doubled = index - start == 2;
      let boundary = matches!(operator, b';' | b'|' | b'&');
      let observation = matches!(operator, b'<' | b'>') || (matches!(operator, b'|' | b'&') && !doubled);
      output[count] = Token {
        start: start as u32,
        end: index as u32,
        flags: (u8::from(boundary) * TOKEN_BOUNDARY) | (u8::from(observation) * TOKEN_OBSERVATION),
      };
      count += 1;
      continue;
    }

    let mut flags = 0;
    let (start, end) = if matches!(bytes[index], b'\'' | b'"') {
      let quote = bytes[index];
      index += 1;
      let start = index;
      while index < bytes.len() && bytes[index] != quote {
        if bytes[index] == b'\\' {
          flags |= TOKEN_COMPLEX;
          index = (index + 2).min(bytes.len());
        } else {
          index += 1;
        }
      }
      if index == bytes.len() {
        return Err("quoted token is not terminated");
      }
      let end = index;
      if bytes[start..end].iter().any(u8::is_ascii_whitespace) {
        flags |= TOKEN_COMPLEX;
      }
      index += 1;
      if index < bytes.len() && !bytes[index].is_ascii_whitespace() && !matches!(bytes[index], b';' | b'|' | b'&' | b'<' | b'>' | b'#') {
        flags |= TOKEN_COMPLEX;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() && !matches!(bytes[index], b';' | b'|' | b'&' | b'<' | b'>') {
          index += 1;
        }
      }
      (start, end)
    } else {
      let start = index;
      while index < bytes.len() && !bytes[index].is_ascii_whitespace() && !matches!(bytes[index], b';' | b'|' | b'&' | b'<' | b'>') {
        if matches!(bytes[index], b'\'' | b'"') {
          flags |= TOKEN_COMPLEX;
        }
        index += 1;
      }
      (start, index)
    };
    output[count] = Token {
      start: start as u32,
      end: end as u32,
      flags,
    };
    count += 1;
  }
  Ok(count)
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), CheckError> {
  if cancellation.is_cancelled() { Err(CheckError::cancelled()) } else { Ok(()) }
}

#[derive(Debug)]
pub(crate) struct CheckError {
  cancelled: bool,
  path: Option<String>,
  message: String,
}

impl CheckError {
  fn invalid(message: impl Into<String>) -> Self {
    Self {
      cancelled: false,
      path: None,
      message: message.into(),
    }
  }

  fn at(path: &Path, message: impl Into<String>) -> Self {
    Self {
      cancelled: false,
      path: Some(normalized_path(path)),
      message: message.into(),
    }
  }

  fn cancelled() -> Self {
    Self {
      cancelled: true,
      path: None,
      message: "compatibility scan was cancelled".into(),
    }
  }

  pub(crate) fn is_cancelled(&self) -> bool {
    self.cancelled
  }

  pub(crate) fn path(&self) -> Option<&str> {
    self.path.as_deref()
  }
}

impl fmt::Display for CheckError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    if let Some(path) = &self.path {
      write!(formatter, "{path}: {}", self.message)
    } else {
      formatter.write_str(&self.message)
    }
  }
}

impl Error for CheckError {}

#[cfg(test)]
mod tests {
  use super::*;

  fn scan_text(text: &str) -> CompatibilityCheckBatch {
    let manifest = ManifestIndex;
    let mut invocations = Vec::new();
    scan_script(text, 0, &manifest, &mut invocations, &CancellationToken::new(), Path::new("pipeline.yml")).unwrap();
    CompatibilityCheckBatch {
      manifest_version: MANIFEST_VERSION,
      inputs: vec![CompatibilityInputEvent {
        path: "pipeline.yml".into(),
        kind: "script".into(),
        support: CompatibilitySupport::Implemented,
        detail: String::new(),
      }],
      invocations,
    }
  }

  #[test]
  fn script_scan_finds_only_literal_command_positions() {
    let batch = scan_text(
      "# dotnet publish\n- run: dotnet restore App.csproj\necho dotnet publish\ndotnet build App.csproj && nuget restore App.sln\nWrite-Output \"dotnet test\"\n",
    );
    assert_eq!(batch.invocations.len(), 3);
    assert_eq!(batch.invocations.iter().map(|finding| finding.line).collect::<Vec<_>>(), [2, 4, 4]);
    assert_eq!(
      batch.invocations.iter().map(|finding| finding.tool.as_str()).collect::<Vec<_>>(),
      ["dotnet", "dotnet", "nuget"]
    );
    assert_eq!(
      batch.invocations.iter().map(|finding| finding.support).collect::<Vec<_>>(),
      [CompatibilitySupport::Partial, CompatibilitySupport::Partial, CompatibilitySupport::Missing]
    );
  }

  #[test]
  fn sdk_queries_are_supported_but_redirected_output_is_uncheckable() {
    let batch = scan_text("dotnet --version\ndotnet --info > sdk.txt\n");
    assert_eq!(batch.invocations.len(), 2);
    assert_eq!(batch.invocations[0].support, CompatibilitySupport::Implemented);
    assert_eq!(batch.invocations[1].support, CompatibilitySupport::Uncheckable);
  }

  #[test]
  fn dynamic_executable_selection_marks_the_input_uncheckable() {
    let manifest = ManifestIndex;
    let mut invocations = Vec::new();
    let dynamic = scan_script(
      "$tool restore App.csproj\n%TOOL% build App.csproj\n",
      0,
      &manifest,
      &mut invocations,
      &CancellationToken::new(),
      Path::new("pipeline.ps1"),
    )
    .unwrap();

    assert!(dynamic);
    assert!(invocations.is_empty());
  }

  #[test]
  fn scanner_redacts_sensitive_command_values() {
    let batch = scan_text("nuget push package.nupkg --api-key very-secret\n");
    assert_eq!(batch.invocations.len(), 1);
    assert!(!batch.invocations[0].command.contains("very-secret"));
    assert!(batch.invocations[0].command.contains("<redacted>"));
  }

  #[test]
  fn project_scan_uses_xml_structure_and_extracts_exec_commands() {
    let manifest = ManifestIndex;
    let mut invocations = Vec::new();
    let input = scan_msbuild_xml(
      r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup>
  <Target Name="Probe"><Exec Command="dotnet restore App.csproj &amp;&amp; nuget restore App.sln" /></Target>
</Project>"#,
      0,
      &manifest,
      &mut invocations,
      &CancellationToken::new(),
      Path::new("App.csproj"),
    )
    .unwrap();

    assert_eq!(input.support, CompatibilitySupport::Implemented);
    assert_eq!(invocations.len(), 2);
    assert_eq!(invocations.iter().map(|invocation| invocation.line).collect::<Vec<_>>(), [3, 3]);
    assert_eq!(
      invocations.iter().map(|invocation| invocation.tool.as_str()).collect::<Vec<_>>(),
      ["dotnet", "nuget"]
    );
  }

  #[test]
  fn project_scan_rejects_document_types() {
    let manifest = ManifestIndex;
    let mut invocations = Vec::new();
    let error = scan_msbuild_xml(
      r#"<!DOCTYPE Project [<!ENTITY tfm "net10.0">]><Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>&tfm;</TargetFramework></PropertyGroup></Project>"#,
      0,
      &manifest,
      &mut invocations,
      &CancellationToken::new(),
      Path::new("App.csproj"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("document types are not accepted"));
  }

  #[test]
  fn tokenizer_rejects_unbounded_or_unterminated_lines() {
    let mut tokens = [Token::default(); MAX_LINE_TOKENS];
    assert_eq!(tokenize_line("dotnet \"unterminated", &mut tokens), Err("quoted token is not terminated"));
    let oversized = std::iter::repeat_n("x", MAX_LINE_TOKENS + 1).collect::<Vec<_>>().join(" ");
    assert_eq!(tokenize_line(&oversized, &mut tokens), Err("token count exceeds the 64-token line limit"));
  }
}
