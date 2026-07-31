use std::{
  collections::HashSet,
  error::Error,
  fmt, fs,
  mem::{align_of, size_of},
  path::{Path, PathBuf},
};

use quick_xml::{Reader, XmlVersion, events::Event};

use crate::{PackageResolution, ProjectConfiguration, ProjectOutputType, ProjectSpec, SdkInventory, TargetFramework};

const FRAMEWORK_IDENTIFIER: &str = ".NETCoreApp";
const FRAMEWORK_PACK: &str = "Microsoft.NETCore.App.Ref";
const SDK_ANALYZERS: [&str; 2] = ["Microsoft.CodeAnalysis.CSharp.NetAnalyzers.dll", "Microsoft.CodeAnalysis.NetAnalyzers.dll"];
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextSpan {
  start: u32,
  len: u32,
}

const _: () = assert!(size_of::<TextSpan>() == 8);
const _: () = assert!(align_of::<TextSpan>() == 4);

/// One immutable Roslyn input plan.
///
/// All variable text is owned by one contiguous allocation. Ordered batches
/// contain eight-byte spans and are read linearly by reporters and the future
/// compiler host. Assuming a 64-byte cache line, eight spans fit per line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompilerPlan {
  text: Box<str>,
  project: TextSpan,
  sdk_version: TextSpan,
  compiler: TextSpan,
  framework_pack_version: TextSpan,
  framework_pack: TextSpan,
  language_version: TextSpan,
  output_assembly: TextSpan,
  output_pdb: TextSpan,
  reference_output: TextSpan,
  sources: Box<[TextSpan]>,
  generated_sources: Box<[TextSpan]>,
  references: Box<[TextSpan]>,
  analyzers: Box<[TextSpan]>,
  analyzer_configs: Box<[TextSpan]>,
  defines: Box<[TextSpan]>,
  configuration: ProjectConfiguration,
  output_type: ProjectOutputType,
  nullable: bool,
  deterministic: bool,
  warning_level: u16,
}

impl CompilerPlan {
  /// Returns the project file represented by this plan.
  pub fn project(&self) -> &str {
    self.get(self.project)
  }

  /// Returns the selected SDK version.
  pub fn sdk_version(&self) -> &str {
    self.get(self.sdk_version)
  }

  /// Returns the selected Roslyn compiler assembly.
  pub fn compiler(&self) -> &str {
    self.get(self.compiler)
  }

  /// Returns the selected framework reference-pack version.
  pub fn framework_pack_version(&self) -> &str {
    self.get(self.framework_pack_version)
  }

  /// Returns the selected framework reference-pack directory.
  pub fn framework_pack(&self) -> &str {
    self.get(self.framework_pack)
  }

  /// Returns the C# language version fixed by the target framework.
  pub fn language_version(&self) -> &str {
    self.get(self.language_version)
  }

  /// Returns the build configuration.
  pub fn configuration(&self) -> ProjectConfiguration {
    self.configuration
  }

  /// Returns the Roslyn output kind.
  pub fn output_type(&self) -> ProjectOutputType {
    self.output_type
  }

  /// Returns whether nullable analysis is enabled.
  pub fn nullable_enabled(&self) -> bool {
    self.nullable
  }

  /// Returns whether deterministic output is required.
  pub fn deterministic(&self) -> bool {
    self.deterministic
  }

  /// Returns the compiler warning level.
  pub fn warning_level(&self) -> u16 {
    self.warning_level
  }

  /// Returns the planned output assembly.
  pub fn output_assembly(&self) -> &str {
    self.get(self.output_assembly)
  }

  /// Returns the planned portable PDB.
  pub fn output_pdb(&self) -> &str {
    self.get(self.output_pdb)
  }

  /// Returns the planned reference assembly.
  pub fn reference_output(&self) -> &str {
    self.get(self.reference_output)
  }

  /// Iterates user source paths in deterministic project order.
  pub fn sources(&self) -> impl ExactSizeIterator<Item = &str> {
    self.values(&self.sources)
  }

