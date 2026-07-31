use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::Diagnostic;

/// Current version of the JSON event protocol.
pub const EVENT_SCHEMA_VERSION: u16 = 1;

/// The result of a command or work item.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
  /// The operation completed successfully.
  Succeeded,
  /// The operation failed.
  Failed,
  /// The operation was deliberately skipped.
  Skipped,
  /// The operation was cancelled before completion.
  Cancelled,
}

/// The result of a cache lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheOutcome {
  /// The requested data was found and reused.
  Hit,
  /// The requested data was not found.
  Miss,
  /// Stored data existed but was not valid for this request.
  Invalid,
}

/// One event in a command's ordered event stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Event {
  /// Wire protocol version.
  pub schema_version: u16,
  /// Zero-based position in this command's event stream.
  pub sequence: u64,
  /// Microseconds elapsed since command start.
  pub elapsed_us: u64,
  /// Event-specific data.
  #[serde(flatten)]
  pub payload: EventPayload,
}

impl Event {
  /// Creates an event using the current wire protocol version.
  pub fn new(sequence: u64, elapsed_us: u64, payload: EventPayload) -> Self {
    Self {
      schema_version: EVENT_SCHEMA_VERSION,
      sequence,
      elapsed_us,
      payload,
    }
  }
}

/// One SDK record materialized at the reporter boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SdkInstallationEvent {
  /// Installed SDK version.
  pub version: String,
  /// Full SDK directory.
  pub path: String,
  /// Whether this record was selected.
  pub selected: bool,
}

/// Event variants emitted by command execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
  /// The parsed command is about to execute.
  CommandStarted {
    /// Stable command name.
    command: String,
    /// Command arguments after the executable name.
    args: Vec<String>,
  },
  /// A batch of work is about to execute.
  WorkStarted {
    /// Stable operation name.
    operation: String,
    /// Number of items in the batch.
    item_count: u64,
  },
  /// A batch of work has completed.
  WorkFinished {
    /// Stable operation name matching the start event.
    operation: String,
    /// Number of items processed.
    item_count: u64,
    /// Time spent in the operation.
    duration_us: u64,
    /// Terminal result.
    outcome: Outcome,
  },
  /// A cache key was classified without exposing cache contents.
  CacheDecision {
    /// Cache namespace, such as `packages` or `build`.
    cache: String,
    /// Stable key or digest text.
    key: String,
    /// Lookup result.
    outcome: CacheOutcome,
  },
  /// The current SDK was selected.
  SdkSelected {
    /// Selected SDK version.
    version: String,
    /// Full SDK directory.
    path: String,
    /// Installation root.
    root: String,
    /// `global.json` used for selection, when present.
    global_json: Option<String>,
  },
  /// The installed SDK batch was discovered.
  SdkInventory {
    /// SDK records in deterministic resolver order.
    installations: Vec<SdkInstallationEvent>,
    /// `global.json` used for selection, when present.
    global_json: Option<String>,
  },
  /// A structured diagnostic was produced.
  Diagnostic {
    /// Diagnostic data.
    diagnostic: Diagnostic,
  },
  /// The command reached a terminal state.
  CommandFinished {
    /// Stable command name matching the start event.
    command: String,
    /// Total command time.
    duration_us: u64,
    /// Terminal result.
    outcome: Outcome,
  },
}

/// A malformed event batch rejected before reporting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventStreamError {
  /// An event uses an unsupported schema.
  UnsupportedSchema {
    /// Position in the supplied batch.
    index: usize,
    /// Schema found at that position.
    found: u16,
  },
  /// Sequence numbers are not contiguous and zero-based.
  UnexpectedSequence {
    /// Position in the supplied batch.
    index: usize,
    /// Required sequence number.
    expected: u64,
    /// Sequence number found.
    found: u64,
  },
  /// Elapsed time moved backwards.
  ElapsedTimeRegressed {
    /// Position in the supplied batch.
    index: usize,
    /// Previous elapsed time.
    previous_us: u64,
    /// Current elapsed time.
    found_us: u64,
  },
}

impl fmt::Display for EventStreamError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::UnsupportedSchema { index, found } => write!(formatter, "event {index} uses schema {found}; expected {EVENT_SCHEMA_VERSION}"),
      Self::UnexpectedSequence { index, expected, found } => write!(formatter, "event {index} has sequence {found}; expected {expected}"),
      Self::ElapsedTimeRegressed { index, previous_us, found_us } => {
        write!(formatter, "event {index} elapsed time regressed from {previous_us}us to {found_us}us")
      },
    }
  }
}

impl Error for EventStreamError {}

/// Validates the schema, ordering, and monotonic time of an event batch.
pub fn validate_events(events: &[Event]) -> Result<(), EventStreamError> {
  let mut previous_us = 0;

  for (index, event) in events.iter().enumerate() {
    if event.schema_version != EVENT_SCHEMA_VERSION {
      return Err(EventStreamError::UnsupportedSchema {
        index,
        found: event.schema_version,
      });
    }

    let expected = index as u64;
    if event.sequence != expected {
      return Err(EventStreamError::UnexpectedSequence {
        index,
        expected,
        found: event.sequence,
      });
    }

    if index > 0 && event.elapsed_us < previous_us {
      return Err(EventStreamError::ElapsedTimeRegressed {
        index,
        previous_us,
        found_us: event.elapsed_us,
      });
    }
    previous_us = event.elapsed_us;
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn event(sequence: u64, elapsed_us: u64) -> Event {
    Event::new(
      sequence,
      elapsed_us,
      EventPayload::WorkStarted {
        operation: "parse_projects".into(),
        item_count: 3,
      },
    )
  }

  #[test]
  fn valid_batch_is_contiguous_and_monotonic() {
    assert!(validate_events(&[event(0, 0), event(1, 5)]).is_ok());
  }

  #[test]
  fn rejects_sequence_gaps() {
    assert!(matches!(
      validate_events(&[event(0, 0), event(2, 5)]),
      Err(EventStreamError::UnexpectedSequence { .. })
    ));
  }

  #[test]
  fn rejects_time_regression() {
    assert!(matches!(
      validate_events(&[event(0, 5), event(1, 4)]),
      Err(EventStreamError::ElapsedTimeRegressed { .. })
    ));
  }
}
