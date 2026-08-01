# Explicit Project Selection Baseline - Windows - 2026-08-02

This like-for-like baseline measures `WS-002` direct explicit-file selection
as part of project evaluation. Both commands evaluate the same immutable
`SmallConsole.csproj` and produce the same requested property and item batch.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- release binaries and maximum Cargo compiler concurrency
- 30 retained samples after 3 warm-ups; warm OS caches

## Commands

```text
dotnet msbuild SmallConsole.csproj --nologo -getProperty:TargetFramework,OutputType,Nullable,ImplicitUsings,AssemblyName,RootNamespace,Configuration,Deterministic -getItem:Compile,ProjectReference,PackageReference
C:\Projects\dv\target\release\dv.exe project inspect --project SmallConsole.csproj --json
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 289.046 ms | 302.358 ms | 282.437 ms | 303.880 ms |
| `dv` | 5.326 ms | 6.219 ms | 4.631 ms | 6.490 ms |

`dv` is **54.3x faster** at the median. The timed interval includes process
startup, borrowed option parsing, one project metadata query, project reading,
source discovery, evaluation, JSON serialization, and output capture. Fixture
setup and exact semantic comparison run outside timing, and neither command
mutates the fixture.

Candidate-shaped wrong-kind paths are not part of the timed success case.
Focused tests prove that they fail before filesystem I/O; missing, non-regular,
and unreadable C# paths fail before XML parsing.

Reproduce:

```powershell
cargo bench-all --case project_select_named --samples 30 --warmups 3 --output benchmarks/results/2026-08-02-explicit-project-selection-windows.json
```