  /// Iterates generated source paths in compiler order.
  pub fn generated_sources(&self) -> impl ExactSizeIterator<Item = &str> {
    self.values(&self.generated_sources)
  }

  /// Iterates framework reference assemblies in manifest order.
  pub fn references(&self) -> impl ExactSizeIterator<Item = &str> {
    self.values(&self.references)
  }

  /// Iterates SDK and framework analyzers in compiler order.
  pub fn analyzers(&self) -> impl ExactSizeIterator<Item = &str> {
    self.values(&self.analyzers)
  }

  /// Iterates analyzer configuration files in precedence order.
  pub fn analyzer_configs(&self) -> impl ExactSizeIterator<Item = &str> {
    self.values(&self.analyzer_configs)
  }

  /// Iterates preprocessor symbols in deterministic order.
  pub fn defines(&self) -> impl ExactSizeIterator<Item = &str> {
    self.values(&self.defines)
  }

  fn values<'a>(&'a self, spans: &'a [TextSpan]) -> impl ExactSizeIterator<Item = &'a str> {
    spans.iter().map(|span| self.get(*span))
  }

  fn get(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }
}

/// Stable category for framework-pack and compiler-plan failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompilerPlanErrorKind {
  /// Required filesystem data could not be read.
  Io,
  /// No compatible installed reference pack exists.
  PackNotFound,
  /// A reference-pack manifest is malformed or incompatible.
  InvalidManifest,
  /// A manifest or SDK tool points to a missing asset.
  MissingAsset,
  /// A selected SDK cannot compile the current stable target.
  UnsupportedSdk,
  /// A path cannot be represented by the UTF-8 plan table.
  NonUnicodePath,
  /// The compact text table exceeded its four-GiB range.
  TextOverflow,
  /// Package-bearing projects were not paired with a resolved package graph.
  PackageResolution,
}

/// A compiler planning failure with stable path context.
#[derive(Debug)]
pub struct CompilerPlanError {
  kind: CompilerPlanErrorKind,
  path: PathBuf,
  message: String,
}

impl CompilerPlanError {
  /// Returns the stable failure category.
  pub fn kind(&self) -> CompilerPlanErrorKind {
    self.kind
  }

  /// Returns the asset or directory associated with the failure.
  pub fn path(&self) -> &Path {
    &self.path
  }

  fn new(kind: CompilerPlanErrorKind, path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
    Self {
      kind,
      path: path.into(),
      message: message.into(),
    }
  }
}

impl fmt::Display for CompilerPlanError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl Error for CompilerPlanError {}

struct FrameworkAssets {
  version: String,
  root: PathBuf,
  references: Vec<PathBuf>,
  analyzers: Vec<PathBuf>,
}

/// Plans compiler inputs for a project batch using one selected SDK inventory.
///
/// Pack and SDK discovery happen once. Each project is then materialized into
/// independent immutable storage. Empty input returns an empty batch without
/// touching the filesystem.
pub fn plan_compiler_inputs(projects: &[&ProjectSpec], inventory: &SdkInventory) -> Result<Vec<CompilerPlan>, CompilerPlanError> {
  if let Some(project) = projects.iter().find(|project| !project.package_references().is_empty()) {
    return Err(CompilerPlanError::new(
      CompilerPlanErrorKind::PackageResolution,
      project.project_path(),
      "package-bearing projects require a resolved package graph",
    ));
  }
  plan_compiler_inputs_inner(projects, inventory, &[])
}

/// Plans compiler inputs with one package resolution per project.
pub fn plan_compiler_inputs_with_packages(
  projects: &[&ProjectSpec],
  inventory: &SdkInventory,
  packages: &[PackageResolution],
) -> Result<Vec<CompilerPlan>, CompilerPlanError> {
  if projects.len() != packages.len() {
    return Err(CompilerPlanError::new(
      CompilerPlanErrorKind::PackageResolution,
      PathBuf::new(),
      format!("compiler planning received {} projects but {} package graphs", projects.len(), packages.len()),
    ));
  }
  for (project, packages) in projects.iter().zip(packages) {
    if !packages.matches_project(project) {
      return Err(CompilerPlanError::new(
        CompilerPlanErrorKind::PackageResolution,
        project.project_path(),
        "package graph does not match the project target, identity, or direct references",
      ));
    }
  }
  plan_compiler_inputs_inner(projects, inventory, packages)
}

