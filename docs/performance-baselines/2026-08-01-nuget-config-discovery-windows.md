# NuGet configuration discovery baseline - Windows - 2026-08-01

This baseline promotes `NUGET-001` after effective configuration and package
parity across machine, user, repository, and explicit-file tests.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- warm OS and package caches; setup outside timed intervals
- two machine fragments, two additional-user fragments, one main user file,
  and one repository file

## Commands

```text
dotnet restore ConfigHierarchy.csproj --locked-mode --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet
C:\Projects\dv\target\release\dv.exe restore ConfigHierarchy.csproj --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 532.948 ms | 545.049 ms | 524.718 ms | 547.638 ms |
| `dv` | 5.651 ms | 6.296 ms | 5.209 ms | 6.510 ms |

`dv` was 94.3x faster at the median. Its slowest retained sample was 80.6x
faster than the fastest retained Microsoft sample.

## State Boundary

Both workspaces begin with the same config fragments and repository config.
Setup populates an isolated `.packages` directory and each tool's native lock.
The timed command performs process startup, platform config discovery and
merge, project evaluation, locked validation of one package, and output. It
performs no network request or download.

Preflight compares the selected `.packages` path relative to each workspace,
the effective v3 source, and the exact `Newtonsoft.Json` `13.0.3` package hash.
Fixture preparation, lock construction, and validation are outside timing.

Reproduce:

```powershell
cargo bench-all --case nuget_config_hierarchy --samples 30 --warmups 3
```
