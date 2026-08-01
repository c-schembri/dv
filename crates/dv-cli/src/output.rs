use std::{borrow::Cow, ffi::OsStr};

use dv_core::redact_url_for_output;

pub(crate) const REDACTED_VALUE: &str = "<redacted>";

pub(crate) fn is_sensitive_name(name: &str) -> bool {
  let mut normalized = [0_u8; 64];
  let mut len = 0;
  for byte in name.bytes().filter(u8::is_ascii_alphanumeric) {
    if len == normalized.len() {
      return true;
    }
    normalized[len] = byte.to_ascii_lowercase();
    len += 1;
  }
  let normalized = &normalized[..len];
  [
    b"accesskey".as_slice(),
    b"apikey".as_slice(),
    b"authorization".as_slice(),
    b"authtoken".as_slice(),
    b"clientsecret".as_slice(),
    b"connectionstring".as_slice(),
    b"credential".as_slice(),
    b"password".as_slice(),
    b"passwd".as_slice(),
    b"privatekey".as_slice(),
    b"saskey".as_slice(),
    b"secret".as_slice(),
    b"token".as_slice(),
    b"username".as_slice(),
  ]
  .iter()
  .any(|needle| normalized.windows(needle.len()).any(|window| window == *needle))
    || matches!(normalized, b"k" | b"sk")
}

pub(crate) fn redact_argument_batch<'a>(arguments: impl Iterator<Item = &'a OsStr>, capacity: usize) -> Vec<String> {
  let mut output = Vec::with_capacity(capacity);
  let mut pending = PendingValue::None;
  for argument in arguments {
    let value = argument.to_string_lossy();
    let redacted = match pending {
      PendingValue::Secret => REDACTED_VALUE.to_owned(),
      PendingValue::Environment => redact_environment_assignment(&value).into_owned(),
      PendingValue::None => redact_argument_text_value(&value).into_owned(),
    };
    pending = if matches!(pending, PendingValue::None) {
      next_value_policy(&value)
    } else {
      PendingValue::None
    };
    output.push(redacted);
  }
  output
}

pub(crate) fn redact_os_argument(argument: &OsStr) -> String {
  redact_argument_text(argument).into_owned()
}

pub(crate) fn redact_argument_text(argument: &OsStr) -> Cow<'_, str> {
  let value = argument.to_string_lossy();
  match redact_argument_text_value(&value) {
    Cow::Borrowed(_) => value,
    Cow::Owned(redacted) => Cow::Owned(redacted),
  }
}

pub(crate) fn quoted_os_argument(argument: &OsStr) -> String {
  format!("{:?}", redact_os_argument(argument))
}

fn next_value_policy(value: &str) -> PendingValue {
  if matches!(value, "-e" | "--environment") {
    PendingValue::Environment
  } else if matches!(value.as_bytes().first(), Some(b'-' | b'/')) && is_sensitive_name(value.trim_start_matches(['-', '/'])) && !value.contains(['=', ':']) {
    PendingValue::Secret
  } else {
    PendingValue::None
  }
}

fn redact_argument_text_value(value: &str) -> Cow<'_, str> {
  let url_marker = value.find("://");
  let assignment = value.find('=');
  if value.starts_with("//") || url_marker.is_some_and(|marker| assignment.is_none_or(|separator| marker < separator)) {
    let url = redact_url_for_output(value);
    if matches!(url, Cow::Owned(_)) {
      return url;
    }
  }

  if let Some(inner) = value.strip_prefix("[env:").and_then(|value| value.strip_suffix(']')) {
    let redacted = redact_environment_assignment(inner);
    return match redacted {
      Cow::Borrowed(_) => Cow::Borrowed(value),
      Cow::Owned(redacted) => Cow::Owned(format!("[env:{redacted}]")),
    };
  }

  let option = value.trim_start_matches(['-', '/']);
  if option.len() != value.len()
    && let Some((name, _)) = option.split_once(':')
    && is_sensitive_name(name)
  {
    let prefix = &value[..value.len() - option.len()];
    return Cow::Owned(format!("{prefix}{name}:{REDACTED_VALUE}"));
  }

  let Some((name, raw_value)) = value.split_once('=') else {
    return Cow::Borrowed(value);
  };
  if name.eq_ignore_ascii_case("--environment") {
    return Cow::Owned(format!("{name}={}", redact_environment_assignment(raw_value)));
  }
  if is_sensitive_name(name.trim_start_matches('-')) {
    return Cow::Owned(format!("{name}={REDACTED_VALUE}"));
  }

  let redacted = redact_url_for_output(raw_value);
  match redacted {
    Cow::Borrowed(_) => Cow::Borrowed(value),
    Cow::Owned(redacted) => Cow::Owned(format!("{name}={redacted}")),
  }
}

fn redact_environment_assignment(value: &str) -> Cow<'_, str> {
  let Some((name, raw_value)) = value.split_once('=') else {
    return Cow::Borrowed(value);
  };
  if is_sensitive_name(name) {
    Cow::Owned(format!("{name}={REDACTED_VALUE}"))
  } else {
    let redacted = redact_url_for_output(raw_value);
    match redacted {
      Cow::Borrowed(_) => Cow::Borrowed(value),
      Cow::Owned(redacted) => Cow::Owned(format!("{name}={redacted}")),
    }
  }
}

#[derive(Clone, Copy)]
enum PendingValue {
  None,
  Secret,
  Environment,
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::ffi::OsString;

  #[test]
  fn argument_views_redact_secret_shapes_without_mutating_public_values() {
    let arguments = [
      "run",
      "[env:PUBLIC=value]",
      "[env:DV_TOKEN=directive-secret]",
      "--environment",
      "PASSWORD=command-secret",
      "--api-key=option-secret",
      "--client-secret",
      "separate-secret",
      "--source=https://user:pass@example.test/feed?sig=value#fragment",
      "https://user:secret@/broken?token=value",
      "plain",
    ]
    .map(OsString::from);

    let redacted = redact_argument_batch(arguments.iter().map(OsString::as_os_str), arguments.len());

    assert_eq!(
      redacted,
      [
        "run",
        "[env:PUBLIC=value]",
        "[env:DV_TOKEN=<redacted>]",
        "--environment",
        "PASSWORD=<redacted>",
        "--api-key=<redacted>",
        "--client-secret",
        "<redacted>",
        "--source=https://example.test/feed",
        "<redacted-url>",
        "plain",
      ]
    );
  }

  #[test]
  fn sensitive_name_matching_is_case_and_separator_insensitive() {
    for name in ["password", "NUGET_API_KEY", "client-secret", "Access.Token", "UserName"] {
      assert!(is_sensitive_name(name), "{name:?} was not classified as sensitive");
    }
    for name in ["PATH", "DV_CLI013_ORACLE", "MONKEY", "authentication_types"] {
      assert!(!is_sensitive_name(name), "{name:?} was over-classified");
    }
  }
}
