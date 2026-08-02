use std::{
  error::Error,
  fmt, fs, io,
  mem::{align_of, size_of},
  path::{Path, PathBuf},
};

use crate::{BENCHMARK_CACHE_LINE_BYTES, absolute_lexical};

const KIND_COUNT: usize = 5;
const INLINE_INPUT_CAPACITY: usize = 8;
const NUGET_CONFIG_NAMES: [&str; 3] = ["nuget.config", "NuGet.config", "NuGet.Config"];

/// One ancestor-owned input family with an explicit precedence rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AncestorInputKind {
  /// The nearest `global.json` controls SDK selection.
  GlobalJson,
  /// Every `NuGet.Config` is retained from root to leaf.
  NugetConfig,
  /// The nearest `Directory.Build.props` is selected.
  DirectoryBuildProps,
  /// The nearest `Directory.Build.targets` is selected.
  DirectoryBuildTargets,
  /// The nearest `Directory.Packages.props` is selected.
  DirectoryPackagesProps,
}

const _: () = assert!(size_of::<AncestorInputKind>() == 1);
const _: () = assert!(align_of::<AncestorInputKind>() == 1);

impl AncestorInputKind {
  const ORDERED: [Self; KIND_COUNT] = [
    Self::GlobalJson,
    Self::NugetConfig,
    Self::DirectoryBuildProps,
    Self::DirectoryBuildTargets,
    Self::DirectoryPackagesProps,
  ];

  /// Returns the stable event and diagnostic name.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::GlobalJson => "global_json",
      Self::NugetConfig => "nuget_config",
      Self::DirectoryBuildProps => "directory_build_props",
      Self::DirectoryBuildTargets => "directory_build_targets",
      Self::DirectoryPackagesProps => "directory_packages_props",
    }
  }

  const fn index(self) -> usize {
    self as usize
  }

  const fn mask(self) -> u8 {
    1 << self.index()
  }

  fn file_name(self, spelling: u8) -> &'static str {
    match self {
      Self::GlobalJson => "global.json",
      Self::NugetConfig => NUGET_CONFIG_NAMES[usize::from(spelling)],
      Self::DirectoryBuildProps => "Directory.Build.props",
      Self::DirectoryBuildTargets => "Directory.Build.targets",
      Self::DirectoryPackagesProps => "Directory.Packages.props",
    }
  }
}

/// A compact set of ancestor input families requested by one consumer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct AncestorInputRequest(u8);

const _: () = assert!(size_of::<AncestorInputRequest>() == 1);
const _: () = assert!(align_of::<AncestorInputRequest>() == 1);

impl AncestorInputRequest {
  pub const GLOBAL_JSON: Self = Self(AncestorInputKind::GlobalJson.mask());
  pub const NUGET_CONFIG: Self = Self(AncestorInputKind::NugetConfig.mask());
  pub const DIRECTORY_BUILD_PROPS: Self = Self(AncestorInputKind::DirectoryBuildProps.mask());
  pub const DIRECTORY_BUILD_TARGETS: Self = Self(AncestorInputKind::DirectoryBuildTargets.mask());
  pub const DIRECTORY_PACKAGES_PROPS: Self = Self(AncestorInputKind::DirectoryPackagesProps.mask());
  pub const ALL: Self = Self((1 << KIND_COUNT) - 1);

  /// Combines two input-family requests without allocation.
  pub const fn union(self, other: Self) -> Self {
    Self(self.0 | other.0)
  }

  const fn contains(self, kind: AncestorInputKind) -> bool {
    self.0 & kind.mask() != 0
  }
}

/// One discovered input encoded as a kind, filename spelling, and parent depth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct AncestorInput {
  file_len: u32,
  depth: u16,
  kind: AncestorInputKind,
  spelling: u8,
}

const EMPTY_INPUT: AncestorInput = AncestorInput {
  file_len: 0,
  depth: 0,
  kind: AncestorInputKind::GlobalJson,
  spelling: 0,
};

