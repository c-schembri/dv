use std::{
  io,
  path::{Component, Path, PathBuf},
};

/// Makes a path absolute and removes lexical dot segments without resolving
/// filesystem links or requiring the final path to exist.
pub(crate) fn absolute_lexical(path: &Path) -> io::Result<PathBuf> {
  let absolute = if path.is_absolute() { path.to_owned() } else { std::path::absolute(path)? };
  let mut normalized = PathBuf::with_capacity(absolute.as_os_str().len());
  for component in absolute.components() {
    match component {
      Component::CurDir => {},
      Component::ParentDir => {
        normalized.pop();
      },
      Component::Prefix(_) | Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
    }
  }
  Ok(normalized)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn absolute_lexical_removes_dot_segments_without_requiring_the_path() {
    let root = std::env::temp_dir().join(format!("dv-missing-path-{}", std::process::id()));
    let input = root.join("alpha/./beta/../gamma/Absent.csproj");

    let normalized = absolute_lexical(&input).unwrap();

    assert_eq!(normalized, root.join("alpha/gamma/Absent.csproj"));
    assert!(!normalized.exists());
  }
}
