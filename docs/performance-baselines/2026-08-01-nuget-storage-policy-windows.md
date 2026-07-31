# NuGet storage-policy baseline - Windows - 2026-08-01

This baseline promotes `NUGET-004` after official storage, signature, proxy,
audit-property, package-folder, and package parity.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- warm OS and package caches; setup outside timed intervals
- one machine fragment, one main user file, and one repository file

## Commands

```text
dotnet restore StoragePolicy.csproj --locked-mode --no-http-cache --nologo --verbosity quiet
C:\Projects\dv\target\release\dv.exe restore StoragePolicy.csproj --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 523.051 ms | 605.407 ms | 515.044 ms | 688.535 ms |
| `dv` | 5.370 ms | 6.526 ms | 4.945 ms | 6.642 ms |

`dv` was 97.4x faster at the median. Its slowest retained sample was 77.5x
faster than the fastest retained Microsoft sample.

## State Boundary

Both workspaces begin with the same three-level configuration and a matching
tool-native lock. Setup places `Newtonsoft.Json` `13.0.3` only in the selected
fallback folder and removes the writable global cache. The timed command
performs process startup, project evaluation, policy discovery and merge,
fallback lookup, locked package validation, asset materialization, and output.
It performs no network request or package download.

Outside timing, an adapter built against the selected SDK's
`NuGet.Configuration.dll` verifies the effective global-packages, fallback,
HTTP-cache, scratch, signature-validation, proxy, and bypass values. An MSBuild
query verifies audit enabled state, mode, and level. Preflight then compares
Microsoft's package-folder list with `dv`'s retained roots and compares the
resolved package identity, version, SHA-512, and fallback-selected compile
asset.

Reproduce:

```powershell
cargo bench-all --case nuget_storage_policy --samples 30 --warmups 3
```
