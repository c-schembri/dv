use std::{
  env, fs,
  path::PathBuf,
  process::Command,
  sync::atomic::{AtomicU64, Ordering},
  time::{SystemTime, UNIX_EPOCH},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
  fn new() -> Self {
    let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let path = env::temp_dir().join(format!("dv-cli-test-{}-{time}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    Self(path)
  }
}

impl Drop for TempDirectory {
  fn drop(&mut self) {
    fs::remove_dir_all(&self.0).unwrap();
  }
}

fn dv() -> Command {
  Command::new(env!("CARGO_BIN_EXE_dv"))
}

#[test]
fn help_exposes_the_initial_command_surface() {
  let output = dv().arg("--help").output().unwrap();

  assert!(output.status.success());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("dv <command>"));
  assert!(stdout.contains("sync"));
  assert!(stdout.contains("--json"));
}

#[test]
fn unknown_command_is_an_explicit_failure() {
  let output = dv().arg("frobnicate").output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  let stderr = String::from_utf8(output.stderr).unwrap();
  assert!(stderr.contains("error[DV0001]"));
  assert!(stderr.contains("unknown command"));
}

#[test]
fn json_failure_is_a_versioned_event_batch() {
  let output = dv().args(["build", "--json"]).output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(output.stderr.is_empty());
  let stdout = String::from_utf8(output.stdout).unwrap();
  let lines: Vec<&str> = stdout.lines().collect();
  assert_eq!(lines.len(), 3);
  assert!(lines[0].contains("\"schema_version\":1"));
  assert!(lines[1].contains("\"code\":\"DV0003\""));
  assert!(lines[2].contains("\"outcome\":\"failed\""));
}

#[test]
fn sdk_current_discovers_without_executing_dotnet() {
  let temp = TempDirectory::new();
  fs::create_dir_all(temp.0.join("sdk/9.0.308")).unwrap();
  fs::write(temp.0.join(format!("dotnet{}", env::consts::EXE_SUFFIX)), b"not an executable").unwrap();

  let output = dv().args(["sdk", "current"]).current_dir(&temp.0).env("PATH", &temp.0).output().unwrap();

  assert!(output.status.success());
  assert_eq!(String::from_utf8(output.stdout).unwrap(), "9.0.308\n");
  assert!(output.stderr.is_empty());
}

#[test]
fn sdk_current_json_reports_selected_path() {
  let temp = TempDirectory::new();
  fs::create_dir_all(temp.0.join("sdk/10.0.100")).unwrap();
  fs::write(temp.0.join(format!("dotnet{}", env::consts::EXE_SUFFIX)), b"not an executable").unwrap();

  let output = dv()
    .args(["sdk", "current", "--json"])
    .current_dir(&temp.0)
    .env("PATH", &temp.0)
    .output()
    .unwrap();

  assert!(output.status.success());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"sdk_selected\""));
  assert!(stdout.contains("\"version\":\"10.0.100\""));
  assert!(stdout.contains("\"outcome\":\"succeeded\""));
}
