# Runtime Pack Plan Baseline - Windows - 2026-08-01

This baseline promotes `PACKS-006` after `dv` matched Microsoft SDK runtime
pack, runtime asset, host pack, and apphost selection for a self-contained
`net10.0` `win-x64` executable.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- runtime pack `Microsoft.NETCore.App.Runtime.win-x64` `10.0.0`
- host pack `Microsoft.NETCore.App.Host.win-x64` `10.0.0`
- 172 managed and 15 native runtime assets
- 30 retained samples after 3 warm-ups
- warm OS file caches; fixture restore and Rust builds outside timing

## Commands

```text
dotnet msbuild RuntimePackProject.csproj --nologo -p:SelfContained=true -p:UseAppHost=true "-t:ProcessFrameworkReferences;ResolveFrameworkReferences;ResolveRuntimePackAssets;_GetAppHostPaths" -getProperty:RuntimeIdentifier,AppHostSourcePath -getItem:ResolvedRuntimePack,RuntimePackAsset,ResolvedAppHostPack
C:\Projects\dv\target\release\dv.exe project runtime-packs RuntimePackProject.csproj --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 376.764 ms | 416.959 ms | 350.505 ms | 432.781 ms |
| `dv` | 8.030 ms | 9.425 ms | 6.927 ms | 9.649 ms |

`dv` was `46.9x` faster at the median.

## Parity Gate

The fixture is restored for `win-x64` outside the timed interval. Before
sampling, the harness compares the requested RID, selected runtime RID, runtime
pack identity/version/root, every ordered managed and native runtime asset,
selected host RID/root, resolved apphost item, and `AppHostSourcePath`.

Reproduce:

```powershell
cargo bench-all --case runtime_pack_plan --samples 30 --warmups 3
```
