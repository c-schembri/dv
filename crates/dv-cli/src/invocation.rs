use std::{
  ffi::{OsStr, OsString},
  mem::{align_of, size_of},
};

pub(crate) const COMMAND_SYNTAX_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum InvocationMode {
  Native,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InvocationRequest {
  command_index: usize,
  syntax_version: u16,
  mode: InvocationMode,
  command: CommandKind,
}

impl InvocationRequest {
  pub(crate) fn command(self) -> CommandKind {
    self.command
  }
}

const _: () = assert!(size_of::<InvocationRequest>() == 16);
const _: () = assert!(align_of::<InvocationRequest>() == align_of::<usize>());

pub(crate) struct InvocationBatch {
  raw_arguments: RawArguments,
  request: InvocationRequest,
  json: bool,
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
    let json = raw_arguments.iter().any(|argument| argument == "--json");
    let command_index = raw_arguments.iter().position(|argument| argument != "--json");
    let command = command_index.map_or(CommandKind::Help, |index| {
      classify_command(raw_arguments.get(index).expect("classified argument index is valid"))
    });
    Self {
      raw_arguments,
      request: InvocationRequest {
        command_index: command_index.unwrap_or(usize::MAX),
        syntax_version: COMMAND_SYNTAX_VERSION,
        mode: InvocationMode::Native,
        command,
      },
      json,
    }
  }

  pub(crate) fn request(&self) -> InvocationRequest {
    self.request
  }

  pub(crate) fn json(&self) -> bool {
    self.json
  }

  pub(crate) fn command_text(&self) -> Option<&str> {
    self.command_os().and_then(OsStr::to_str)
  }

  pub(crate) fn command_os(&self) -> Option<&OsStr> {
    self.raw_arguments.get(self.request.command_index).map(OsString::as_os_str)
  }

  pub(crate) fn command_arguments(&self) -> &[OsString] {
    let start = self.request.command_index.saturating_add(1);
    self.raw_arguments.after(start)
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
    assert!(batch.json());
    assert_eq!(batch.command_text(), Some("restore"));
    assert_eq!(batch.command_arguments(), ["", "App.csproj"]);
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