fn plan_compiler_inputs_inner(
  projects: &[&ProjectSpec],
  inventory: &SdkInventory,
  packages: &[PackageResolution],
) -> Result<Vec<CompilerPlan>, CompilerPlanError> {
  if projects.is_empty() {
    return Ok(Vec::new());
  }

  let selected = inventory.selected();
  let target = projects[0].target();
  if projects.iter().any(|project| project.target() != target) {
    return Err(CompilerPlanError::new(
      CompilerPlanErrorKind::UnsupportedSdk,
      projects[0].project_path(),
      "one compiler planning batch currently requires a single target framework",
    ));
  }
  if selected.version.major() < u32::from(target.major()) {
    return Err(CompilerPlanError::new(
      CompilerPlanErrorKind::UnsupportedSdk,
      inventory.installation_path(selected),
      format!(".NET SDK {} cannot compile target {}", selected.version, projects[0].target_framework()),
    ));
  }

  let sdk_root = inventory.installation_path(selected);
  let compiler = require_file(sdk_root.join("Roslyn/bincore/csc.dll"), "Roslyn compiler")?;
  let sdk_analyzer_root = sdk_root.join("Sdks/Microsoft.NET.Sdk/analyzers");
  let mut sdk_analyzers = Vec::with_capacity(SDK_ANALYZERS.len());
  for analyzer in SDK_ANALYZERS {
    sdk_analyzers.push(require_file(sdk_analyzer_root.join(analyzer), "SDK analyzer")?);
  }
  let analysis_level = target
    .analysis_level()
    .map_err(|error| CompilerPlanError::new(CompilerPlanErrorKind::UnsupportedSdk, projects[0].project_path(), error.to_string()))?;
  let analysis_config = require_file(
    sdk_analyzer_root.join(format!("build/config/analysislevel_{analysis_level}_default.globalconfig")),
    "SDK analyzer configuration",
  )?;
  let framework = discover_framework_assets(inventory.root(selected), target, projects[0].target_framework())?;

  let mut plans = Vec::with_capacity(projects.len());
  for (index, project) in projects.iter().enumerate() {
    plans.push(materialize_plan(
      project,
      selected.version.as_str(),
      &compiler,
      &sdk_analyzers,
      &analysis_config,
      &framework,
      packages.get(index),
    )?);
  }
  Ok(plans)
}

