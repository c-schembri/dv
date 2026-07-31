# Runtime target evaluation baseline - Windows - 2026-08-01

This baseline promotes `EVAL-022` after `dv` matched the .NET 10 SDK's
`TargetFramework`, `RuntimeIdentifier`, and `RuntimeIdentifiers` values and
materialized the same unique runtime target dimensions.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after 3 warm-ups
- warm OS file caches; compiler build work outside timed intervals
- Cargo compiler concurrency restricted to one job

## Commands

```text
dotnet msbuild RuntimeProject.csproj --nologo -getProperty:TargetFramework,RuntimeIdentifier,RuntimeIdentifiers
C:\Projects\dv\target\release\dv.exe project inspect RuntimeProject.csproj --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 321.215 ms | 330.112 ms | 315.290 ms | 334.574 ms |
| `dv` | 5.687 ms | 6.897 ms | 4.841 ms | 6.946 ms |

`dv` was 56.5x faster at the median.

## Parity gate

Before timing, the harness compares the evaluated target framework and
selected RID directly, splits the Microsoft `RuntimeIdentifiers` property into
its ordered values, and verifies both the plural property batch and the unique
target-dimension batch emitted by `dv`. A mismatch rejects the entire run.

Reproduce:

```powershell
cargo bench-all --case runtime_evaluate --samples 30 --warmups 3
```
