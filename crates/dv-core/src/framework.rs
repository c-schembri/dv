use std::{error::Error, fmt};

/// A recognized target-framework family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameworkFamily {
  /// Modern unified .NET (`net5.0` and later).
  Net,
  /// Pre-unification .NET Core (`netcoreapp`).
  NetCoreApp,
  /// .NET Standard.
  NetStandard,
  /// .NET Framework (`net48`, `net472`, and related TFMs).
  NetFramework,
}

/// Parsed target-framework data shared by evaluation and downstream planners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetFramework {
  family: FrameworkFamily,
  major: u16,
  minor: u16,
}

impl TargetFramework {
  /// Parses a canonical target-framework moniker without consulting an SDK.
  pub fn parse(value: &str) -> Result<Self, TargetFrameworkError> {
    let lower = value.to_ascii_lowercase();
    if let Some(version) = lower.strip_prefix("netcoreapp") {
      let (major, minor) = dotted_version(version, value)?;
      return Ok(Self {
        family: FrameworkFamily::NetCoreApp,
        major,
        minor,
      });
    }
    if let Some(version) = lower.strip_prefix("netstandard") {
      let (major, minor) = dotted_version(version, value)?;
      return Ok(Self {
        family: FrameworkFamily::NetStandard,
        major,
        minor,
      });
    }
    let Some(version) = lower.strip_prefix("net") else {
      return Err(TargetFrameworkError(value.into()));
    };
    if version.contains('.') {
      let (major, minor) = dotted_version(version, value)?;
      return Ok(Self {
        family: FrameworkFamily::Net,
        major,
        minor,
      });
    }
    if (2..=3).contains(&version.len()) && version.bytes().all(|byte| byte.is_ascii_digit()) {
      let digits: Vec<u16> = version.bytes().map(|byte| u16::from(byte - b'0')).collect();
      return Ok(Self {
        family: FrameworkFamily::NetFramework,
        major: digits[0],
        minor: if digits.len() == 2 { digits[1] } else { digits[1] * 10 + digits[2] },
      });
    }
    Err(TargetFrameworkError(value.into()))
  }

  /// Returns the target family.
  pub fn family(self) -> FrameworkFamily {
    self.family
  }

  /// Returns the major framework version.
  pub fn major(self) -> u16 {
    self.major
  }

  /// Returns the minor framework version.
  pub fn minor(self) -> u16 {
    self.minor
  }

  /// Returns whether the initial SDK/pack pipeline supports this target.
  pub fn is_modern_net(self) -> bool {
    self.family == FrameworkFamily::Net && self.major >= 5
  }

  /// Returns the framework version used by Microsoft.NETCore.App.Ref manifests.
  pub fn framework_version(self) -> String {
    format!("{}.{}", self.major, self.minor)
  }

  /// Returns the SDK analysis-level stem for supported modern targets.
  pub fn analysis_level(self) -> Result<u16, TargetFrameworkError> {
    if self.is_modern_net() {
      Ok(self.major)
    } else {
      Err(TargetFrameworkError(format!("{self:?}")))
    }
  }

  /// Returns current known C# defaults for supported modern target generations.
  ///
  /// The relation is centralized here and deliberately rejects unknown future
  /// generations until their SDK behavior is captured.
  pub fn csharp_language_major(self) -> Result<u16, TargetFrameworkError> {
    if self.is_modern_net() && self.major <= 10 && self.minor == 0 {
      Ok(self.major + 4)
    } else {
      Err(TargetFrameworkError(format!("{self:?}")))
    }
  }
}

/// A malformed or currently unsupported target-framework value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetFrameworkError(String);

impl fmt::Display for TargetFrameworkError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "invalid or unsupported target framework {:?}", self.0)
  }
}

impl Error for TargetFrameworkError {}

fn dotted_version(value: &str, original: &str) -> Result<(u16, u16), TargetFrameworkError> {
  let mut parts = value.split('.');
  let major = parts
    .next()
    .filter(|part| !part.is_empty())
    .and_then(|part| part.parse().ok())
    .ok_or_else(|| TargetFrameworkError(original.into()))?;
  let minor = parts
    .next()
    .filter(|part| !part.is_empty())
    .and_then(|part| part.parse().ok())
    .ok_or_else(|| TargetFrameworkError(original.into()))?;
  if parts.next().is_some() {
    return Err(TargetFrameworkError(original.into()));
  }
  Ok((major, minor))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_modern_and_legacy_families_without_conflating_them() {
    assert_eq!(TargetFramework::parse("net10.0").unwrap().family(), FrameworkFamily::Net);
    assert_eq!(TargetFramework::parse("netcoreapp3.1").unwrap().family(), FrameworkFamily::NetCoreApp);
    assert_eq!(TargetFramework::parse("netstandard2.0").unwrap().family(), FrameworkFamily::NetStandard);
    assert_eq!(TargetFramework::parse("net48").unwrap().family(), FrameworkFamily::NetFramework);
    assert_eq!(TargetFramework::parse("net48").unwrap().minor(), 8);
    assert_eq!(TargetFramework::parse("net472").unwrap().minor(), 72);
  }

  #[test]
  fn compiler_defaults_reject_unknown_future_generations() {
    assert_eq!(TargetFramework::parse("net8.0").unwrap().csharp_language_major().unwrap(), 12);
    assert!(TargetFramework::parse("net11.0").unwrap().csharp_language_major().is_err());
  }
}