fn discover_framework_assets(dotnet_root: &Path, target: TargetFramework, target_text: &str) -> Result<FrameworkAssets, CompilerPlanError> {
  let packs_root = dotnet_root.join("packs").join(FRAMEWORK_PACK);
  let entries = fs::read_dir(&packs_root).map_err(|error| {
    CompilerPlanError::new(
      CompilerPlanErrorKind::PackNotFound,
      &packs_root,
      format!("failed to enumerate {FRAMEWORK_PACK}: {error}"),
    )
  })?;
  let mut selected: Option<((u32, u32, u32), String, PathBuf)> = None;
  for entry in entries {
    let entry = entry.map_err(|error| CompilerPlanError::new(CompilerPlanErrorKind::Io, &packs_root, error.to_string()))?;
    if !entry
      .file_type()
      .map_err(|error| CompilerPlanError::new(CompilerPlanErrorKind::Io, entry.path(), error.to_string()))?
      .is_dir()
    {
      continue;
    }
    let Some(version_text) = entry.file_name().to_str().map(str::to_owned) else {
      continue;
    };
    let Some(version) = parse_stable_pack_version(&version_text) else {
      continue;
    };
    if version.0 != u32::from(target.major()) || version.1 != u32::from(target.minor()) {
      continue;
    }
    if selected.as_ref().is_none_or(|current| version > current.0) {
      selected = Some((version, version_text, entry.path()));
    }
  }
  let (_, version, root) = selected.ok_or_else(|| {
    CompilerPlanError::new(
      CompilerPlanErrorKind::PackNotFound,
      &packs_root,
      format!("no installed {FRAMEWORK_PACK} pack supports {target_text}"),
    )
  })?;

  let manifest = root.join("data/FrameworkList.xml");
  let bytes =
    fs::read(&manifest).map_err(|error| CompilerPlanError::new(CompilerPlanErrorKind::Io, &manifest, format!("failed to read framework manifest: {error}")))?;
  let expected_version = target.framework_version();
  let (reference_paths, analyzer_paths) = parse_framework_manifest(&manifest, &bytes, &expected_version, target_text)?;
  let references = validate_reference_assemblies(&root, reference_paths, target_text)?;
  let mut analyzers = Vec::with_capacity(analyzer_paths.len());
  for path in analyzer_paths {
    analyzers.push(require_file(root.join(path), "framework analyzer")?);
  }
  if references.is_empty() {
    return Err(CompilerPlanError::new(
      CompilerPlanErrorKind::InvalidManifest,
      &manifest,
      "framework manifest contains no managed reference assemblies",
    ));
  }

  Ok(FrameworkAssets {
    version,
    root,
    references,
    analyzers,
  })
}

fn validate_reference_assemblies(root: &Path, manifest_paths: Vec<String>, target_text: &str) -> Result<Vec<PathBuf>, CompilerPlanError> {
  let reference_root = root.join("ref").join(target_text);
  let entries = fs::read_dir(&reference_root).map_err(|error| {
    CompilerPlanError::new(
      CompilerPlanErrorKind::Io,
      &reference_root,
      format!("failed to enumerate framework references: {error}"),
    )
  })?;
  let mut installed = HashSet::with_capacity(manifest_paths.len());
  for entry in entries {
    let entry = entry.map_err(|error| CompilerPlanError::new(CompilerPlanErrorKind::Io, &reference_root, error.to_string()))?;
    if entry
      .file_type()
      .map_err(|error| CompilerPlanError::new(CompilerPlanErrorKind::Io, entry.path(), error.to_string()))?
      .is_file()
    {
      installed.insert(entry.path());
    }
  }

  let mut references = Vec::with_capacity(manifest_paths.len());
  for relative in manifest_paths {
    let path = root.join(relative);
    if !installed.contains(&path) {
      return Err(CompilerPlanError::new(
        CompilerPlanErrorKind::MissingAsset,
        &path,
        format!("framework reference assembly {} is missing", path.display()),
      ));
    }
    references.push(path);
  }
  Ok(references)
}

fn parse_stable_pack_version(value: &str) -> Option<(u32, u32, u32)> {
  if value.contains(['-', '+']) {
    return None;
  }
  let mut parts = value.split('.');
  let version = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
  parts.next().is_none().then_some(version)
}

