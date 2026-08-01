# Named project selection baseline - Windows - 2026-08-01

This baseline promotes `CLI-008` after every current project-bearing command
was routed through one borrowed project/solution selector. The common zero- or
one-path parse performs no path allocation; restore allocates a tail batch only
for a second positional root.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after 3 warm-ups
- warm OS file caches; release builds and parity checks outside timed intervals
- default Cargo compiler concurrency

## Commands

```text
dotnet msbuild SmallConsole.csproj --nologo -getProperty:TargetFramework,OutputType,Nullable,ImplicitUsings,AssemblyName,RootNamespace,Configuration,Deterministic -getItem:Compile,ProjectReference,PackageReference
C:\Projects\dv\target\release\dv.exe project inspect --project SmallConsole.csproj --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 328.778 ms | 502.383 ms | 282.750 ms | 685.852 ms |
| `dv` | 6.204 ms | 8.214 ms | 5.133 ms | 8.749 ms |

`dv` was 53.0x faster at the median.

## Parity gate

Before timing, the harness evaluates the same explicit `SmallConsole.csproj`
with the .NET 10 MSBuild property/item query and `dv --project`. It compares
target framework, output type, nullable and implicit-usings policy, assembly
and namespace names, configuration, determinism, ordered compile items, and
the empty project/package-reference batches. Any mismatch rejects the run.

Focused CLI tests additionally cover the no-argument current-directory
default, explicit directories, `--project=PATH`, lossless non-Unicode paths,
repeated and mixed selector rejection before I/O, restore's positional batch,
and the explicit unsupported boundary for `.sln` until Wave 7.

Reproduce:

```powershell
cargo bench-all --case project_select_named --samples 30 --warmups 3
```
