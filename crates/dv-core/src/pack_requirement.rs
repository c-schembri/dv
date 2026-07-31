use std::mem::{align_of, size_of};

/// The role of a missing SDK, runtime, or framework pack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackKind {
  /// Reference assemblies used by the compiler.
  Targeting,
  /// Managed and native assets carried by a self-contained application.
  Runtime,
  /// The native executable template for a target RID.
  Host,
  /// An installed framework used by a framework-dependent application.
  SharedFramework,
}

impl PackKind {
  /// Returns the stable reporter spelling.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Targeting => "targeting_pack",
      Self::Runtime => "runtime_pack",
      Self::Host => "host_pack",
      Self::SharedFramework => "shared_framework",
    }
  }
}

/// The concrete action which can satisfy a pack requirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackAcquisition {
  /// Install an SDK which carries the targeting pack.
  InstallSdk,
  /// Install an SDK carrying the targeting pack or restore its package.
  InstallSdkOrRestorePackage,
  /// Restore the named package into the configured global package cache.
  RestorePackage,
  /// Install the named shared runtime.
  InstallRuntime,
  /// Choose a RID advertised by the selected SDK pack manifest.
  ChooseRuntimeIdentifier,
}

impl PackAcquisition {
  /// Returns the stable reporter spelling.
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::InstallSdk => "install_sdk",
      Self::InstallSdkOrRestorePackage => "install_sdk_or_restore_package",
      Self::RestorePackage => "restore_package",
      Self::InstallRuntime => "install_runtime",
      Self::ChooseRuntimeIdentifier => "choose_runtime_identifier",
    }
  }

  /// Returns the actionable human guidance for this acquisition.
  pub const fn help(self) -> &'static str {
    match self {
      Self::InstallSdk => "Install a .NET SDK that provides the required targeting pack.",
      Self::InstallSdkOrRestorePackage => "Install an SDK that provides the targeting pack or restore the named pack from a configured package source.",
      Self::RestorePackage => "Restore the required pack from a configured package source.",
      Self::InstallRuntime => "Install the required shared .NET runtime or adjust the project's RollForward policy.",
      Self::ChooseRuntimeIdentifier => "Choose a RuntimeIdentifier supported by the selected SDK's portable RID graph and pack manifest.",
    }
  }
}

/// One actionable unavailable-pack requirement retained by planning errors.
///
/// All variable text lives in one allocation. Native-width spans avoid an
/// artificial error-path size limit while reporters expose stable typed fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackRequirement {
  text: Box<str>,
  identity_end: usize,
  version_end: usize,
  target_framework_end: usize,
  runtime_identifier_end: usize,
  kind: PackKind,
  acquisition: PackAcquisition,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<PackRequirement>() == 56);
const _: () = assert!(align_of::<PackRequirement>() == align_of::<usize>());

impl PackRequirement {
  pub(crate) fn new(
    kind: PackKind,
    identity: &str,
    version: Option<&str>,
    target_framework: &str,
    runtime_identifier: Option<&str>,
    acquisition: PackAcquisition,
  ) -> Self {
    let capacity = identity.len() + version.map_or(0, str::len) + target_framework.len() + runtime_identifier.map_or(0, str::len);
    let mut text = String::with_capacity(capacity);
    text.push_str(identity);
    let identity_end = text.len();
    text.extend(version);
    let version_end = text.len();
    text.push_str(target_framework);
    let target_framework_end = text.len();
    text.extend(runtime_identifier);
    let runtime_identifier_end = text.len();
    Self {
      text: text.into_boxed_str(),
      identity_end,
      version_end,
      target_framework_end,
      runtime_identifier_end,
      kind,
      acquisition,
    }
  }

  /// Returns the role of the unavailable pack.
  pub fn kind(&self) -> PackKind {
    self.kind
  }

  /// Returns the required package or shared-framework identity.
  pub fn identity(&self) -> &str {
    &self.text[..self.identity_end]
  }

  /// Returns the exact required version when selection reached one.
  pub fn version(&self) -> Option<&str> {
    (self.version_end != self.identity_end).then(|| &self.text[self.identity_end..self.version_end])
  }

  /// Returns the evaluated target framework.
  pub fn target_framework(&self) -> &str {
    &self.text[self.version_end..self.target_framework_end]
  }

  /// Returns the requested runtime identifier when the requirement is RID-specific.
  pub fn runtime_identifier(&self) -> Option<&str> {
    (self.runtime_identifier_end != self.target_framework_end).then(|| &self.text[self.target_framework_end..self.runtime_identifier_end])
  }

  /// Returns the concrete acquisition action.
  pub fn acquisition(&self) -> PackAcquisition {
    self.acquisition
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn requirement_keeps_optional_fields_in_one_text_allocation() {
    let requirement = PackRequirement::new(
      PackKind::Runtime,
      "Microsoft.NETCore.App.Runtime.linux-arm",
      Some("10.0.0"),
      "net10.0",
      Some("linux-arm"),
      PackAcquisition::RestorePackage,
    );

    assert_eq!(requirement.kind(), PackKind::Runtime);
    assert_eq!(requirement.identity(), "Microsoft.NETCore.App.Runtime.linux-arm");
    assert_eq!(requirement.version(), Some("10.0.0"));
    assert_eq!(requirement.target_framework(), "net10.0");
    assert_eq!(requirement.runtime_identifier(), Some("linux-arm"));
    assert_eq!(requirement.acquisition(), PackAcquisition::RestorePackage);
  }
}