fn parse_framework_manifest(path: &Path, bytes: &[u8], expected_version: &str, target_text: &str) -> Result<(Vec<String>, Vec<String>), CompilerPlanError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  let mut root_seen = false;
  let mut references = Vec::new();
  let mut analyzers = Vec::new();

  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) | Ok(Event::Empty(element)) if element.name().as_ref() == b"FileList" => {
        if root_seen {
          return Err(invalid_manifest(path, "framework manifest contains multiple FileList roots"));
        }
        let identifier = xml_attribute(&reader, &element, b"TargetFrameworkIdentifier", path)?;
        let version = xml_attribute(&reader, &element, b"TargetFrameworkVersion", path)?;
        if identifier.as_deref() != Some(FRAMEWORK_IDENTIFIER) || version.as_deref() != Some(expected_version) {
          return Err(invalid_manifest(path, format!("framework manifest target does not match {target_text}")));
        }
        root_seen = true;
      },
      Ok(Event::Start(element)) | Ok(Event::Empty(element)) if element.name().as_ref() == b"File" => {
        if !root_seen {
          return Err(invalid_manifest(path, "framework asset appears before the FileList root"));
        }
        let kind = xml_attribute(&reader, &element, b"Type", path)?;
        let asset = xml_attribute(&reader, &element, b"Path", path)?.ok_or_else(|| invalid_manifest(path, "framework asset has no Path attribute"))?;
        if Path::new(&asset).is_absolute() || Path::new(&asset).components().any(|part| matches!(part, std::path::Component::ParentDir)) {
          return Err(invalid_manifest(path, "framework asset path escapes the selected pack"));
        }
        match kind.as_deref() {
          Some("Managed") => references.push(asset),
          Some("Analyzer") if xml_attribute(&reader, &element, b"Language", path)?.as_deref() == Some("cs") => analyzers.push(asset),
          _ => {},
        }
      },
      Ok(Event::Eof) => break,
      Ok(_) => {},
      Err(error) => return Err(invalid_manifest(path, format!("invalid framework manifest XML: {error}"))),
    }
  }
  if !root_seen {
    return Err(invalid_manifest(path, "framework manifest has no FileList root"));
  }
  Ok((references, analyzers))
}

fn xml_attribute(reader: &Reader<&[u8]>, element: &quick_xml::events::BytesStart<'_>, name: &[u8], path: &Path) -> Result<Option<String>, CompilerPlanError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| invalid_manifest(path, format!("invalid framework manifest attribute: {error}")))?;
    if attribute.key.as_ref() == name {
      return attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
        .map(|value| Some(value.into_owned()))
        .map_err(|error| invalid_manifest(path, format!("invalid framework manifest attribute value: {error}")));
    }
  }
  Ok(None)
}

fn invalid_manifest(path: &Path, message: impl Into<String>) -> CompilerPlanError {
  CompilerPlanError::new(CompilerPlanErrorKind::InvalidManifest, path, message)
}

fn require_file(path: PathBuf, meaning: &str) -> Result<PathBuf, CompilerPlanError> {
  if path.is_file() {
    Ok(path)
  } else {
    Err(CompilerPlanError::new(
      CompilerPlanErrorKind::MissingAsset,
      &path,
      format!("{meaning} {} is missing", path.display()),
    ))
  }
}

