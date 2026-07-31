# NuGet source-section baseline - Windows - 2026-08-01

This baseline promotes `NUGET-003` after official source, disabled-state,
protocol, audit-source, mapping, package-directory, and package parity.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- warm OS and package caches; setup outside timed intervals
- one machine fragment, one additional-user fragment, one main user file, and
  one repository file

## Commands

```text
dotnet restore SourceSections.csproj --locked-mode --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet
C:\Projects\dv\target\release\dv.exe restore SourceSections.csproj --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 527.659 ms | 537.783 ms | 517.007 ms | 540.912 ms |
| `dv` | 5.850 ms | 8.229 ms | 4.935 ms | 8.574 ms |

`dv` was 90.2x faster at the median. Its slowest retained sample was 60.3x
faster than the fastest retained Microsoft sample.

## State Boundary

Both workspaces begin with the same four-level source-policy configuration.
Setup populates the environment-selected `.packages` directory and each tool's
native lock. The timed command performs process startup, configuration
discovery, typed source/audit/mapping merge, project evaluation, locked
validation of one package, and output. It performs no network request or
download.

Outside timing, an adapter built against the selected SDK's
`NuGet.Configuration.dll` verifies three final named package sources, enabled
state, v2/v3 protocols, one audit source, three positive mapping queries, and
one cleared mapping query. An enabled v2 decoy precedes the selected v3 source
in `dv`; restore preflight reads Microsoft's `.nupkg.metadata` and the `dv`
event to prove mapping selected v3 before comparing the relative cache root,
`Newtonsoft.Json` identity/version, and archive SHA-512.

Reproduce:

```powershell
cargo bench-all --case nuget_source_sections --samples 30 --warmups 3
```
