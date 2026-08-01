use std::{
  ffi::{OsStr, OsString},
  mem::{align_of, size_of},
  ops::Index,
};

pub(crate) const COMMAND_SYNTAX_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum InvocationMode {
  Native,
  Dotnet,
  Msbuild,
  Nuget,
  Vstest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum FailureClass {
  Usage,
  Unsupported,
  Operation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum CommandKind {
  Help,
  Version,
  Sdk,
  Project,
  Build,
  Restore,
  Sync,
  KnownUnimplemented,
  Unknown,
  InvalidText,
  InvalidOptions,
}

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

  pub(crate) fn failure_exit_code(self, class: FailureClass) -> u8 {
    match (self.mode, class) {
      (InvocationMode::Native, FailureClass::Usage | FailureClass::Unsupported | FailureClass::Operation) => 2,
      (
        InvocationMode::Dotnet | InvocationMode::Msbuild | InvocationMode::Nuget | InvocationMode::Vstest,
        FailureClass::Usage | FailureClass::Unsupported | FailureClass::Operation,
      ) => 1,
    }
  }
}

const _: () = assert!(size_of::<InvocationOptions>() == 4);
const _: () = assert!(align_of::<InvocationOptions>() == 1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InvocationRequest {
  command_index: usize,
  syntax_version: u16,
  mode: InvocationMode,
  command: CommandKind,
  globals: GlobalOptions,
}

impl InvocationRequest {
  pub(crate) fn command(self) -> CommandKind {
    self.command
  }

  pub(crate) fn options(self) -> InvocationOptions {
    InvocationOptions {
      mode: self.mode,
      globals: self.globals,
    }
  }
}

const _: () = assert!(size_of::<InvocationRequest>() == 16);
const _: () = assert!(align_of::<InvocationRequest>() == align_of::<usize>());

pub(crate) struct InvocationBatch {
  raw_arguments: RawArguments,
  semantic_indices: Option<SemanticIndices>,
  request: InvocationRequest,
  option_error: Option<String>,
}

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
}

impl InvocationBatch {
  pub(crate) fn capture(arguments: impl IntoIterator<Item = OsString>) -> Self {
    let raw_arguments = RawArguments::capture(arguments);
    let mut globals = GlobalOptions::default();
    let mut mode = InvocationMode::Native;
    let mut compat_explicit = false;
    let mut color_explicit = false;
    let mut command_index = None;
    let mut semantic_indices = None::<SemanticIndices>;
    let mut option_error = None;
    let mut index = 0;
    while raw_arguments.get(index).is_some() {
      match parse_global_option(&raw_arguments, index, &mut globals, &mut mode, &mut compat_explicit, &mut color_explicit) {
        Ok(Some(width)) => {
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
    if option_error.is_none() && globals.json && color_explicit {
      option_error = Some("explicit color options cannot be combined with --json".into());
    }
    let command = if option_error.is_some() {
      CommandKind::InvalidOptions
    } else {
      command_index.map_or(CommandKind::Help, |index| {
        classify_command(raw_arguments.get(index).expect("classified argument index is valid"))
      })
    };
    Self {
      raw_arguments,
      semantic_indices,
      request: InvocationRequest {
        command_index: command_index.unwrap_or(usize::MAX),
        syntax_version: COMMAND_SYNTAX_VERSION,
        mode,
        command,
        globals,
      },
      option_error,
    }
  }

  pub(crate) fn request(&self) -> InvocationRequest {
    self.request
  }

  pub(crate) fn command_text(&self) -> Option<&str> {
    self.command_os().and_then(OsStr::to_str)
  }

  pub(crate) fn command_os(&self) -> Option<&OsStr> {
    self.raw_arguments.get(self.request.command_index).map(OsString::as_os_str)
  }

  pub(crate) fn command_arguments(&self) -> CommandArguments<'_> {
    let start = self.request.command_index.saturating_add(1);
    CommandArguments {
      storage: self.semantic_indices.as_ref().map_or_else(
        || CommandArgumentStorage::Direct(self.raw_arguments.after(start)),
        |indices| CommandArgumentStorage::Indexed {
          raw: &self.raw_arguments,
          indices,
        },
      ),
      start: 0,
    }
  }

  pub(crate) fn option_error(&self) -> Option<&str> {
    self.option_error.as_deref()
  }

  pub(crate) fn event_arguments(&self, include: bool) -> Vec<String> {
    if include {
      self.raw_arguments.iter().map(|argument| argument.to_string_lossy().into_owned()).collect()
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

fn parse_global_option(
  arguments: &RawArguments,
  index: usize,
  globals: &mut GlobalOptions,
  mode: &mut InvocationMode,
  compat_explicit: &mut bool,
  color_explicit: &mut bool,
) -> Result<Option<usize>, String> {
  let argument = arguments.get(index).expect("global option index is valid");
  match argument.to_str() {
    Some("--json") => globals.json = true,
    Some("--verbose") => globals.verbosity = DiagnosticVerbosity::Detailed,
    Some("--quiet") => globals.verbosity = DiagnosticVerbosity::Quiet,
    Some("--color") => {
      globals.color = ColorChoice::Always;
      *color_explicit = true;
    },
    Some("--no-color") => {
      globals.color = ColorChoice::Never;
      *color_explicit = true;
    },
    Some("--compat") => {
      if *compat_explicit {
        return Err("--compat may be specified only once".into());
      }
      let value = arguments.get(index + 1).ok_or("--compat requires dotnet, msbuild, nuget, or vstest")?;
      *mode = parse_compatibility_mode(value)?;
      *compat_explicit = true;
      return Ok(Some(2));
    },
    Some(value) if value.starts_with("--compat=") => {
      if *compat_explicit {
        return Err("--compat may be specified only once".into());
      }
      *mode = parse_compatibility_mode(OsStr::new(&value["--compat=".len()..]))?;
      *compat_explicit = true;
    },
    Some("--verbosity") => {
      let value = arguments
        .get(index + 1)
        .ok_or("--verbosity requires quiet, minimal, normal, detailed, or diagnostic")?;
      globals.verbosity = parse_verbosity(value)?;
      return Ok(Some(2));
    },
    Some(value) if value.starts_with("--verbosity=") => {
      globals.verbosity = parse_verbosity(OsStr::new(&value["--verbosity=".len()..]))?;
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
    _ => Err(format!("unsupported compatibility mode {:?}", value.to_string_lossy())),
  }
}

fn parse_verbosity(value: &OsStr) -> Result<DiagnosticVerbosity, String> {
  match value.to_str() {
    Some("quiet") => Ok(DiagnosticVerbosity::Quiet),
    Some("minimal") => Ok(DiagnosticVerbosity::Minimal),
    Some("normal") => Ok(DiagnosticVerbosity::Normal),
    Some("detailed") => Ok(DiagnosticVerbosity::Detailed),
    Some("diagnostic") => Ok(DiagnosticVerbosity::Diagnostic),
    _ => Err(format!("unsupported diagnostic verbosity {:?}", value.to_string_lossy())),
  }
}

fn classify_command(command: &OsStr) -> CommandKind {
  match command.to_str() {
    Some("-h" | "--help" | "help") => CommandKind::Help,
    Some("-V" | "--version" | "version") => CommandKind::Version,
    Some("sdk") => CommandKind::Sdk,
    Some("project") => CommandKind::Project,
    Some("build") => CommandKind::Build,
    Some("restore") => CommandKind::Restore,
    Some("sync") => CommandKind::Sync,
    Some("init" | "add" | "remove" | "run" | "test" | "pack" | "publish") => CommandKind::KnownUnimplemented,
    Some(_) => CommandKind::Unknown,
    None => CommandKind::InvalidText,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn captures_one_lossless_batch_and_classifies_before_execution() {
    let batch = InvocationBatch::capture(["--json", "restore", "", "App.csproj"].map(OsString::from));

    assert_eq!(batch.request().syntax_version, COMMAND_SYNTAX_VERSION);
    assert_eq!(batch.request().mode, InvocationMode::Native);
    assert_eq!(batch.request().command, CommandKind::Restore);
    assert!(batch.request().options().json());
    assert_eq!(batch.command_text(), Some("restore"));
    assert_eq!(batch.command_arguments().iter().collect::<Vec<_>>(), [OsStr::new(""), OsStr::new("App.csproj")]);
    assert_eq!(batch.raw_arguments(), ["--json", "restore", "", "App.csproj"]);
    assert!(batch.event_arguments(false).is_empty());
    assert_eq!(batch.event_arguments(true), ["--json", "restore", "", "App.csproj"]);
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
  fn globals_normalize_before_and_after_the_command() {
    let batch = InvocationBatch::capture(["--quiet", "restore", "App.csproj", "--verbosity", "diagnostic", "--no-color"].map(OsString::from));

    assert_eq!(batch.request().command, CommandKind::Restore);
    assert_eq!(batch.request().options().verbosity(), DiagnosticVerbosity::Diagnostic);
    assert_eq!(batch.request().options().color(), ColorChoice::Never);
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
  fn compatibility_mode_is_typed_and_removed_from_command_operands() {
    for (name, expected) in [
      ("dotnet", InvocationMode::Dotnet),
      ("msbuild", InvocationMode::Msbuild),
      ("nuget", InvocationMode::Nuget),
      ("vstest", InvocationMode::Vstest),
    ] {
      let batch = InvocationBatch::capture([
        OsString::from("sdk"),
        OsString::from("--compat"),
        OsString::from(name),
        OsString::from("current"),
      ]);

      assert_eq!(batch.request().options().mode, expected);
      assert_eq!(batch.command_arguments().iter().collect::<Vec<_>>(), [OsStr::new("current")]);
    }
  }

  #[test]
  fn compatibility_mode_uses_reference_failure_codes() {
    let native = InvocationBatch::capture([OsString::from("frobnicate")]).request().options();
    let compat = InvocationBatch::capture([OsString::from("--compat=dotnet"), OsString::from("frobnicate")])
      .request()
      .options();

    for class in [FailureClass::Usage, FailureClass::Unsupported, FailureClass::Operation] {
      assert_eq!(native.failure_exit_code(class), 2);
      assert_eq!(compat.failure_exit_code(class), 1);
    }
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
    ] {
      let batch = InvocationBatch::capture(arguments);
      assert_eq!(batch.request().command, CommandKind::InvalidOptions);
      assert!(batch.option_error().is_some());
    }
  }

  #[cfg(unix)]
  #[test]
  fn retains_non_unicode_operands_without_decoding_them() {
    use std::os::unix::ffi::OsStringExt;

    let path = OsString::from_vec(vec![b'p', 0x80]);
    let batch = InvocationBatch::capture([OsString::from("restore"), path.clone()]);

    assert_eq!(batch.request().command, CommandKind::Restore);
    assert_eq!(batch.command_arguments().first(), Some(&path));
  }
}
