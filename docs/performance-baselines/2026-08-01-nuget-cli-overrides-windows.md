# NuGet CLI override baseline - Windows - 2026-08-01

This baseline promotes `NUGET-005` after CLI source, explicit-config, package
folder, package identity, version, and hash parity.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- warm OS and package caches; setup outside timed intervals

## Commands

```text
dotnet restore CliOverrides.csproj --locked-mode --source https://api.nuget.org/v3/index.json --configfile config/selected.config --packages policy/cli-global --no-http-cache --nologo --verbosity quiet
C:\Projects\dv\target\release\dv.exe restore CliOverrides.csproj --source https://api.nuget.org/v3/index.json --configfile config/selected.config --packages policy/cli-global --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 524.597 ms | 548.166 ms | 510.131 ms | 551.664 ms |
| `dv` | 5.103 ms | 5.986 ms | 4.760 ms | 6.619 ms |

`dv` was 102.8x faster at the median. Its slowest retained sample was 77.1x
faster than the fastest retained Microsoft sample.

## State Boundary

The implicit config, selected explicit config, and `NUGET_PACKAGES` each name
conflicting sources or package folders. Both timed commands select the same
explicit config, NuGet v3 source, CLI package folder, populated one-package
cache, and matching native lock. Timed work includes process startup, project
evaluation, explicit config parsing, CLI precedence application, locked
package validation, asset materialization, and output. It performs no network
request or package download.

Preflight reads Microsoft's `project.assets.json` and package metadata to
verify source, config-file isolation, package folder, identity, version, and
SHA-512. It requires the lower-precedence environment and config folders to
remain unused, then checks the same values in `dv`'s structured event.

Reproduce:

```powershell
cargo bench-all --case nuget_cli_overrides --samples 30 --warmups 3
```
