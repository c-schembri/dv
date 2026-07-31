# Unavailable pack diagnostic baseline - Windows - 2026-08-01

This baseline promotes `PACKS-009` after both tools named the same unavailable
runtime pack and `dv` emitted the complete typed acquisition requirement.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- fresh fixture copy and empty isolated package cache before every sample
- checked-in empty local package source; no network requests
- warm OS caches; background Windows scheduling was not controlled

## Commands

```text
dotnet restore UnavailablePackProject.csproj --source offline-source --packages .packages --no-cache --disable-build-servers -p:NuGetAudit=false --nologo --verbosity minimal
C:\Projects\dv\target\release\dv.exe project runtime-packs UnavailablePackProject.csproj --packages .packages --json
```

Both commands are expected to fail. Their expected-failure status and semantic
diagnostic output are validated after every timed process.

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 532.652 ms | 596.360 ms | 521.626 ms | 603.630 ms |
| `dv` | 6.378 ms | 6.931 ms | 5.718 ms | 7.155 ms |

`dv` was 83.5x faster at the median. Its slowest retained sample was 72.9x
faster than the fastest reference sample.

## Parity Gate

The immutable fixture targets `net10.0`, selects the SDK-recognized
`linux-arm` RID, and requests a self-contained application. An empty local
source and isolated cache make pack absence deterministic without measuring
network behavior.

Microsoft restore must emit `NU1101` and name
`Microsoft.NETCore.App.Runtime.linux-arm`. `dv` must emit `DV0124`, the same
identity, exact version `10.0.0`, TFM `net10.0`, RID `linux-arm`, kind
`runtime_pack`, action `restore_package`, and stable human guidance. Fixture
copying and release compilation occur outside the timed interval.

Reproduce:

```powershell
cargo bench-all --case pack_diagnostic --samples 30 --warmups 3
```