fn materialize_plan(
  project: &ProjectSpec,
  sdk_version: &str,
  compiler: &Path,
  sdk_analyzers: &[PathBuf],
  analysis_config: &Path,
  framework: &FrameworkAssets,
  packages: Option<&PackageResolution>,
) -> Result<CompilerPlan, CompilerPlanError> {
  let project_directory = project.project_directory();
  let target_text = project.target_framework();
  let intermediate = project_directory.join("obj").join(project.configuration().as_str()).join(target_text);
  let assembly = project.assembly_name();
  let source_paths: Vec<PathBuf> = project.sources().map(|source| project_directory.join(source)).collect();
  let generated_paths = [
    intermediate.join(format!("{assembly}.GlobalUsings.g.cs")),
    intermediate.join(format!(".NETCoreApp,Version=v{}.AssemblyAttributes.cs", project.target().framework_version())),
    intermediate.join(format!("{assembly}.AssemblyInfo.cs")),
  ];
  let package_analyzer_count = packages.map_or(0, |packages| packages.analyzers().len());
  let mut analyzer_paths = Vec::with_capacity(sdk_analyzers.len() + framework.analyzers.len() + package_analyzer_count);
  analyzer_paths.extend_from_slice(sdk_analyzers);
  analyzer_paths.extend_from_slice(&framework.analyzers);
  if let Some(packages) = packages {
    analyzer_paths.extend(packages.analyzers().map(Path::to_owned));
  }
  let package_reference_count = packages.map_or(0, |packages| packages.compile_assets().len());
  let mut reference_paths = Vec::with_capacity(framework.references.len() + package_reference_count);
  reference_paths.extend_from_slice(&framework.references);
  if let Some(packages) = packages {
    reference_paths.extend(packages.compile_assets().map(Path::to_owned));
  }
  let mut config_paths = discover_editor_configs(project_directory);
  config_paths.push(intermediate.join(format!("{assembly}.GeneratedMSBuildEditorConfig.editorconfig")));
  config_paths.push(analysis_config.to_owned());
  let define_values = framework_defines(project.target(), project.configuration())?;
  let language_major = project
    .target()
    .csharp_language_major()
    .map_err(|error| CompilerPlanError::new(CompilerPlanErrorKind::UnsupportedSdk, project.project_path(), error.to_string()))?;
  let language_version = format!("{language_major}.0");

  let estimated_text = source_paths
    .iter()
    .chain(&generated_paths)
    .chain(&reference_paths)
    .chain(&analyzer_paths)
    .chain(&config_paths)
    .map(|p| p.as_os_str().len())
    .sum::<usize>()
    + define_values.iter().map(String::len).sum::<usize>()
    + language_version.len()
    + 1024;
  let mut table = TextTable::with_capacity(estimated_text);
  let project_path = table.push_path(project.project_path())?;
  let sdk_version_span = table.push(sdk_version)?;
  let compiler_span = table.push_path(compiler)?;
  let pack_version_span = table.push(&framework.version)?;
  let pack_span = table.push_path(&framework.root)?;
  let language_version_span = table.push(&language_version)?;
  let output_assembly = table.push_path(&intermediate.join(format!("{assembly}.dll")))?;
  let output_pdb = table.push_path(&intermediate.join(format!("{assembly}.pdb")))?;
  let reference_output = table.push_path(&intermediate.join("refint").join(format!("{assembly}.dll")))?;
  let sources = table.push_paths(&source_paths)?;
  let generated_sources = table.push_paths(&generated_paths)?;
  let references = table.push_paths(&reference_paths)?;
  let analyzers = table.push_paths(&analyzer_paths)?;
  let analyzer_configs = table.push_paths(&config_paths)?;
  let define_refs: Vec<&str> = define_values.iter().map(String::as_str).collect();
  let defines = table.push_values(&define_refs)?;

  Ok(CompilerPlan {
    text: table.text.into_boxed_str(),
    project: project_path,
    sdk_version: sdk_version_span,
    compiler: compiler_span,
    framework_pack_version: pack_version_span,
    framework_pack: pack_span,
    language_version: language_version_span,
    output_assembly,
    output_pdb,
    reference_output,
    sources,
    generated_sources,
    references,
    analyzers,
    analyzer_configs,
    defines,
    configuration: project.configuration(),
    output_type: project.output_type(),
    nullable: project.nullable_enabled(),
    deterministic: project.deterministic(),
    warning_level: project
      .target()
      .analysis_level()
      .map_err(|error| CompilerPlanError::new(CompilerPlanErrorKind::UnsupportedSdk, project.project_path(), error.to_string()))?,
  })
}

fn framework_defines(target: TargetFramework, configuration: ProjectConfiguration) -> Result<Vec<String>, CompilerPlanError> {
  if target.csharp_language_major().is_err() {
    return Err(CompilerPlanError::new(
      CompilerPlanErrorKind::UnsupportedSdk,
      PathBuf::new(),
      format!("compiler defines for {target:?} have not been captured"),
    ));
  }
  let mut values = Vec::with_capacity(20);
  values.push("TRACE".into());
  if configuration == ProjectConfiguration::Debug {
    values.push("DEBUG".into());
  }
  values.push("NET".into());
  values.push(format!("NET{}_{}", target.major(), target.minor()));
  values.push("NETCOREAPP".into());
  for major in 5..=target.major() {
    values.push(format!("NET{major}_0_OR_GREATER"));
  }
  for version in ["1_0", "1_1", "2_0", "2_1", "2_2", "3_0", "3_1"] {
    values.push(format!("NETCOREAPP{version}_OR_GREATER"));
  }
  Ok(values)
}

