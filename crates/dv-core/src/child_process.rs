use std::{
  fmt, io,
  mem::{align_of, size_of},
  num::NonZeroI32,
  process::ExitStatus,
};

/// How an owning command maps a completed child into its own process result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ChildExitPolicy {
  /// Return a normally exited child's numeric code unchanged.
  Preserve,
  /// Collapse every child failure into the owning command's failure code.
  MapToCommandFailure,
}

const _: () = assert!(size_of::<ChildExitPolicy>() == 1);
const _: () = assert!(align_of::<ChildExitPolicy>() == 1);

impl ChildExitPolicy {
  /// Stable diagnostic spelling for this policy.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Preserve => "preserve",
      Self::MapToCommandFailure => "map_to_command_failure",
    }
  }

  /// Maps one classified termination without allocating or consulting the OS.
  pub const fn process_exit_code(self, termination: ChildTermination, command_failure: NonZeroI32) -> i32 {
    match (self, termination) {
      (_, ChildTermination::Exited(0)) => 0,
      (Self::Preserve, ChildTermination::Exited(code)) => code,
      (Self::Preserve | Self::MapToCommandFailure, ChildTermination::Exited(_) | ChildTermination::Signalled(_) | ChildTermination::Unknown) => {
        command_failure.get()
      },
    }
  }
}

/// The terminal state reported by the operating system after a child was
/// launched successfully and reaped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C, u8)]
pub enum ChildTermination {
  Exited(i32),
  Signalled(u8),
  Unknown,
}

const _: () = assert!(size_of::<ChildTermination>() == 8);
const _: () = assert!(align_of::<ChildTermination>() == 4);

impl ChildTermination {
  /// Returns the exact child code when the platform supplied one.
  pub fn exit_code(self) -> Option<i32> {
    match self {
      Self::Exited(code) => Some(code),
      Self::Signalled(_) | Self::Unknown => None,
    }
  }

  /// Returns the terminating Unix signal when the platform supplied one.
  pub fn signal(self) -> Option<u8> {
    match self {
      Self::Signalled(signal) => Some(signal),
      Self::Exited(_) | Self::Unknown => None,
    }
  }
}

/// Classifies a reaped child without formatting, allocating, or discarding its
/// numeric exit code.
pub fn classify_child_termination(status: ExitStatus) -> ChildTermination {
  if let Some(code) = status.code() {
    return ChildTermination::Exited(code);
  }

  #[cfg(unix)]
  {
    use std::os::unix::process::ExitStatusExt as _;
    if let Some(signal) = status.signal().and_then(|value| u8::try_from(value).ok()) {
      return ChildTermination::Signalled(signal);
    }
  }

  ChildTermination::Unknown
}

/// The OS operation that failed before a child termination record existed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ChildProcessFailureStage {
  Launch,
  Wait,
}

/// A cold-path process error. This is deliberately separate from
/// `ChildTermination`: a program that exits nonzero still launched and ran.
#[derive(Debug)]
pub struct ChildProcessFailure {
  stage: ChildProcessFailureStage,
  source: io::Error,
}

impl ChildProcessFailure {
  pub fn launch(source: io::Error) -> Self {
    Self {
      stage: ChildProcessFailureStage::Launch,
      source,
    }
  }

  pub fn wait(source: io::Error) -> Self {
    Self {
      stage: ChildProcessFailureStage::Wait,
      source,
    }
  }

  pub fn stage(&self) -> ChildProcessFailureStage {
    self.stage
  }

  pub fn source_error(&self) -> &io::Error {
    &self.source
  }
}

impl fmt::Display for ChildProcessFailure {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    let operation = match self.stage {
      ChildProcessFailureStage::Launch => "launch child process",
      ChildProcessFailureStage::Wait => "wait for child process",
    };
    write!(formatter, "failed to {operation}: {}", self.source)
  }
}

impl std::error::Error for ChildProcessFailure {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    Some(&self.source)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::{ffi::OsString, process::Command};

  fn exit_command(code: u8) -> (OsString, Vec<OsString>) {
    #[cfg(windows)]
    {
      ("cmd.exe".into(), vec!["/d".into(), "/c".into(), format!("exit {code}").into()])
    }
    #[cfg(unix)]
    {
      ("sh".into(), vec!["-c".into(), format!("exit {code}").into()])
    }
  }

  #[test]
  fn preserves_numeric_child_exit_codes() {
    for expected in [0, 37, 211] {
      let (program, arguments) = exit_command(expected);
      let status = Command::new(program).args(arguments).status().unwrap();
      assert_eq!(classify_child_termination(status), ChildTermination::Exited(i32::from(expected)));
    }
  }

  #[test]
  fn command_policy_preserves_only_the_contractually_owned_numeric_exit() {
    let failure = NonZeroI32::new(2).unwrap();

    assert_eq!(ChildExitPolicy::Preserve.process_exit_code(ChildTermination::Exited(37), failure), 37);
    assert_eq!(ChildExitPolicy::MapToCommandFailure.process_exit_code(ChildTermination::Exited(37), failure), 2);
    assert_eq!(ChildExitPolicy::Preserve.process_exit_code(ChildTermination::Exited(0), failure), 0);
    assert_eq!(ChildExitPolicy::Preserve.process_exit_code(ChildTermination::Unknown, failure), 2);
  }

  #[test]
  fn launch_failure_is_not_a_child_termination() {
    let missing = format!("dv-missing-child-{}", std::process::id());
    let result = Command::new(missing)
      .status()
      .map(classify_child_termination)
      .map_err(ChildProcessFailure::launch);
    let error = result.unwrap_err();
    assert_eq!(error.stage(), ChildProcessFailureStage::Launch);
    assert_ne!(error.source_error().kind(), io::ErrorKind::Interrupted);
  }

  #[test]
  fn launch_and_wait_failures_keep_distinct_stages() {
    let launch = ChildProcessFailure::launch(io::Error::new(io::ErrorKind::NotFound, "missing"));
    let wait = ChildProcessFailure::wait(io::Error::new(io::ErrorKind::BrokenPipe, "lost child"));

    assert_eq!(launch.stage(), ChildProcessFailureStage::Launch);
    assert_eq!(wait.stage(), ChildProcessFailureStage::Wait);
    assert!(launch.to_string().contains("launch child process"));
    assert!(wait.to_string().contains("wait for child process"));
  }

  #[cfg(unix)]
  #[test]
  fn retains_unix_signal_separately_from_exit_codes() {
    let status = Command::new("sh").args(["-c", "kill -TERM $$"]).status().unwrap();
    assert_eq!(classify_child_termination(status), ChildTermination::Signalled(15));
  }
}
