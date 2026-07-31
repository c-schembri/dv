use std::{
  collections::BTreeMap,
  error::Error,
  fmt, fs, io,
  path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::SdkInventory;

const PORTABLE_GRAPH_FILE: &str = "PortableRuntimeIdentifierGraph.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextSpan {
  start: u32,
  len: u32,
}

const _: () = assert!(size_of::<TextSpan>() == 8);
const _: () = assert!(align_of::<TextSpan>() == 4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IndexRange {
  start: u32,
  len: u32,
}

const _: () = assert!(size_of::<IndexRange>() == 8);
const _: () = assert!(align_of::<IndexRange>() == 4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RuntimeNode {
  identifier: TextSpan,
  imports: IndexRange,
}

const _: () = assert!(size_of::<RuntimeNode>() == 16);
const _: () = assert!(align_of::<RuntimeNode>() == 4);

/// The selected SDK's portable runtime-identifier compatibility graph.
///
/// Runtime names live once in one immutable text table. Sorted 16-byte nodes
/// point into contiguous direct-edge ranges, while a separate range table
/// indexes precomputed breadth-first compatibility batches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeIdentifierGraph {
  source: Box<Path>,
  text: Box<str>,
  nodes: Box<[RuntimeNode]>,
  imports: Box<[u32]>,
  compatibility_ranges: Box<[IndexRange]>,
  compatibilities: Box<[u32]>,
}

impl RuntimeIdentifierGraph {
  /// Loads and compiles one portable runtime graph.
  pub fn load(path: &Path) -> Result<Self, RuntimeGraphError> {
    let bytes = fs::read(path).map_err(|error| io_error("read", path, error))?;
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let mut document: GraphDocument = serde_json::from_slice(bytes).map_err(|error| {
      RuntimeGraphError::new(
        RuntimeGraphErrorKind::InvalidJson,
        path,
        format!("invalid portable RID graph {}: {error}", path.display()),
      )
    })?;

    for identifier in document.runtimes.keys() {
      validate_identifier(path, identifier)?;
    }
    let mut missing = Vec::new();
    for runtime in document.runtimes.values() {
      for import in &runtime.imports {
        validate_identifier(path, import)?;
        if !document.runtimes.contains_key(import) && !missing.contains(import) {
          missing.push(import.clone());
        }
      }
    }
    for identifier in missing {
      document.runtimes.insert(identifier, RawRuntime::default());
    }

    let node_count = document.runtimes.len();
    let _ = u32_len(node_count, path, "runtime node batch")?;
    let text_capacity = document.runtimes.keys().map(String::len).sum();
    let mut text = TextTable::with_capacity(text_capacity);
    let mut nodes = Vec::with_capacity(node_count);
    for identifier in document.runtimes.keys() {
      nodes.push(RuntimeNode {
        identifier: text.push(identifier, path)?,
        imports: IndexRange { start: 0, len: 0 },
      });
    }

    let edge_capacity = document.runtimes.values().map(|runtime| runtime.imports.len()).sum();
    let mut imports = Vec::with_capacity(edge_capacity);
    for (index, runtime) in document.runtimes.values().enumerate() {
      let start = u32_len(imports.len(), path, "runtime import batch")?;
      for import in &runtime.imports {
        let imported = nodes
          .binary_search_by(|node| text.get(node.identifier).cmp(import))
          .expect("missing imported runtimes were materialized as leaf nodes");
        imports.push(u32_len(imported, path, "runtime node index")?);
      }
      let end = u32_len(imports.len(), path, "runtime import batch")?;
      nodes[index].imports = IndexRange { start, len: end - start };
    }

    let compatibility = compile_compatibility_ranges(path, &nodes, &imports)?;
    Ok(Self {
      source: path.into(),
      text: text.text.into_boxed_str(),
      nodes: nodes.into_boxed_slice(),
      imports: imports.into_boxed_slice(),
      compatibility_ranges: compatibility.ranges,
      compatibilities: compatibility.indices,
    })
  }

  /// Returns the graph file selected from the SDK installation.
  pub fn source(&self) -> &Path {
    &self.source
  }

  /// Returns the number of explicit and imported-leaf runtime nodes.
  pub fn node_count(&self) -> usize {
    self.nodes.len()
  }

  /// Returns the number of direct compatibility edges.
  pub fn edge_count(&self) -> usize {
    self.imports.len()
  }

  /// Returns the number of precomputed compatibility indices.
  pub fn compatibility_count(&self) -> usize {
    self.compatibilities.len()
  }

  /// Iterates direct imports for a known runtime identifier.
  pub fn direct_imports(&self, runtime_identifier: &str) -> Option<impl ExactSizeIterator<Item = &str>> {
    let index = self.find_node(runtime_identifier)?;
    let range = range(self.nodes[index].imports);
    Some(self.imports[range].iter().map(|index| self.node_identifier(*index)))
  }

  /// Iterates compatible RIDs in NuGet's breadth-first nearest-first order.
  ///
  /// Unknown RIDs are compatible only with themselves. No RID is inferred by
  /// splitting or otherwise interpreting its text.
  pub fn compatible_rids<'a>(&'a self, runtime_identifier: &'a str) -> impl ExactSizeIterator<Item = &'a str> + 'a {
    let tail = self.find_node(runtime_identifier).map_or(&[][..], |index| {
      let compatibility = &self.compatibilities[range(self.compatibility_ranges[index])];
      &compatibility[1..]
    });
    CompatibleRids {
      graph: self,
      requested: Some(runtime_identifier),
      tail: tail.iter(),
    }
  }

  /// Tests compatibility using graph data and ordinal RID comparison.
  pub fn are_compatible(&self, requested: &str, provided: &str) -> bool {
    if requested == provided {
      return true;
    }
    let Some(requested_index) = self.find_node(requested) else {
      return false;
    };
    let Some(provided_index) = self.find_node(provided) else {
      return false;
    };
    self.compatibilities[range(self.compatibility_ranges[requested_index])].contains(&(provided_index as u32))
  }

  fn find_node(&self, runtime_identifier: &str) -> Option<usize> {
    self.nodes.binary_search_by(|node| self.get(node.identifier).cmp(runtime_identifier)).ok()
  }

  fn node_identifier(&self, index: u32) -> &str {
    self.get(self.nodes[index as usize].identifier)
  }

  fn get(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }
}

