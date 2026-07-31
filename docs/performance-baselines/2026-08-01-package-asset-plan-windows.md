# Package asset plan baseline - Windows - 2026-08-01

This baseline promotes `PACKS-008` after `dv` matched every portable package
asset family selected by the .NET SDK for the 203-package acceptance graph.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- populated isolated package caches and matching tool-native locks
- warm OS caches; background Windows scheduling was not controlled

## Commands

```text
dotnet restore MassivePackageGraph.csproj --locked-mode --packages .packages -p:NuGetAudit=false --nologo --verbosity quiet
C:\Projects\dv\target\release\dv.exe restore MassivePackageGraph.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 702.904 ms | 1301.984 ms | 527.886 ms | 1390.525 ms |
| `dv` | 107.385 ms | 132.369 ms | 92.990 ms | 138.166 ms |

`dv` was 6.5x faster at the median. Every retained `dv` sample resolved 203
packages, downloaded zero packages, issued zero HTTP requests, and processed
zero network payload bytes. Even with substantial host variance, the slowest
`dv` sample was faster than the fastest reference sample.

## Parity gate

Before timing, isolated cold restores compare target framework; all 203
package identities, normalized versions, and archive SHA-512 values; compile,
runtime, analyzer, resource, content, build, build-multitargeting,
build-transitive, and native asset paths; plus every RID-specific runtime
target path, RID, and kind. The timed commands then prove the same locked graph
from populated caches.

The retained plan stores all nine portable families in one span allocation.
Its 72-byte range table points into 8-byte spans, and the
`PackageResolution` header is 248 bytes with pointer alignment. This replaces
nine persistent asset allocations and removes 56 header bytes. Warm planning
checks one immutable completion marker per package rather than issuing a file
metadata request for every selected asset; concrete consumers validate assets
when opening or copying them.

Reproduce:

```powershell
cargo bench-all --case package_asset_plan --samples 30 --warmups 3
```
