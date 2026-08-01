use std::{
  env,
  ffi::{OsStr, OsString},
  mem::{align_of, size_of},
  num::NonZeroUsize,
  ops::Index,
};

use crate::{environment::directive_assignment, output};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub(crate) struct CommandSyntaxVersion(u16);

impl CommandSyntaxVersion {
  pub(crate) const fn get(self) -> u16 {
    self.0
  }
}

const _: () = assert!(size_of::<CommandSyntaxVersion>() == 2);
const _: () = assert!(align_of::<CommandSyntaxVersion>() == 2);

pub(crate) const COMMAND_SYNTAX_VERSION: CommandSyntaxVersion = CommandSyntaxVersion(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum InvocationMode {
  Native,
  Dotnet,
  Msbuild,
  Nuget,
  Vstest,
}

impl InvocationMode {
  const fn profile(self) -> Option<&'static str> {
    match self {
      Self::Native => None,
      Self::Dotnet => Some("dotnet"),
      Self::Msbuild => Some("msbuild"),
      Self::Nuget => Some("nuget"),
      Self::Vstest => Some("vstest"),
    }
  }

  fn argument_is_option(self, argument: &OsStr) -> bool {
    match argument.as_encoded_bytes().first().copied() {
      Some(b'-') => true,
      #[cfg(windows)]
      Some(b'/') => matches!(self, Self::Dotnet | Self::Msbuild | Self::Vstest),
      _ => false,
    }
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum ExitClass {
  Success,
  Usage,
  Unsupported,
  Operation,
  BuildFailure,
  RestoreFailure,
  TestFailure,
  NoTests,
  Cancelled,
}

const EXIT_CLASSES: [ExitClass; 9] = [
  ExitClass::Success,
  ExitClass::Usage,
  ExitClass::Unsupported,
  ExitClass::Operation,
  ExitClass::BuildFailure,
  ExitClass::RestoreFailure,
  ExitClass::TestFailure,
  ExitClass::NoTests,
  ExitClass::Cancelled,
];
const EXIT_CLASS_COUNT: usize = EXIT_CLASSES.len();
const INVOCATION_MODE_COUNT: usize = InvocationMode::Vstest as usize + 1;
const EXIT_NOT_APPLICABLE: u8 = u8::MAX;

// Rows are invocation profiles; columns are ExitClass discriminants. The hot
// terminal path is one bounds-proven lookup with no formatting or allocation.
const EXIT_CODES: [[u8; EXIT_CLASS_COUNT]; INVOCATION_MODE_COUNT] = [
  [0, 2, 2, 2, 2, 2, 2, 0, 2],
  [0, 1, 1, 1, 1, 1, 1, 0, 1],
  [0, 1, 1, 1, 1, 1, EXIT_NOT_APPLICABLE, EXIT_NOT_APPLICABLE, 1],
  [0, 1, 1, 1, EXIT_NOT_APPLICABLE, 1, EXIT_NOT_APPLICABLE, EXIT_NOT_APPLICABLE, 1],
  [0, 1, 1, 1, EXIT_NOT_APPLICABLE, EXIT_NOT_APPLICABLE, 1, 0, 1],
];

const _: () = assert!(size_of::<ExitClass>() == 1);
const _: () = assert!(align_of::<ExitClass>() == 1);
const _: () = assert!(size_of::<[[u8; EXIT_CLASS_COUNT]; INVOCATION_MODE_COUNT]>() == 45);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum CommandKind {
  Help,
  Version,
  SdkVersion,
  SdkInfo,
  Sdk,
  Project,
  Build,
  Restore,
  Compat,
  Init,
  Add,
  Remove,
  Run,
  Test,
  Pack,
  Publish,
  DotnetList,
  NugetRestore,
  NugetPack,
  NugetPush,
  NugetList,
  NugetAdd,
  NugetRemove,
  NugetUpdate,
  MsbuildInput,
  VstestInput,
  Unknown,
  InvalidText,
  InvalidOptions,
}

const _: () = assert!(size_of::<CommandKind>() == 1);
const _: () = assert!(align_of::<CommandKind>() == 1);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum ColorChoice {
  #[default]
  Auto,
  Always,
  Never,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub(crate) enum DiagnosticVerbosity {
  Quiet,
  Minimal,
  #[default]
  Normal,
  Detailed,
  Diagnostic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum EnvironmentSetting<T> {
  Missing,
  Value(T),
  Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InvocationEnvironment {
  color: EnvironmentSetting<ColorChoice>,
  verbosity: EnvironmentSetting<DiagnosticVerbosity>,
  no_color: bool,
}

const _: () = assert!(size_of::<InvocationEnvironment>() == 5);
const _: () = assert!(align_of::<InvocationEnvironment>() == 1);

impl InvocationEnvironment {
  fn capture() -> Self {
    Self::capture_with(|name| env::var_os(name))
  }

  fn capture_with(mut lookup: impl FnMut(&str) -> Option<OsString>) -> Self {
    let color = lookup("DV_COLOR").map_or(EnvironmentSetting::Missing, |value| match value.to_str() {
      Some("auto") => EnvironmentSetting::Value(ColorChoice::Auto),
      Some("always") => EnvironmentSetting::Value(ColorChoice::Always),
      Some("never") => EnvironmentSetting::Value(ColorChoice::Never),
      _ => EnvironmentSetting::Invalid,
    });
    let verbosity = lookup("DV_VERBOSITY").map_or(EnvironmentSetting::Missing, |value| match value.to_str() {
      Some("quiet") => EnvironmentSetting::Value(DiagnosticVerbosity::Quiet),
      Some("minimal") => EnvironmentSetting::Value(DiagnosticVerbosity::Minimal),
      Some("normal") => EnvironmentSetting::Value(DiagnosticVerbosity::Normal),
      Some("detailed") => EnvironmentSetting::Value(DiagnosticVerbosity::Detailed),
      Some("diagnostic") => EnvironmentSetting::Value(DiagnosticVerbosity::Diagnostic),
      _ => EnvironmentSetting::Invalid,
    });
    let no_color = lookup("NO_COLOR").is_some_and(|value| !value.is_empty());

    Self { color, verbosity, no_color }
  }
}

impl Default for InvocationEnvironment {
  fn default() -> Self {
    Self {
      color: EnvironmentSetting::Missing,
      verbosity: EnvironmentSetting::Missing,
      no_color: false,
    }
  }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct GlobalOptions {
  color: ColorChoice,
  verbosity: DiagnosticVerbosity,
  json: bool,
}

impl GlobalOptions {
  pub(crate) fn json(self) -> bool {
    self.json
  }

  pub(crate) fn color(self) -> ColorChoice {
    self.color
  }

  pub(crate) fn verbosity(self) -> DiagnosticVerbosity {
    self.verbosity
  }
}

const _: () = assert!(size_of::<GlobalOptions>() == 3);
const _: () = assert!(align_of::<GlobalOptions>() == 1);

const MODE_EXPLICIT: u8 = 1 << 0;
const COLOR_EXPLICIT: u8 = 1 << 1;
const VERBOSITY_EXPLICIT: u8 = 1 << 2;
const OPTIONS_CLOSED: u8 = 1 << 3;
const COMMAND_LITERAL: u8 = 1 << 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InvocationScan {
  globals: GlobalOptions,
  mode: InvocationMode,
  explicit: u8,
}

impl InvocationScan {
  fn is_explicit(self, dimension: u8) -> bool {
    self.explicit & dimension != 0
  }

  fn select_mode(&mut self, value: &OsStr) -> Result<(), String> {
    if self.is_explicit(MODE_EXPLICIT) {
      self.mode = InvocationMode::Native;
      return Err("--compat may be specified only once".into());
    }
    self.mode = parse_compatibility_mode(value)?;
    self.explicit |= MODE_EXPLICIT;
    Ok(())
  }
}

impl Default for InvocationScan {
  fn default() -> Self {
    Self {
      globals: GlobalOptions::default(),
      mode: InvocationMode::Native,
      explicit: 0,
    }
  }
}

const _: () = assert!(size_of::<InvocationScan>() == 5);
const _: () = assert!(align_of::<InvocationScan>() == 1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InvocationOptions {
  mode: InvocationMode,
  globals: GlobalOptions,
}

impl InvocationOptions {
  pub(crate) fn json(self) -> bool {
    self.globals.json()
  }

  pub(crate) fn color(self) -> ColorChoice {
    self.globals.color()
  }

  pub(crate) fn verbosity(self) -> DiagnosticVerbosity {
    self.globals.verbosity()
  }

  pub(crate) fn compatibility_profile(self) -> Option<&'static str> {
    self.mode.profile()
  }

  pub(crate) fn argument_is_option(self, argument: &OsStr) -> bool {
    self.mode.argument_is_option(argument)
  }

  pub(crate) fn argument_is_help(self, argument: &OsStr) -> bool {
    argument.to_str().is_some_and(|argument| command_help_token(self.mode, argument))
  }

  pub(crate) const fn exit_code(self, class: ExitClass) -> Option<u8> {
    let code = EXIT_CODES[self.mode as usize][class as usize];
    if code == EXIT_NOT_APPLICABLE { None } else { Some(code) }
  }
}

const _: () = assert!(size_of::<InvocationOptions>() == 4);
const _: () = assert!(align_of::<InvocationOptions>() == 1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InvocationRequest {
  syntax_version: CommandSyntaxVersion,
  command: CommandKind,
  globals: GlobalOptions,
}

impl InvocationRequest {
  pub(crate) fn syntax_version(self) -> CommandSyntaxVersion {
    self.syntax_version
  }

  pub(crate) fn command(self) -> CommandKind {
    self.command
  }
}

const _: () = assert!(size_of::<InvocationRequest>() == 6);
const _: () = assert!(align_of::<InvocationRequest>() == 2);

pub(crate) struct InvocationBatch {
  raw_arguments: RawArguments,
  semantic_indices: Option<SemanticIndices>,
  forwarded_index: Option<NonZeroUsize>,
  command_index: usize,
  request: InvocationRequest,
  mode: InvocationMode,
  option_error: Option<String>,
}

const _: () = assert!(size_of::<Option<NonZeroUsize>>() == size_of::<usize>());

const INLINE_SEMANTIC_ARGUMENTS: usize = 16;

enum SemanticIndices {
  Inline { values: [u16; INLINE_SEMANTIC_ARGUMENTS], len: u8 },
  Heap(Vec<usize>),
}

impl SemanticIndices {
  fn new() -> Self {
    Self::Inline {
      values: [0; INLINE_SEMANTIC_ARGUMENTS],
      len: 0,
    }
  }

  fn push(&mut self, index: usize) {
    match self {
      Self::Inline { values, len } if usize::from(*len) < values.len() && u16::try_from(index).is_ok() => {
        values[usize::from(*len)] = index as u16;
        *len += 1;
      },
      Self::Inline { values, len } => {
        let mut indices = Vec::with_capacity(usize::from(*len) + 1);
        indices.extend(values[..usize::from(*len)].iter().map(|value| usize::from(*value)));
        indices.push(index);
        *self = Self::Heap(indices);
      },
      Self::Heap(indices) => {
        indices.push(index);
      },
    }
  }

  fn len(&self) -> usize {
    match self {
      Self::Inline { len, .. } => usize::from(*len),
      Self::Heap(indices) => indices.len(),
    }
  }

  fn get(&self, index: usize) -> Option<usize> {
    match self {
      Self::Inline { values, len } => (index < usize::from(*len)).then(|| usize::from(values[index])),
      Self::Heap(indices) => indices.get(index).copied(),
    }
  }
}

enum RawArguments {
  Empty,
  One(OsString),
  Many(Box<[OsString]>),
}

impl RawArguments {
  fn capture(arguments: impl IntoIterator<Item = OsString>) -> Self {
    let mut arguments = arguments.into_iter();
    let Some(first) = arguments.next() else {
      return Self::Empty;
    };
    let Some(second) = arguments.next() else {
      return Self::One(first);
    };

    // Multi-token OS input is externally variable, so this is the one required
    // allocation that owns its exact process-lifetime encoding.
    let (lower, _) = arguments.size_hint();
    let mut owned = Vec::with_capacity(lower.saturating_add(2));
    owned.push(first);
    owned.push(second);
    owned.extend(arguments);
    Self::Many(owned.into_boxed_slice())
  }

  fn iter(&self) -> impl Iterator<Item = &OsString> {
    let (one, many) = match self {
      Self::Empty => (None, &[][..]),
      Self::One(argument) => (Some(argument), &[][..]),
      Self::Many(arguments) => (None, arguments.as_ref()),
    };
    one.into_iter().chain(many)
  }

  fn len(&self) -> usize {
    match self {
      Self::Empty => 0,
      Self::One(_) => 1,
      Self::Many(arguments) => arguments.len(),
    }
  }

  fn get(&self, index: usize) -> Option<&OsString> {
    match self {
      Self::Empty => None,
      Self::One(argument) => (index == 0).then_some(argument),
      Self::Many(arguments) => arguments.get(index),
    }
  }

  fn after(&self, index: usize) -> &[OsString] {
    match self {
      Self::Many(arguments) => arguments.get(index..).unwrap_or_default(),
      Self::Empty | Self::One(_) => &[],
    }
  }

  fn range(&self, start: usize, end: usize) -> &[OsString] {
    match self {
      Self::Many(arguments) => arguments.get(start..end).unwrap_or_default(),
      Self::Empty | Self::One(_) => &[],
    }
  }
}

impl InvocationBatch {
  #[cfg(test)]
  pub(crate) fn capture(arguments: impl IntoIterator<Item = OsString>) -> Self {
    Self::capture_with_environment(arguments, InvocationEnvironment::default())
  }

  pub(crate) fn capture_process(arguments: impl IntoIterator<Item = OsString>) -> Self {
    Self::capture_with_environment(arguments, InvocationEnvironment::capture())
  }

  fn capture_with_environment(arguments: impl IntoIterator<Item = OsString>, environment: InvocationEnvironment) -> Self {
    let raw_arguments = RawArguments::capture(arguments);
    let mut scan = InvocationScan::default();
    let mut command_index = None;
    let mut semantic_indices = None::<SemanticIndices>;
    let mut forwarded_index = None;
    let mut environment_directive_seen = false;
    let mut option_error = None;
    let mut index = 0;
    while raw_arguments.get(index).is_some() {
      if raw_arguments.get(index).is_some_and(|argument| argument == "--") {
        if let Some(command) = command_index {
          if accepts_forwarded_arguments(scan.mode, raw_arguments.get(command).expect("command index is valid")) {
            forwarded_index = NonZeroUsize::new(index + 1);
          } else if let Some(indices) = &mut semantic_indices {
            for semantic_index in index..raw_arguments.len() {
              indices.push(semantic_index);
            }
          }
          break;
        }
        scan.explicit |= OPTIONS_CLOSED | COMMAND_LITERAL;
        index += 1;
        continue;
      }
      let environment_directive = raw_arguments
        .get(index)
        .and_then(|argument| argument.to_str())
        .is_some_and(|value| value.starts_with("[env:"));
      let parsed_global = if scan.is_explicit(OPTIONS_CLOSED) {
        Ok(None)
      } else {
        parse_global_option(&raw_arguments, index, &mut scan)
      };
      match parsed_global {
        Ok(Some(width)) => {
          environment_directive_seen |= environment_directive;
          if let Some(command) = command_index
            && semantic_indices.is_none()
          {
            let mut indices = SemanticIndices::new();
            for semantic_index in command + 1..index {
              indices.push(semantic_index);
            }
            semantic_indices = Some(indices);
          }
          index += width;
        },
        Ok(None) => {
          if command_index.is_none() {
            let argument = raw_arguments.get(index).expect("global option index is valid");
            if !scan.is_explicit(OPTIONS_CLOSED)
              && scan.mode.argument_is_option(argument)
              && matches!(classify_command(scan.mode, argument), CommandKind::Unknown)
            {
              option_error = Some(format!("unknown global option {}", output::quoted_os_argument(argument)));
              break;
            }
            command_index = Some(index);
          } else if let Some(indices) = &mut semantic_indices {
            indices.push(index);
          }
          index += 1;
        },
        Err(error) => {
          option_error = Some(error);
          break;
        },
      }
    }
    if option_error.is_none() && !scan.is_explicit(COLOR_EXPLICIT) {
      match environment.color {
        EnvironmentSetting::Missing => {
          if environment.no_color {
            scan.globals.color = ColorChoice::Never;
          }
        },
        EnvironmentSetting::Value(color) => scan.globals.color = color,
        EnvironmentSetting::Invalid => option_error = Some("DV_COLOR must be auto, always, or never".into()),
      }
    }
    if option_error.is_none() && !scan.is_explicit(VERBOSITY_EXPLICIT) {
      match environment.verbosity {
        EnvironmentSetting::Missing => {},
        EnvironmentSetting::Value(verbosity) => scan.globals.verbosity = verbosity,
        EnvironmentSetting::Invalid => option_error = Some("DV_VERBOSITY must be quiet, minimal, normal, detailed, or diagnostic".into()),
      }
    }
    if option_error.is_none()
      && environment_directive_seen
      && !command_index.is_some_and(|index| accepts_forwarded_arguments(scan.mode, raw_arguments.get(index).expect("command index is valid")))
    {
      option_error = Some("environment directives are supported only by run and test".into());
    }
    if option_error.is_none() && scan.globals.json && scan.is_explicit(COLOR_EXPLICIT) {
      option_error = Some("explicit color options cannot be combined with --json".into());
    }
    let command = if option_error.is_some() {
      CommandKind::InvalidOptions
    } else {
      command_index.map_or(CommandKind::Help, |index| {
        let command = raw_arguments.get(index).expect("classified argument index is valid");
        if scan.is_explicit(COMMAND_LITERAL) {
          if command.to_str().is_some() {
            CommandKind::Unknown
          } else {
            CommandKind::InvalidText
          }
        } else {
          classify_command(scan.mode, command)
        }
      })
    };
    Self {
      raw_arguments,
      semantic_indices,
      forwarded_index,
      command_index: command_index.unwrap_or(usize::MAX),
      request: InvocationRequest {
        syntax_version: COMMAND_SYNTAX_VERSION,
        command,
        globals: scan.globals,
      },
      mode: scan.mode,
      option_error,
    }
  }

  pub(crate) fn request(&self) -> InvocationRequest {
    self.request
  }

  pub(crate) fn options(&self) -> InvocationOptions {
    InvocationOptions {
      mode: self.mode,
      globals: self.request.globals,
    }
  }

  pub(crate) fn command_text(&self) -> Option<&str> {
    self.command_os().and_then(OsStr::to_str)
  }

  pub(crate) fn command_os(&self) -> Option<&OsStr> {
    self.raw_arguments.get(self.command_index).map(OsString::as_os_str)
  }

  pub(crate) fn command_arguments(&self) -> CommandArguments<'_> {
    let start = self.command_index.saturating_add(1);
    let end = self.forwarded_index.map(|index| index.get() - 1);
    CommandArguments {
      storage: self.semantic_indices.as_ref().map_or_else(
        || CommandArgumentStorage::Direct(end.map_or_else(|| self.raw_arguments.after(start), |end| self.raw_arguments.range(start, end))),
        |indices| CommandArgumentStorage::Indexed {
          raw: &self.raw_arguments,
          indices,
        },
      ),
      start: 0,
    }
  }

  pub(crate) fn forwarded_arguments(&self) -> Option<ForwardedArguments<'_>> {
    self.forwarded_index.map(|index| ForwardedArguments {
      values: self.raw_arguments.after(index.get()),
    })
  }

  pub(crate) fn environment_directives(&self) -> impl Iterator<Item = &str> {
    let end = self.forwarded_index.map_or(self.raw_arguments.len(), |index| index.get() - 1);
    self
      .raw_arguments
      .iter()
      .take(end)
      .filter_map(|argument| argument.to_str().and_then(|value| directive_assignment(value).ok().flatten()))
  }

  pub(crate) fn option_error(&self) -> Option<&str> {
    self.option_error.as_deref()
  }

  pub(crate) fn event_arguments(&self, include: bool) -> Vec<String> {
    if include {
      output::redact_argument_batch(self.raw_arguments.iter().map(OsString::as_os_str), self.raw_arguments.len())
    } else {
      Vec::new()
    }
  }

  #[cfg(test)]
  fn raw_arguments(&self) -> Vec<&OsString> {
    self.raw_arguments.iter().collect()
  }
}

#[derive(Clone, Copy)]
pub(crate) struct ForwardedArguments<'a> {
  values: &'a [OsString],
}

const _: () = assert!(size_of::<ForwardedArguments<'static>>() == 16);
const _: () = assert!(align_of::<ForwardedArguments<'static>>() == align_of::<usize>());

impl<'a> ForwardedArguments<'a> {
  pub(crate) fn as_slice(self) -> &'a [OsString] {
    self.values
  }
}

#[derive(Clone, Copy)]
pub(crate) struct CommandArguments<'a> {
  storage: CommandArgumentStorage<'a>,
  start: usize,
}

#[derive(Clone, Copy)]
enum CommandArgumentStorage<'a> {
  Direct(&'a [OsString]),
  Indexed { raw: &'a RawArguments, indices: &'a SemanticIndices },
}

const _: () = assert!(size_of::<CommandArguments<'static>>() == 32);
const _: () = assert!(align_of::<CommandArguments<'static>>() == align_of::<usize>());

impl<'a> CommandArguments<'a> {
  pub(crate) fn len(self) -> usize {
    let len = match self.storage {
      CommandArgumentStorage::Direct(arguments) => arguments.len(),
      CommandArgumentStorage::Indexed { indices, .. } => indices.len(),
    };
    len.saturating_sub(self.start)
  }

  pub(crate) fn is_empty(self) -> bool {
    self.len() == 0
  }

  pub(crate) fn get(self, index: usize) -> Option<&'a OsStr> {
    let index = self.start.checked_add(index)?;
    match self.storage {
      CommandArgumentStorage::Direct(arguments) => arguments.get(index).map(OsString::as_os_str),
      CommandArgumentStorage::Indexed { raw, indices } => raw.get(indices.get(index)?).map(OsString::as_os_str),
    }
  }

  pub(crate) fn first(self) -> Option<&'a OsStr> {
    self.get(0)
  }

  pub(crate) fn slice_from(self, start: usize) -> Self {
    Self {
      start: self.start.saturating_add(start).min(self.start + self.len()),
      ..self
    }
  }

  pub(crate) fn iter(self) -> CommandArgumentIter<'a> {
    CommandArgumentIter { arguments: self, index: 0 }
  }
}

impl Index<usize> for CommandArguments<'_> {
  type Output = OsStr;

  fn index(&self, index: usize) -> &Self::Output {
    (*self).get(index).expect("command argument index is in range")
  }
}

pub(crate) struct CommandArgumentIter<'a> {
  arguments: CommandArguments<'a>,
  index: usize,
}

impl<'a> Iterator for CommandArgumentIter<'a> {
  type Item = &'a OsStr;

  fn next(&mut self) -> Option<Self::Item> {
    let argument = self.arguments.get(self.index)?;
    self.index += 1;
    Some(argument)
  }
}

fn parse_global_option(arguments: &RawArguments, index: usize, scan: &mut InvocationScan) -> Result<Option<usize>, String> {
  let argument = arguments.get(index).expect("global option index is valid");
  match argument.to_str() {
    Some("--json") => scan.globals.json = true,
    Some("--verbose") => {
      scan.globals.verbosity = DiagnosticVerbosity::Detailed;
      scan.explicit |= VERBOSITY_EXPLICIT;
    },
    Some("--quiet") => {
      scan.globals.verbosity = DiagnosticVerbosity::Quiet;
      scan.explicit |= VERBOSITY_EXPLICIT;
    },
    Some("--color") => {
      scan.globals.color = ColorChoice::Always;
      scan.explicit |= COLOR_EXPLICIT;
    },
    Some("--no-color") => {
      scan.globals.color = ColorChoice::Never;
      scan.explicit |= COLOR_EXPLICIT;
    },
    Some("--compat") => {
      let value = arguments.get(index + 1).ok_or("--compat requires dotnet, msbuild, nuget, or vstest")?;
      scan.select_mode(value)?;
      return Ok(Some(2));
    },
    Some(value) if value.starts_with("--compat=") => {
      scan.select_mode(OsStr::new(&value["--compat=".len()..]))?;
    },
    Some("--verbosity") => {
      let value = arguments
        .get(index + 1)
        .ok_or("--verbosity requires quiet, minimal, normal, detailed, or diagnostic")?;
      scan.globals.verbosity = parse_verbosity(value)?;
      scan.explicit |= VERBOSITY_EXPLICIT;
      return Ok(Some(2));
    },
    Some(value) if value.starts_with("--verbosity=") => {
      scan.globals.verbosity = parse_verbosity(OsStr::new(&value["--verbosity=".len()..]))?;
      scan.explicit |= VERBOSITY_EXPLICIT;
    },
    Some(value) if value.starts_with("[env:") => {
      directive_assignment(value).map_err(|error| error.to_string())?;
    },
    _ => return Ok(None),
  }
  Ok(Some(1))
}

fn parse_compatibility_mode(value: &OsStr) -> Result<InvocationMode, String> {
  match value.to_str() {
    Some("dotnet") => Ok(InvocationMode::Dotnet),
    Some("msbuild") => Ok(InvocationMode::Msbuild),
    Some("nuget") => Ok(InvocationMode::Nuget),
    Some("vstest") => Ok(InvocationMode::Vstest),
    _ => Err(format!("unsupported compatibility mode {:?}", output::redact_argument_text(value))),
  }
}

fn parse_verbosity(value: &OsStr) -> Result<DiagnosticVerbosity, String> {
  match value.to_str() {
    Some("quiet") => Ok(DiagnosticVerbosity::Quiet),
    Some("minimal") => Ok(DiagnosticVerbosity::Minimal),
    Some("normal") => Ok(DiagnosticVerbosity::Normal),
    Some("detailed") => Ok(DiagnosticVerbosity::Detailed),
    Some("diagnostic") => Ok(DiagnosticVerbosity::Diagnostic),
    _ => Err(format!("unsupported diagnostic verbosity {:?}", output::redact_argument_text(value))),
  }
}

const AMBIGUOUS_COMMAND_PRECEDENCE: [[CommandKind; 5]; 7] = [
  [
    CommandKind::Restore,
    CommandKind::Restore,
    CommandKind::MsbuildInput,
    CommandKind::NugetRestore,
    CommandKind::VstestInput,
  ],
  [
    CommandKind::Pack,
    CommandKind::Pack,
    CommandKind::MsbuildInput,
    CommandKind::NugetPack,
    CommandKind::VstestInput,
  ],
  [
    CommandKind::Unknown,
    CommandKind::Unknown,
    CommandKind::MsbuildInput,
    CommandKind::NugetPush,
    CommandKind::VstestInput,
  ],
  [
    CommandKind::DotnetList,
    CommandKind::DotnetList,
    CommandKind::MsbuildInput,
    CommandKind::NugetList,
    CommandKind::VstestInput,
  ],
  [
    CommandKind::Add,
    CommandKind::Add,
    CommandKind::MsbuildInput,
    CommandKind::NugetAdd,
    CommandKind::VstestInput,
  ],
  [
    CommandKind::Remove,
    CommandKind::Remove,
    CommandKind::MsbuildInput,
    CommandKind::NugetRemove,
    CommandKind::VstestInput,
  ],
  [
    CommandKind::Unknown,
    CommandKind::Unknown,
    CommandKind::MsbuildInput,
    CommandKind::NugetUpdate,
    CommandKind::VstestInput,
  ],
];

const _: () = assert!(InvocationMode::Native as usize == 0);
const _: () = assert!(InvocationMode::Dotnet as usize == 1);
const _: () = assert!(InvocationMode::Msbuild as usize == 2);
const _: () = assert!(InvocationMode::Nuget as usize == 3);
const _: () = assert!(InvocationMode::Vstest as usize == 4);
const _: () = assert!(size_of::<[[CommandKind; 5]; 7]>() == 35);
const _: () = assert!(align_of::<[[CommandKind; 5]; 7]>() == 1);

fn classify_command(mode: InvocationMode, command: &OsStr) -> CommandKind {
  let Some(command) = command.to_str() else {
    return CommandKind::InvalidText;
  };
  if root_help_token(mode, command) {
    return CommandKind::Help;
  }
  if let Some(row) = ambiguous_command_row(command).or_else(|| nuget_ambiguous_command_row(mode, command)) {
    return AMBIGUOUS_COMMAND_PRECEDENCE[row][mode as usize];
  }
  match mode {
    InvocationMode::Native => classify_native_command(command),
    InvocationMode::Dotnet => classify_dotnet_compatibility_command(command),
    InvocationMode::Nuget => CommandKind::Unknown,
    InvocationMode::Msbuild => CommandKind::MsbuildInput,
    InvocationMode::Vstest => CommandKind::VstestInput,
  }
}

fn root_help_token(mode: InvocationMode, argument: &str) -> bool {
  if command_help_token(mode, argument) {
    return true;
  }
  matches!(mode, InvocationMode::Native | InvocationMode::Dotnet) && argument == "help"
}

fn command_help_token(mode: InvocationMode, argument: &str) -> bool {
  let dash = match mode {
    InvocationMode::Native => matches!(argument, "help" | "-h" | "--help"),
    InvocationMode::Dotnet => matches!(argument, "-h" | "-?" | "--help"),
    InvocationMode::Msbuild => matches!(argument, "-?" | "-h") || argument.eq_ignore_ascii_case("-help") || argument.eq_ignore_ascii_case("--help"),
    InvocationMode::Nuget => matches!(argument, "-?" | "-h" | "--help"),
    InvocationMode::Vstest => matches!(argument, "-?" | "-h") || argument.eq_ignore_ascii_case("--help"),
  };
  dash || windows_slash_help_token(mode, argument)
}

#[cfg(windows)]
fn windows_slash_help_token(mode: InvocationMode, argument: &str) -> bool {
  match mode {
    InvocationMode::Dotnet => argument == "/?",
    InvocationMode::Msbuild | InvocationMode::Vstest => matches!(argument, "/?" | "/h") || argument.eq_ignore_ascii_case("/help"),
    InvocationMode::Nuget => argument == "/?",
    InvocationMode::Native => false,
  }
}

#[cfg(not(windows))]
const fn windows_slash_help_token(_mode: InvocationMode, _argument: &str) -> bool {
  false
}

fn nuget_ambiguous_command_row(mode: InvocationMode, command: &str) -> Option<usize> {
  if mode != InvocationMode::Nuget {
    return None;
  }
  if command.eq_ignore_ascii_case("restore") {
    Some(0)
  } else if command.eq_ignore_ascii_case("pack") {
    Some(1)
  } else if command.eq_ignore_ascii_case("push") {
    Some(2)
  } else if command.eq_ignore_ascii_case("list") {
    Some(3)
  } else if command.eq_ignore_ascii_case("add") {
    Some(4)
  } else if command.eq_ignore_ascii_case("remove") {
    Some(5)
  } else if command.eq_ignore_ascii_case("update") {
    Some(6)
  } else {
    None
  }
}

fn ambiguous_command_row(command: &str) -> Option<usize> {
  match command {
    "restore" => Some(0),
    "pack" => Some(1),
    "push" => Some(2),
    "list" => Some(3),
    "add" => Some(4),
    "remove" => Some(5),
    "update" => Some(6),
    _ => None,
  }
}

fn classify_native_command(command: &str) -> CommandKind {
  match command {
    "-h" | "--help" | "help" => CommandKind::Help,
    "-V" | "--version" | "version" => CommandKind::Version,
    "sdk" => CommandKind::Sdk,
    "project" => CommandKind::Project,
    "build" => CommandKind::Build,
    "sync" => CommandKind::Restore,
    "compat" => CommandKind::Compat,
    "init" => CommandKind::Init,
    "run" => CommandKind::Run,
    "test" => CommandKind::Test,
    "publish" => CommandKind::Publish,
    _ => CommandKind::Unknown,
  }
}

fn classify_dotnet_compatibility_command(command: &str) -> CommandKind {
  match command {
    "--version" => CommandKind::SdkVersion,
    "--info" => CommandKind::SdkInfo,
    "-V" | "version" => CommandKind::Unknown,
    _ => classify_native_command(command),
  }
}

fn accepts_forwarded_arguments(mode: InvocationMode, command: &OsStr) -> bool {
  matches!(classify_command(mode, command), CommandKind::Run | CommandKind::Test)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn captures_one_lossless_batch_and_classifies_before_execution() {
    let batch = InvocationBatch::capture(["--json", "restore", "", "App.csproj"].map(OsString::from));

    assert_eq!(batch.request().syntax_version, COMMAND_SYNTAX_VERSION);
    assert_eq!(batch.mode, InvocationMode::Native);
    assert_eq!(batch.request().command, CommandKind::Restore);
    assert!(batch.options().json());
    assert_eq!(batch.command_text(), Some("restore"));
    assert_eq!(batch.command_arguments().iter().collect::<Vec<_>>(), [OsStr::new(""), OsStr::new("App.csproj")]);
    assert_eq!(batch.raw_arguments(), ["--json", "restore", "", "App.csproj"]);
    assert!(batch.event_arguments(false).is_empty());
    assert_eq!(batch.event_arguments(true), ["--json", "restore", "", "App.csproj"]);
  }

  #[test]
  fn command_line_output_controls_override_the_typed_environment_batch() {
    let environment = InvocationEnvironment::capture_with(|name| match name {
      "DV_COLOR" => Some("always".into()),
      "DV_VERBOSITY" => Some("diagnostic".into()),
      "NO_COLOR" => Some("present".into()),
      _ => None,
    });
    let inherited = InvocationBatch::capture_with_environment([OsString::from("version")], environment);
    let overridden = InvocationBatch::capture_with_environment(["--no-color", "--verbosity", "minimal", "version"].map(OsString::from), environment);

    assert_eq!(inherited.options().color(), ColorChoice::Always);
    assert_eq!(inherited.options().verbosity(), DiagnosticVerbosity::Diagnostic);
    assert_eq!(overridden.options().color(), ColorChoice::Never);
    assert_eq!(overridden.options().verbosity(), DiagnosticVerbosity::Minimal);

    let standard = InvocationEnvironment::capture_with(|name| (name == "NO_COLOR").then(|| "1".into()));
    let no_color = InvocationBatch::capture_with_environment([OsString::from("version")], standard);
    assert_eq!(no_color.options().color(), ColorChoice::Never);
  }

  #[test]
  fn overridden_environment_errors_never_retain_the_supplied_value() {
    let environment = InvocationEnvironment::capture_with(|name| match name {
      "DV_COLOR" => Some("color-environment-secret".into()),
      "DV_VERBOSITY" => Some("verbosity-environment-secret".into()),
      _ => None,
    });
    let overridden = InvocationBatch::capture_with_environment(["--color", "--quiet", "version"].map(OsString::from), environment);
    assert_eq!(overridden.request().command(), CommandKind::Version);
    assert!(overridden.option_error().is_none());

    let invalid = InvocationBatch::capture_with_environment([OsString::from("version")], environment);
    assert_eq!(invalid.request().command(), CommandKind::InvalidOptions);
    assert_eq!(invalid.option_error(), Some("DV_COLOR must be auto, always, or never"));
    assert!(!invalid.option_error().unwrap().contains("environment-secret"));

    let non_unicode = InvocationEnvironment::capture_with(|name| (name == "DV_VERBOSITY").then(non_unicode_argument));
    let invalid = InvocationBatch::capture_with_environment([OsString::from("version")], non_unicode);
    assert_eq!(invalid.request().command(), CommandKind::InvalidOptions);
    assert_eq!(
      invalid.option_error(),
      Some("DV_VERBOSITY must be quiet, minimal, normal, detailed, or diagnostic")
    );
  }

  #[test]
  fn structured_argument_reporting_redacts_sensitive_shapes_in_place() {
    let batch = InvocationBatch::capture(
      [
        "--json",
        "frobnicate",
        "--api-key",
        "separate-secret",
        "--client-secret=joined-secret",
        "-p:Password=property-secret",
        "NuGetPackageSourceCredentials_private=credential-secret",
        "https://user:password@example.test/v3/index.json?sig=query-secret#fragment",
        "ordinary",
      ]
      .map(OsString::from),
    );

    assert_eq!(
      batch.event_arguments(true),
      [
        "--json",
        "frobnicate",
        "--api-key",
        "<redacted>",
        "--client-secret=<redacted>",
        "-p:Password=<redacted>",
        "NuGetPackageSourceCredentials_private=<redacted>",
        "https://example.test/v3/index.json",
        "ordinary",
      ]
    );
  }

  #[test]
  fn unknown_combined_secret_options_are_redacted_before_diagnostics() {
    let batch = InvocationBatch::capture([OsString::from("--api-key=diagnostic-secret")]);

    assert_eq!(batch.request().command(), CommandKind::InvalidOptions);
    assert_eq!(batch.option_error(), Some("unknown global option \"--api-key=<redacted>\""));
  }

  #[test]
  fn environment_directives_are_typed_globals_but_forwarded_values_stay_opaque() {
    let batch = InvocationBatch::capture(
      [
        "[env:PUBLIC=directive]",
        "run",
        "--environment",
        "DV_TOKEN=command-secret",
        "--",
        "[env:FORWARDED_TOKEN=opaque-secret]",
      ]
      .map(OsString::from),
    );

    assert_eq!(batch.environment_directives().collect::<Vec<_>>(), ["PUBLIC=directive"]);
    assert_eq!(
      batch.command_arguments().iter().collect::<Vec<_>>(),
      [OsStr::new("--environment"), OsStr::new("DV_TOKEN=command-secret")]
    );
    assert_eq!(
      batch.forwarded_arguments().unwrap().as_slice(),
      [OsString::from("[env:FORWARDED_TOKEN=opaque-secret]")]
    );
    assert_eq!(
      batch.event_arguments(true),
      [
        "[env:PUBLIC=directive]",
        "run",
        "--environment",
        "DV_TOKEN=<redacted>",
        "--",
        "[env:FORWARDED_TOKEN=<redacted>]",
      ]
    );
  }

  #[test]
  fn malformed_environment_directives_fail_without_echoing_values() {
    for directive in ["[env:MISSING]", "[env:=directive-secret]", "[env:TOKEN=unterminated-secret"] {
      let batch = InvocationBatch::capture([OsString::from(directive), OsString::from("run")]);
      assert_eq!(batch.request().command(), CommandKind::InvalidOptions);
      assert!(!batch.option_error().unwrap().contains("secret"));
    }
  }

  #[test]
  fn environment_directives_reject_non_child_commands_instead_of_becoming_noops() {
    let batch = InvocationBatch::capture([OsString::from("[env:PUBLIC=value]"), OsString::from("sdk"), OsString::from("current")]);

    assert_eq!(batch.request().command(), CommandKind::InvalidOptions);
    assert_eq!(batch.option_error(), Some("environment directives are supported only by run and test"));
  }

  #[test]
  fn delimiter_splits_one_borrowed_lossless_forwarding_batch() {
    let opaque = non_unicode_argument();
    let batch = InvocationBatch::capture([
      OsString::from("run"),
      OsString::from("--quiet"),
      OsString::from("project.csproj"),
      OsString::from("--"),
      OsString::from("--json"),
      OsString::from(""),
      OsString::from("--"),
      OsString::from("--compat=msbuild"),
      opaque.clone(),
    ]);

    assert_eq!(batch.request().command, CommandKind::Run);
    assert_eq!(batch.options().verbosity(), DiagnosticVerbosity::Quiet);
    assert_eq!(batch.command_arguments().iter().collect::<Vec<_>>(), [OsStr::new("project.csproj")]);
    let forwarded = batch.forwarded_arguments().expect("delimiter creates a forwarded batch");
    assert_eq!(batch.mode, InvocationMode::Native);
    assert_eq!(forwarded.as_slice().len(), 5);
    assert_eq!(
      forwarded.as_slice(),
      [
        OsString::from("--json"),
        OsString::from(""),
        OsString::from("--"),
        OsString::from("--compat=msbuild"),
        opaque
      ]
    );
  }

  #[test]
  fn empty_forwarding_tail_is_distinct_from_no_delimiter() {
    let delimited = InvocationBatch::capture([OsString::from("test"), OsString::from("--")]);
    let plain = InvocationBatch::capture([OsString::from("test")]);

    assert!(delimited.forwarded_arguments().is_some_and(|arguments| arguments.as_slice().is_empty()));
    assert!(plain.forwarded_arguments().is_none());
  }

  #[test]
  fn delimiter_stops_global_parsing_for_non_child_commands_without_losing_tokens() {
    let batch = InvocationBatch::capture(["--compat", "dotnet", "build", "--quiet", "--", "--json", "", "--compat=nuget"].map(OsString::from));

    assert_eq!(batch.request().command(), CommandKind::Build);
    assert_eq!(batch.options().mode, InvocationMode::Dotnet);
    assert_eq!(batch.options().verbosity(), DiagnosticVerbosity::Quiet);
    assert_eq!(
      batch.command_arguments().iter().collect::<Vec<_>>(),
      [OsStr::new("--"), OsStr::new("--json"), OsStr::new(""), OsStr::new("--compat=nuget")]
    );
    assert!(batch.forwarded_arguments().is_none());
  }

  #[test]
  fn leading_delimiter_keeps_the_following_command_literal() {
    let option_command = InvocationBatch::capture(["--", "--version", "--json", ""].map(OsString::from));
    let named_command = InvocationBatch::capture(["--compat=dotnet", "--", "build", "--json"].map(OsString::from));

    assert_eq!(option_command.request().command(), CommandKind::Unknown);
    assert_eq!(option_command.command_text(), Some("--version"));
    assert!(!option_command.options().json());
    assert_eq!(
      option_command.command_arguments().iter().collect::<Vec<_>>(),
      [OsStr::new("--json"), OsStr::new("")]
    );
    assert_eq!(named_command.request().command(), CommandKind::Unknown);
    assert!(!named_command.options().json());
    assert_eq!(named_command.command_arguments().first(), Some(OsStr::new("--json")));
  }

  #[test]
  fn leading_delimiter_still_rejects_non_unicode_command_text() {
    let batch = InvocationBatch::capture([OsString::from("--"), non_unicode_argument()]);

    assert_eq!(batch.request().command(), CommandKind::InvalidText);
  }

  #[test]
  fn delimiter_keeps_the_direct_non_child_argument_slice() {
    let batch = InvocationBatch::capture(["restore", "App.csproj", "--", "--json", "two words", r#"a"b"#, ""].map(OsString::from));

    assert!(batch.semantic_indices.is_none());
    assert_eq!(
      batch.command_arguments().iter().collect::<Vec<_>>(),
      [
        OsStr::new("App.csproj"),
        OsStr::new("--"),
        OsStr::new("--json"),
        OsStr::new("two words"),
        OsStr::new(r#"a"b"#),
        OsStr::new("")
      ]
    );
  }

  #[test]
  fn large_forwarding_tail_remains_one_direct_slice() {
    let mut arguments = vec![OsString::from("run"), OsString::from("--")];
    arguments.extend((0..64).map(|index| OsString::from(format!("value-{index}"))));
    let batch = InvocationBatch::capture(arguments);

    assert!(batch.semantic_indices.is_none());
    assert_eq!(batch.command_arguments().len(), 0);
    let forwarded = batch.forwarded_arguments().expect("delimiter creates a forwarded batch");
    assert_eq!(forwarded.as_slice().len(), 64);
    assert_eq!(forwarded.as_slice().first().map(OsString::as_os_str), Some(OsStr::new("value-0")));
    assert_eq!(forwarded.as_slice().last().map(OsString::as_os_str), Some(OsStr::new("value-63")));
  }

  #[cfg(unix)]
  fn non_unicode_argument() -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(vec![0xff, b'x'])
  }

  #[cfg(windows)]
  fn non_unicode_argument() -> OsString {
    use std::os::windows::ffi::OsStringExt;
    OsString::from_wide(&[0xd800, b'x' as u16])
  }

  #[test]
  fn empty_and_global_only_batches_are_help_requests() {
    for arguments in [Vec::new(), vec![OsString::from("--json")]] {
      let batch = InvocationBatch::capture(arguments);
      assert_eq!(batch.request().command, CommandKind::Help);
      assert!(batch.command_os().is_none());
    }
  }

  #[test]
  fn single_token_fast_paths_do_not_allocate_a_token_container() {
    let batch = InvocationBatch::capture([OsString::from("--version")]);

    assert!(matches!(batch.raw_arguments, RawArguments::One(_)));
    assert_eq!(batch.request().command, CommandKind::Version);
  }

  #[test]
  fn compatibility_manifest_query_has_a_typed_command_kind() {
    let batch = InvocationBatch::capture([OsString::from("compat"), OsString::from("manifest")]);

    assert_eq!(batch.request().command, CommandKind::Compat);
    assert_eq!(batch.command_arguments().iter().collect::<Vec<_>>(), [OsStr::new("manifest")]);
  }

  #[test]
  fn every_accepted_spelling_normalizes_without_losing_the_raw_command() {
    let cases = [
      ("-h", CommandKind::Help),
      ("--help", CommandKind::Help),
      ("help", CommandKind::Help),
      ("-V", CommandKind::Version),
      ("--version", CommandKind::Version),
      ("version", CommandKind::Version),
      ("sdk", CommandKind::Sdk),
      ("project", CommandKind::Project),
      ("build", CommandKind::Build),
      ("restore", CommandKind::Restore),
      ("sync", CommandKind::Restore),
      ("compat", CommandKind::Compat),
      ("init", CommandKind::Init),
      ("add", CommandKind::Add),
      ("remove", CommandKind::Remove),
      ("run", CommandKind::Run),
      ("test", CommandKind::Test),
      ("pack", CommandKind::Pack),
      ("publish", CommandKind::Publish),
      ("list", CommandKind::DotnetList),
    ];

    for (spelling, expected) in cases {
      let batch = InvocationBatch::capture([OsString::from(spelling), OsString::from("operand")]);
      assert_eq!(batch.request().command(), expected, "{spelling}");
      assert_eq!(batch.command_text(), Some(spelling));
      assert_eq!(batch.command_arguments().first(), Some(OsStr::new("operand")));
    }
  }

  #[test]
  fn aliases_and_compatibility_provenance_share_the_semantic_request() {
    let restore = InvocationBatch::capture([OsString::from("restore"), OsString::from("App.csproj")]);
    let sync = InvocationBatch::capture([OsString::from("sync"), OsString::from("App.csproj")]);
    let dotnet = InvocationBatch::capture([
      OsString::from("--compat"),
      OsString::from("dotnet"),
      OsString::from("restore"),
      OsString::from("App.csproj"),
    ]);

    assert_eq!(restore.request(), sync.request());
    assert_eq!(restore.request(), dotnet.request());
    assert_eq!(
      restore.command_arguments().iter().collect::<Vec<_>>(),
      sync.command_arguments().iter().collect::<Vec<_>>()
    );
    assert_eq!(
      restore.command_arguments().iter().collect::<Vec<_>>(),
      dotnet.command_arguments().iter().collect::<Vec<_>>()
    );
    assert_eq!(restore.command_text(), Some("restore"));
    assert_eq!(sync.command_text(), Some("sync"));
    assert_eq!(dotnet.command_text(), Some("restore"));
    assert_eq!(restore.options().mode, InvocationMode::Native);
    assert_eq!(dotnet.options().mode, InvocationMode::Dotnet);
  }

  #[test]
  fn globals_normalize_before_and_after_the_command() {
    let batch = InvocationBatch::capture(["--quiet", "restore", "App.csproj", "--verbosity", "diagnostic", "--no-color"].map(OsString::from));

    assert_eq!(batch.request().command, CommandKind::Restore);
    assert_eq!(batch.options().verbosity(), DiagnosticVerbosity::Diagnostic);
    assert_eq!(batch.options().color(), ColorChoice::Never);
    assert_eq!(batch.command_arguments().iter().collect::<Vec<_>>(), [OsStr::new("App.csproj")]);
    assert!(matches!(batch.semantic_indices, Some(SemanticIndices::Inline { .. })));
  }

  #[test]
  fn unusually_large_semantic_batches_promote_without_losing_indices() {
    let mut arguments = vec![OsString::from("restore")];
    arguments.extend((0..17).map(|index| OsString::from(format!("Project{index}.csproj"))));
    arguments.push(OsString::from("--quiet"));
    let batch = InvocationBatch::capture(arguments);

    assert!(matches!(batch.semantic_indices, Some(SemanticIndices::Heap(_))));
    assert_eq!(batch.command_arguments().len(), 17);
  }

  #[test]
  fn malformed_and_inapplicable_global_options_are_typed_failures() {
    for arguments in [
      vec![OsString::from("--verbosity")],
      vec![OsString::from("--verbosity=loud"), OsString::from("sdk"), OsString::from("current")],
      vec![
        OsString::from("sdk"),
        OsString::from("current"),
        OsString::from("--json"),
        OsString::from("--color"),
      ],
    ] {
      let batch = InvocationBatch::capture(arguments);
      assert_eq!(batch.request().command, CommandKind::InvalidOptions);
      assert!(batch.option_error().is_some());
    }
  }

  #[test]
  fn unknown_global_options_fail_before_command_classification() {
    for arguments in [
      vec![OsString::from("--definitely-unknown")],
      vec![OsString::from("--quiet"), OsString::from("--definitely-unknown"), OsString::from("sdk")],
    ] {
      let batch = InvocationBatch::capture(arguments);
      assert_eq!(batch.request().command, CommandKind::InvalidOptions);
      assert_eq!(batch.option_error(), Some("unknown global option \"--definitely-unknown\""));
      assert!(batch.command_os().is_none());
    }

    let help = InvocationBatch::capture([OsString::from("--help")]);
    let version = InvocationBatch::capture([OsString::from("--version")]);
    assert_eq!(help.request().command, CommandKind::Help);
    assert_eq!(version.request().command, CommandKind::Version);
  }

  #[test]
  fn compatibility_mode_is_typed_and_removed_from_command_operands() {
    for (name, expected_mode, expected_command) in [
      ("dotnet", InvocationMode::Dotnet, CommandKind::Sdk),
      ("msbuild", InvocationMode::Msbuild, CommandKind::MsbuildInput),
      ("nuget", InvocationMode::Nuget, CommandKind::Unknown),
      ("vstest", InvocationMode::Vstest, CommandKind::VstestInput),
    ] {
      let combined = OsString::from(format!("--compat={name}"));
      for arguments in [
        vec![
          OsString::from("--compat"),
          OsString::from(name),
          OsString::from("sdk"),
          OsString::from("current"),
        ],
        vec![
          OsString::from("sdk"),
          OsString::from("--compat"),
          OsString::from(name),
          OsString::from("current"),
        ],
        vec![OsString::from("sdk"), OsString::from("current"), combined.clone()],
        vec![OsString::from("--quiet"), combined.clone(), OsString::from("sdk"), OsString::from("current")],
      ] {
        let batch = InvocationBatch::capture(arguments);

        assert_eq!(batch.request().command(), expected_command, "{name}");
        assert_eq!(batch.options().mode, expected_mode, "{name}");
        assert_eq!(batch.command_arguments().iter().collect::<Vec<_>>(), [OsStr::new("current")], "{name}");
      }
    }
  }

  #[test]
  fn ambiguous_words_follow_the_explicit_profile_precedence_table() {
    for (word, native, dotnet, nuget) in [
      ("restore", CommandKind::Restore, CommandKind::Restore, CommandKind::NugetRestore),
      ("pack", CommandKind::Pack, CommandKind::Pack, CommandKind::NugetPack),
      ("push", CommandKind::Unknown, CommandKind::Unknown, CommandKind::NugetPush),
      ("list", CommandKind::DotnetList, CommandKind::DotnetList, CommandKind::NugetList),
      ("add", CommandKind::Add, CommandKind::Add, CommandKind::NugetAdd),
      ("remove", CommandKind::Remove, CommandKind::Remove, CommandKind::NugetRemove),
      ("update", CommandKind::Unknown, CommandKind::Unknown, CommandKind::NugetUpdate),
    ] {
      let native_batch = InvocationBatch::capture([OsString::from(word)]);
      let dotnet_batch = InvocationBatch::capture([OsString::from("--compat=dotnet"), OsString::from(word)]);
      let nuget_batch = InvocationBatch::capture([OsString::from("--compat=nuget"), OsString::from(word)]);
      let msbuild_batch = InvocationBatch::capture([OsString::from("--compat=msbuild"), OsString::from(word)]);
      let vstest_batch = InvocationBatch::capture([OsString::from("--compat=vstest"), OsString::from(word)]);

      assert_eq!(native_batch.request().command(), native, "native {word}");
      assert_eq!(dotnet_batch.request().command(), dotnet, "dotnet {word}");
      assert_eq!(nuget_batch.request().command(), nuget, "nuget {word}");
      assert_eq!(msbuild_batch.request().command(), CommandKind::MsbuildInput, "msbuild {word}");
      assert_eq!(vstest_batch.request().command(), CommandKind::VstestInput, "vstest {word}");
    }
  }

  #[test]
  fn command_case_rules_follow_the_selected_reference_tool() {
    let dotnet = InvocationBatch::capture(["--compat=dotnet", "BUILD"].map(OsString::from));
    let native = InvocationBatch::capture([OsString::from("BUILD")]);
    let nuget = InvocationBatch::capture(["--compat=nuget", "ReStOrE"].map(OsString::from));

    assert_eq!(dotnet.request().command(), CommandKind::Unknown);
    assert_eq!(native.request().command(), CommandKind::Unknown);
    assert_eq!(nuget.request().command(), CommandKind::NugetRestore);
  }

  #[test]
  fn root_help_forms_follow_the_selected_reference_tool() {
    for (profile, accepted) in [
      ("dotnet", &["-h", "-?", "--help", "help"][..]),
      ("msbuild", &["-h", "-?", "-help", "-Help", "--help", "--Help"][..]),
      ("nuget", &["-h", "-?", "--help"][..]),
      ("vstest", &["-h", "-?", "--help", "--Help"][..]),
    ] {
      for help in accepted {
        let batch = InvocationBatch::capture([OsString::from(format!("--compat={profile}")), OsString::from(help)]);
        assert_eq!(batch.request().command(), CommandKind::Help, "profile {profile}, help {help}");
      }
    }

    for (profile, rejected) in [("dotnet", "--Help"), ("msbuild", "help"), ("nuget", "help"), ("vstest", "help")] {
      let batch = InvocationBatch::capture([OsString::from(format!("--compat={profile}")), OsString::from(rejected)]);
      assert_ne!(batch.request().command(), CommandKind::Help, "profile {profile}, help {rejected}");
    }
  }

  #[test]
  fn dotnet_version_and_info_select_sdk_queries_without_changing_native_version() {
    let native = InvocationBatch::capture([OsString::from("--version")]);
    let sdk_version = InvocationBatch::capture([OsString::from("--compat=dotnet"), OsString::from("--version")]);
    let sdk_info = InvocationBatch::capture([OsString::from("--compat=dotnet"), OsString::from("--info")]);

    assert_eq!(native.request().command(), CommandKind::Version);
    assert_eq!(InvocationBatch::capture([OsString::from("info")]).request().command(), CommandKind::Unknown);
    assert_eq!(sdk_version.request().command(), CommandKind::SdkVersion);
    assert_eq!(sdk_info.request().command(), CommandKind::SdkInfo);
    let short = InvocationBatch::capture([OsString::from("--compat=dotnet"), OsString::from("-V")]);
    let word = InvocationBatch::capture([OsString::from("--compat=dotnet"), OsString::from("version")]);
    assert_eq!(short.request().command(), CommandKind::InvalidOptions);
    assert_eq!(word.request().command(), CommandKind::Unknown);
  }

  #[cfg(windows)]
  #[test]
  fn windows_slash_help_forms_follow_the_selected_reference_tool() {
    for profile in ["dotnet", "msbuild", "nuget", "vstest"] {
      let question = InvocationBatch::capture([OsString::from(format!("--compat={profile}")), OsString::from("/?")]);
      assert_eq!(question.request().command(), CommandKind::Help, "profile {profile}");
    }
    for profile in ["msbuild", "vstest"] {
      for help in ["/help", "/Help"] {
        let batch = InvocationBatch::capture([OsString::from(format!("--compat={profile}")), OsString::from(help)]);
        assert_eq!(batch.request().command(), CommandKind::Help, "profile {profile}, help {help}");
      }
    }

    for (profile, rejected) in [("dotnet", "/help"), ("nuget", "/help")] {
      let batch = InvocationBatch::capture([OsString::from(format!("--compat={profile}")), OsString::from(rejected)]);
      assert_ne!(batch.request().command(), CommandKind::Help, "profile {profile}, help {rejected}");
    }
  }

  #[cfg(windows)]
  #[test]
  fn slash_option_prefix_is_limited_to_windows_reference_tools_that_accept_it() {
    let native = InvocationBatch::capture([OsString::from("build")]).options();
    let dotnet = InvocationBatch::capture(["--compat=dotnet", "build"].map(OsString::from)).options();
    let msbuild = InvocationBatch::capture(["--compat=msbuild", "project.csproj"].map(OsString::from)).options();
    let nuget = InvocationBatch::capture(["--compat=nuget", "restore"].map(OsString::from)).options();
    let vstest = InvocationBatch::capture(["--compat=vstest", "tests.dll"].map(OsString::from)).options();

    assert!(!native.argument_is_option(OsStr::new("/target:Build")));
    assert!(dotnet.argument_is_option(OsStr::new("/target:Build")));
    assert!(msbuild.argument_is_option(OsStr::new("/target:Build")));
    assert!(!nuget.argument_is_option(OsStr::new("/target:Build")));
    assert!(vstest.argument_is_option(OsStr::new("/Tests:Example")));
  }

  #[test]
  fn wrong_profile_run_words_do_not_activate_the_child_delimiter() {
    let batch = InvocationBatch::capture(["--compat=nuget", "run", "--", "--json", "argument"].map(OsString::from));

    assert_eq!(batch.request().command(), CommandKind::Unknown);
    assert!(batch.forwarded_arguments().is_none());
    assert_eq!(
      batch.command_arguments().iter().collect::<Vec<_>>(),
      [OsStr::new("--"), OsStr::new("--json"), OsStr::new("argument")]
    );
  }

  #[test]
  fn compatibility_mode_uses_reference_exit_codes() {
    let native = InvocationBatch::capture([OsString::from("frobnicate")]).options();
    assert_eq!(native.exit_code(ExitClass::Success), Some(0));
    let failures = [
      ExitClass::Usage,
      ExitClass::Unsupported,
      ExitClass::Operation,
      ExitClass::BuildFailure,
      ExitClass::RestoreFailure,
      ExitClass::TestFailure,
      ExitClass::Cancelled,
    ];
    for class in failures {
      assert_eq!(native.exit_code(class), Some(2));
    }

    let dotnet = InvocationBatch::capture([OsString::from("--compat=dotnet"), OsString::from("frobnicate")]).options();
    assert_eq!(dotnet.exit_code(ExitClass::Success), Some(0));
    for class in failures {
      assert_eq!(dotnet.exit_code(class), Some(1), "class {class:?}");
    }

    assert_eq!(native.exit_code(ExitClass::NoTests), Some(0));
    assert_eq!(dotnet.exit_code(ExitClass::NoTests), Some(0));
  }

  #[test]
  fn reference_profiles_mark_inapplicable_outcomes_instead_of_guessing() {
    let msbuild = InvocationBatch::capture([OsString::from("--compat=msbuild"), OsString::from("project.csproj")]).options();
    let nuget = InvocationBatch::capture([OsString::from("--compat=nuget"), OsString::from("restore")]).options();
    let vstest = InvocationBatch::capture([OsString::from("--compat=vstest"), OsString::from("tests.dll")]).options();

    assert_eq!(msbuild.exit_code(ExitClass::TestFailure), None);
    assert_eq!(msbuild.exit_code(ExitClass::NoTests), None);
    assert_eq!(nuget.exit_code(ExitClass::BuildFailure), None);
    assert_eq!(nuget.exit_code(ExitClass::TestFailure), None);
    assert_eq!(nuget.exit_code(ExitClass::NoTests), None);
    assert_eq!(vstest.exit_code(ExitClass::BuildFailure), None);
    assert_eq!(vstest.exit_code(ExitClass::RestoreFailure), None);
    assert_eq!(vstest.exit_code(ExitClass::TestFailure), Some(1));
    assert_eq!(vstest.exit_code(ExitClass::NoTests), Some(0));
  }

  #[test]
  fn malformed_or_repeated_compatibility_mode_is_rejected() {
    for arguments in [
      vec![OsString::from("--compat")],
      vec![OsString::from("--compat=mono"), OsString::from("sdk")],
      vec![
        OsString::from("--compat=dotnet"),
        OsString::from("sdk"),
        OsString::from("--compat"),
        OsString::from("msbuild"),
      ],
      vec![OsString::from("--compat"), non_unicode_argument(), OsString::from("sdk")],
    ] {
      let batch = InvocationBatch::capture(arguments);
      assert_eq!(batch.request().command, CommandKind::InvalidOptions);
      assert!(batch.option_error().is_some());
      assert_eq!(batch.options().compatibility_profile(), None);
    }
  }

  #[cfg(unix)]
  #[test]
  fn retains_non_unicode_operands_without_decoding_them() {
    use std::os::unix::ffi::OsStringExt;

    let path = OsString::from_vec(vec![b'p', 0x80]);
    let batch = InvocationBatch::capture([OsString::from("restore"), path.clone()]);

    assert_eq!(batch.request().command, CommandKind::Restore);
    assert_eq!(batch.command_arguments().first(), Some(path.as_os_str()));
  }
}