struct CompatibleRids<'a> {
  graph: &'a RuntimeIdentifierGraph,
  requested: Option<&'a str>,
  tail: std::slice::Iter<'a, u32>,
}

impl<'a> Iterator for CompatibleRids<'a> {
  type Item = &'a str;

  fn next(&mut self) -> Option<Self::Item> {
    self
      .requested
      .take()
      .or_else(|| self.tail.next().map(|index| self.graph.node_identifier(*index)))
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    let len = self.len();
    (len, Some(len))
  }
}

impl ExactSizeIterator for CompatibleRids<'_> {
  fn len(&self) -> usize {
    usize::from(self.requested.is_some()) + self.tail.len()
  }
}

/// Loads the portable RID graph owned by the selected SDK installation.
pub fn load_portable_runtime_graph(inventory: &SdkInventory) -> Result<RuntimeIdentifierGraph, RuntimeGraphError> {
  let path = inventory.installation_path(inventory.selected()).join(PORTABLE_GRAPH_FILE);
  if !path.is_file() {
    return Err(RuntimeGraphError::new(
      RuntimeGraphErrorKind::NotFound,
      &path,
      format!("selected SDK does not contain {}", path.display()),
    ));
  }
  RuntimeIdentifierGraph::load(&path)
}

struct CompiledCompatibility {
  ranges: Box<[IndexRange]>,
  indices: Box<[u32]>,
}

