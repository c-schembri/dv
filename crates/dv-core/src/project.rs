use std::{
  error::Error,
  fmt, fs,
  path::{Component, Path, PathBuf},
};

use quick_xml::{
  Reader, XmlVersion,
  events::{BytesRef, BytesStart, Event},
};

use crate::TargetFramework;

const SUPPORTED_SDK: &str = "Microsoft.NET.Sdk";
const MAX_XML_DEPTH: usize = 8;

/// A supported build configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectConfiguration {
  /// Developer-oriented outputs.
  Debug,
  /// Optimized outputs.
  Release,
}

impl ProjectConfiguration {
  /// Parses a configuration name using MSBuild-compatible casing.
  pub fn parse(value: &str) -> Option<Self> {
    if value.eq_ignore_ascii_case("Debug") {
      Some(Self::Debug)
    } else if value.eq_ignore_ascii_case("Release") {
      Some(Self::Release)
    } else {
      None
    }
  }

  /// Returns the canonical configuration text.
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Debug => "Debug",
      Self::Release => "Release",
    }
  }
}

/// The managed artifact produced by a project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectOutputType {
  /// An executable assembly.
  Exe,
  /// A library assembly.
  Library,
}

impl ProjectOutputType {
  /// Returns the canonical MSBuild property text.
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Exe => "Exe",
      Self::Library => "Library",
    }
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextSpan {
  start: u32,
  len: u32,
}

const _: () = assert!(size_of::<TextSpan>() == 8);
const _: () = assert!(align_of::<TextSpan>() == 4);

/// One exact package dependency stored as two compact text-table spans.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageReference {
  id: TextSpan,
  version: TextSpan,
}

const _: () = assert!(size_of::<PackageReference>() == 16);
const _: () = assert!(align_of::<PackageReference>() == 4);

/// One evaluated SDK-style project.
///
/// Variable text is stored once in `text`; source and reference batches contain
/// compact spans into that immutable buffer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSpec {
  text: Box<str>,
  project_path: TextSpan,
  project_directory: TextSpan,
  target_framework_text: TextSpan,
  assembly_name: TextSpan,
  root_namespace: TextSpan,
  sources: Box<[TextSpan]>,
  project_references: Box<[TextSpan]>,
  package_references: Box<[PackageReference]>,
  configuration: ProjectConfiguration,
  output_type: ProjectOutputType,
  nullable: bool,
  implicit_usings: bool,
  deterministic: bool,
  target_framework: TargetFramework,
}

impl ProjectSpec {
  /// Returns the full project-file path.
  pub fn project_path(&self) -> &Path {
    Path::new(self.text(self.project_path))
  }

  /// Returns the full project directory.
  pub fn project_directory(&self) -> &Path {
    Path::new(self.text(self.project_directory))
  }

  /// Returns the selected SDK declaration.
  pub fn sdk(&self) -> &'static str {
    SUPPORTED_SDK
  }

  /// Returns the selected target framework.
  pub fn target_framework(&self) -> &str {
    self.text(self.target_framework_text)
  }

  /// Returns parsed target-framework data for downstream selection.
  pub fn target(&self) -> TargetFramework {
    self.target_framework
  }

  /// Returns the selected build configuration.
  pub fn configuration(&self) -> ProjectConfiguration {
    self.configuration
  }

  /// Returns the managed output type.
  pub fn output_type(&self) -> ProjectOutputType {
    self.output_type
  }

  /// Returns the output assembly name.
  pub fn assembly_name(&self) -> &str {
    self.text(self.assembly_name)
  }

  /// Returns the generated root namespace.
  pub fn root_namespace(&self) -> &str {
    self.text(self.root_namespace)
  }

  /// Returns whether nullable annotations and warnings are enabled.
  pub fn nullable_enabled(&self) -> bool {
    self.nullable
  }

  /// Returns whether SDK implicit global usings are enabled.
  pub fn implicit_usings_enabled(&self) -> bool {
    self.implicit_usings
  }

  /// Returns whether deterministic compiler output is required.
  pub fn deterministic(&self) -> bool {
    self.deterministic
  }

  /// Iterates source paths relative to the project directory.
  pub fn sources(&self) -> impl ExactSizeIterator<Item = &str> {
    self.sources.iter().map(|span| self.text(*span))
  }

  /// Iterates normalized project-reference paths relative to the project.
  pub fn project_references(&self) -> impl ExactSizeIterator<Item = &str> {
    self.project_references.iter().map(|span| self.text(*span))
  }

  /// Returns the compact package-reference batch.
  pub fn package_references(&self) -> &[PackageReference] {
    &self.package_references
  }

  /// Returns a package identifier.
  pub fn package_id(&self, package: PackageReference) -> &str {
    self.text(package.id)
  }

  /// Returns a literal package version or range.
  pub fn package_version(&self, package: PackageReference) -> &str {
    self.text(package.version)
  }

  fn text(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }
}

