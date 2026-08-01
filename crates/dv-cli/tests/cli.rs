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

fn dv() -> Command {
  Command::new(env!("CARGO_BIN_EXE_dv"))
}

#[cfg(unix)]
#[test]
fn sigint_cancels_in_flight_package_io_with_a_cancelled_event() {
  use std::{
    io::Read,
    net::TcpListener,
    process::Stdio,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
  };

  let temp = TempDirectory::new();
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let address = listener.local_addr().unwrap();
  let (ready_tx, ready_rx) = mpsc::sync_channel(1);
  let server = thread::spawn(move || {
    let (mut stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut bytes = [0u8; 1024];
    let _ = stream.read(&mut bytes).unwrap();
    ready_tx.send(()).unwrap();
    let _ = stream.read(&mut bytes);
  });
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
  <ItemGroup><PackageReference Include="Cancellation.Package" Version="1.0.0" /></ItemGroup>
</Project>"#,
  );
  temp.write(
    "NuGet.Config",
    &format!(
      r#"<configuration><packageSources><clear /><add key="stall" value="http://{address}/v3/index.json" protocolVersion="3" allowInsecureConnections="true" /></packageSources></configuration>"#,
    ),
  );
  let mut child = dv()
    .args(["--json", "restore", "App.csproj", "--packages", "packages"])
    .current_dir(&temp.0)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
  if ready_rx.recv_timeout(Duration::from_secs(10)).is_err() {
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    panic!("dv did not reach cancellable package I/O: {}", String::from_utf8_lossy(&output.stdout));
  }

  let started = Instant::now();
  let signal = Command::new("kill").args(["-INT", &child.id().to_string()]).output().unwrap();
  assert!(signal.status.success(), "{}", String::from_utf8_lossy(&signal.stderr));
  let output = child.wait_with_output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(started.elapsed() < Duration::from_secs(5));
  assert!(output.stderr.is_empty());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"code\":\"DV0005\""), "{stdout}");
  assert!(stdout.contains("\"outcome\":\"cancelled\""), "{stdout}");
  assert!(!temp.0.join("dv.lock.json").exists());
  server.join().unwrap();
}

#[test]
fn help_exposes_the_initial_command_surface() {
  let output = dv().arg("--help").output().unwrap();

  assert!(output.status.success());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("dv <command>"));
  assert!(stdout.contains("restore"));
  assert!(stdout.contains("sync"));
  assert!(stdout.contains("project"));
  assert!(stdout.contains("--json"));
  assert!(stdout.contains("--verbosity LEVEL"));
  assert!(stdout.contains("--color | --no-color"));
  assert!(stdout.contains("--compat dotnet|msbuild|nuget|vstest"));
  assert!(stdout.contains("compat"));
}

#[test]
fn compatibility_help_forms_expose_profile_and_canonical_syntax_before_io() {
  let temp = TempDirectory::new();
  temp.write("global.json", "{ malformed");
  temp.write("Broken.csproj", "<Project><Broken>");

  let cases: &[(&str, &[&str], &str, &str)] = &[
    ("dotnet", &["--compat", "dotnet", "-?"], "dv --compat dotnet <command>", "dv <command>"),
    (
      "msbuild",
      &["--compat", "msbuild", "--Help"],
      "dv --compat msbuild [MSBUILD-ARGUMENTS]",
      "dv build --plan",
    ),
    ("nuget", &["--compat", "nuget", "-h"], "dv --compat nuget <command>", "dv restore"),
    ("vstest", &["--compat", "vstest", "--Help"], "dv --compat vstest [TEST-CONTAINER]", "dv test"),
  ];
  for (profile, arguments, compatibility, canonical) in cases {
    let output = dv().args(*arguments).current_dir(&temp.0).output().unwrap();
    assert!(output.status.success(), "profile {profile}: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(), "profile {profile}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
      stdout.contains(&format!("Selected compatibility profile: {profile}")),
      "profile {profile}: {stdout}"
    );
    assert!(stdout.contains(compatibility), "profile {profile}: {stdout}");
    assert!(stdout.contains(canonical), "profile {profile}: {stdout}");
  }

  for arguments in [&["--compat", "nuget", "-?"][..], &["--compat", "vstest", "-h"]] {
    let output = dv().args(arguments).current_dir(&temp.0).output().unwrap();
    assert_eq!(output.status.code(), Some(1), "arguments={arguments:?}");
    assert!(output.stdout.is_empty(), "arguments={arguments:?}");
    assert!(!String::from_utf8(output.stderr).unwrap().contains("Selected compatibility profile"));
  }

  #[cfg(windows)]
  for (profile, help) in [("dotnet", "/?"), ("msbuild", "/Help"), ("vstest", "/?")] {
    let output = dv().args(["--compat", profile, help]).current_dir(&temp.0).output().unwrap();
    assert!(output.status.success(), "profile {profile}: {}", String::from_utf8_lossy(&output.stderr));
  }

  #[cfg(windows)]
  for (profile, help) in [("nuget", "/?"), ("vstest", "/h")] {
    let output = dv().args(["--compat", profile, help]).current_dir(&temp.0).output().unwrap();
    assert_eq!(output.status.code(), Some(1), "profile {profile}, help {help}");
    assert!(output.stdout.is_empty(), "profile {profile}, help {help}");
  }

  assert!(!temp.0.join("obj").exists());
  assert!(!temp.0.join("dv.lock.json").exists());
}

#[test]
fn phase_one_command_help_forms_skip_work_and_show_both_spellings() {
  let temp = TempDirectory::new();
  temp.write("global.json", "{ malformed");
  temp.write("Broken.csproj", "<Project><Broken>");

  for arguments in [
    &["--compat", "dotnet", "build", "-?"][..],
    &["--compat", "dotnet", "restore", "--help"],
    &["--compat", "dotnet", "run", "-h"],
    &["--compat", "dotnet", "test", "-?"],
    &["--compat", "nuget", "restore", "-h"],
    &["--compat", "dotnet", "help", "build"],
  ] {
    let output = dv().args(arguments).current_dir(&temp.0).output().unwrap();
    assert!(output.status.success(), "arguments={arguments:?}: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(), "arguments={arguments:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("dv --compat"), "arguments={arguments:?}: {stdout}");
    assert!(stdout.contains("dv "), "arguments={arguments:?}: {stdout}");
  }

  assert!(!temp.0.join("obj").exists());
  assert!(!temp.0.join("dv.lock.json").exists());
}

#[test]
fn compatibility_manifest_query_writes_the_checked_in_artifact_without_discovery() {
  let expected = include_bytes!("../../../compatibility/manifest.json");
  let plain = dv().args(["compat", "manifest"]).output().unwrap();
  let json_global = dv().args(["--json", "compat", "manifest"]).output().unwrap();
  let json_after = dv().args(["compat", "manifest", "--json"]).output().unwrap();

  for output in [plain, json_global, json_after] {
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout, expected);
  }

  let manifest: serde_json::Value = serde_json::from_slice(expected).unwrap();
  assert_eq!(manifest["schema_version"], 1);
  assert_eq!(manifest["manifest_version"], 1);
  assert_eq!(manifest["command_syntax_version"], 4);
  assert_eq!(manifest["event_schema_version"], 22);
  assert!(!manifest["reference"]["dotnet_sdk"].as_str().unwrap().is_empty());
  assert!(manifest["commands"].as_array().unwrap().len() >= 100);
  assert_eq!(manifest["parity_rows"].as_array().unwrap().len(), 468);
}

#[test]
fn malformed_compatibility_manifest_queries_fail_explicitly() {
  for arguments in [&["compat", "unknown"][..], &["compat", "manifest", "unexpected"]] {
    let output = dv().args(arguments).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr).unwrap().contains("unknown compat query"));
  }
}

#[test]
fn compatibility_check_scans_literal_scripts_and_projects_without_execution() {
  let temp = TempDirectory::new();
  let script = temp.write(
    "ci.yml",
    r#"steps:
  - run: dotnet --version
  - run: dotnet restore App.csproj
  - run: dotnet clean App.csproj
  - run: dotnet --version > should-not-exist.txt
"#,
  );
  let project = temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
  );

  let output = dv()
    .arg("--json")
    .args(["compat", "check"])
    .arg(&script)
    .arg(&project)
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(output.stderr.is_empty());
  assert!(!temp.0.join("should-not-exist.txt").exists());
  assert!(!temp.0.join("obj").exists());
  let events: Vec<serde_json::Value> = String::from_utf8(output.stdout)
    .unwrap()
    .lines()
    .map(|line| serde_json::from_str(line).unwrap())
    .collect();
  assert!(events.iter().all(|event| event["schema_version"] == 22));
  let report = events.iter().find(|event| event["type"] == "compatibility_checked").unwrap();
  assert_eq!(report["manifest_version"], 1);
  assert_eq!(report["inputs"].as_array().unwrap().len(), 2);
  assert_eq!(report["invocations"].as_array().unwrap().len(), 4);
  assert_eq!(report["invocations"][0]["support"], "implemented");
  assert_eq!(report["invocations"][1]["support"], "partial");
  let unresolved = report["invocations"][1]["parity_rows"].as_array().unwrap();
  assert!(!unresolved.is_empty());
  assert!(unresolved.iter().all(|row| row != "DROP-022"));
  assert_eq!(report["invocations"][2]["support"], "missing");
  assert_eq!(report["invocations"][3]["support"], "uncheckable");
  assert_eq!(events.last().unwrap()["outcome"], "failed");
}

#[test]
fn compatibility_check_succeeds_for_fully_supported_literal_inputs() {
  let temp = TempDirectory::new();
  let script = temp.write("version.ps1", "dotnet --version\n");
  let project = temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
  );

  let output = dv().args(["compat", "check"]).arg(&script).arg(&project).current_dir(&temp.0).output().unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  assert!(output.stderr.is_empty());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("Manifest  1"));
  assert!(stdout.contains("SUPPORTED"));
  assert!(stdout.contains("2 inputs | 1 invocations | 0 unresolved"));
}

