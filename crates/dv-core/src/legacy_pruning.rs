use crate::FrameworkFamily;
use std::mem::{align_of, size_of};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PruningFramework {
  Default,
  Core,
  AspNetCore,
  WindowsDesktop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyTargetFamily {
  NetCoreApp,
  NetStandard,
}

#[derive(Clone, Copy)]
pub(crate) struct LegacyPrunePackage {
  pub(crate) id: &'static str,
  pub(crate) numbers: [u32; 4],
}

// ASSUMPTION: benchmark machines use 64-byte cache lines. Two immutable rows
// occupy each line. The 4,032 generated rows retain 126 KiB before the string
// pool; a selected Core + ASP.NET + WindowsDesktop batch is at most 435 source
// rows and is compacted into the resolver's owned text/row buffers once.
const _: () = assert!(size_of::<LegacyPrunePackage>() == 32);
const _: () = assert!(align_of::<LegacyPrunePackage>() == 8);

struct LegacyPruneSet {
  family: LegacyTargetFamily,
  major: u16,
  minor: u16,
  framework: PruningFramework,
  packages: &'static [LegacyPrunePackage],
}

include!("legacy_pruning_data.rs");

pub(crate) fn exact_legacy_pruning(
  target_family: FrameworkFamily,
  target_major: u16,
  target_minor: u16,
  framework: PruningFramework,
) -> Option<&'static [LegacyPrunePackage]> {
  let family = legacy_family(target_family)?;
  LEGACY_PRUNE_SETS
    .iter()
    .find(|set| set.family == family && set.major == target_major && set.minor == target_minor && set.framework == framework)
    .map(|set| set.packages)
}

pub(crate) fn nearest_legacy_pruning(
  target_family: FrameworkFamily,
  target_major: u16,
  target_minor: u16,
  framework: PruningFramework,
) -> Option<&'static [LegacyPrunePackage]> {
  let family = legacy_family(target_family)?;
  LEGACY_PRUNE_SETS
    .iter()
    .filter(|set| set.family == family && set.framework == framework && (set.major, set.minor) <= (target_major, target_minor))
    .max_by_key(|set| (set.major, set.minor))
    .map(|set| set.packages)
}

fn legacy_family(family: FrameworkFamily) -> Option<LegacyTargetFamily> {
  match family {
    FrameworkFamily::Net | FrameworkFamily::NetCoreApp => Some(LegacyTargetFamily::NetCoreApp),
    FrameworkFamily::NetStandard => Some(LegacyTargetFamily::NetStandard),
    FrameworkFamily::NetFramework => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::TargetFramework;

  #[test]
  fn generated_tables_preserve_inheritance_removals_and_nearest_reduction() {
    let net8 = TargetFramework::parse("net8.0").unwrap();
    let net9 = TargetFramework::parse("net9.0").unwrap();
    let net10 = TargetFramework::parse("net10.0").unwrap();
    let net8_asp = exact_legacy_pruning(net8.family(), net8.major(), net8.minor(), PruningFramework::AspNetCore).unwrap();
    let net9_asp = exact_legacy_pruning(net9.family(), net9.major(), net9.minor(), PruningFramework::AspNetCore).unwrap();
    let net10_windows = nearest_legacy_pruning(net10.family(), net10.major(), net10.minor(), PruningFramework::WindowsDesktop).unwrap();

    assert!(net8_asp.iter().any(|package| package.id == "system.io.pipelines"));
    assert!(!net9_asp.iter().any(|package| package.id == "system.io.pipelines"));
    assert!(
      net10_windows
        .iter()
        .any(|package| package.id == "system.windows.extensions" && package.numbers == [9, 0, 0, 0])
    );
  }
}