const _: () = assert!(size_of::<AncestorInput>() == 8);
const _: () = assert!(align_of::<AncestorInput>() == 4);
const _: () = assert!(BENCHMARK_CACHE_LINE_BYTES / size_of::<AncestorInput>() == 8);

impl AncestorInput {
  /// Returns the input family.
  pub const fn kind(self) -> AncestorInputKind {
    self.kind
  }

  /// Returns how many parents separate the start directory from this input.
  pub const fn ancestor_depth(self) -> u16 {
    self.depth
  }

  /// Returns the file length captured by the successful metadata probe.
  pub const fn file_len(self) -> u32 {
    self.file_len
  }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
struct InputRange {
  start: u16,
  len: u16,
}

const _: () = assert!(size_of::<InputRange>() == 4);
const _: () = assert!(align_of::<InputRange>() == 2);

#[derive(Debug)]
struct InputRows {
  inline: [AncestorInput; INLINE_INPUT_CAPACITY],
  spill: Vec<AncestorInput>,
  len: u16,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<InputRows>() == 96);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<InputRows>() == align_of::<usize>());

impl InputRows {
  fn new() -> Self {
    Self {
      inline: [EMPTY_INPUT; INLINE_INPUT_CAPACITY],
      spill: Vec::new(),
      len: 0,
    }
  }

  fn len(&self) -> usize {
    usize::from(self.len)
  }

  fn push(&mut self, input: AncestorInput, path: &Path) -> Result<(), AncestorInputError> {
    if self.len == u16::MAX {
      return Err(AncestorInputError::new(
        AncestorInputErrorKind::LimitExceeded,
        path,
        "ancestor input discovery exceeds 65,535 rows",
      ));
    }
    let len = self.len();
    if self.spill.is_empty() && len < INLINE_INPUT_CAPACITY {
      self.inline[len] = input;
    } else {
      if self.spill.is_empty() {
        self.spill.reserve(INLINE_INPUT_CAPACITY * 2);
        self.spill.extend_from_slice(&self.inline);
      }
      self.spill.push(input);
    }
    self.len += 1;
    Ok(())
  }

  fn reverse_prefix(&mut self, len: usize) {
    if self.spill.is_empty() {
      self.inline[..len].reverse();
    } else {
      self.spill[..len].reverse();
    }
  }

  fn as_slice(&self) -> &[AncestorInput] {
    if self.spill.is_empty() {
      &self.inline[..self.len()]
    } else {
      &self.spill[..self.len()]
    }
  }

  fn retained_bytes(&self) -> usize {
    if self.spill.is_empty() {
      size_of::<[AncestorInput; INLINE_INPUT_CAPACITY]>()
    } else {
      size_of::<[AncestorInput; INLINE_INPUT_CAPACITY]>() + self.spill.capacity() * size_of::<AncestorInput>()
    }
  }
}

/// One command-local batch of ancestor-owned build inputs.
#[derive(Debug)]
pub struct AncestorInputBatch {
  start_directory: PathBuf,
  rows: InputRows,
  ranges: [InputRange; KIND_COUNT],
  metadata_probes: u32,
  ancestor_count: u16,
  directory_enumerations: u16,
}

#[cfg(all(target_pointer_width = "64", windows))]
const _: () = assert!(size_of::<AncestorInputBatch>() == 160);
#[cfg(all(target_pointer_width = "64", not(windows)))]
const _: () = assert!(size_of::<AncestorInputBatch>() == 152);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(align_of::<AncestorInputBatch>() == align_of::<usize>());

impl AncestorInputBatch {
  /// Returns the absolute directory from which the parent walk began.
  pub fn start_directory(&self) -> &Path {
    &self.start_directory
  }