#[test]
fn global_output_options_normalize_before_and_after_the_command() {
  let before = dv().args(["--quiet", "--json", "sdk", "current"]).output().unwrap();
  let after = dv().args(["sdk", "--verbosity", "quiet", "current", "--json"]).output().unwrap();

  assert!(before.status.success());
  assert!(after.status.success());
  assert!(before.stderr.is_empty());
  assert!(after.stderr.is_empty());
  assert!(String::from_utf8(before.stdout).unwrap().contains("\"type\":\"sdk_selected\""));
  assert!(String::from_utf8(after.stdout).unwrap().contains("\"type\":\"sdk_selected\""));
}

#[test]
fn malformed_globals_fail_before_project_discovery() {
  let output = dv().args(["restore", "DefinitelyMissing.csproj", "--verbosity", "loud"]).output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  let stderr = String::from_utf8(output.stderr).unwrap();
  assert!(stderr.contains("unsupported diagnostic verbosity"));
  assert!(!stderr.contains("DefinitelyMissing.csproj"));
}

#[test]
fn unknown_options_fail_at_the_active_command_boundary_before_io() {
  let temp = TempDirectory::new();
  temp.write("global.json", "{ definitely not JSON");
  temp.write("Broken.csproj", "<Project><Broken>");

  for arguments in [
    vec!["--definitely-unknown"],
    vec!["--version", "--definitely-unknown"],
    vec!["--help", "--definitely-unknown"],
    vec!["sdk", "--help", "--definitely-unknown"],
    vec!["sdk", "current", "--definitely-unknown"],
    vec!["sdk", "current", "unexpected", "--definitely-unknown"],
    vec!["sdk", "list", "--definitely-unknown"],
    vec!["sdk", "compatible-rids", "--definitely-unknown"],
    vec!["sdk", "compatible-rids", "linux-x64", "--definitely-unknown"],
    vec!["project", "--definitely-unknown"],
    vec!["project", "--help", "--definitely-unknown"],
    vec!["project", "inspect", "--definitely-unknown"],
    vec!["project", "frameworks", "--definitely-unknown"],
    vec!["project", "runtime-packs", "--definitely-unknown"],
    vec!["project", "package-sources", "--definitely-unknown"],
    vec!["build", "--definitely-unknown"],
    vec!["build", "--plan", "--definitely-unknown"],
    vec!["restore", "--definitely-unknown"],
    vec!["sync", "--definitely-unknown"],
  ] {
    let output = dv().args(&arguments).current_dir(&temp.0).output().unwrap();
    assert_eq!(output.status.code(), Some(2), "arguments={arguments:?}");
    assert!(output.stdout.is_empty(), "arguments={arguments:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[DV0002]"), "arguments={arguments:?}: {stderr}");
    assert!(stderr.contains("unknown"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV01"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV02"), "arguments={arguments:?}: {stderr}");
  }

  assert!(!temp.0.join("obj").exists());
  assert!(!temp.0.join(".packages").exists());
}

#[test]
fn compatibility_unknown_options_use_reference_failure_exit_without_io() {
  let temp = TempDirectory::new();
  temp.write("global.json", "{ definitely not JSON");
  temp.write("Broken.csproj", "<Project><Broken>");

  for (arguments, code, message) in [
    (
      vec!["--compat", "dotnet", "build", "--definitely-unknown"],
      "error[DV0002]",
      "unknown build option \"--definitely-unknown\"",
    ),
    (
      vec!["build", "--compat=msbuild", "--definitely-unknown"],
      "error[DV0002]",
      "unknown build option \"--definitely-unknown\"",
    ),
    (
      vec!["--compat=nuget", "build", "--definitely-unknown"],
      "error[DV0001]",
      "unknown command \"build\"",
    ),
    (
      vec!["build", "--definitely-unknown", "--compat", "vstest"],
      "error[DV0002]",
      "unknown build option \"--definitely-unknown\"",
    ),
  ] {
    let output = dv().args(&arguments).current_dir(&temp.0).output().unwrap();

    assert_eq!(output.status.code(), Some(1), "arguments={arguments:?}");
    assert!(output.stdout.is_empty(), "arguments={arguments:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(code), "arguments={arguments:?}: {stderr}");
    assert!(stderr.contains(message), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV01"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV02"), "arguments={arguments:?}: {stderr}");
  }
  assert!(!temp.0.join("obj").exists());
}

#[test]
fn compatibility_lexical_rules_reject_before_project_io() {
  let temp = TempDirectory::new();
  temp.write("global.json", "{ definitely not JSON");
  temp.write("Broken.csproj", "<Project><Broken>");

  for arguments in [
    vec!["--compat", "dotnet", "build", "--configuration=Debug", "-c:Release", "Broken.csproj"],
    vec!["--compat", "dotnet", "build", "--Configuration=Release", "Broken.csproj"],
  ] {
    let output = dv().args(&arguments).current_dir(&temp.0).output().unwrap();

    assert_eq!(output.status.code(), Some(1), "arguments={arguments:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[DV0002]"), "arguments={arguments:?}: {stderr}");
    assert!(stderr.contains("compatibility_profile: dotnet"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV01"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV02"), "arguments={arguments:?}: {stderr}");
  }

  #[cfg(windows)]
  {
    let output = dv()
      .args(["--compat", "dotnet", "build", "/definitely-invalid", "Broken.csproj"])
      .current_dir(&temp.0)
      .output()
      .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unknown build option \"/definitely-invalid\""), "{stderr}");
    assert!(!stderr.contains("error[DV01"), "{stderr}");
    assert!(!stderr.contains("error[DV02"), "{stderr}");
  }

  assert!(!temp.0.join("obj").exists());
}

#[test]
fn child_delimiter_keeps_trailing_global_spellings_opaque() {
  for command in ["run", "test"] {
    let exit_policy = if command == "run" { "preserve" } else { "map_to_command_failure" };
    let human = dv().args([command, "--", "--json", "--verbosity", "loud", ""]).output().unwrap();
    assert_eq!(human.status.code(), Some(2));
    assert!(human.stdout.is_empty());
    let stderr = String::from_utf8(human.stderr).unwrap();
    assert!(stderr.contains("error[DV0003]"));
    assert!(stderr.contains("forwarded_argument_count: 4"));
    assert!(stderr.contains(&format!("child_exit_policy: {exit_policy}")));
    assert!(stderr.contains("cancellation_grace_ms: 2000"));

    let json = dv().args(["--json", command, "--", "--no-color", "--verbosity", "loud", ""]).output().unwrap();
    assert_eq!(json.status.code(), Some(2));
    assert!(json.stderr.is_empty());
    let stdout = String::from_utf8(json.stdout).unwrap();
    assert!(stdout.contains("\"code\":\"DV0003\""));
    assert!(stdout.contains("\"name\":\"forwarded_argument_count\",\"value\":\"4\""));
    assert!(stdout.contains(&format!("\"name\":\"child_exit_policy\",\"value\":\"{exit_policy}\"")));
    assert!(stdout.contains("\"name\":\"cancellation_grace_ms\",\"value\":\"2000\""));
    assert!(stdout.contains("\"args\":[\"--json\""));
    assert!(stdout.contains("\"--no-color\",\"--verbosity\",\"loud\",\"\"]"));
  }
}

#[test]
fn child_options_affect_the_typed_boundary_or_fail_before_io() {
  let temp = TempDirectory::new();
  temp.write("App.csproj", "<Project><Broken>");

  let typed = dv()
    .args([
      "--json",
      "--compat",
      "dotnet",
      "test",
      "--project",
      "App.csproj",
      "-c:Release",
      "--environment",
      "PUBLIC=value",
    ])
    .current_dir(&temp.0)
    .output()
    .unwrap();
  assert_eq!(typed.status.code(), Some(1));
  assert!(typed.stderr.is_empty());
  let stdout = String::from_utf8(typed.stdout).unwrap();
  assert!(stdout.contains("\"code\":\"DV0003\""));
  assert!(stdout.contains("\"name\":\"project\",\"value\":\"App.csproj\""));
  assert!(stdout.contains("\"name\":\"configuration\",\"value\":\"Release\""));
  assert!(stdout.contains("\"name\":\"environment_edit_count\",\"value\":\"1\""));

  let rejected = dv()
    .args(["--compat", "dotnet", "test", "--definitely-unknown"])
    .current_dir(&temp.0)
    .output()
    .unwrap();
  assert_eq!(rejected.status.code(), Some(1));
  assert!(rejected.stdout.is_empty());
  let stderr = String::from_utf8(rejected.stderr).unwrap();
  assert!(stderr.contains("error[DV0002]"), "{stderr}");
  assert!(stderr.contains("unknown test option \"--definitely-unknown\""), "{stderr}");
  assert!(!stderr.contains("error[DV0003]"), "{stderr}");
  assert!(!temp.0.join("obj").exists());
}

#[test]
fn delimiter_on_non_child_commands_fails_before_discovery() {
  let temp = TempDirectory::new();
  temp.write("global.json", "{ definitely not JSON");
  temp.write("Broken.csproj", "<Project><Broken>");

  for arguments in [
    vec!["sdk", "current", "--", "opaque"],
    vec!["project", "inspect", "Broken.csproj", "--", "opaque"],
    vec!["build", "--plan", "Broken.csproj", "--", "opaque"],
    vec!["restore", "Broken.csproj", "--", "--offline"],
    vec!["sync", "Broken.csproj", "--"],
  ] {
    let output = dv().args(&arguments).current_dir(&temp.0).output().unwrap();
    assert_eq!(output.status.code(), Some(2), "arguments={arguments:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[DV0002]"), "arguments={arguments:?}: {stderr}");
    assert!(stderr.contains("unknown"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV01"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV02"), "arguments={arguments:?}: {stderr}");
  }

  assert!(!temp.0.join("obj").exists());
}

#[test]
fn explicit_color_policy_affects_only_human_diagnostics() {
  let colored = dv().args(["frobnicate", "--color"]).output().unwrap();
  let plain = dv().args(["frobnicate", "--no-color"]).output().unwrap();
  let invalid_json = dv().args(["frobnicate", "--json", "--color"]).output().unwrap();

  assert!(String::from_utf8(colored.stderr).unwrap().contains("\u{1b}[31merror[DV0001]"));
  assert!(!String::from_utf8(plain.stderr).unwrap().contains('\u{1b}'));
  assert!(invalid_json.stderr.is_empty());
  assert!(
    String::from_utf8(invalid_json.stdout)
      .unwrap()
      .contains("explicit color options cannot be combined with --json")
  );
}