fn compile_compatibility_ranges(path: &Path, nodes: &[RuntimeNode], imports: &[u32]) -> Result<CompiledCompatibility, RuntimeGraphError> {
  let mut ranges = Vec::with_capacity(nodes.len());
  let mut compatibility = Vec::new();
  let mut queue = Vec::with_capacity(nodes.len());
  let mut visited = vec![0u32; nodes.len()];

  for start_node in 0..nodes.len() {
    let generation = u32_len(start_node, path, "runtime traversal generation")? + 1;
    let start = u32_len(compatibility.len(), path, "runtime compatibility batch")?;
    queue.clear();
    queue.push(start_node as u32);
    visited[start_node] = generation;

    let mut cursor = 0;
    while cursor < queue.len() {
      let node_index = queue[cursor];
      compatibility.push(node_index);
      for imported in &imports[range(nodes[node_index as usize].imports)] {
        let imported_index = *imported as usize;
        if visited[imported_index] != generation {
          visited[imported_index] = generation;
          queue.push(*imported);
        }
      }
      cursor += 1;
    }
    let end = u32_len(compatibility.len(), path, "runtime compatibility batch")?;
    ranges.push(IndexRange { start, len: end - start });
  }

  Ok(CompiledCompatibility {
    ranges: ranges.into_boxed_slice(),
    indices: compatibility.into_boxed_slice(),
  })
}

fn validate_identifier(path: &Path, identifier: &str) -> Result<(), RuntimeGraphError> {
  if identifier.is_empty() {
    return Err(RuntimeGraphError::new(
      RuntimeGraphErrorKind::InvalidGraph,
      path,
      "portable RID graph contains an empty runtime identifier".into(),
    ));
  }
  Ok(())
}

fn u32_len(value: usize, path: &Path, batch: &str) -> Result<u32, RuntimeGraphError> {
  u32::try_from(value).map_err(|_| {
    RuntimeGraphError::new(
      RuntimeGraphErrorKind::TextOverflow,
      path,
      format!("{batch} exceeds the compact 32-bit index space"),
    )
  })
}

fn range(value: IndexRange) -> std::ops::Range<usize> {
  value.start as usize..(value.start + value.len) as usize
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

  fn push(&mut self, value: &str, path: &Path) -> Result<TextSpan, RuntimeGraphError> {
    let start = u32_len(self.text.len(), path, "runtime text table")?;
    let len = u32_len(value.len(), path, "runtime identifier")?;
    start.checked_add(len).ok_or_else(|| {
      RuntimeGraphError::new(
        RuntimeGraphErrorKind::TextOverflow,
        path,
        "runtime text table exceeds the compact 32-bit index space".into(),
      )
    })?;
    self.text.push_str(value);
    Ok(TextSpan { start, len })
  }

  fn get(&self, span: TextSpan) -> &str {
    let start = span.start as usize;
    &self.text[start..start + span.len as usize]
  }
}

#[derive(Deserialize)]
struct GraphDocument {
  runtimes: BTreeMap<String, RawRuntime>,
}

#[derive(Default, Deserialize)]
struct RawRuntime {
  #[serde(default, rename = "#import")]
  imports: Vec<String>,
}

/// Stable portable runtime-graph failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeGraphErrorKind {
  /// The selected SDK does not contain a portable graph.
  NotFound,
  /// Reading the selected graph failed.
  Io,
  /// The graph is not valid JSON in the supported schema.
  InvalidJson,
  /// The graph contains invalid runtime data.
  InvalidGraph,
  /// Graph data exceeds compact index limits.
  TextOverflow,
}

/// A portable runtime-graph failure with source-path context.
#[derive(Debug)]
pub struct RuntimeGraphError {
  kind: RuntimeGraphErrorKind,
  path: PathBuf,
  message: String,
}

impl RuntimeGraphError {
  fn new(kind: RuntimeGraphErrorKind, path: impl Into<PathBuf>, message: String) -> Self {
    Self {
      kind,
      path: path.into(),
      message,
    }
  }

  /// Returns the stable failure category.
  pub fn kind(&self) -> RuntimeGraphErrorKind {
    self.kind
  }

  /// Returns the graph path associated with the failure.
  pub fn path(&self) -> &Path {
    &self.path
  }
}

impl fmt::Display for RuntimeGraphError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.message.fmt(formatter)
  }
}

impl Error for RuntimeGraphError {}