  /// Returns one precedence-ordered input-family view.
  pub fn inputs(&self, kind: AncestorInputKind) -> &[AncestorInput] {
    let range = self.ranges[kind.index()];
    let start = usize::from(range.start);
    &self.rows.as_slice()[start..start + usize::from(range.len)]
  }

  /// Materializes one discovered path into a caller-owned reusable buffer.
  pub fn write_path(&self, input: AncestorInput, output: &mut PathBuf) {
    output.clear();
    output.push(&self.start_directory);
    for _ in 0..input.depth {
      let popped = output.pop();
      debug_assert!(popped, "validated ancestor depth remains within the start path");
    }
    output.push(input.kind.file_name(input.spelling));
  }

  /// Materializes one discovered path for a cold ownership boundary.
  pub fn path(&self, input: AncestorInput) -> PathBuf {
    let mut output = PathBuf::with_capacity(self.start_directory.as_os_str().len() + 25);
    self.write_path(input, &mut output);
    output
  }

  /// Consumes a singleton batch and reuses its owned path allocation.
  pub fn into_nearest_path(mut self, kind: AncestorInputKind) -> Option<PathBuf> {
    let input = self.inputs(kind).first().copied()?;
    for _ in 0..input.depth {
      self.start_directory.pop();
    }
    self.start_directory.push(input.kind.file_name(input.spelling));
    Some(self.start_directory)
  }

  /// Returns the number of filesystem metadata queries performed.
  pub const fn metadata_probes(&self) -> u32 {
    self.metadata_probes
  }

  /// Returns the number of ancestor directories visited.
  pub const fn ancestor_count(&self) -> u16 {
    self.ancestor_count
  }

  /// Returns casing-preservation directory enumerations performed on macOS.
  pub const fn directory_enumerations(&self) -> u16 {
    self.directory_enumerations
  }

  /// Returns bytes retained by the compact row storage and start path.
  pub fn working_set_bytes(&self) -> usize {
    self.start_directory.as_os_str().len() + self.rows.retained_bytes()
  }
}

/// Stable ancestor-input discovery failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AncestorInputErrorKind {
  /// The requested start path does not exist.
  NotFound,
  /// A filesystem operation failed.
  Io,
  /// A known input exists but is not a regular file.
  UnsupportedFileType,
  /// Compact depth, row, or operation counts exceeded their valid range.
  LimitExceeded,
}

/// An ancestor-input failure with its exact filesystem boundary.
#[derive(Debug)]
pub struct AncestorInputError {
  kind: AncestorInputErrorKind,
  path: PathBuf,
  message: Box<str>,
}

impl AncestorInputError {
  fn new(kind: AncestorInputErrorKind, path: &Path, message: impl Into<Box<str>>) -> Self {
    Self {
      kind,
      path: path.to_owned(),
      message: message.into(),
    }
  }

  /// Returns the stable failure category.
  pub const fn kind(&self) -> AncestorInputErrorKind {
    self.kind
  }

  /// Returns the path at which discovery failed.
  pub fn path(&self) -> &Path {
    &self.path
  }
}

impl fmt::Display for AncestorInputError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl Error for AncestorInputError {}

