use std::{
  fmt,
  mem::{align_of, size_of},
};

use crate::{invocation::CommandArguments, output::is_sensitive_name};

const INLINE_ENVIRONMENT_EDITS: usize = 4;

/// Precedence increases with the numeric value. Equal-source edits are applied
/// in input order, so the final occurrence wins without sorting or hashing.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub(crate) enum EnvironmentSource {
  Ambient,
  Directive,
  LaunchProfile,
  CommandLine,
}

const _: () = assert!(size_of::<EnvironmentSource>() == 1);

#[derive(Clone, Copy)]
struct EnvironmentEdit<'a> {
  assignment: &'a str,
  name_end: u32,
  source: EnvironmentSource,
  sensitive: bool,
}

impl EnvironmentEdit<'static> {
  const EMPTY: Self = Self {
    assignment: "",
    name_end: 0,
    source: EnvironmentSource::Ambient,
    sensitive: false,
  };
}

const _: () = assert!(size_of::<EnvironmentEdit<'static>>() == 24);
const _: () = assert!(align_of::<EnvironmentEdit<'static>>() == align_of::<usize>());

impl<'a> EnvironmentEdit<'a> {
  fn parse(source: EnvironmentSource, assignment: &'a str) -> Result<Self, EnvironmentError> {
    let Some(separator) = assignment.find('=') else {
      return Err(EnvironmentError::MissingSeparator { source });
    };
    if separator == 0 {
      return Err(EnvironmentError::EmptyName { source });
    }
    if assignment.contains('\0') {
      return Err(EnvironmentError::EmbeddedNul { source });
    }
    let name_end = u32::try_from(separator).map_err(|_| EnvironmentError::AssignmentTooLarge { source })?;
    Ok(Self {
      assignment,
      name_end,
      source,
      sensitive: is_sensitive_name(&assignment[..separator]),
    })
  }

  fn name(self) -> &'a str {
    &self.assignment[..self.name_end as usize]
  }

  fn value(self) -> &'a str {
    &self.assignment[self.name_end as usize + 1..]
  }
}

impl fmt::Debug for EnvironmentEdit<'_> {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter
      .debug_struct("EnvironmentEdit")
      .field("name", &self.name())
      .field("value", &if self.sensitive { "<redacted>" } else { self.value() })
      .field("source", &self.source)
      .finish()
  }
}

#[derive(Default)]
enum EnvironmentStorage<'a> {
  #[default]
  Empty,
  Inline {
    values: [EnvironmentEdit<'a>; INLINE_ENVIRONMENT_EDITS],
    len: u8,
  },
  Heap(Vec<EnvironmentEdit<'a>>),
}

impl<'a> EnvironmentStorage<'a> {
  fn push(&mut self, edit: EnvironmentEdit<'a>) {
    match self {
      Self::Empty => {
        let mut values = [EnvironmentEdit::EMPTY; INLINE_ENVIRONMENT_EDITS];
        values[0] = edit;
        *self = Self::Inline { values, len: 1 };
      },
      Self::Inline { values, len } if usize::from(*len) < values.len() => {
        values[usize::from(*len)] = edit;
        *len += 1;
      },
      Self::Inline { values, len } => {
        let mut edits = Vec::with_capacity(INLINE_ENVIRONMENT_EDITS * 2);
        edits.extend_from_slice(&values[..usize::from(*len)]);
        edits.push(edit);
        *self = Self::Heap(edits);
      },
      Self::Heap(values) => values.push(edit),
    }
  }

  fn as_slice(&self) -> &[EnvironmentEdit<'a>] {
    match self {
      Self::Empty => &[],
      Self::Inline { values, len } => &values[..usize::from(*len)],
      Self::Heap(values) => values,
    }
  }
}

/// Command-lifetime child environment edits in increasing precedence order.
/// Ambient values are inherited rather than copied into this batch.
pub(crate) struct ChildEnvironmentPlan<'a> {
  storage: EnvironmentStorage<'a>,
}

const _: () = assert!(size_of::<ChildEnvironmentPlan<'static>>() == 104);
const _: () = assert!(align_of::<ChildEnvironmentPlan<'static>>() == align_of::<usize>());

impl<'a> ChildEnvironmentPlan<'a> {
  pub(crate) fn capture(directives: impl IntoIterator<Item = &'a str>, arguments: CommandArguments<'a>) -> Result<Self, EnvironmentError> {
    let mut plan = Self {
      storage: EnvironmentStorage::default(),
    };
    plan.extend(EnvironmentSource::Directive, directives)?;
    plan.extend(EnvironmentSource::LaunchProfile, std::iter::empty())?;

    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
      match argument.to_str() {
        Some("-e" | "--environment") => {
          let assignment = arguments.get(index + 1).ok_or(EnvironmentError::MissingCommandLineValue)?;
          let assignment = assignment.to_str().ok_or(EnvironmentError::NonUnicodeCommandLineValue)?;
          plan.push(EnvironmentSource::CommandLine, assignment)?;
          index += 2;
        },
        Some(value) if value.starts_with("--environment=") => {
          plan.push(EnvironmentSource::CommandLine, &value["--environment=".len()..])?;
          index += 1;
        },
        _ => index += 1,
      }
    }
    Ok(plan)
  }

  #[cfg(test)]
  fn from_batches(
    directives: impl IntoIterator<Item = &'a str>,
    launch_profile: impl IntoIterator<Item = &'a str>,
    command_line: impl IntoIterator<Item = &'a str>,
  ) -> Result<Self, EnvironmentError> {
    let mut plan = Self {
      storage: EnvironmentStorage::default(),
    };
    plan.extend(EnvironmentSource::Directive, directives)?;
    plan.extend(EnvironmentSource::LaunchProfile, launch_profile)?;
    plan.extend(EnvironmentSource::CommandLine, command_line)?;
    Ok(plan)
  }

  fn extend(&mut self, source: EnvironmentSource, assignments: impl IntoIterator<Item = &'a str>) -> Result<(), EnvironmentError> {
    for assignment in assignments {
      self.push(source, assignment)?;
    }
    Ok(())
  }

  fn push(&mut self, source: EnvironmentSource, assignment: &'a str) -> Result<(), EnvironmentError> {
    self.storage.push(EnvironmentEdit::parse(source, assignment)?);
    Ok(())
  }

  pub(crate) fn edit_count(&self) -> usize {
    self.storage.as_slice().len()
  }

  pub(crate) fn sensitive_edit_count(&self) -> usize {
    self.storage.as_slice().iter().filter(|edit| edit.sensitive).count()
  }

  #[cfg(test)]
  fn effective(&self, name: &str) -> Option<&'a str> {
    self
      .storage
      .as_slice()
      .iter()
      .rev()
      .find(|edit| environment_names_equal(edit.name(), name))
      .map(|edit| edit.value())
  }
}