#[test]
fn invocation_environment_has_explicit_precedence_and_secret_free_errors() {
  let environment_selected = dv()
    .arg("frobnicate")
    .env("NO_COLOR", "no-color-environment-secret")
    .env("DV_COLOR", "always")
    .output()
    .unwrap();
  assert_eq!(environment_selected.status.code(), Some(2));
  let stderr = String::from_utf8(environment_selected.stderr).unwrap();
  assert!(stderr.contains("\u{1b}[31m"));
  assert!(!stderr.contains("environment-secret"));

  let command_line_selected = dv()
    .args(["--no-color", "--quiet", "frobnicate"])
    .env("NO_COLOR", "no-color-environment-secret")
    .env("DV_COLOR", "always")
    .env("DV_VERBOSITY", "diagnostic")
    .output()
    .unwrap();
  assert_eq!(command_line_selected.status.code(), Some(2));
  let stderr = String::from_utf8(command_line_selected.stderr).unwrap();
  assert!(!stderr.contains('\u{1b}'));
  assert!(!stderr.contains("environment-secret"));

  let invalid = dv()
    .args(["--json", "frobnicate"])
    .env("DV_COLOR", "color-environment-secret")
    .output()
    .unwrap();
  assert_eq!(invalid.status.code(), Some(2));
  assert!(invalid.stderr.is_empty());
  let stdout = String::from_utf8(invalid.stdout).unwrap();
  assert!(stdout.contains("DV_COLOR must be auto, always, or never"));
  assert!(!stdout.contains("environment-secret"));

  let ignored_invalid = dv()
    .args(["--color", "--quiet", "frobnicate"])
    .env("DV_COLOR", "color-environment-secret")
    .env("DV_VERBOSITY", "verbosity-environment-secret")
    .output()
    .unwrap();
  assert_eq!(ignored_invalid.status.code(), Some(2));
  let stderr = String::from_utf8(ignored_invalid.stderr).unwrap();
  assert!(stderr.contains("unknown command"));
  assert!(!stderr.contains("DV_COLOR must"));
  assert!(!stderr.contains("environment-secret"));
}

#[test]
fn sensitive_cli_inputs_are_redacted_from_human_and_json_output() {
  let json = dv()
    .args([
      "--json",
      "frobnicate",
      "--api-key",
      "separate-cli-secret",
      "--client-secret=joined-cli-secret",
      "-p:Password=property-cli-secret",
      "https://user:password@example.test/v3/index.json?sig=query-cli-secret#fragment",
    ])
    .output()
    .unwrap();
  assert_eq!(json.status.code(), Some(2));
  assert!(json.stderr.is_empty());
  let stdout = String::from_utf8(json.stdout).unwrap();
  for secret in [
    "separate-cli-secret",
    "joined-cli-secret",
    "property-cli-secret",
    "password",
    "query-cli-secret",
  ] {
    assert!(!stdout.contains(secret), "JSON output exposed {secret:?}: {stdout}");
  }
  assert!(stdout.contains("<redacted>"));

  let human = dv().args(["sdk", "current", "--api-key=human-cli-secret"]).output().unwrap();
  assert_eq!(human.status.code(), Some(2));
  let stderr = String::from_utf8(human.stderr).unwrap();
  assert!(stderr.contains("--api-key=<redacted>"));
  assert!(!stderr.contains("human-cli-secret"));

  let human_url = dv()
    .args(["project", "https://user:password@example.test/path?sig=human-url-secret"])
    .output()
    .unwrap();
  assert_eq!(human_url.status.code(), Some(2));
  let stderr = String::from_utf8(human_url.stderr).unwrap();
  assert!(stderr.contains("https://example.test/path"));
  assert!(!stderr.contains("password"));
  assert!(!stderr.contains("human-url-secret"));
}

#[test]
fn child_environment_precedence_is_typed_and_reporter_views_are_redacted() {
  let output = dv()
    .args([
      "--json",
      "[env:PUBLIC_VALUE=directive]",
      "run",
      "--environment",
      "PUBLIC_VALUE=command-line",
      "--environment=DV_TOKEN=command-secret",
      "--",
      "DV_PASSWORD=forwarded-secret",
    ])
    .env("PUBLIC_VALUE", "ambient")
    .output()
    .unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(output.stderr.is_empty());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(!stdout.contains("command-secret"));
  assert!(!stdout.contains("forwarded-secret"));
  assert!(stdout.contains("DV_TOKEN=<redacted>"));
  assert!(stdout.contains("DV_PASSWORD=<redacted>"));

  let events: Vec<serde_json::Value> = stdout.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
  let diagnostic = events.iter().find(|event| event["type"] == "diagnostic").unwrap();
  assert_eq!(diagnostic["diagnostic"]["code"], "DV0003");
  let context = diagnostic["diagnostic"]["context"].as_array().unwrap();
  assert!(context.iter().any(|field| field["name"] == "environment_edit_count" && field["value"] == "3"));
  assert!(
    context
      .iter()
      .any(|field| field["name"] == "sensitive_environment_edit_count" && field["value"] == "1")
  );
}

#[test]
fn malformed_environment_inputs_fail_before_discovery_without_echoing_values() {
  let root = TempDirectory::new();
  root.write("global.json", "{ malformed");

  for arguments in [
    vec!["[env:=directive-secret]", "run"],
    vec!["test", "--environment", "=command-secret"],
    vec!["run", "--environment"],
  ] {
    let output = dv().args(arguments).current_dir(&root.0).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(text.contains("error[DV0002]"), "{text}");
    assert!(!text.contains("secret"), "{text}");
    assert!(!text.contains("global.json"), "{text}");
  }
}

#[test]
fn child_environment_directives_reject_non_child_commands_before_discovery() {
  let root = TempDirectory::new();
  root.write("global.json", "{ malformed");

  let output = dv()
    .args(["[env:PUBLIC_VALUE=directive]", "sdk", "current"])
    .current_dir(&root.0)
    .output()
    .unwrap();

  assert_eq!(output.status.code(), Some(2));
  let stderr = String::from_utf8(output.stderr).unwrap();
  assert!(stderr.contains("environment directives are supported only by run and test"), "{stderr}");
  assert!(!stderr.contains("global.json"), "{stderr}");
}

