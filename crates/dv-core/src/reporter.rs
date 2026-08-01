use std::io::{self, Write};

use crate::{Event, validate_events};

/// Writes a validated event batch as one JSON object per line.
pub fn write_json_lines(events: &[Event], mut writer: impl Write) -> io::Result<()> {
  validate_events(events).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

  for event in events {
    serde_json::to_writer(&mut writer, event)?;
    writer.write_all(b"\n")?;
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use crate::{EventPayload, Outcome};

  use super::*;

  #[test]
  fn reporter_emits_one_round_trippable_event_per_line() {
    let events = [
      Event::new(
        0,
        0,
        EventPayload::CommandStarted {
          command_syntax_version: 1,
          command: "build".into(),
          args: vec!["--json".into()],
        },
      ),
      Event::new(
        1,
        10,
        EventPayload::CommandFinished {
          command: "build".into(),
          duration_us: 10,
          outcome: Outcome::Succeeded,
        },
      ),
    ];
    let mut output = Vec::new();

    write_json_lines(&events, &mut output).unwrap();

    let decoded: Vec<Event> = String::from_utf8(output)
      .unwrap()
      .lines()
      .map(|line| serde_json::from_str(line).unwrap())
      .collect();
    assert_eq!(decoded, events);
  }
}