/// Discovers requested ancestor inputs in one bounded nearest-to-root walk.
pub fn discover_ancestor_inputs(start: &Path, request: AncestorInputRequest) -> Result<AncestorInputBatch, AncestorInputError> {
  let mut cursor = absolute_path(start)?;
  let metadata = match fs::metadata(&cursor) {
    Ok(metadata) => metadata,
    Err(error) if error.kind() == io::ErrorKind::NotFound => {
      return Err(AncestorInputError::new(
        AncestorInputErrorKind::NotFound,
        &cursor,
        format!("ancestor input search start {} does not exist", cursor.display()),
      ));
    },
    Err(error) => return Err(io_error("inspect ancestor input search start", &cursor, error)),
  };
  if metadata.is_file() {
    if !cursor.pop() {
      return Err(AncestorInputError::new(
        AncestorInputErrorKind::UnsupportedFileType,
        start,
        format!("ancestor input search start {} has no parent", start.display()),
      ));
    }
  } else if !metadata.is_dir() {
    return Err(AncestorInputError::new(
      AncestorInputErrorKind::UnsupportedFileType,
      &cursor,
      format!("ancestor input search start {} is not a file or directory", cursor.display()),
    ));
  }

  let start_directory = cursor.clone();
  let mut rows = InputRows::new();
  let mut selected = [None; KIND_COUNT];
  let mut metadata_probes = 0_u32;
  let mut directory_enumerations = 0_u16;
  let mut depth = 0_u16;
  let needs_nuget = request.contains(AncestorInputKind::NugetConfig);

  loop {
    for kind in [
      AncestorInputKind::GlobalJson,
      AncestorInputKind::DirectoryBuildProps,
      AncestorInputKind::DirectoryBuildTargets,
      AncestorInputKind::DirectoryPackagesProps,
    ] {
      if request.contains(kind) && selected[kind.index()].is_none() {
        let discovered = probe_input(&mut cursor, kind, &mut metadata_probes, &mut directory_enumerations)?;
        if let Some((spelling, file_len)) = discovered {
          selected[kind.index()] = Some(AncestorInput {
            file_len,
            depth,
            kind,
            spelling,
          });
        }
      }
    }

    if needs_nuget
      && let Some((spelling, file_len)) = probe_input(&mut cursor, AncestorInputKind::NugetConfig, &mut metadata_probes, &mut directory_enumerations)?
    {
      rows.push(
        AncestorInput {
          file_len,
          depth,
          kind: AncestorInputKind::NugetConfig,
          spelling,
        },
        &cursor,
      )?;
    }

    let singleton_work_remaining = [
      AncestorInputKind::GlobalJson,
      AncestorInputKind::DirectoryBuildProps,
      AncestorInputKind::DirectoryBuildTargets,
      AncestorInputKind::DirectoryPackagesProps,
    ]
    .into_iter()
    .any(|kind| request.contains(kind) && selected[kind.index()].is_none());
    if !needs_nuget && !singleton_work_remaining {
      break;
    }
    if !cursor.pop() {
      break;
    }
    depth = depth.checked_add(1).ok_or_else(|| {
      AncestorInputError::new(
        AncestorInputErrorKind::LimitExceeded,
        &cursor,
        "ancestor input discovery exceeds 65,535 parent levels",
      )
    })?;
  }

  let mut ranges = [InputRange::default(); KIND_COUNT];
  let nuget_count = rows.len();
  rows.reverse_prefix(nuget_count);
  ranges[AncestorInputKind::NugetConfig.index()] = InputRange {
    start: 0,
    len: nuget_count as u16,
  };
  for kind in AncestorInputKind::ORDERED {
    if kind == AncestorInputKind::NugetConfig {
      continue;
    }
    let start = rows.len() as u16;
    if let Some(input) = selected[kind.index()] {
      rows.push(input, &start_directory)?;
    }
    ranges[kind.index()] = InputRange {
      start,
      len: rows.len() as u16 - start,
    };
  }

  let ancestor_count = depth.checked_add(1).ok_or_else(|| {
    AncestorInputError::new(
      AncestorInputErrorKind::LimitExceeded,
      &start_directory,
      "ancestor input discovery exceeds 65,535 visited directories",
    )
  })?;

  Ok(AncestorInputBatch {
    start_directory,
    rows,
    ranges,
    metadata_probes,
    ancestor_count,
    directory_enumerations,
  })
}