#[test]
fn sdk_help_exposes_portable_runtime_compatibility() {
  let output = dv().args(["sdk", "--help"]).output().unwrap();

  assert!(output.status.success());
  assert!(String::from_utf8(output.stdout).unwrap().contains("sdk compatible-rids RID"));
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
fn compatibility_profiles_preserve_reference_failure_codes() {
  for (profile, diagnostic) in [
    ("dotnet", "error[DV0001]"),
    ("msbuild", "error[DV0003]"),
    ("nuget", "error[DV0001]"),
    ("vstest", "error[DV0003]"),
  ] {
    let output = dv().args(["--compat", profile, "frobnicate"]).output().unwrap();

    assert_eq!(output.status.code(), Some(1), "profile {profile}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(diagnostic));
    assert!(stderr.contains(&format!("compatibility_profile: {profile}")));
  }
}

#[test]
fn phase_one_build_and_restore_failures_use_the_selected_exit_profile() {
  let temp = TempDirectory::new();
  for (arguments, expected, profile) in [
    (vec!["restore", "DefinitelyMissing.csproj"], 2, None),
    (vec!["build", "--plan", "DefinitelyMissing.csproj"], 2, None),
    (vec!["--compat", "dotnet", "restore", "DefinitelyMissing.csproj"], 1, Some("dotnet")),
    (vec!["--compat", "dotnet", "build", "--plan", "DefinitelyMissing.csproj"], 1, Some("dotnet")),
  ] {
    let output = dv().args(&arguments).current_dir(&temp.0).output().unwrap();

    assert_eq!(output.status.code(), Some(expected), "arguments={arguments:?}");
    assert!(output.stdout.is_empty(), "arguments={arguments:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("DefinitelyMissing.csproj"), "arguments={arguments:?}: {stderr}");
    assert_eq!(
      stderr.contains("compatibility_profile: dotnet"),
      profile.is_some(),
      "arguments={arguments:?}: {stderr}"
    );
  }

  assert!(!temp.0.join("obj").exists());
  assert!(!temp.0.join(".packages").exists());
}

#[test]
fn compatibility_profile_is_one_structured_context_row_and_native_omits_it() {
  let json = dv().args(["--json", "--compat", "nuget", "frobnicate"]).output().unwrap();
  let native = dv().arg("frobnicate").output().unwrap();

  assert_eq!(json.status.code(), Some(1));
  assert!(json.stderr.is_empty());
  let stdout = String::from_utf8(json.stdout).unwrap();
  assert_eq!(stdout.matches("\"name\":\"compatibility_profile\"").count(), 1);
  assert!(stdout.contains("\"name\":\"compatibility_profile\",\"value\":\"nuget\""));
  assert!(!String::from_utf8(native.stderr).unwrap().contains("compatibility_profile"));
}

#[test]
fn ambiguous_compatibility_words_route_before_project_io() {
  let temp = TempDirectory::new();
  temp.write("global.json", "{ definitely not JSON");
  temp.write("Broken.csproj", "<Project><Broken>");

  for arguments in [
    ["--compat", "nuget", "restore", "Broken.csproj"],
    ["--compat", "msbuild", "restore", "Broken.csproj"],
    ["--compat", "vstest", "restore", "Broken.csproj"],
  ] {
    let output = dv().args(arguments).current_dir(&temp.0).output().unwrap();

    assert_eq!(output.status.code(), Some(1), "arguments={arguments:?}");
    assert!(output.stdout.is_empty(), "arguments={arguments:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[DV0003]"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV01"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV02"), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("error[DV04"), "arguments={arguments:?}: {stderr}");
  }

  assert!(!temp.0.join("obj").exists());
  assert!(!temp.0.join(".packages").exists());
}

#[test]
fn compatibility_mode_is_selected_before_discovery_and_removed_from_operands() {
  let selected = dv().args(["sdk", "--compat=dotnet", "current"]).output().unwrap();
  let failed = dv().args(["--compat", "dotnet", "restore", "DefinitelyMissing.csproj"]).output().unwrap();

  assert!(selected.status.success(), "{}", String::from_utf8_lossy(&selected.stderr));
  assert_eq!(failed.status.code(), Some(1));
  assert!(String::from_utf8(failed.stderr).unwrap().contains("DefinitelyMissing.csproj"));
}

#[test]
fn invalid_compatibility_mode_is_a_native_usage_failure() {
  let output = dv().args(["--compat", "mono", "restore", "DefinitelyMissing.csproj"]).output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  let stderr = String::from_utf8(output.stderr).unwrap();
  assert!(stderr.contains("unsupported compatibility mode"));
  assert!(!stderr.contains("DefinitelyMissing.csproj"));
}

#[test]
fn repeated_compatibility_mode_is_an_unselected_native_usage_failure() {
  let output = dv().args(["--compat", "dotnet", "--compat", "nuget", "restore"]).output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  let stderr = String::from_utf8(output.stderr).unwrap();
  assert!(stderr.contains("--compat may be specified only once"));
  assert!(!stderr.contains("compatibility_profile"));
}

#[test]
fn json_failure_is_a_versioned_event_batch() {
  let output = dv().args(["build", "--json"]).output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(output.stderr.is_empty());
  let stdout = String::from_utf8(output.stdout).unwrap();
  let lines: Vec<&str> = stdout.lines().collect();
  assert_eq!(lines.len(), 3);
  assert!(lines[0].contains("\"schema_version\":22"));
  assert!(lines[0].contains("\"command_syntax_version\":4"));
  assert!(lines[1].contains("\"code\":\"DV0003\""));
  assert!(lines[2].contains("\"outcome\":\"failed\""));
}

#[test]
fn version_aliases_share_one_independently_versioned_json_contract() {
  for arguments in [&["--json", "version"][..], &["--json", "--version"], &["-V", "--json"]] {
    let output = dv().args(arguments).output().unwrap();
    assert!(output.status.success(), "arguments={arguments:?}: {}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(), "arguments={arguments:?}");
    let events = output
      .stdout
      .split(|byte| *byte == b'\n')
      .filter(|line| !line.is_empty())
      .map(serde_json::from_slice::<serde_json::Value>)
      .collect::<Result<Vec<_>, _>>()
      .unwrap();

    assert_eq!(events.len(), 3, "arguments={arguments:?}");
    assert!(
      events
        .iter()
        .all(|event| event.get("schema_version").and_then(serde_json::Value::as_u64) == Some(22))
    );
    assert_eq!(events[0].get("type").and_then(serde_json::Value::as_str), Some("command_started"));
    assert_eq!(events[0].get("command").and_then(serde_json::Value::as_str), Some("version"));
    assert_eq!(events[0].get("command_syntax_version").and_then(serde_json::Value::as_u64), Some(4));
    assert_eq!(events[1].get("type").and_then(serde_json::Value::as_str), Some("tool_version"));
    assert_eq!(events[1].get("version").and_then(serde_json::Value::as_str), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(events[1].get("command_syntax_version").and_then(serde_json::Value::as_u64), Some(4));
    assert_eq!(events[1].get("event_schema_version").and_then(serde_json::Value::as_u64), Some(22));
    assert_eq!(events[2].get("type").and_then(serde_json::Value::as_str), Some("command_finished"));
  }
}

#[test]
fn package_source_inspection_reports_the_effective_offline_batch() {
  let temp = TempDirectory::new();
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
  );
  temp.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear /><add key="offline" value="local-feed" /></packageSources></configuration>"#,
  );
  fs::create_dir_all(temp.0.join("local-feed")).unwrap();

  let output = dv()
    .args(["project", "package-sources", "App.csproj", "--offline", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"package_sources_inspected\""));
  assert!(stdout.contains("\"name\":\"offline\""));
  assert!(stdout.contains("\"location\":"));
  assert!(stdout.contains("\"protocol\":\"local\""));
  assert!(stdout.contains("\"endpoints\":[]"));
  assert!(stdout.contains("\"network_requests\":0"));
  assert!(stdout.contains("\"downloaded_bytes\":0"));
}

#[test]
fn package_source_inspection_surfaces_explicit_transport_risks() {
  let temp = TempDirectory::new();
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
  );
  temp.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear />
<add key="http" value="http://packages.example.test/v3/index.json" protocolVersion="3" allowInsecureConnections="true" />
<add key="tls" value="https://private.example.test/v3/index.json" protocolVersion="3" disableTLSCertificateValidation="true" />
</packageSources></configuration>"#,
  );

  let output = dv()
    .args(["project", "package-sources", "App.csproj", "--offline", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"allow_insecure_connections\":true"));
  assert!(stdout.contains("\"disable_tls_certificate_validation\":true"));
  assert!(stdout.contains("\"tls_validation\":false"));
  assert!(stdout.contains("\"network_requests\":0"));
}

#[test]
fn sdk_current_discovers_without_executing_dotnet() {
  let temp = TempDirectory::new();
  fs::create_dir_all(temp.0.join("sdk/9.0.308")).unwrap();
  temp.write("sdk/9.0.308/dotnet.dll", "");
  fs::write(temp.0.join(format!("dotnet{}", env::consts::EXE_SUFFIX)), b"not an executable").unwrap();

  let output = dv().args(["sdk", "current"]).current_dir(&temp.0).env("PATH", &temp.0).output().unwrap();

  assert!(output.status.success());
  assert_eq!(String::from_utf8(output.stdout).unwrap(), "9.0.308\n");
  assert!(output.stderr.is_empty());
}

#[test]
fn dotnet_sdk_queries_share_the_canonical_selected_sdk_result() {
  let compatibility_version = dv().args(["--compat", "dotnet", "--version"]).output().unwrap();
  let canonical_version = dv().args(["sdk", "current"]).output().unwrap();
  assert!(compatibility_version.status.success());
  assert_eq!(compatibility_version.stdout, canonical_version.stdout);
  assert!(compatibility_version.stderr.is_empty());

  let compatibility_info = dv().args(["--compat", "dotnet", "--info"]).output().unwrap();
  let canonical_info = dv().args(["sdk", "info"]).output().unwrap();
  assert!(compatibility_info.status.success());
  assert_eq!(compatibility_info.stdout, canonical_info.stdout);
  assert!(compatibility_info.stderr.is_empty());
  let info = String::from_utf8(compatibility_info.stdout).unwrap();
  assert!(info.contains("dv --compat dotnet --info"));
  assert!(info.contains("dv sdk info"));
}

#[test]
fn dotnet_inventory_queries_match_the_reference_row_shapes() {
  let temp = TempDirectory::new();
  temp.write("sdk/9.0.308/dotnet.dll", "");
  fs::create_dir_all(temp.0.join("sdk/10.0.100-stale")).unwrap();
  fs::create_dir_all(temp.0.join("shared/Microsoft.NETCore.App/10.0.0")).unwrap();
  fs::create_dir_all(temp.0.join("shared/Microsoft.NETCore.App/9.0.11")).unwrap();
  fs::create_dir_all(temp.0.join("shared/Microsoft.AspNetCore.App/10.0.0")).unwrap();
  fs::write(temp.0.join(format!("dotnet{}", env::consts::EXE_SUFFIX)), b"not an executable").unwrap();
  #[cfg(windows)]
  let expected_root = temp.0.clone();
  #[cfg(not(windows))]
  let expected_root = fs::canonicalize(&temp.0).unwrap();

  let sdks = dv().args(["--compat", "dotnet", "--list-sdks"]).env("PATH", &temp.0).output().unwrap();
  assert!(sdks.status.success(), "{}", String::from_utf8_lossy(&sdks.stderr));
  assert_eq!(
    String::from_utf8(sdks.stdout).unwrap(),
    format!("9.0.308 [{}]\n", expected_root.join("sdk").display())
  );

  let runtimes = dv().args(["--compat", "dotnet", "--list-runtimes"]).env("PATH", &temp.0).output().unwrap();
  assert!(runtimes.status.success(), "{}", String::from_utf8_lossy(&runtimes.stderr));
  assert_eq!(
    String::from_utf8(runtimes.stdout).unwrap(),
    format!(
      "Microsoft.AspNetCore.App 10.0.0 [{}]\nMicrosoft.NETCore.App 9.0.11 [{}]\nMicrosoft.NETCore.App 10.0.0 [{}]\n",
      expected_root.join("shared").join("Microsoft.AspNetCore.App").display(),
      expected_root.join("shared").join("Microsoft.NETCore.App").display(),
      expected_root.join("shared").join("Microsoft.NETCore.App").display()
    )
  );
}

#[test]
fn dotnet_inventory_architecture_selects_the_current_host_root() {
  let architecture = match env::consts::ARCH {
    "arm" => "arm",
    "aarch64" => "arm64",
    "loongarch64" => "loongarch64",
    "powerpc64" => "ppc64le",
    "riscv64" => "riscv64",
    "s390x" => "s390x",
    "x86_64" => "x64",
    "x86" => "x86",
    "wasm32" | "wasm64" => "wasm",
    unsupported => panic!("unsupported test architecture {unsupported}"),
  };
  let uppercase_architecture = architecture.to_ascii_uppercase();
  let temp = TempDirectory::new();
  temp.write("sdk/10.0.100/dotnet.dll", "");
  fs::create_dir_all(temp.0.join("shared/Microsoft.NETCore.App/10.0.0")).unwrap();
  fs::write(temp.0.join(format!("dotnet{}", env::consts::EXE_SUFFIX)), b"not an executable").unwrap();
  #[cfg(windows)]
  let expected_root = temp.0.clone();
  #[cfg(not(windows))]
  let expected_root = fs::canonicalize(&temp.0).unwrap();

  let sdks = dv()
    .args(["--compat", "dotnet", "--list-sdks", "--ARCH", &uppercase_architecture])
    .env("PATH", &temp.0)
    .output()
    .unwrap();
  assert!(sdks.status.success(), "{}", String::from_utf8_lossy(&sdks.stderr));
  assert_eq!(
    String::from_utf8(sdks.stdout).unwrap(),
    format!("10.0.100 [{}]\n", expected_root.join("sdk").display())
  );

  let runtimes = dv()
    .args(["--compat", "dotnet", "--list-runtimes", "--arch", architecture])
    .env("PATH", &temp.0)
    .output()
    .unwrap();
  assert!(runtimes.status.success(), "{}", String::from_utf8_lossy(&runtimes.stderr));
  assert_eq!(
    String::from_utf8(runtimes.stdout).unwrap(),
    format!(
      "Microsoft.NETCore.App 10.0.0 [{}]\n",
      expected_root.join("shared").join("Microsoft.NETCore.App").display()
    )
  );
}