fn discover_editor_configs(project_directory: &Path) -> Vec<PathBuf> {
  let mut configs = Vec::new();
  let mut current = Some(project_directory);
  while let Some(directory) = current {
    let candidate = directory.join(".editorconfig");
    if candidate.is_file() {
      configs.push(candidate);
    }
    current = directory.parent();
  }
  configs.reverse();
  configs
}

struct TextTable {
  text: String,
}

impl TextTable {
  fn with_capacity(capacity: usize) -> Self {
    Self {
      text: String::with_capacity(capacity),
    }
  }

  fn push(&mut self, value: &str) -> Result<TextSpan, CompilerPlanError> {
    let start = u32::try_from(self.text.len())
      .map_err(|_| CompilerPlanError::new(CompilerPlanErrorKind::TextOverflow, PathBuf::new(), "compiler plan text exceeds 4 GiB"))?;
    let len = u32::try_from(value.len())
      .map_err(|_| CompilerPlanError::new(CompilerPlanErrorKind::TextOverflow, PathBuf::new(), "one compiler plan value exceeds 4 GiB"))?;
    self.text.push_str(value);
    Ok(TextSpan { start, len })
  }

  fn push_path(&mut self, path: &Path) -> Result<TextSpan, CompilerPlanError> {
    let value = path.to_str().ok_or_else(|| {
      CompilerPlanError::new(
        CompilerPlanErrorKind::NonUnicodePath,
        path,
        format!("compiler input path {} is not valid Unicode", path.display()),
      )
    })?;
    self.push(value)
  }

  fn push_paths(&mut self, paths: &[PathBuf]) -> Result<Box<[TextSpan]>, CompilerPlanError> {
    paths.iter().map(|path| self.push_path(path)).collect()
  }

  fn push_values(&mut self, values: &[&str]) -> Result<Box<[TextSpan]>, CompilerPlanError> {
    values.iter().map(|value| self.push(value)).collect()
  }
}

#[cfg(test)]
mod tests {
  use std::time::{SystemTime, UNIX_EPOCH};

  use crate::{SdkVersion, evaluate_project_path};