fn probe_input(
  directory: &mut PathBuf,
  kind: AncestorInputKind,
  metadata_probes: &mut u32,
  directory_enumerations: &mut u16,
) -> Result<Option<(u8, u32)>, AncestorInputError> {
  #[cfg(not(target_os = "macos"))]
  let _ = directory_enumerations;
  if kind != AncestorInputKind::NugetConfig {
    return probe_file(directory, kind, 0, metadata_probes);
  }

  let spellings: &[u8] = if cfg!(windows) { &[2] } else { &[0, 1, 2] };
  for spelling in spellings {
    if let Some((_, file_len)) = probe_file(directory, kind, *spelling, metadata_probes)? {
      #[cfg(target_os = "macos")]
      {
        *directory_enumerations = directory_enumerations.checked_add(1).ok_or_else(|| {
          AncestorInputError::new(
            AncestorInputErrorKind::LimitExceeded,
            directory,
            "ancestor input discovery exceeds 65,535 directory enumerations",
          )
        })?;
        return actual_nuget_spelling(directory).map(|actual| Some((actual.unwrap_or(*spelling), file_len)));
      }
      #[cfg(not(target_os = "macos"))]
      return Ok(Some((*spelling, file_len)));
    }
  }
  Ok(None)
}

fn probe_file(directory: &mut PathBuf, kind: AncestorInputKind, spelling: u8, metadata_probes: &mut u32) -> Result<Option<(u8, u32)>, AncestorInputError> {
  directory.push(kind.file_name(spelling));
  *metadata_probes = metadata_probes.checked_add(1).ok_or_else(|| {
    AncestorInputError::new(
      AncestorInputErrorKind::LimitExceeded,
      directory,
      "ancestor input discovery exceeds 4,294,967,295 metadata probes",
    )
  })?;
  let metadata = match fs::metadata(&*directory) {
    Ok(metadata) => metadata,
    Err(error) if error.kind() == io::ErrorKind::NotFound => {
      directory.pop();
      return Ok(None);
    },
    Err(error) => return Err(io_error("inspect ancestor input", directory, error)),
  };
  if !metadata.is_file() {
    return Err(AncestorInputError::new(
      AncestorInputErrorKind::UnsupportedFileType,
      directory,
      format!("ancestor input {} is not a regular file", directory.display()),
    ));
  }
  let file_len = u32::try_from(metadata.len()).map_err(|_| {
    AncestorInputError::new(
      AncestorInputErrorKind::LimitExceeded,
      directory,
      format!("ancestor input {} exceeds the 4 GiB discovery limit", directory.display()),
    )
  })?;
  directory.pop();
  Ok(Some((spelling, file_len)))
}

#[cfg(target_os = "macos")]
fn actual_nuget_spelling(directory: &Path) -> Result<Option<u8>, AncestorInputError> {
  let entries = fs::read_dir(directory).map_err(|error| io_error("enumerate NuGet.Config parent", directory, error))?;
  let mut selected = None;
  for entry in entries {
    let entry = entry.map_err(|error| io_error("read NuGet.Config parent entry", directory, error))?;
    let Some(spelling) = entry
      .file_name()
      .to_str()
      .and_then(|name| NUGET_CONFIG_NAMES.iter().position(|candidate| *candidate == name))
      .map(|index| index as u8)
    else {
      continue;
    };
    if selected.is_none_or(|existing| spelling < existing) {
      selected = Some(spelling);
    }
  }
  Ok(selected)
}

fn absolute_path(path: &Path) -> Result<PathBuf, AncestorInputError> {
  absolute_lexical(path).map_err(|error| io_error("resolve ancestor input search start", path, error))
}