/// Stable category for project discovery and evaluation failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectErrorKind {
  /// No project exists at the requested location.
  NotFound,
  /// Implicit project selection found more than one candidate.
  Ambiguous,
  /// A filesystem operation failed.
  Io,
  /// Project XML is malformed.
  InvalidXml,
  /// The project uses behavior outside the initial compatibility contract.
  Unsupported,
  /// A supported property has an invalid value.
  InvalidProperty,
  /// A required path cannot be represented by the initial UTF-8 path table.
  NonUnicodePath,
}

/// A project failure with stable category and path context.
#[derive(Debug)]
pub struct ProjectError {
  kind: ProjectErrorKind,
  path: PathBuf,
  message: String,
}

impl ProjectError {
  /// Returns the stable error category.
  pub fn kind(&self) -> ProjectErrorKind {
    self.kind
  }

  /// Returns the project or directory associated with the failure.
  pub fn path(&self) -> &Path {
    &self.path
  }

  fn new(kind: ProjectErrorKind, path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
    Self {
      kind,
      path: path.into(),
      message: message.into(),
    }
  }
}

impl fmt::Display for ProjectError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl Error for ProjectError {}

#[derive(Clone, Copy)]
enum Property {
  TargetFramework,
  TargetFrameworks,
  OutputType,
  Nullable,
  ImplicitUsings,
  AssemblyName,
  RootNamespace,
  Deterministic,
}

#[derive(Clone, Copy)]
enum Element {
  Document,
  Project,
  PropertyGroup,
  ItemGroup,
  Property(Property),
  ProjectReference,
  PackageReference(usize),
  PackageVersion(usize),
}

#[derive(Default)]
struct RawProject {
  target_framework: Option<String>,
  output_type: Option<String>,
  nullable: Option<String>,
  implicit_usings: Option<String>,
  assembly_name: Option<String>,
  root_namespace: Option<String>,
  deterministic: Option<String>,
  project_references: Vec<String>,
  package_references: Vec<RawPackageReference>,
}

struct RawPackageReference {
  id: String,
  version: Option<String>,
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

  fn push(&mut self, value: &str, path: &Path) -> Result<TextSpan, ProjectError> {
    let start = u32::try_from(self.text.len()).map_err(|_| ProjectError::new(ProjectErrorKind::Unsupported, path, "project text table exceeds 4 GiB"))?;
    let len = u32::try_from(value.len()).map_err(|_| ProjectError::new(ProjectErrorKind::Unsupported, path, "one project value exceeds 4 GiB"))?;
    self.text.push_str(value);
    Ok(TextSpan { start, len })
  }
}

/// Finds exactly one `.csproj` in a directory and evaluates it.
pub fn evaluate_project(start_directory: &Path, configuration: ProjectConfiguration) -> Result<ProjectSpec, ProjectError> {
  let project_path = discover_project(start_directory)?;
  evaluate_project_path(&project_path, configuration)
}