#[test]
fn runtime_inventory_json_uses_the_shared_event_batch() {
  let output = dv().args(["--compat", "dotnet", "--list-runtimes", "--json"]).output().unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"schema_version\":22"), "{stdout}");
  assert!(stdout.contains("\"type\":\"runtime_inventory\""), "{stdout}");
  assert!(stdout.contains("\"family\":\"Microsoft.NETCore.App\""), "{stdout}");
  assert!(stdout.contains("\"outcome\":\"succeeded\""), "{stdout}");
}

#[test]
fn malformed_dotnet_sdk_queries_fail_before_sdk_discovery() {
  let temp = TempDirectory::new();
  temp.write("global.json", "{ malformed");

  for (arguments, command) in [
    (&["--compat", "dotnet", "--version", "unexpected"][..], "--version"),
    (&["--compat", "dotnet", "--info", "unexpected"][..], "--info"),
    (&["--compat", "dotnet", "--list-sdks", "unexpected"][..], "--list-sdks"),
    (&["--compat", "dotnet", "--list-runtimes", "unexpected"][..], "--list-runtimes"),
  ] {
    let output = dv().args(arguments).current_dir(&temp.0).output().unwrap();
    assert_eq!(output.status.code(), Some(1), "arguments={arguments:?}");
    assert!(output.stdout.is_empty(), "arguments={arguments:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(&format!("unexpected {command} argument")), "arguments={arguments:?}: {stderr}");
    assert!(!stderr.contains("global.json"), "arguments={arguments:?}: {stderr}");
  }
}

#[test]
fn sdk_current_json_reports_selected_path() {
  let temp = TempDirectory::new();
  fs::create_dir_all(temp.0.join("sdk/10.0.100")).unwrap();
  temp.write("sdk/10.0.100/dotnet.dll", "");
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

#[test]
fn sdk_compatible_rids_loads_the_selected_graph_without_inference() {
  let temp = TempDirectory::new();
  fs::create_dir_all(temp.0.join("sdk/10.0.100")).unwrap();
  temp.write("sdk/10.0.100/dotnet.dll", "");
  fs::write(temp.0.join(format!("dotnet{}", env::consts::EXE_SUFFIX)), b"not an executable").unwrap();
  temp.write(
    "sdk/10.0.100/PortableRuntimeIdentifierGraph.json",
    r##"{"runtimes":{"base":{"#import":[]},"any":{"#import":["base"]},"linux":{"#import":["any"]},"linux-x64":{"#import":["linux"]}}}"##,
  );

  let output = dv()
    .args(["sdk", "compatible-rids", "linux-x64", "--json"])
    .current_dir(&temp.0)
    .env("PATH", &temp.0)
    .output()
    .unwrap();

  assert!(output.status.success());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"runtime_compatibility\""));
  assert!(stdout.contains("\"runtime_identifier\":\"linux-x64\""));
  assert!(stdout.contains("\"compatible_runtimes\":[\"linux-x64\",\"linux\",\"any\",\"base\"]"));
  assert!(stdout.contains("\"node_count\":4"));
}

#[test]
fn project_inspect_discovers_and_prints_one_project() {
  let temp = TempDirectory::new();
  temp.write("Program.cs", "Console.WriteLine(\"hello\");");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net10.0</TargetFramework>
    <RuntimeIdentifier>win-x64</RuntimeIdentifier>
    <RuntimeIdentifiers>win-x64;linux-x64</RuntimeIdentifiers>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>
</Project>"#,
  );

  let output = dv().args(["project", "inspect"]).current_dir(&temp.0).output().unwrap();

  assert!(output.status.success());
  assert!(output.stderr.is_empty());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("Assembly            App"));
  assert!(stdout.contains("Target              net10.0"));
  assert!(stdout.contains("Runtime             win-x64"));
  assert!(stdout.contains("Runtime dimensions  2"));
  assert!(stdout.contains("  Program.cs"));
}

#[test]
fn project_inspect_json_reports_the_evaluated_batch() {
  let temp = TempDirectory::new();
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net10.0</TargetFramework>
    <RuntimeIdentifier>win-x64</RuntimeIdentifier>
    <RuntimeIdentifiers>win-x64;linux-x64</RuntimeIdentifiers>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Example.Package" Version="1.2.3" />
  </ItemGroup>
</Project>"#,
  );

  let output = dv()
    .args(["project", "inspect", "App.csproj", "--configuration", "Release", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success());
  assert!(output.stderr.is_empty());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"project_evaluated\""));
  assert!(stdout.contains("\"configuration\":\"Release\""));
  assert!(stdout.contains("\"runtime_identifier\":\"win-x64\""));
  assert!(stdout.contains("\"runtime_identifiers\":[\"win-x64\",\"linux-x64\"]"));
  assert!(stdout.contains("\"runtime_dimensions\":[\"win-x64\",\"linux-x64\"]"));
  assert!(stdout.contains("\"sources\":[\"Program.cs\"]"));
  assert!(stdout.contains("\"id\":\"Example.Package\",\"version\":\"1.2.3\""));
}

#[test]
fn project_inspect_rejects_ambiguous_selection() {
  let temp = TempDirectory::new();
  let project = r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#;
  temp.write("A.csproj", project);
  temp.write("B.csproj", project);

  let output = dv().args(["project", "inspect"]).current_dir(&temp.0).output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  let stderr = String::from_utf8(output.stderr).unwrap();
  assert!(stderr.contains("error[DV0201]"));
  let first = stderr.find("candidate: A.csproj (C# project)").unwrap();
  let second = stderr.find("candidate: B.csproj (C# project)").unwrap();
  assert!(first < second, "{stderr}");
  assert!(stderr.contains("help: Pass one project or solution path explicitly."));

  let json = dv().args(["--json", "project", "inspect"]).current_dir(&temp.0).output().unwrap();
  assert_eq!(json.status.code(), Some(2));
  assert!(json.stderr.is_empty());
  let stdout = String::from_utf8(json.stdout).unwrap();
  let first = stdout.find(r#""name":"candidate","value":"A.csproj (C# project)""#).unwrap();
  let second = stdout.find(r#""name":"candidate","value":"B.csproj (C# project)""#).unwrap();
  assert!(first < second, "{stdout}");
}

#[test]
fn project_root_discovers_git_without_project_selection() {
  let temp = TempDirectory::new();
  fs::create_dir(temp.0.join(".git")).unwrap();
  fs::write(temp.0.join(".git/fixture-marker"), []).unwrap();
  let nested = temp.0.join("src/tool");
  fs::create_dir_all(&nested).unwrap();

  let human = dv().args(["project", "root"]).current_dir(&nested).output().unwrap();
  assert!(human.status.success(), "{}", String::from_utf8_lossy(&human.stderr));
  assert!(human.stderr.is_empty());
  let human_root = PathBuf::from(String::from_utf8(human.stdout).unwrap().trim());
  assert!(human_root.join(".git/fixture-marker").is_file());

  let json = dv().args(["--json", "project", "root"]).arg(&nested).output().unwrap();
  assert!(json.status.success(), "{}", String::from_utf8_lossy(&json.stderr));
  assert!(json.stderr.is_empty());
  let event = String::from_utf8(json.stdout)
    .unwrap()
    .lines()
    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
    .find(|event| event["type"] == "repository_root_discovered")
    .unwrap();
  assert_eq!(event["schema_version"], 22);
  assert_eq!(event["root"], temp.0.to_string_lossy().as_ref());
  assert_eq!(event["kind"], "git");
  assert_eq!(event["marker_probes"], 3);
}

#[test]
fn project_root_validates_operands_before_marker_io() {
  let temp = TempDirectory::new();

  let output = dv().args(["project", "root", "first", "second"]).current_dir(&temp.0).output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  let stderr = String::from_utf8(output.stderr).unwrap();
  assert!(stderr.contains("error[DV0002]"), "{stderr}");
  assert!(stderr.contains("at most one path"), "{stderr}");
  assert!(!stderr.contains("repository marker"), "{stderr}");
}

#[test]
fn project_inspect_accepts_named_file_and_directory_selection() {
  let temp = TempDirectory::new();
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
  );

  for arguments in [
    vec!["project", "inspect", "--project", "App.csproj", "--json"],
    vec!["project", "inspect", "--project=App.csproj", "--json"],
    vec!["project", "inspect", "--project", ".", "--json"],
  ] {
    let output = dv().args(arguments).current_dir(&temp.0).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8(output.stdout).unwrap().contains("\"type\":\"project_evaluated\""));
  }
}

#[test]
fn project_inspect_validates_explicit_candidate_files_before_evaluation() {
  let temp = TempDirectory::new();
  fs::create_dir(temp.0.join("Directory.csproj")).unwrap();

  for (project, code, message) in [
    ("Missing.csproj", "DV0200", "does not exist"),
    ("Missing.fsproj", "DV0204", "accepts only C# .csproj files"),
    ("Directory.csproj", "DV0204", "not a regular file"),
  ] {
    let output = dv().args(["project", "inspect", "--project", project]).current_dir(&temp.0).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(&format!("error[{code}]")), "{project}: {stderr}");
    assert!(stderr.contains(message), "{project}: {stderr}");
    assert!(!stderr.contains("invalid XML"), "{project}: {stderr}");
  }
}

#[test]
fn malformed_project_selection_fails_before_project_io() {
  let temp = TempDirectory::new();
  for arguments in [
    vec!["project", "inspect", "Missing.csproj", "--project", "Other.csproj"],
    vec!["project", "inspect", "--project", "Missing.csproj", "--project", "Other.csproj"],
    vec!["project", "inspect", "--project="],
    vec!["project", "inspect", "--project", "--configuration", "Release"],
  ] {
    let output = dv().args(arguments).current_dir(&temp.0).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[DV0002]"));
    assert!(!stderr.contains("error[DV020"));
  }
}

