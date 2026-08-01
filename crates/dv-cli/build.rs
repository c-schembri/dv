use std::{collections::BTreeMap, env, fmt::Write as _, fs, path::PathBuf};

use serde_json::Value;

fn main() {
  println!("cargo:rerun-if-changed=../../compatibility/manifest.json");
  let manifest_path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR")).join("../../compatibility/manifest.json");
  let bytes = fs::read(&manifest_path).expect("read compatibility manifest");
  let manifest: Value = serde_json::from_slice(&bytes).expect("parse compatibility manifest");
  assert_eq!(manifest["schema_version"].as_u64(), Some(1), "compatibility scanner requires manifest schema 1");
  let manifest_version = manifest["manifest_version"].as_u64().expect("manifest version is an integer");
  let parity_rows = manifest["parity_rows"].as_array().expect("manifest parity_rows is an array");
  let parity: BTreeMap<_, _> = parity_rows
    .iter()
    .map(|row| {
      (
        row["id"].as_str().expect("parity row id is text"),
        row["status"].as_str().expect("parity row status is text"),
      )
    })
    .collect();
  assert_eq!(parity.len(), parity_rows.len(), "compatibility parity row IDs must be unique");

  let commands = manifest["commands"].as_array().expect("manifest commands is an array");
  assert!(commands.len() <= usize::from(u16::MAX), "compatibility command count must fit u16 indices");
  let mut source = String::with_capacity(12 * 1024);
  writeln!(source, "const MANIFEST_VERSION: u16 = {manifest_version};").unwrap();
  let tools = ["dotnet", "msbuild", "nuget", "vstest"];
  let mut ranges = Vec::with_capacity(tools.len());
  let mut emitted = 0_usize;
  writeln!(source, "const MANIFEST_COMMANDS: &[ManifestCommand] = &[").unwrap();
  for tool in tools {
    let start = emitted;
    for command in commands.iter().filter(|command| command["tool"].as_str() == Some(tool)) {
      let path = command["path"].as_array().expect("command path is an array");
      let support = support_token(command["support"].as_str().expect("command support is text"));
      let rows = command["parity_rows"].as_array().expect("command parity_rows is an array");
      write!(source, "  ManifestCommand {{ path: &[").unwrap();
      for part in path {
        write!(source, "{:?},", part.as_str().expect("command path part is text")).unwrap();
      }
      write!(source, "], support: CompatibilitySupport::{support}, parity_rows: &[").unwrap();
      for row in rows {
        let id = row.as_str().expect("command parity row is text");
        let status = parity.get(id).unwrap_or_else(|| panic!("command references unknown parity row {id}"));
        if *status != "implemented" {
          write!(source, "{id:?},").unwrap();
        }
      }
      writeln!(source, "] }},").unwrap();
      emitted += 1;
    }
    ranges.push((start, emitted - start));
  }
  writeln!(source, "];").unwrap();
  assert_eq!(emitted, commands.len(), "manifest contains an unknown tool command");
  writeln!(source, "const MANIFEST_COMMAND_RANGES: [CommandRange; 4] = [").unwrap();
  for (start, len) in ranges {
    writeln!(source, "  CommandRange {{ start: {start}, len: {len} }},").unwrap();
  }
  writeln!(source, "];").unwrap();

  let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("compatibility_check_index.rs");
  fs::write(output, source).expect("write generated compatibility scanner index");
}

fn support_token(value: &str) -> &'static str {
  match value {
    "implemented" => "Implemented",
    "partial" => "Partial",
    "missing" => "Missing",
    other => panic!("unsupported compatibility state {other:?}"),
  }
}