/// Evaluates one explicit SDK-style `.csproj`.
pub fn evaluate_project_path(project_path: &Path, configuration: ProjectConfiguration) -> Result<ProjectSpec, ProjectError> {
  if !is_csproj(project_path) {
    return Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      project_path,
      "the initial evaluator accepts only C# .csproj files",
    ));
  }
  if !project_path.is_file() {
    return Err(ProjectError::new(
      ProjectErrorKind::NotFound,
      project_path,
      format!("project {} does not exist", project_path.display()),
    ));
  }

  let project_path = absolute_path(project_path)?;
  let project_directory = project_path
    .parent()
    .ok_or_else(|| ProjectError::new(ProjectErrorKind::NotFound, &project_path, "project path has no parent directory"))?
    .to_owned();
  let bytes = fs::read(&project_path).map_err(|error| io_error("read", &project_path, error))?;
  let raw = parse_project(&project_path, &bytes)?;
  materialize_project(project_path, &project_directory, configuration, raw)
}

fn discover_project(directory: &Path) -> Result<PathBuf, ProjectError> {
  let mut projects = Vec::new();
  let entries = fs::read_dir(directory).map_err(|error| io_error("enumerate", directory, error))?;
  for entry in entries {
    let entry = entry.map_err(|error| io_error("enumerate", directory, error))?;
    let path = entry.path();
    if entry.file_type().map_err(|error| io_error("inspect", &path, error))?.is_file() && is_csproj(&path) {
      projects.push(path);
    }
  }
  projects.sort_unstable();

  match projects.len() {
    0 => Err(ProjectError::new(
      ProjectErrorKind::NotFound,
      directory,
      format!("no C# project was found in {}", directory.display()),
    )),
    1 => Ok(projects.pop().expect("one project exists")),
    count => Err(ProjectError::new(
      ProjectErrorKind::Ambiguous,
      directory,
      format!("{count} C# projects were found in {}; pass one project path explicitly", directory.display()),
    )),
  }
}

fn parse_project(path: &Path, bytes: &[u8]) -> Result<RawProject, ProjectError> {
  let mut reader = Reader::from_reader(bytes);
  reader.config_mut().trim_text(true);
  reader.config_mut().expand_empty_elements = true;

  let mut raw = RawProject::default();
  let mut stack = [Element::Document; MAX_XML_DEPTH];
  let mut depth = 1;
  let mut text = String::new();
  let mut root_seen = false;

  loop {
    match reader.read_event() {
      Ok(Event::Start(element)) => {
        let parent = stack[depth - 1];
        let next = start_element(path, &reader, &element, parent, &mut raw, &mut root_seen)?;
        if depth == MAX_XML_DEPTH {
          return Err(ProjectError::new(ProjectErrorKind::Unsupported, path, "project XML nesting is too deep"));
        }
        if matches!(next, Element::Property(_) | Element::PackageVersion(_)) {
          text.clear();
        }
        stack[depth] = next;
        depth += 1;
      },
      Ok(Event::End(_)) => {
        if depth == 1 {
          return Err(ProjectError::new(
            ProjectErrorKind::InvalidXml,
            path,
            "project XML contains an unexpected closing element",
          ));
        }
        depth -= 1;
        finish_element(path, stack[depth], &mut raw, &text)?;
      },
      Ok(Event::Text(value)) => {
        let current = stack[depth - 1];
        if matches!(current, Element::Property(_) | Element::PackageVersion(_)) {
          text.push_str(
            &value
              .xml10_content()
              .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("project text is invalid: {error}")))?,
          );
        } else {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            "project contains text outside a supported property",
          ));
        }
      },
      Ok(Event::GeneralRef(value)) => {
        let current = stack[depth - 1];
        if !matches!(current, Element::Property(_) | Element::PackageVersion(_)) {
          return Err(ProjectError::new(
            ProjectErrorKind::Unsupported,
            path,
            "project contains an entity outside a supported property",
          ));
        }
        append_reference(path, &value, &mut text)?;
      },
      Ok(Event::Comment(_) | Event::Decl(_)) => {},
      Ok(Event::CData(_) | Event::PI(_) | Event::DocType(_)) => {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          path,
          "CDATA, processing instructions, and document types are not supported",
        ));
      },
      Ok(Event::Empty(_)) => unreachable!("empty elements are expanded by the reader"),
      Ok(Event::Eof) => break,
      Err(error) => {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidXml,
          path,
          format!("invalid project XML at byte {}: {error}", reader.error_position()),
        ));
      },
    }
  }

  if depth != 1 || !root_seen {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidXml,
      path,
      "project XML does not contain one complete Project element",
    ));
  }
  Ok(raw)
}