#[test]
fn explicit_solution_selection_is_typed_before_solution_evaluation() {
  let temp = TempDirectory::new();
  temp.write("App.sln", "Microsoft Visual Studio Solution File, Format Version 12.00\n");
  temp.write("App.slnx", "<Solution />\n");

  for solution in ["App.sln", "App.slnx"] {
    let output = dv().args(["project", "inspect", "--project", solution]).current_dir(&temp.0).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[DV0204]"));
    assert!(stderr.contains("accepts only C# .csproj files"));
    assert!(!stderr.contains("ambiguous"));
  }
}

#[test]
fn restore_applies_the_selected_configuration_before_package_validation() {
  let temp = TempDirectory::new();
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
  <ItemGroup><PackageReference Include="Debug.Only" Condition="'$(Configuration)' == 'Debug'" /></ItemGroup>
</Project>"#,
  );
  temp.write("NuGet.Config", r#"<configuration><packageSources><clear /></packageSources></configuration>"#);

  let output = dv()
    .args(["restore", "App.csproj", "--configuration=Release", "--offline", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"package_resolution_created\""));
  assert!(stdout.contains("\"packages\":[]"));
  assert!(stdout.contains("\"network_requests\":0"));
}

#[test]
fn sync_and_restore_share_the_verified_offline_operation() {
  let temp = TempDirectory::new();
  temp.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear /><add key="legacy" value="https://packages.example.test/api/v2/" protocolVersion="2" /></packageSources></configuration>"#,
  );
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net8.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Sample.Package" Version="1.2.3" /></ItemGroup></Project>"#,
  );
  temp.write(
    "packages/sample.package/1.2.3/sample.package.nuspec",
    r#"<package><metadata><id>Sample.Package</id><version>1.2.3</version></metadata></package>"#,
  );
  temp.write("packages/sample.package/1.2.3/sample.package.1.2.3.nupkg", "");
  temp.write(
    "packages/sample.package/1.2.3/sample.package.1.2.3.nupkg.sha512",
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
  );
  temp.write("packages/sample.package/1.2.3/.dv.metadata.json", "{}");
  temp.write("packages/sample.package/1.2.3/lib/net6.0/Sample.Package.dll", "");
  temp.write("packages/sample.package/1.2.3/lib/net10.0/Sample.Package.dll", "");

  let output = dv()
    .args(["sync", "App.csproj", "--packages", "packages", "--offline", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"package_resolution_created\""));
  assert!(stdout.contains("\"target_framework\":\"net8.0\""));
  assert!(stdout.contains("\"source_protocol\":\"v2\""));
  assert!(stdout.contains("\"network_requests\":0"));
  assert!(stdout.contains("lib\\\\net6.0\\\\Sample.Package.dll") || stdout.contains("lib/net6.0/Sample.Package.dll"));

  let alias = dv()
    .args(["restore", "App.csproj", "--packages", "packages", "--offline", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(alias.status.success(), "{}", String::from_utf8_lossy(&alias.stderr));
  let stdout = String::from_utf8(alias.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"command_started\",\"command_syntax_version\":4,\"command\":\"restore\""));
  assert!(stdout.contains("\"type\":\"package_resolution_created\""));
  assert!(stdout.contains("\"network_requests\":0"));
  assert!(stdout.contains("\"type\":\"command_finished\",\"command\":\"restore\""));
}

#[test]
fn restore_accepts_a_project_batch_and_emits_one_resolution_per_project() {
  let temp = TempDirectory::new();
  let project = r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Sample.Package" Version="1.2.3" /></ItemGroup></Project>"#;
  temp.write("left/Program.cs", "");
  temp.write("right/Program.cs", "");
  temp.write("left/Left.csproj", project);
  temp.write("right/Right.csproj", project);
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><ProjectReference Include="left/Left.csproj" /><ProjectReference Include="right/Right.csproj" /></ItemGroup></Project>"#,
  );
  temp.write(
    "packages/sample.package/1.2.3/sample.package.nuspec",
    r#"<package><metadata><id>Sample.Package</id><version>1.2.3</version></metadata></package>"#,
  );
  temp.write("packages/sample.package/1.2.3/sample.package.1.2.3.nupkg", "");
  temp.write(
    "packages/sample.package/1.2.3/sample.package.1.2.3.nupkg.sha512",
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
  );
  temp.write("packages/sample.package/1.2.3/.dv.metadata.json", "{}");
  temp.write("packages/sample.package/1.2.3/lib/net10.0/Sample.Package.dll", "");

  let output = dv()
    .args(["restore", "App.csproj", "--packages", "packages", "--offline", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert_eq!(stdout.matches("\"type\":\"package_resolution_created\"").count(), 3);
  assert!(stdout.contains("App.csproj"));
  assert!(stdout.contains("Left.csproj"));
  assert!(stdout.contains("Right.csproj"));
  assert!(stdout.contains("\"outcome\":\"succeeded\""));
}

#[test]
fn restore_reports_unmapped_package_before_source_discovery() {
  let temp = TempDirectory::new();
  temp.write(
    "NuGet.Config",
    r#"<configuration>
<packageSources><clear /><add key="decoy" value="http://127.0.0.1:9/v3/index.json" protocolVersion="3" allowInsecureConnections="true" /></packageSources>
<packageSourceMapping><packageSource key="decoy"><package pattern="Mapped.*" /></packageSource></packageSourceMapping>
</configuration>"#,
  );
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Unmapped.Package" Version="1.0.0" /></ItemGroup></Project>"#,
  );

  let output = dv()
    .args(["restore", "App.csproj", "--packages", "packages", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(output.stderr.is_empty());
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"code\":\"DV0412\""));
  assert!(stdout.contains("\"name\":\"package_id\",\"value\":\"unmapped.package\""));
  assert!(stdout.contains("Add a matching packageSourceMapping pattern"));
  assert!(!stdout.contains("DV0404"));
}

#[test]
fn restore_distinguishes_a_missing_identity_from_a_missing_version() {
  let missing_identity = TempDirectory::new();
  missing_identity.write("feed/.keep", "");
  missing_identity.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear /><add key="local" value="feed" /></packageSources></configuration>"#,
  );
  missing_identity.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Absent.Package" Version="[1.0.0]" /></ItemGroup></Project>"#,
  );
  let output = dv()
    .args(["restore", "App.csproj", "--packages", "packages", "--offline", "--json"])
    .current_dir(&missing_identity.0)
    .output()
    .unwrap();
  assert_eq!(output.status.code(), Some(2));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"code\":\"DV0416\""), "{stdout}");
  assert!(stdout.contains("\"name\":\"package_id\",\"value\":\"absent.package\""));

  let missing_version = TempDirectory::new();
  missing_version.write("feed/.keep", "");
  missing_version.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear /><add key="local" value="feed" /></packageSources></configuration>"#,
  );
  missing_version.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Known.Package" Version="[3.0.0]" /></ItemGroup></Project>"#,
  );
  for version in ["1.0.0", "2.0.0"] {
    let marker = format!("packages/known.package/{version}/.dv.metadata.json");
    missing_version.write(&marker, "{}");
  }
  let output = dv()
    .args(["restore", "App.csproj", "--packages", "packages", "--offline", "--json"])
    .current_dir(&missing_version.0)
    .output()
    .unwrap();
  assert_eq!(output.status.code(), Some(2));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"code\":\"DV0417\""), "{stdout}");
  assert!(stdout.contains("\"name\":\"nearest_version\",\"value\":\"2.0.0\""));
}

#[test]
fn restore_reports_incompatible_package_asset_frameworks() {
  let temp = TempDirectory::new();
  temp.write("feed/.keep", "");
  temp.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear /><add key="local" value="feed" /></packageSources></configuration>"#,
  );
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Future.Package" Version="[1.0.0]" /></ItemGroup></Project>"#,
  );
  temp.write(
    "packages/future.package/1.0.0/future.package.nuspec",
    r#"<package><metadata><id>Future.Package</id><version>1.0.0</version></metadata></package>"#,
  );
  temp.write("packages/future.package/1.0.0/future.package.1.0.0.nupkg", "");
  temp.write(
    "packages/future.package/1.0.0/future.package.1.0.0.nupkg.sha512",
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
  );
  temp.write("packages/future.package/1.0.0/.dv.metadata.json", "{}");
  temp.write("packages/future.package/1.0.0/lib/net11.0/Future.Package.dll", "");

  let output = dv()
    .args(["restore", "App.csproj", "--packages", "packages", "--offline", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();
  assert_eq!(output.status.code(), Some(2));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"code\":\"DV0402\""), "{stdout}");
  assert!(stdout.contains("\"name\":\"target_framework\",\"value\":\"net10.0\""));
  assert!(stdout.contains("\"name\":\"supported_frameworks\",\"value\":\"net11.0\""));
}

#[test]
fn restore_reports_the_complete_dependency_cycle() {
  let temp = TempDirectory::new();
  temp.write("feed/.keep", "");
  temp.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear /><add key="local" value="feed" /></packageSources></configuration>"#,
  );
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Cycle.A" Version="[1.0.0]" /></ItemGroup></Project>"#,
  );
  for (id, lower, dependency) in [("Cycle.A", "cycle.a", "Cycle.B"), ("Cycle.B", "cycle.b", "Cycle.A")] {
    let root = format!("packages/{lower}/1.0.0");
    temp.write(
      &format!("{root}/{lower}.nuspec"),
      &format!(
        r#"<package><metadata><id>{id}</id><version>1.0.0</version><dependencies><group targetFramework="net10.0"><dependency id="{dependency}" version="[1.0.0]" /></group></dependencies></metadata></package>"#,
      ),
    );
    temp.write(&format!("{root}/{lower}.1.0.0.nupkg"), "");
    temp.write(
      &format!("{root}/{lower}.1.0.0.nupkg.sha512"),
      "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
    );
    temp.write(&format!("{root}/.dv.metadata.json"), "{}");
    temp.write(&format!("{root}/lib/net10.0/{id}.dll"), "");
  }

  let output = dv()
    .args(["restore", "App.csproj", "--packages", "packages", "--offline", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();
  assert_eq!(output.status.code(), Some(2));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"code\":\"DV0415\""), "{stdout}");
  assert!(stdout.contains("\"name\":\"cycle\",\"value\":\"cycle.a -> cycle.b -> cycle.a\""));
  assert!(stdout.contains("package metadata contains a circular dependency chain"));
}

