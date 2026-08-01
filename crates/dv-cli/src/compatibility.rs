use std::io::{self, Write};

pub(crate) const COMPATIBILITY_MANIFEST_BYTES: usize = include_bytes!("../../../compatibility/manifest.json").len();

const COMPATIBILITY_MANIFEST: &[u8; COMPATIBILITY_MANIFEST_BYTES] = include_bytes!("../../../compatibility/manifest.json");

pub(crate) fn write_manifest(mut destination: impl Write) -> io::Result<()> {
  destination.write_all(COMPATIBILITY_MANIFEST)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn embedded_manifest_is_the_checked_in_artifact() {
    let mut output = Vec::new();
    write_manifest(&mut output).unwrap();

    assert_eq!(output.as_slice(), COMPATIBILITY_MANIFEST);
    assert_eq!(output.len(), COMPATIBILITY_MANIFEST_BYTES);
  }
}