fn start_element(
  path: &Path,
  reader: &Reader<&[u8]>,
  element: &BytesStart<'_>,
  parent: Element,
  raw: &mut RawProject,
  root_seen: &mut bool,
) -> Result<Element, ProjectError> {
  let qualified_name = element.name();
  let name = element_name(path, qualified_name.as_ref())?;
  match parent {
    Element::Document if name == "Project" => {
      if *root_seen {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidXml,
          path,
          "project XML contains multiple root elements",
        ));
      }
      *root_seen = true;
      let sdk = required_attribute(path, reader, element, "Sdk", &[])?;
      if sdk != SUPPORTED_SDK {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          path,
          format!("project SDK {sdk:?} is unsupported; use {SUPPORTED_SDK}"),
        ));
      }
      Ok(Element::Project)
    },
    Element::Project if name == "PropertyGroup" => {
      validate_attributes(path, reader, element, &["Label"])?;
      Ok(Element::PropertyGroup)
    },
    Element::Project if name == "ItemGroup" => {
      validate_attributes(path, reader, element, &["Label"])?;
      Ok(Element::ItemGroup)
    },
    Element::Project if matches!(name, "Import" | "Target" | "UsingTask" | "Sdk") => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("{name} is outside the initial project compatibility contract"),
    )),
    Element::PropertyGroup => {
      validate_attributes(path, reader, element, &[])?;
      Ok(Element::Property(property(path, name)?))
    },
    Element::ItemGroup if name == "ProjectReference" => {
      let include = required_attribute(path, reader, element, "Include", &[])?;
      raw.project_references.push(normalize_project_reference(path, &include)?);
      Ok(Element::ProjectReference)
    },
    Element::ItemGroup if name == "PackageReference" => {
      let include = required_attribute(path, reader, element, "Include", &["Version"])?;
      let version = optional_attribute(path, reader, element, "Version")?;
      let index = raw.package_references.len();
      raw.package_references.push(RawPackageReference { id: include, version });
      Ok(Element::PackageReference(index))
    },
    Element::PackageReference(index) if name == "Version" => {
      validate_attributes(path, reader, element, &[])?;
      Ok(Element::PackageVersion(index))
    },
    Element::ProjectReference | Element::PackageReference(_) => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("metadata element {name} is not supported here"),
    )),
    Element::Property(_) | Element::PackageVersion(_) => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("nested element {name} is not supported in a property value"),
    )),
    _ => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("element {name} is outside the initial project compatibility contract"),
    )),
  }
}

fn finish_element(path: &Path, element: Element, raw: &mut RawProject, text: &str) -> Result<(), ProjectError> {
  match element {
    Element::Property(property) => match property {
      Property::TargetFramework => raw.target_framework = Some(text.to_owned()),
      Property::TargetFrameworks => {
        return Err(ProjectError::new(
          ProjectErrorKind::Unsupported,
          path,
          "multi-target projects are unsupported; use TargetFramework",
        ));
      },
      Property::OutputType => raw.output_type = Some(text.to_owned()),
      Property::Nullable => raw.nullable = Some(text.to_owned()),
      Property::ImplicitUsings => raw.implicit_usings = Some(text.to_owned()),
      Property::AssemblyName => raw.assembly_name = Some(text.to_owned()),
      Property::RootNamespace => raw.root_namespace = Some(text.to_owned()),
      Property::Deterministic => raw.deterministic = Some(text.to_owned()),
    },
    Element::PackageVersion(index) => {
      let package = &mut raw.package_references[index];
      if package.version.is_some() {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          path,
          format!("package {:?} declares Version more than once", package.id),
        ));
      }
      package.version = Some(text.to_owned());
    },
    _ => {},
  }
  Ok(())
}