#[test]
fn explicit_nuget_config_replaces_the_implicit_hierarchy() {
  let temp = TempDirectory::new();
  temp.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear /><add key="insecure" value="http://invalid.example.test/v2" /></packageSources></configuration>"#,
  );
  temp.write(
    "config/selected.config",
    r#"<configuration>
<config><add key="globalPackagesFolder" value="../packages" /></config>
<packageSources><clear /><add key="selected" value="https://packages.example.test/api/v2/" protocolVersion="2" /></packageSources>
</configuration>"#,
  );
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net8.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Sample.Package" Version="1.2.3" /></ItemGroup></Project>"#,
  );
  temp.write(
    "packages/sample.package/1.2.3/sample.package.nuspec",
    r#"<package><metadata><id>Sample.Package</id><version>1.2.3</version></metadata></package>"#,
  );
  temp.write("packages/sample.package/1.2.3/sample.package.1.2.3.nupkg", "");
  temp.write(
    "packages/sample.package/1.2.3/sample.package.1.2.3.nupkg.sha512",
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
  );
  temp.write("packages/sample.package/1.2.3/.dv.metadata.json", "{}");
  temp.write("packages/sample.package/1.2.3/lib/net6.0/Sample.Package.dll", "");

  let output = dv()
    .args(["restore", "App.csproj", "--configfile", "config/selected.config", "--offline", "--json"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"source\":\"selected\""));
  assert!(stdout.contains("\"source_protocol\":\"v2\""));
  assert!(stdout.contains("\"source_work\":[{\"name\":\"selected\",\"protocol\":\"v2\",\"requests\":0,\"downloaded_bytes\":0,\"duration_us\":0}]"));
  assert!(stdout.contains("\"cache_outcome\":\"hit\""));
  assert!(stdout.contains("\"network_requests\":0"));
}

#[test]
fn package_cli_overrides_replace_sources_and_win_cache_precedence() {
  let temp = TempDirectory::new();
  temp.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear /><add key="implicit" value="http://invalid.example.test/v2" /></packageSources></configuration>"#,
  );
  temp.write(
    "config/selected.config",
    r#"<configuration>
<config><add key="globalPackagesFolder" value="../wrong-packages" /></config>
<packageSources><clear /><add key="configured" value="https://configured.example.test/api/v2/" protocolVersion="2" /></packageSources>
</configuration>"#,
  );
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net8.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Sample.Package" Version="1.2.3" /></ItemGroup></Project>"#,
  );
  temp.write(
    "cli-packages/sample.package/1.2.3/sample.package.nuspec",
    r#"<package><metadata><id>Sample.Package</id><version>1.2.3</version></metadata></package>"#,
  );
  temp.write("cli-packages/sample.package/1.2.3/sample.package.1.2.3.nupkg", "");
  temp.write(
    "cli-packages/sample.package/1.2.3/sample.package.1.2.3.nupkg.sha512",
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
  );
  temp.write("cli-packages/sample.package/1.2.3/.dv.metadata.json", "{}");
  temp.write("cli-packages/sample.package/1.2.3/lib/net6.0/Sample.Package.dll", "");

  let output = dv()
    .args([
      "restore",
      "App.csproj",
      "--configfile",
      "config/selected.config",
      "--source",
      "https://cli.example.test/api/v2/",
      "--packages",
      "cli-packages",
      "--offline",
      "--json",
    ])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"source\":\"https://cli.example.test/api/v2/\""));
  assert!(stdout.contains("\"cache_root\":"));
  assert!(stdout.contains("cli-packages"));
  assert!(!stdout.contains("wrong-packages"));
  assert!(stdout.contains("\"network_requests\":0"));
}

#[test]
fn package_cli_rejects_repeated_singleton_overrides_before_project_io() {
  for arguments in [
    ["restore", "missing.csproj", "--packages", "one", "--packages", "two"],
    ["restore", "missing.csproj", "--configfile", "one", "--configfile", "two"],
  ] {
    let output = dv().args(arguments).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr).unwrap().contains("cannot be specified more than once"));
  }
}

#[test]
fn restore_cli_overrides_replace_sources_config_and_package_directory() {
  let temp = TempDirectory::new();
  temp.write(
    "NuGet.Config",
    r#"<configuration><packageSources><clear /><add key="implicit" value="http://invalid.example.test/v2" /></packageSources></configuration>"#,
  );
  temp.write(
    "config/selected.config",
    r#"<configuration>
<config><add key="globalPackagesFolder" value="../config-packages" /></config>
<packageSources><clear /><add key="configured" value="https://configured.example.test/api/v2" protocolVersion="2" /></packageSources>
</configuration>"#,
  );
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net8.0</TargetFramework><NuGetAudit>false</NuGetAudit></PropertyGroup>
<ItemGroup><PackageReference Include="Sample.Package" Version="1.2.3" /></ItemGroup></Project>"#,
  );
  temp.write(
    "cli-packages/sample.package/1.2.3/sample.package.nuspec",
    r#"<package><metadata><id>Sample.Package</id><version>1.2.3</version></metadata></package>"#,
  );
  temp.write("cli-packages/sample.package/1.2.3/sample.package.1.2.3.nupkg", "");
  temp.write(
    "cli-packages/sample.package/1.2.3/sample.package.1.2.3.nupkg.sha512",
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
  );
  temp.write("cli-packages/sample.package/1.2.3/.dv.metadata.json", "{}");
  temp.write("cli-packages/sample.package/1.2.3/lib/net6.0/Sample.Package.dll", "");

  let output = dv()
    .args([
      "restore",
      "App.csproj",
      "-s=https://packages.example.test/v3/index.json",
      "--packages=cli-packages",
      "--configfile=config/selected.config",
      "--offline",
      "--json",
    ])
    .current_dir(&temp.0)
    .env("NUGET_PACKAGES", temp.0.join("env-packages"))
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"source\":\"https://packages.example.test/v3/index.json\""));
  assert!(stdout.contains("\"source_protocol\":\"v3\""));
  assert!(stdout.contains("cli-packages"));
  assert!(stdout.contains("\"network_requests\":0"));
  assert!(!temp.0.join("env-packages").exists());
  assert!(!temp.0.join("config-packages").exists());
}

#[test]
fn restore_rejects_repeated_single_value_overrides() {
  let output = dv().args(["restore", "--packages", "one", "--packages", "two", "--json"]).output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(
    String::from_utf8(output.stdout)
      .unwrap()
      .contains("--packages cannot be specified more than once")
  );
}

#[test]
fn restore_rejects_unsupported_cli_sources_before_project_io() {
  let output = dv()
    .args(["restore", "missing.csproj", "--source", "ftp://unsupported.example.test/v2"])
    .output()
    .unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(String::from_utf8(output.stderr).unwrap().contains("requires HTTP, HTTPS, file://"));
}

#[test]
fn restore_accepts_a_relative_local_cli_source() {
  let temp = TempDirectory::new();
  let output = dv()
    .args(["restore", "missing.csproj", "--source", "relative-feed"])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(String::from_utf8(output.stderr).unwrap().contains("missing.csproj does not exist"));
}

#[test]
fn package_source_inspection_keeps_offline_discovery_network_free() {
  let temp = TempDirectory::new();
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
  );
  let output = dv()
    .args([
      "project",
      "package-sources",
      "App.csproj",
      "--source",
      "https://api.nuget.org/v3/index.json",
      "--offline",
      "--json",
    ])
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let event = String::from_utf8(output.stdout)
    .unwrap()
    .lines()
    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
    .find(|event| event.get("type").and_then(serde_json::Value::as_str) == Some("package_sources_inspected"))
    .unwrap();
  assert_eq!(event.get("network_requests").and_then(serde_json::Value::as_u64), Some(0));
  assert_eq!(event.pointer("/sources/0/protocol").and_then(serde_json::Value::as_str), Some("v3"));
  assert_eq!(
    event.pointer("/sources/0/endpoints").and_then(serde_json::Value::as_array).map(Vec::len),
    Some(0)
  );
}

#[test]
fn package_source_credentials_report_only_the_authentication_kind() {
  let temp = TempDirectory::new();
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>"#,
  );
  temp.write(
    "NuGet.Config",
    r#"<configuration>
<packageSources><clear /><add key="private" value="https://packages.example.test/v3/index.json" protocolVersion="3" /></packageSources>
<packageSourceCredentials><private>
  <add key="Username" value="config-decoy-user" />
  <add key="ClearTextPassword" value="config-decoy-secret" />
  <add key="ValidAuthenticationTypes" value="negotiate" />
</private></packageSourceCredentials>
</configuration>"#,
  );
  let output = dv()
    .args(["project", "package-sources", "App.csproj", "--offline", "--json"])
    .env(
      "NuGetPackageSourceCredentials_private",
      "Username=environment-user;Password=environment-pat;ValidAuthenticationTypes=basic",
    )
    .current_dir(&temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  let stderr = String::from_utf8(output.stderr).unwrap();
  assert!(stdout.contains("\"authentication\":\"basic\""));
  for secret in ["environment-user", "environment-pat", "config-decoy-user", "config-decoy-secret"] {
    assert!(!stdout.contains(secret));
    assert!(!stderr.contains(secret));
  }

  let human = dv()
    .args(["project", "package-sources", "App.csproj", "--offline"])
    .env(
      "NuGetPackageSourceCredentials_private",
      "Username=environment-user;Password=environment-pat;ValidAuthenticationTypes=basic",
    )
    .current_dir(&temp.0)
    .output()
    .unwrap();
  assert!(human.status.success(), "{}", String::from_utf8_lossy(&human.stderr));
  let human = format!("{}{}", String::from_utf8_lossy(&human.stdout), String::from_utf8_lossy(&human.stderr));
  assert!(human.contains("private (v3, basic,"), "{human}");
  for secret in ["environment-user", "environment-pat", "config-decoy-user", "config-decoy-secret"] {
    assert!(!human.contains(secret));
  }
  assert!(!temp.0.join("dv.lock.json").exists());
}

