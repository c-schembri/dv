# Framework Reference Plan Baseline - Windows - 2026-08-01

This baseline promotes `PACKS-007` after `dv` matched Microsoft SDK framework
references, requested runtime versions, targeting packs, and actual shared
framework selection for a .NET 10 application using ASP.NET Core.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- `Microsoft.NETCore.App` and `Microsoft.AspNetCore.App` `10.0.0`
- two resolved framework rows
- 30 retained samples after 3 warm-ups
- warm OS file caches; restore, host-oracle build, and Rust builds outside timing

## Commands

```text
dotnet msbuild FrameworkReferenceProject.csproj --nologo -t:ResolveTargetingPackAssets -getProperty:TargetFramework,RollForward,SelfContained -getItem:RuntimeFramework,ResolvedFrameworkReference
C:\Projects\dv\target\release\dv.exe project frameworks FrameworkReferenceProject.csproj --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 352.715 ms | 390.432 ms | 333.005 ms | 392.531 ms |
| `dv` | 5.585 ms | 6.530 ms | 4.820 ms | 7.264 ms |

`dv` was `63.2x` faster at the median.

## Parity Gate

The harness restores outside timing and compares project properties plus every
resolved framework's identity, runtime name/version/profile, and targeting
pack identity/version/root. It also builds and launches the fixture outside
timing, then checks that `dv` selected the same installed Core and ASP.NET
shared-framework versions as the Microsoft host.

Reproduce:

```powershell
cargo bench-all --case framework_reference_plan --samples 30 --warmups 3
```