fn io_error(operation: &str, path: &Path, error: io::Error) -> AncestorInputError {
  AncestorInputError::new(AncestorInputErrorKind::Io, path, format!("failed to {operation} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
  use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
  };

  use super::*;

  static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
      let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let path = std::env::temp_dir().join(format!("dv-ancestor-input-test-{}-{time}-{nonce}", std::process::id()));
      fs::create_dir_all(&path).unwrap();
      Self(path)
    }

    fn write(&self, relative: &str) -> PathBuf {
      let path = self.0.join(relative);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(&path, []).unwrap();
      path
    }
  }

  impl Drop for TempDirectory {
    fn drop(&mut self) {
      let _ = fs::remove_dir_all(&self.0);
    }
  }

  #[test]
  fn mixed_inputs_keep_each_precedence_rule_in_compact_rows() {
    let temp = TempDirectory::new();
    temp.write("global.json");
    temp.write("NuGet.Config");
    temp.write("Directory.Build.props");
    temp.write("team/NuGet.Config");
    temp.write("team/Directory.Build.targets");
    temp.write("team/src/Directory.Packages.props");
    let start = temp.0.join("team/src/app");
    fs::create_dir_all(&start).unwrap();

    let batch = discover_ancestor_inputs(&start, AncestorInputRequest::ALL).unwrap();

    let paths = |kind| batch.inputs(kind).iter().copied().map(|input| batch.path(input)).collect::<Vec<_>>();
    assert_eq!(paths(AncestorInputKind::GlobalJson), [temp.0.join("global.json")]);
    assert_eq!(
      paths(AncestorInputKind::NugetConfig),
      [temp.0.join("NuGet.Config"), temp.0.join("team/NuGet.Config")]
    );
    assert_eq!(paths(AncestorInputKind::DirectoryBuildProps), [temp.0.join("Directory.Build.props")]);
    assert_eq!(paths(AncestorInputKind::DirectoryBuildTargets), [temp.0.join("team/Directory.Build.targets")]);
    assert_eq!(
      paths(AncestorInputKind::DirectoryPackagesProps),
      [temp.0.join("team/src/Directory.Packages.props")]
    );
    assert_eq!(batch.inputs(AncestorInputKind::GlobalJson)[0].ancestor_depth(), 3);
    assert_eq!(batch.inputs(AncestorInputKind::NugetConfig)[0].ancestor_depth(), 3);
    assert_eq!(batch.inputs(AncestorInputKind::NugetConfig)[1].ancestor_depth(), 2);
    assert_eq!(batch.inputs(AncestorInputKind::DirectoryBuildTargets)[0].ancestor_depth(), 2);
    assert_eq!(batch.inputs(AncestorInputKind::DirectoryPackagesProps)[0].ancestor_depth(), 1);
    assert_eq!(batch.rows.spill.capacity(), 0);
  }

  #[test]
  fn singleton_request_stops_after_the_nearest_match_and_reuses_its_path() {
    let temp = TempDirectory::new();
    let expected = temp.write("nested/global.json");
    temp.write("global.json");
    let start = temp.0.join("nested/src/App.csproj");
    fs::create_dir_all(start.parent().unwrap()).unwrap();
    fs::write(&start, []).unwrap();

    let batch = discover_ancestor_inputs(&start, AncestorInputRequest::GLOBAL_JSON).unwrap();

    assert_eq!(batch.metadata_probes(), 2);
    assert_eq!(batch.inputs(AncestorInputKind::GlobalJson)[0].ancestor_depth(), 1);
    assert_eq!(batch.into_nearest_path(AncestorInputKind::GlobalJson), Some(expected));
  }

  #[test]
  fn absent_requested_input_returns_an_empty_batch() {
    let temp = TempDirectory::new();

    let batch = discover_ancestor_inputs(&temp.0, AncestorInputRequest::DIRECTORY_BUILD_TARGETS).unwrap();

    assert!(batch.inputs(AncestorInputKind::DirectoryBuildTargets).is_empty());
    assert!(batch.metadata_probes() > 0);
  }

  #[test]
  fn known_input_directory_fails_instead_of_falling_through_to_a_parent() {
    let temp = TempDirectory::new();
    fs::create_dir(temp.0.join("global.json")).unwrap();

    let error = discover_ancestor_inputs(&temp.0, AncestorInputRequest::GLOBAL_JSON).unwrap_err();

    assert_eq!(error.kind(), AncestorInputErrorKind::UnsupportedFileType);
    assert_eq!(error.path(), temp.0.join("global.json"));
  }
}