#[test]
fn build_plan_json_reports_framework_and_compiler_inputs() {
  let temp = TempDirectory::new();
  temp.write("sdk/10.0.100/dotnet.dll", "");
  temp.write("Program.cs", "Console.WriteLine(\"hello\");");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings></PropertyGroup></Project>"#,
  );
  for relative in [
    "sdk/10.0.100/Roslyn/bincore/csc.dll",
    "sdk/10.0.100/Sdks/Microsoft.NET.Sdk/analyzers/Microsoft.CodeAnalysis.CSharp.NetAnalyzers.dll",
    "sdk/10.0.100/Sdks/Microsoft.NET.Sdk/analyzers/Microsoft.CodeAnalysis.NetAnalyzers.dll",
    "sdk/10.0.100/Sdks/Microsoft.NET.Sdk/analyzers/build/config/analysislevel_10_default.globalconfig",
    "packs/Microsoft.NETCore.App.Ref/10.0.0/ref/net10.0/System.Runtime.dll",
    "packs/Microsoft.NETCore.App.Ref/10.0.0/analyzers/dotnet/cs/Generator.dll",
  ] {
    temp.write(relative, "");
  }
  temp.write(
    "packs/Microsoft.NETCore.App.Ref/10.0.0/data/FrameworkList.xml",
    r#"<FileList TargetFrameworkIdentifier=".NETCoreApp" TargetFrameworkVersion="10.0"><File Type="Managed" Path="ref/net10.0/System.Runtime.dll" /><File Type="Analyzer" Language="cs" Path="analyzers/dotnet/cs/Generator.dll" /></FileList>"#,
  );
  temp.write(&format!("dotnet{}", env::consts::EXE_SUFFIX), "not an executable");

  let output = dv()
    .args(["build", "--plan", "App.csproj", "--json"])
    .current_dir(&temp.0)
    .env("PATH", &temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"compiler_plan_created\""));
  assert!(stdout.contains("\"sdk_version\":\"10.0.100\""));
  assert!(stdout.contains("\"framework_pack_version\":\"10.0.0\""));
  assert!(stdout.contains("\"language_version\":\"14.0\""));
  assert!(stdout.contains("\"references\":["));
  assert!(stdout.contains("\"outcome\":\"succeeded\""));
}

#[test]
fn runtime_pack_json_reports_manifest_selected_assets_and_apphost() {
  let temp = TempDirectory::new();
  temp.write("sdk/10.0.100/dotnet.dll", "");
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><RuntimeIdentifier>child-x64</RuntimeIdentifier></PropertyGroup></Project>"#,
  );
  temp.write(
    "sdk/10.0.100/Microsoft.NETCoreSdk.BundledVersions.props",
    r#"<Project><ItemGroup>
      <KnownFrameworkReference Include="Microsoft.NETCore.App" TargetFramework="net10.0" DefaultRuntimeFrameworkVersion="10.0.0" LatestRuntimeFrameworkVersion="10.0.7" RuntimePackNamePatterns="Runtime.**RID**" RuntimePackRuntimeIdentifiers="base-x64" />
      <KnownAppHostPack Include="Microsoft.NETCore.App" TargetFramework="net10.0" AppHostPackNamePattern="Host.**RID**" AppHostPackVersion="10.0.7" AppHostRuntimeIdentifiers="base-x64" />
    </ItemGroup></Project>"#,
  );
  temp.write(
    "sdk/10.0.100/PortableRuntimeIdentifierGraph.json",
    r##"{"runtimes":{"base-x64":{"#import":[]},"child-x64":{"#import":["base-x64"]}}}"##,
  );
  temp.write(
    "packages/runtime.base-x64/10.0.7/data/RuntimeList.xml",
    r#"<FileList TargetFrameworkVersion="10.0" FrameworkName="Microsoft.NETCore.App"><File Type="Managed" Path="runtimes/base-x64/lib/net10.0/Core.dll"/><File Type="Native" Path="runtimes/base-x64/native/core.dll"/></FileList>"#,
  );
  temp.write("packages/runtime.base-x64/10.0.7/runtimes/base-x64/lib/net10.0/Core.dll", "");
  temp.write("packages/runtime.base-x64/10.0.7/runtimes/base-x64/native/core.dll", "");
  temp.write("packs/Host.base-x64/10.0.7/runtimes/base-x64/native/apphost", "");
  temp.write(&format!("dotnet{}", env::consts::EXE_SUFFIX), "not an executable");

  let output = dv()
    .args(["project", "runtime-packs", "App.csproj", "--packages", "packages", "--json"])
    .current_dir(&temp.0)
    .env("PATH", &temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"runtime_pack_plan_created\""));
  assert!(stdout.contains("\"requested_runtime_identifier\":\"child-x64\""));
  assert!(stdout.contains("\"runtime_identifier\":\"base-x64\""));
  assert!(stdout.contains("\"runtime_pack_id\":\"Runtime.base-x64\""));
  assert!(stdout.contains("\"runtime_pack_version\":\"10.0.7\""));
  assert!(stdout.contains("Core.dll"));
  assert!(stdout.contains("core.dll"));
  assert!(stdout.contains("apphost"));
}

#[test]
fn unavailable_runtime_pack_reports_identity_version_dimensions_and_action() {
  let temp = TempDirectory::new();
  temp.write("sdk/10.0.100/dotnet.dll", "");
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><RuntimeIdentifier>linux-arm</RuntimeIdentifier><SelfContained>true</SelfContained></PropertyGroup></Project>"#,
  );
  temp.write(
    "sdk/10.0.100/Microsoft.NETCoreSdk.BundledVersions.props",
    r#"<Project><ItemGroup>
      <KnownFrameworkReference Include="Microsoft.NETCore.App" TargetFramework="net10.0" DefaultRuntimeFrameworkVersion="10.0.0" LatestRuntimeFrameworkVersion="10.0.0" RuntimePackNamePatterns="Microsoft.NETCore.App.Runtime.**RID**" RuntimePackRuntimeIdentifiers="linux-arm" />
      <KnownAppHostPack Include="Microsoft.NETCore.App" TargetFramework="net10.0" AppHostPackNamePattern="Microsoft.NETCore.App.Host.**RID**" AppHostPackVersion="10.0.0" AppHostRuntimeIdentifiers="linux-arm" />
    </ItemGroup></Project>"#,
  );
  temp.write(
    "sdk/10.0.100/PortableRuntimeIdentifierGraph.json",
    r##"{"runtimes":{"linux-arm":{"#import":[]}}}"##,
  );
  fs::create_dir_all(temp.0.join("packages")).unwrap();
  temp.write(&format!("dotnet{}", env::consts::EXE_SUFFIX), "not an executable");

  let output = dv()
    .args(["project", "runtime-packs", "App.csproj", "--packages", "packages", "--json"])
    .current_dir(&temp.0)
    .env("PATH", &temp.0)
    .output()
    .unwrap();

  assert_eq!(output.status.code(), Some(2));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"code\":\"DV0124\""));
  assert!(stdout.contains("\"name\":\"pack_kind\",\"value\":\"runtime_pack\""));
  assert!(stdout.contains("\"name\":\"pack_identity\",\"value\":\"Microsoft.NETCore.App.Runtime.linux-arm\""));
  assert!(stdout.contains("\"name\":\"pack_version\",\"value\":\"10.0.0\""));
  assert!(stdout.contains("\"name\":\"target_framework\",\"value\":\"net10.0\""));
  assert!(stdout.contains("\"name\":\"runtime_identifier\",\"value\":\"linux-arm\""));
  assert!(stdout.contains("\"name\":\"acquisition\",\"value\":\"restore_package\""));
  assert!(stdout.contains("\"help\":\"Restore the required pack from a configured package source.\""));
}

#[test]
fn framework_plan_json_resolves_explicit_reference_and_shared_runtime() {
  let temp = TempDirectory::new();
  temp.write("sdk/10.0.100/dotnet.dll", "");
  temp.write("Program.cs", "");
  temp.write(
    "App.csproj",
    r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework><RollForward>LatestPatch</RollForward></PropertyGroup><ItemGroup><FrameworkReference Include="Microsoft.AspNetCore.App" /></ItemGroup></Project>"#,
  );
  temp.write(
    "sdk/10.0.100/Microsoft.NETCoreSdk.BundledVersions.props",
    r#"<Project><ItemGroup>
      <KnownFrameworkReference Include="Microsoft.NETCore.App" TargetFramework="net10.0" RuntimeFrameworkName="Microsoft.NETCore.App" DefaultRuntimeFrameworkVersion="10.0.0" LatestRuntimeFrameworkVersion="10.0.1" TargetingPackName="Microsoft.NETCore.App.Ref" TargetingPackVersion="10.0.1" />
      <KnownFrameworkReference Include="Microsoft.AspNetCore.App" TargetFramework="net10.0" RuntimeFrameworkName="Microsoft.AspNetCore.App" DefaultRuntimeFrameworkVersion="10.0.0" LatestRuntimeFrameworkVersion="10.0.1" TargetingPackName="Microsoft.AspNetCore.App.Ref" TargetingPackVersion="10.0.1" />
    </ItemGroup></Project>"#,
  );
  fs::create_dir_all(temp.0.join("packs/Microsoft.NETCore.App.Ref/10.0.1")).unwrap();
  fs::create_dir_all(temp.0.join("packs/Microsoft.AspNetCore.App.Ref/10.0.1")).unwrap();
  fs::create_dir_all(temp.0.join("shared/Microsoft.NETCore.App/10.0.7")).unwrap();
  fs::create_dir_all(temp.0.join("shared/Microsoft.AspNetCore.App/10.0.7")).unwrap();
  fs::create_dir_all(temp.0.join("packages")).unwrap();
  temp.write(&format!("dotnet{}", env::consts::EXE_SUFFIX), "not an executable");

  let output = dv()
    .args(["project", "frameworks", "App.csproj", "--packages", "packages", "--json"])
    .current_dir(&temp.0)
    .env("PATH", &temp.0)
    .output()
    .unwrap();

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.contains("\"type\":\"framework_reference_plan_created\""));
  assert!(stdout.contains("\"reference\":\"Microsoft.NETCore.App\""));
  assert!(stdout.contains("\"reference\":\"Microsoft.AspNetCore.App\""));
  assert!(stdout.contains("\"requested_version\":\"10.0.0\""));
  assert!(stdout.contains("\"selected_version\":\"10.0.7\""));
  assert!(stdout.contains("\"roll_forward\":\"LatestPatch\""));
}