fn materialize_project(
  project_path: PathBuf,
  project_directory: &Path,
  configuration: ProjectConfiguration,
  raw: RawProject,
) -> Result<ProjectSpec, ProjectError> {
  let target_framework = required_property(&project_path, "TargetFramework", raw.target_framework)?;
  let parsed_target =
    TargetFramework::parse(&target_framework).map_err(|error| ProjectError::new(ProjectErrorKind::InvalidProperty, &project_path, error.to_string()))?;
  if !parsed_target.is_modern_net() {
    return Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      &project_path,
      format!("target framework {target_framework:?} is recognized but its SDK/pack family is not implemented yet"),
    ));
  }

  let output_type = match raw.output_type.as_deref().unwrap_or("Library") {
    "Exe" => ProjectOutputType::Exe,
    "Library" => ProjectOutputType::Library,
    value => {
      return Err(ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("OutputType {value:?} is unsupported; use Exe or Library"),
      ));
    },
  };
  let nullable = parse_toggle(&project_path, "Nullable", raw.nullable.as_deref(), false, true)?;
  let implicit_usings = parse_toggle(&project_path, "ImplicitUsings", raw.implicit_usings.as_deref(), false, false)?;
  let deterministic = parse_bool(&project_path, "Deterministic", raw.deterministic.as_deref(), true)?;
  let default_name = project_path
    .file_stem()
    .and_then(|value| value.to_str())
    .ok_or_else(|| ProjectError::new(ProjectErrorKind::NonUnicodePath, &project_path, "project file name is not valid Unicode"))?;
  let assembly_name = raw.assembly_name.as_deref().unwrap_or(default_name);
  if assembly_name.is_empty() || assembly_name.contains("$(") {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      &project_path,
      "AssemblyName must be a non-empty literal",
    ));
  }
  let root_namespace = raw.root_namespace.as_deref().unwrap_or(assembly_name);
  if root_namespace.is_empty() || root_namespace.contains("$(") {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      &project_path,
      "RootNamespace must be a non-empty literal",
    ));
  }

  let sources = collect_sources(project_directory, &project_path)?;
  let project_path_text = unicode_path(&project_path, &project_path)?;
  let project_directory_text = unicode_path(project_directory, &project_path)?;
  let estimated_text = project_path_text.len()
    + project_directory_text.len()
    + target_framework.len()
    + assembly_name.len()
    + root_namespace.len()
    + sources.iter().map(String::len).sum::<usize>()
    + raw.project_references.iter().map(String::len).sum::<usize>()
    + raw
      .package_references
      .iter()
      .map(|package| package.id.len() + package.version.as_ref().map_or(0, String::len))
      .sum::<usize>();
  let mut table = TextTable::with_capacity(estimated_text);
  let project_path_span = table.push(project_path_text, &project_path)?;
  let project_directory_span = table.push(project_directory_text, &project_path)?;
  let target_framework_span = table.push(&target_framework, &project_path)?;
  let assembly_name_span = table.push(assembly_name, &project_path)?;
  let root_namespace_span = table.push(root_namespace, &project_path)?;
  let source_spans = sources.iter().map(|source| table.push(source, &project_path)).collect::<Result<Box<_>, _>>()?;
  let reference_spans = raw
    .project_references
    .iter()
    .map(|reference| table.push(reference, &project_path))
    .collect::<Result<Box<_>, _>>()?;
  let mut package_references = Vec::with_capacity(raw.package_references.len());
  for package in raw.package_references {
    let version = package.version.ok_or_else(|| {
      ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("package {:?} requires a Version", package.id),
      )
    })?;
    if !is_literal_package_version(&version) {
      return Err(ProjectError::new(
        ProjectErrorKind::InvalidProperty,
        &project_path,
        format!("package {:?} version {version:?} is not a literal version or range", package.id),
      ));
    }
    package_references.push(PackageReference {
      id: table.push(&package.id, &project_path)?,
      version: table.push(&version, &project_path)?,
    });
  }

  Ok(ProjectSpec {
    text: table.text.into_boxed_str(),
    project_path: project_path_span,
    project_directory: project_directory_span,
    target_framework_text: target_framework_span,
    assembly_name: assembly_name_span,
    root_namespace: root_namespace_span,
    sources: source_spans,
    project_references: reference_spans,
    package_references: package_references.into_boxed_slice(),
    configuration,
    output_type,
    nullable,
    implicit_usings,
    deterministic,
    target_framework: parsed_target,
  })
}