  use super::*;

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let path = std::env::temp_dir().join(format!("dv-compiler-test-{}-{unique}", std::process::id()));
      fs::create_dir_all(&path).unwrap();
      Self(path)
    }

    fn write(&self, relative: &str, contents: &str) {
      let path = self.0.join(relative);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(path, contents).unwrap();
    }
  }

  impl Drop for TempDirectory {
    fn drop(&mut self) {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }

  fn fixture() -> (TempDirectory, ProjectSpec, SdkInventory) {
    let temp = TempDirectory::new();
    temp.write(
      "app/App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings><Nullable>enable</Nullable></PropertyGroup></Project>"#,
    );
    temp.write("app/Program.cs", "Console.WriteLine(\"test\");");
    for relative in [
      "dotnet/sdk/10.0.100/Roslyn/bincore/csc.dll",
      "dotnet/sdk/10.0.100/Sdks/Microsoft.NET.Sdk/analyzers/Microsoft.CodeAnalysis.CSharp.NetAnalyzers.dll",
      "dotnet/sdk/10.0.100/Sdks/Microsoft.NET.Sdk/analyzers/Microsoft.CodeAnalysis.NetAnalyzers.dll",
      "dotnet/sdk/10.0.100/Sdks/Microsoft.NET.Sdk/analyzers/build/config/analysislevel_10_default.globalconfig",
      "dotnet/packs/Microsoft.NETCore.App.Ref/10.0.0/ref/net10.0/System.Console.dll",
      "dotnet/packs/Microsoft.NETCore.App.Ref/10.0.0/ref/net10.0/System.Runtime.dll",
      "dotnet/packs/Microsoft.NETCore.App.Ref/10.0.0/analyzers/dotnet/cs/Generator.dll",
    ] {
      temp.write(relative, "");
    }
    temp.write(
      "dotnet/packs/Microsoft.NETCore.App.Ref/10.0.0/data/FrameworkList.xml",
      r#"<FileList TargetFrameworkIdentifier=".NETCoreApp" TargetFrameworkVersion="10.0">
<File Type="Managed" Path="ref/net10.0/System.Console.dll" />
<File Type="Managed" Path="ref/net10.0/System.Runtime.dll" />
<File Type="Analyzer" Language="cs" Path="analyzers/dotnet/cs/Generator.dll" />
</FileList>"#,
    );
    let project = evaluate_project_path(&temp.0.join("app/App.csproj"), ProjectConfiguration::Debug).unwrap();
    let inventory = SdkInventory {
      roots: vec![temp.0.join("dotnet")],
      installations: vec![crate::SdkInstallation {
        version: SdkVersion::parse("10.0.100").unwrap(),
        root_index: 0,
      }],
      selected_index: 0,
      global_json: None,
    };
    (temp, project, inventory)
  }

  #[test]
  fn plans_manifest_references_and_sdk_tools_in_order() {
    let (_temp, project, inventory) = fixture();
    let plans = plan_compiler_inputs(&[&project], &inventory).unwrap();
    let plan = &plans[0];

    assert_eq!(plan.sdk_version(), "10.0.100");
    assert_eq!(plan.framework_pack_version(), "10.0.0");
    assert_eq!(plan.language_version(), "14.0");
    assert_eq!(plan.references().len(), 2);
    assert!(plan.references().next().unwrap().ends_with("System.Console.dll"));
    assert_eq!(plan.analyzers().len(), 3);
    assert_eq!(plan.generated_sources().len(), 3);
    assert!(plan.defines().any(|define| define == "NET10_0_OR_GREATER"));
  }

  #[test]
  fn selects_highest_stable_matching_pack_patch() {
    let (temp, project, inventory) = fixture();
    let source = temp.0.join("dotnet/packs/Microsoft.NETCore.App.Ref/10.0.0");
    let higher = temp.0.join("dotnet/packs/Microsoft.NETCore.App.Ref/10.0.2");
    copy_directory(&source, &higher);
    fs::create_dir_all(temp.0.join("dotnet/packs/Microsoft.NETCore.App.Ref/11.0.0")).unwrap();

    let plans = plan_compiler_inputs(&[&project], &inventory).unwrap();
    assert_eq!(plans[0].framework_pack_version(), "10.0.2");
  }

  #[test]
  fn rejects_manifest_assets_that_are_missing() {
    let (temp, project, inventory) = fixture();
    fs::remove_file(temp.0.join("dotnet/packs/Microsoft.NETCore.App.Ref/10.0.0/ref/net10.0/System.Runtime.dll")).unwrap();

    let error = plan_compiler_inputs(&[&project], &inventory).unwrap_err();
    assert_eq!(error.kind(), CompilerPlanErrorKind::MissingAsset);
  }

  #[test]
  fn rejects_a_manifest_for_another_target() {
    let (temp, project, inventory) = fixture();
    temp.write(
      "dotnet/packs/Microsoft.NETCore.App.Ref/10.0.0/data/FrameworkList.xml",
      r#"<FileList TargetFrameworkIdentifier=".NETCoreApp" TargetFrameworkVersion="9.0" />"#,
    );

    let error = plan_compiler_inputs(&[&project], &inventory).unwrap_err();
    assert_eq!(error.kind(), CompilerPlanErrorKind::InvalidManifest);
  }

  #[test]
  fn empty_batch_does_not_touch_sdk_files() {
    let inventory = SdkInventory {
      roots: Vec::new(),
      installations: Vec::new(),
      selected_index: 0,
      global_json: None,
    };
    assert!(plan_compiler_inputs(&[], &inventory).unwrap().is_empty());
  }

  fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
      let entry = entry.unwrap();
      let target = destination.join(entry.file_name());
      if entry.file_type().unwrap().is_dir() {
        copy_directory(&entry.path(), &target);
      } else {
        fs::copy(entry.path(), target).unwrap();
      }
    }
  }
}
