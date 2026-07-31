# NuGet keyed configuration merge baseline - Windows - 2026-08-01

This baseline promotes `NUGET-002` after keyed merge and effective package
parity across machine, additional-user, user, and repository settings.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- warm OS and package caches; setup outside timed intervals
- four config files, 1,632 XML bytes, and 18 keyed operations

## Commands

```text
dotnet restore ConfigMerge.csproj --locked-mode --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet
C:\Projects\dv\target\release\dv.exe restore ConfigMerge.csproj --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 558.126 ms | 648.298 ms | 523.665 ms | 705.516 ms |
| `dv` | 9.422 ms | 10.606 ms | 8.546 ms | 11.609 ms |

`dv` was 59.2x faster at the median. Its slowest retained sample was 45.1x
faster than the fastest retained Microsoft sample.

## State Boundary

Both workspaces begin with the same four-file configuration hierarchy. Setup
populates an isolated environment-selected `.packages` directory and each
tool's native lock. The timed command performs process startup, platform config
discovery, XML parsing, keyed merge, environment expansion, project evaluation,
locked validation of one package, and output. It performs no network request
or download.

Preflight compares the effective v3 source, excluded source set, package path,
and exact `Newtonsoft.Json` `13.0.3` package hash. Fixture preparation, lock
construction, and parity validation are outside timing.

Reproduce:

```powershell
cargo bench-all --case nuget_config_merge --samples 30 --warmups 3
```