fn collect_sources(project_directory: &Path, project_path: &Path) -> Result<Vec<String>, ProjectError> {
  let mut directories = vec![project_directory.to_owned()];
  let mut sources = Vec::new();
  while let Some(directory) = directories.pop() {
    let entries = fs::read_dir(&directory).map_err(|error| io_error("enumerate", &directory, error))?;
    for entry in entries {
      let entry = entry.map_err(|error| io_error("enumerate", &directory, error))?;
      let path = entry.path();
      let file_type = entry.file_type().map_err(|error| io_error("inspect", &path, error))?;
      if file_type.is_dir() {
        if !is_output_directory(&entry.file_name()) {
          directories.push(path);
        }
      } else if file_type.is_file() && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("cs")) {
        let relative = path.strip_prefix(project_directory).expect("entries discovered below the project directory");
        sources.push(portable_path(relative, project_path)?);
      }
    }
  }
  sources.sort_unstable();
  Ok(sources)
}

fn is_output_directory(name: &std::ffi::OsStr) -> bool {
  name.eq_ignore_ascii_case("bin") || name.eq_ignore_ascii_case("obj")
}

fn parse_toggle(path: &Path, property: &str, value: Option<&str>, default: bool, only_enable: bool) -> Result<bool, ProjectError> {
  let Some(value) = value else {
    return Ok(default);
  };
  if value.eq_ignore_ascii_case("enable") {
    Ok(true)
  } else if !only_enable && value.eq_ignore_ascii_case("disable") {
    Ok(false)
  } else {
    Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("{property} value {value:?} is unsupported"),
    ))
  }
}

fn parse_bool(path: &Path, property: &str, value: Option<&str>, default: bool) -> Result<bool, ProjectError> {
  let Some(value) = value else {
    return Ok(default);
  };
  if value.eq_ignore_ascii_case("true") {
    Ok(true)
  } else if value.eq_ignore_ascii_case("false") {
    Ok(false)
  } else {
    Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("{property} value {value:?} must be true or false"),
    ))
  }
}

fn required_property(path: &Path, name: &str, value: Option<String>) -> Result<String, ProjectError> {
  value
    .filter(|value| !value.is_empty() && !value.contains("$("))
    .ok_or_else(|| ProjectError::new(ProjectErrorKind::InvalidProperty, path, format!("{name} must be a non-empty literal")))
}

fn property(path: &Path, name: &str) -> Result<Property, ProjectError> {
  match name {
    "TargetFramework" => Ok(Property::TargetFramework),
    "TargetFrameworks" => Ok(Property::TargetFrameworks),
    "OutputType" => Ok(Property::OutputType),
    "Nullable" => Ok(Property::Nullable),
    "ImplicitUsings" => Ok(Property::ImplicitUsings),
    "AssemblyName" => Ok(Property::AssemblyName),
    "RootNamespace" => Ok(Property::RootNamespace),
    "Deterministic" => Ok(Property::Deterministic),
    _ => Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("property {name} is outside the initial project compatibility contract"),
    )),
  }
}

fn validate_attributes(path: &Path, reader: &Reader<&[u8]>, element: &BytesStart<'_>, allowed: &[&str]) -> Result<(), ProjectError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid XML attribute: {error}")))?;
    let name = element_name(path, attribute.key.as_ref())?;
    if name == "Condition" {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        "MSBuild Condition evaluation is not supported yet",
      ));
    }
    if !allowed.contains(&name) {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        format!("attribute {name} is outside the initial project compatibility contract"),
      ));
    }
    attribute
      .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
      .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid {name} attribute: {error}")))?;
  }
  Ok(())
}

fn required_attribute(path: &Path, reader: &Reader<&[u8]>, element: &BytesStart<'_>, required: &str, additional: &[&str]) -> Result<String, ProjectError> {
  let mut value = None;
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid XML attribute: {error}")))?;
    let name = element_name(path, attribute.key.as_ref())?;
    if name == "Condition" {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        "MSBuild Condition evaluation is not supported yet",
      ));
    }
    if name != required && !additional.contains(&name) {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        format!("attribute {name} is outside the initial project compatibility contract"),
      ));
    }
    let decoded = attribute
      .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
      .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid {name} attribute: {error}")))?;
    if name == required && value.replace(decoded.into_owned()).is_some() {
      return Err(ProjectError::new(
        ProjectErrorKind::InvalidXml,
        path,
        format!("attribute {required} appears more than once"),
      ));
    }
  }
  value.ok_or_else(|| ProjectError::new(ProjectErrorKind::InvalidProperty, path, format!("element requires a {required} attribute")))
}