fn io_error(operation: &str, path: &Path, error: io::Error) -> RuntimeGraphError {
  RuntimeGraphError::new(RuntimeGraphErrorKind::Io, path, format!("failed to {operation} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
  use std::{
    env,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
  };

  use crate::{SdkInstallation, SdkVersion};

  use super::*;

  static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
      let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let path = env::temp_dir().join(format!("dv-runtime-graph-test-{}-{time}-{nonce}", std::process::id()));
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

  const GRAPH: &str = r##"{
    "runtimes": {
      "base": { "#import": [] },
      "any": { "#import": ["base"] },
      "unix": { "#import": ["any"] },
      "unix-x64": { "#import": ["unix"] },
      "linux": { "#import": ["unix"] },
      "linux-x64": { "#import": ["linux", "unix-x64"] },
      "linux-musl": { "#import": ["linux"] },
      "linux-musl-x64": { "#import": ["linux-musl", "linux-x64"] }
    }
  }"##;

  #[test]
  fn compiles_breadth_first_compatibility_without_splitting_rids() {
    let temp = TempDirectory::new();
    let path = temp.write(PORTABLE_GRAPH_FILE, GRAPH);

    let graph = RuntimeIdentifierGraph::load(&path).unwrap();

    assert_eq!(graph.node_count(), 8);
    assert_eq!(graph.edge_count(), 9);
    assert_eq!(graph.direct_imports("linux-musl-x64").unwrap().collect::<Vec<_>>(), ["linux-musl", "linux-x64"]);
    assert_eq!(
      graph.compatible_rids("linux-musl-x64").collect::<Vec<_>>(),
      ["linux-musl-x64", "linux-musl", "linux-x64", "linux", "unix-x64", "unix", "any", "base"]
    );
    assert!(graph.are_compatible("linux-musl-x64", "unix-x64"));
    assert!(!graph.are_compatible("linux-musl-x64", "win-x64"));
    assert_eq!(graph.compatible_rids("linux-super-x64").collect::<Vec<_>>(), ["linux-super-x64"]);
  }

  #[test]
  fn imported_unknown_runtime_becomes_an_opaque_leaf() {
    let temp = TempDirectory::new();
    let path = temp.write(PORTABLE_GRAPH_FILE, r##"{"runtimes":{"custom":{"#import":["opaque-fallback"]}}}"##);

    let graph = RuntimeIdentifierGraph::load(&path).unwrap();

    assert_eq!(graph.node_count(), 2);
    assert_eq!(graph.compatible_rids("custom").collect::<Vec<_>>(), ["custom", "opaque-fallback"]);
  }

  #[test]
  fn cyclic_graph_is_bounded_by_generation_marks() {
    let temp = TempDirectory::new();
    let path = temp.write(PORTABLE_GRAPH_FILE, r##"{"runtimes":{"a":{"#import":["b"]},"b":{"#import":["a"]}}}"##);

    let graph = RuntimeIdentifierGraph::load(&path).unwrap();

    assert_eq!(graph.compatible_rids("a").collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(graph.compatible_rids("b").collect::<Vec<_>>(), ["b", "a"]);
  }

  #[test]
  fn selected_sdk_owns_the_portable_graph_path() {
    let temp = TempDirectory::new();
    let graph_path = temp.write("sdk/10.0.100/PortableRuntimeIdentifierGraph.json", GRAPH);
    let inventory = SdkInventory {
      roots: vec![temp.0.clone()],
      installations: vec![SdkInstallation {
        version: SdkVersion::parse("10.0.100").unwrap(),
        root_index: 0,
      }],
      selected_index: 0,
      global_json: None,
    };

    let graph = load_portable_runtime_graph(&inventory).unwrap();

    assert_eq!(graph.source(), graph_path);
  }

  #[test]
  fn malformed_graph_has_a_stable_failure_category() {
    let temp = TempDirectory::new();
    let path = temp.write(PORTABLE_GRAPH_FILE, "not json");

    let error = RuntimeIdentifierGraph::load(&path).unwrap_err();

    assert_eq!(error.kind(), RuntimeGraphErrorKind::InvalidJson);
    assert_eq!(error.path(), path);
  }
}