impl fmt::Debug for ChildEnvironmentPlan<'_> {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.debug_list().entries(self.storage.as_slice()).finish()
  }
}

#[cfg(all(test, windows))]
fn environment_names_equal(left: &str, right: &str) -> bool {
  left.eq_ignore_ascii_case(right)
}

#[cfg(all(test, not(windows)))]
fn environment_names_equal(left: &str, right: &str) -> bool {
  left == right
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnvironmentError {
  MalformedDirective,
  MissingCommandLineValue,
  NonUnicodeCommandLineValue,
  MissingSeparator { source: EnvironmentSource },
  EmptyName { source: EnvironmentSource },
  EmbeddedNul { source: EnvironmentSource },
  AssignmentTooLarge { source: EnvironmentSource },
}

impl fmt::Display for EnvironmentError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::MalformedDirective => formatter.write_str("environment directive must use [env:NAME=VALUE]"),
      Self::MissingCommandLineValue => formatter.write_str("--environment requires NAME=VALUE"),
      Self::NonUnicodeCommandLineValue => formatter.write_str("--environment NAME=VALUE must be valid Unicode text"),
      Self::MissingSeparator { source } => write!(formatter, "{} environment input must use NAME=VALUE", source.as_str()),
      Self::EmptyName { source } => write!(formatter, "{} environment input has an empty variable name", source.as_str()),
      Self::EmbeddedNul { source } => write!(formatter, "{} environment input contains a NUL code point", source.as_str()),
      Self::AssignmentTooLarge { source } => write!(formatter, "{} environment input exceeds the supported 4 GiB text boundary", source.as_str()),
    }
  }
}

pub(crate) fn directive_assignment(value: &str) -> Result<Option<&str>, EnvironmentError> {
  let Some(inner) = value.strip_prefix("[env:") else {
    return Ok(None);
  };
  let Some(inner) = inner.strip_suffix(']') else {
    return Err(EnvironmentError::MalformedDirective);
  };
  EnvironmentEdit::parse(EnvironmentSource::Directive, inner)?;
  Ok(Some(inner))
}

impl EnvironmentSource {
  fn as_str(self) -> &'static str {
    match self {
      Self::Ambient => "ambient",
      Self::Directive => "directive",
      Self::LaunchProfile => "launch-profile",
      Self::CommandLine => "command-line",
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::invocation::InvocationBatch;
  use std::ffi::OsString;

  #[test]
  fn precedence_is_ambient_then_directive_profile_and_command_line() {
    let plan = ChildEnvironmentPlan::from_batches(
      ["VALUE=directive", "SAME_SOURCE=first", "SAME_SOURCE=second"],
      ["VALUE=profile"],
      ["VALUE=command", "TOKEN=secret"],
    )
    .unwrap();

    assert_eq!(EnvironmentSource::Ambient as u8, 0);
    assert_eq!(plan.effective("VALUE"), Some("command"));
    assert_eq!(plan.effective("SAME_SOURCE"), Some("second"));
    assert_eq!(plan.edit_count(), 6);
    assert_eq!(plan.sensitive_edit_count(), 1);
    assert!(!format!("{plan:?}").contains("secret"));
  }

  #[test]
  fn command_line_capture_accepts_both_forms_and_spills_only_after_four_edits() {
    for count in [4, 5] {
      let mut arguments = vec![OsString::from("run")];
      for index in 0..count {
        arguments.push(OsString::from("--environment"));
        arguments.push(OsString::from(format!("VALUE_{index}={index}")));
      }
      let invocation = InvocationBatch::capture(arguments);
      let plan = ChildEnvironmentPlan::capture(std::iter::empty(), invocation.command_arguments()).unwrap();

      assert_eq!(plan.edit_count(), count);
      if count == INLINE_ENVIRONMENT_EDITS {
        assert!(matches!(plan.storage, EnvironmentStorage::Inline { .. }));
      } else {
        assert!(matches!(plan.storage, EnvironmentStorage::Heap(_)));
      }
      let name = format!("VALUE_{}", count - 1);
      let expected = (count - 1).to_string();
      assert_eq!(plan.effective(&name), Some(expected.as_str()));
    }
  }

  #[test]
  fn malformed_assignments_fail_without_retaining_values_in_errors() {
    for assignment in ["missing", "=secret", "PUBLIC=value\0hidden-secret"] {
      let error = ChildEnvironmentPlan::from_batches([assignment], [], []).unwrap_err();
      assert!(!error.to_string().contains("secret"));
    }
  }
}