fn optional_attribute(path: &Path, reader: &Reader<&[u8]>, element: &BytesStart<'_>, requested: &str) -> Result<Option<String>, ProjectError> {
  for attribute in element.attributes() {
    let attribute = attribute.map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid XML attribute: {error}")))?;
    let name = element_name(path, attribute.key.as_ref())?;
    if name == requested {
      return attribute
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
        .map(|value| Some(value.into_owned()))
        .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid {name} attribute: {error}")));
    }
  }
  Ok(None)
}

fn append_reference(path: &Path, reference: &BytesRef<'_>, output: &mut String) -> Result<(), ProjectError> {
  if let Some(character) = reference
    .resolve_char_ref()
    .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid character reference: {error}")))?
  {
    output.push(character);
    return Ok(());
  }
  let name = reference
    .decode()
    .map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("invalid entity reference: {error}")))?;
  let value = match name.as_ref() {
    "amp" => '&',
    "lt" => '<',
    "gt" => '>',
    "apos" => '\'',
    "quot" => '"',
    _ => {
      return Err(ProjectError::new(
        ProjectErrorKind::Unsupported,
        path,
        format!("custom XML entity &{name}; is unsupported"),
      ));
    },
  };
  output.push(value);
  Ok(())
}

fn normalize_project_reference(path: &Path, value: &str) -> Result<String, ProjectError> {
  if value.is_empty() || value.contains("$(") || value.contains('*') || value.contains('?') {
    return Err(ProjectError::new(
      ProjectErrorKind::InvalidProperty,
      path,
      format!("ProjectReference Include {value:?} must be one literal path"),
    ));
  }
  let normalized = value.replace('\\', "/");
  if !normalized.to_ascii_lowercase().ends_with(".csproj") {
    return Err(ProjectError::new(
      ProjectErrorKind::Unsupported,
      path,
      format!("project reference {value:?} is not a C# project"),
    ));
  }
  Ok(normalized)
}

fn is_literal_package_version(value: &str) -> bool {
  !value.is_empty() && value.len() <= 256 && !value.contains("$(")
}

fn is_csproj(path: &Path) -> bool {
  path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("csproj"))
}

fn absolute_path(path: &Path) -> Result<PathBuf, ProjectError> {
  let absolute = if path.is_absolute() {
    path.to_owned()
  } else {
    std::path::absolute(path).map_err(|error| io_error("resolve", path, error))?
  };
  Ok(absolute.components().collect())
}

fn unicode_path<'a>(path: &'a Path, project_path: &Path) -> Result<&'a str, ProjectError> {
  path.to_str().ok_or_else(|| {
    ProjectError::new(
      ProjectErrorKind::NonUnicodePath,
      project_path,
      format!("path {} is not valid Unicode", path.display()),
    )
  })
}

fn portable_path(path: &Path, project_path: &Path) -> Result<String, ProjectError> {
  let mut output = String::new();
  for component in path.components() {
    if !output.is_empty() {
      output.push('/');
    }
    match component {
      Component::Normal(value) => output.push_str(value.to_str().ok_or_else(|| {
        ProjectError::new(
          ProjectErrorKind::NonUnicodePath,
          project_path,
          format!("source path {} is not valid Unicode", path.display()),
        )
      })?),
      _ => {
        return Err(ProjectError::new(
          ProjectErrorKind::InvalidProperty,
          project_path,
          format!("source path {} is not relative to its project", path.display()),
        ));
      },
    }
  }
  Ok(output)
}

fn element_name<'a>(path: &Path, name: &'a [u8]) -> Result<&'a str, ProjectError> {
  std::str::from_utf8(name).map_err(|error| ProjectError::new(ProjectErrorKind::InvalidXml, path, format!("element name is not UTF-8: {error}")))
}

