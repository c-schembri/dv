# Workspace Discovery Baseline - Windows - 2026-08-02

This like-for-like baseline measures the `WS-001` immediate workspace scan as
part of implicit project selection and evaluation. Both commands start in the
same immutable `small-console` directory, select its only project without an
explicit path, and produce the same requested property and item batch.

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
| Microsoft | 290.493 ms | 305.210 ms | 281.904 ms | 307.926 ms |
| `dv` | 5.287 ms | 6.048 ms | 4.511 ms | 6.258 ms |

`dv` is **55.0x faster** at the median. The timed interval includes process
startup, immediate candidate discovery, project parsing, source discovery,
evaluation, and output capture. Fixture setup and the exact semantic preflight
run outside timing, and the fixture tree is unchanged by both commands.

The public discovery transform itself performs one root metadata query, one
directory enumeration, and one file-type query per entry. It reads no candidate
contents, does not build a full path for irrelevant entries, launches no
process, and performs no network operation.

Reproduce:

```powershell
cargo bench-all --case workspace_discovery --samples 30 --warmups 3 --output benchmarks/results/2026-08-02-workspace-discovery-windows.json
```
