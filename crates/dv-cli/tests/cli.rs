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
fn json_failure_is_a_versioned_event_batch() {
  let output = dv().args(["build", "--json"]).output().unwrap();

  assert_eq!(output.status.code(), Some(2));
  assert!(output.stderr.is_empty());
  let stdout = String::from_utf8(output.stdout).unwrap();
  let lines: Vec<&str> = stdout.lines().collect();
  assert_eq!(lines.len(), 3);
  assert!(lines[0].contains("\"schema_version\":15"));
  assert!(lines[1].contains("\"code\":\"DV0003\""));
  assert!(lines[2].contains("\"outcome\":\"failed\""));
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

#[test]
fn sdk_compatible_rids_loads_the_selected_graph_without_inference() {
  let temp = TempDirectory::new();
  fs::create_dir_all(temp.0.join("sdk/10.0.100")).unwrap();
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
  assert!(stderr.contains("pass one project path explicitly"));
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
  assert!(stdout.contains("\"type\":\"command_started\",\"command\":\"restore\""));
  assert!(stdout.contains("\"type\":\"package_resolution_created\""));
  assert!(stdout.contains("\"network_requests\":0"));
  assert!(stdout.contains("\"type\":\"command_finished\",\"command\":\"restore\""));
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
  assert!(!temp.0.join("dv.lock.json").exists());
}

#[test]
fn build_plan_json_reports_framework_and_compiler_inputs() {
  let temp = TempDirectory::new();
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
