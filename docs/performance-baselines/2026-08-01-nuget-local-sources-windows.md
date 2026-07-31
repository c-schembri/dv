# NuGet local-source baseline - Windows - 2026-08-01

This baseline promotes `NUGET-006` after flat-folder, hierarchical-feed,
source-mapping, package identity/version/hash, and zero-network parity.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- warm OS page cache; generated local feeds retained outside timed intervals
- cold global package cache and restore outputs before every timed command

## Commands

```text
dotnet restore LocalSources.csproj --packages .packages --no-http-cache --nologo --verbosity quiet
C:\Projects\dv\target\release\dv.exe restore LocalSources.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 670.534 ms | 694.282 ms | 653.217 ms | 701.139 ms |
| `dv` | 64.522 ms | 97.332 ms | 55.270 ms | 128.919 ms |

`dv` was 10.4x faster at the median. Its slowest retained sample was 5.1x
faster than the fastest retained Microsoft sample.

## State Boundary

Setup materializes the same two public package archives into one flat local
feed and one hierarchical feed. Timed work includes process startup, project
and configuration evaluation, source-layout discovery, mapped graph
resolution, 2,980,145 source bytes, SHA-512 validation, ZIP extraction, atomic
publication of two packages, asset selection, and output. It performs no HTTP
request and starts with no global package entry or prior restore output.

Preflight reads Microsoft's `project.assets.json`, requires both configured
local source paths, and compares the selected `Humanizer.Core` `2.14.1` and
`Newtonsoft.Json` `13.0.3` identities, versions, and archive hashes with `dv`'s
structured event.

Reproduce:

```powershell
cargo bench-all --case nuget_local_sources --samples 30 --warmups 3
```