fn io_error(operation: &str, path: &Path, error: std::io::Error) -> ProjectError {
  ProjectError::new(ProjectErrorKind::Io, path, format!("failed to {operation} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
  use std::{
    env,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
  };

  use super::*;

  static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
      let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let path = env::temp_dir().join(format!("dv-project-test-{}-{time}-{nonce}", std::process::id()));
      fs::create_dir_all(&path).unwrap();
      Self(path)
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
      let path = self.0.join(relative);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(&path, contents).unwrap();
      path
    }
  }

  impl Drop for TempDirectory {
    fn drop(&mut self) {
      fs::remove_dir_all(&self.0).unwrap();
    }
  }

  fn project_xml(properties: &str, items: &str) -> String {
    format!(
      r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net10.0</TargetFramework>
    {properties}
  </PropertyGroup>
  {items}
</Project>"#
    )
  }

  #[test]
  fn discovers_and_evaluates_one_project_into_compact_batches() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "class Program {}");
    temp.write("Features/Other.cs", "class Other {}");
    temp.write("obj/Generated.g.cs", "class Generated {}");
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "<OutputType>Exe</OutputType><Nullable>enable</Nullable><ImplicitUsings>enable</ImplicitUsings>",
        r#"<ItemGroup><PackageReference Include="Example.Package" Version="1.2.3" /></ItemGroup>"#,
      ),
    );

    let result = evaluate_project(&temp.0, ProjectConfiguration::Release).unwrap();

    assert_eq!(result.project_path(), project);
    assert_eq!(result.configuration(), ProjectConfiguration::Release);
    assert_eq!(result.output_type(), ProjectOutputType::Exe);
    assert_eq!(result.assembly_name(), "App");
    assert_eq!(result.root_namespace(), "App");
    assert!(result.nullable_enabled());
    assert!(result.implicit_usings_enabled());
    assert!(result.deterministic());
    assert_eq!(result.sources().collect::<Vec<_>>(), ["Features/Other.cs", "Program.cs"]);
    let package = result.package_references()[0];
    assert_eq!(result.package_id(package), "Example.Package");
    assert_eq!(result.package_version(package), "1.2.3");
  }

  #[test]
  fn captures_project_references_and_nested_package_versions() {
    let temp = TempDirectory::new();
    temp.write("Program.cs", "");
    temp.write("Lib/Lib.csproj", &project_xml("", ""));
    let project = temp.write(
      "App.csproj",
      &project_xml(
        "",
        r#"<ItemGroup>
          <ProjectReference Include="Lib\Lib.csproj" />
          <PackageReference Include="Example.Package"><Version>2.0.0-preview.1</Version></PackageReference>
        </ItemGroup>"#,
      ),
    );

    let result = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap();

    assert_eq!(result.project_references().collect::<Vec<_>>(), ["Lib/Lib.csproj"]);
    let package = result.package_references()[0];
    assert_eq!(result.package_id(package), "Example.Package");
    assert_eq!(result.package_version(package), "2.0.0-preview.1");
  }

  #[test]
  fn implicit_selection_rejects_ambiguity() {
    let temp = TempDirectory::new();
    temp.write("A.csproj", &project_xml("", ""));
    temp.write("B.csproj", &project_xml("", ""));

    let error = evaluate_project(&temp.0, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::Ambiguous);
  }

  #[test]
  fn rejects_conditions_instead_of_guessing() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup Condition="'$(Configuration)' == 'Debug'"><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
    );

    let error = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::Unsupported);
    assert!(error.to_string().contains("Condition"));
  }

  #[test]
  fn rejects_multi_targeting() {
    let temp = TempDirectory::new();
    let project = temp.write(
      "App.csproj",
      r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFrameworks>net10.0;net9.0</TargetFrameworks></PropertyGroup></Project>"#,
    );

    let error = evaluate_project_path(&project, ProjectConfiguration::Debug).unwrap_err();

    assert_eq!(error.kind(), ProjectErrorKind::Unsupported);
    assert!(error.to_string().contains("multi-target"));
  }
}
