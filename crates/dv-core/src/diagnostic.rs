use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

/// A stable, machine-readable diagnostic identifier in `DVdddd` form.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DiagnosticCode(String);

impl DiagnosticCode {
  /// Validates and stores a diagnostic code.
  pub fn parse(value: impl Into<String>) -> Result<Self, DiagnosticCodeError> {
    let value = value.into();
    let bytes = value.as_bytes();
    let valid = bytes.len() == 6 && bytes[0] == b'D' && bytes[1] == b'V' && bytes[2..].iter().all(u8::is_ascii_digit);

    if valid { Ok(Self(value)) } else { Err(DiagnosticCodeError(value)) }
  }

  /// Returns the validated code text.
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl fmt::Display for DiagnosticCode {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.0.fmt(formatter)
  }
}

/// An invalid diagnostic identifier rejected at the protocol boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticCodeError(String);

impl fmt::Display for DiagnosticCodeError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "diagnostic code {:?} must match DV followed by four digits", self.0)
  }
}

impl Error for DiagnosticCodeError {}

/// The user-facing impact of a diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
  /// Information that does not imply a problem.
  Info,
  /// A recoverable condition that deserves attention.
  Warning,
  /// A failure that prevents the requested operation.
  Error,
}

/// One ordered piece of diagnostic context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextField {
  /// Stable field name for machines and reporters.
  pub name: String,
  /// Human-readable field value.
  pub value: String,
}

/// A structured failure or warning passed unchanged to reporters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
  /// Stable diagnostic identifier.
  pub code: DiagnosticCode,
  /// User-facing severity.
  pub severity: Severity,
  /// Short description of what happened.
  pub message: String,
  /// Ordered context relevant to this occurrence.
  pub context: Vec<ContextField>,
  /// Causal messages ordered from nearest to root cause.
  pub causes: Vec<String>,
  /// Suggested next action when one is known.
  pub help: Option<String>,
}

impl Diagnostic {
  /// Creates a diagnostic with no optional context.
  pub fn new(code: DiagnosticCode, severity: Severity, message: impl Into<String>) -> Self {
    let message = message.into();
    assert!(!message.is_empty(), "diagnostic message must not be empty");

    Self {
      code,
      severity,
      message,
      context: Vec::new(),
      causes: Vec::new(),
      help: None,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn diagnostic_code_accepts_only_the_wire_format() {
    assert_eq!(DiagnosticCode::parse("DV0001").unwrap().as_str(), "DV0001");

    for invalid in ["", "dv0001", "DV001", "DV00001", "DX0001", "DV00A1"] {
      assert!(DiagnosticCode::parse(invalid).is_err(), "{invalid:?} was accepted");
    }
  }
}
