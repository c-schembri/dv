# Workspace Selection Baseline - Windows - 2026-08-02

This like-for-like baseline measures `WS-003` directory selection as part of
implicit project evaluation. Both commands start in the same immutable
`small-console` directory, select its only project without an explicit path,
and produce the same requested property and item batch.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- one immediate `.csproj` candidate
- release binaries and maximum Cargo compiler concurrency
- 30 retained samples after 3 warm-ups; warm OS caches

## Commands

```text
dotnet msbuild --nologo -getProperty:TargetFramework,OutputType,Nullable,ImplicitUsings,AssemblyName,RootNamespace,Configuration,Deterministic -getItem:Compile,ProjectReference,PackageReference
C:\Projects\dv\target\release\dv.exe project inspect --json
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 303.399 ms | 320.158 ms | 284.776 ms | 322.325 ms |
| `dv` | 6.051 ms | 7.540 ms | 5.205 ms | 7.664 ms |

`dv` is **50.1x faster** at the median. The timed interval includes process
startup, one immediate candidate scan, typed selection, project reading,
source discovery, evaluation, JSON serialization, and output capture. Fixture
setup and semantic validation run outside timing.

The preflight compares the complete requested MSBuild property/item batch. It
also validates typed, ordered, zero-write `dv` failures for empty and ambiguous
directories; these failure-only cases are not mixed into the success timing.

Reproduce:

```powershell
cargo bench-all --case workspace_discovery --samples 30 --warmups 3 --output benchmarks/results/2026-08-02-workspace-selection-windows.json
```
