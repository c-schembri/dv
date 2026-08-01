use std::borrow::Cow;

/// Removes credential-bearing URL components before text reaches a reporter.
///
/// Non-URL text and already-safe URLs remain borrowed. URL user information,
/// query strings, and fragments are never reporter data.
pub fn redact_url_for_output(value: &str) -> Cow<'_, str> {
  if !value.bytes().any(|byte| matches!(byte, b'@' | b'?' | b'#')) {
    return Cow::Borrowed(value);
  }
  let Ok(mut url) = reqwest::Url::parse(value) else {
    return if value.contains("://") || value.starts_with("//") {
      Cow::Owned("<redacted-url>".into())
    } else {
      Cow::Borrowed(value)
    };
  };
  if url.set_username("").is_err() || url.set_password(None).is_err() {
    return Cow::Owned("<redacted-url>".into());
  }
  url.set_query(None);
  url.set_fragment(None);
  Cow::Owned(url.into())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn output_urls_drop_every_credential_bearing_component() {
    assert_eq!(
      redact_url_for_output("https://user:password@example.test/feed?token=value#fragment"),
      "https://example.test/feed"
    );
    assert!(matches!(redact_url_for_output("https://example.test/feed"), Cow::Borrowed(_)));
    assert_eq!(redact_url_for_output("relative?value"), "relative?value");
    assert_eq!(redact_url_for_output("https://user:secret@/broken?token=value"), "<redacted-url>");
    assert_eq!(redact_url_for_output("//user:secret@example.test/feed"), "<redacted-url>");
  }
}
