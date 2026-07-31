# NuGet credential baseline - Windows - 2026-08-01

This baseline promotes `NUGET-008` after config/environment credential
selection parity and plaintext-containment verification.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- warm OS caches; oracle compilation occurs outside timed intervals
- two HTTPS source rows and zero timed network requests

## Commands

```text
dotnet oracle/bin/Release/CredentialOracle.dll .
C:\Projects\dv\target\release\dv.exe project package-sources CredentialProject.csproj --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 73.624 ms | 75.971 ms | 72.002 ms | 76.545 ms |
| `dv` | 4.615 ms | 5.388 ms | 3.955 ms | 5.449 ms |

`dv` was 16.0x faster at the median. Its slowest retained sample was 13.2x
faster than the fastest retained Microsoft sample.

## State Boundary

The reference adapter loads the fixture with the selected SDK's official
`NuGet.Configuration` assembly and proves that one environment credential and
one config-only credential contain the expected values. It emits only source
identity, location, protocol, authentication kind, and a Boolean selection
proof. The `dv` command evaluates the project, discovers `NuGet.Config`,
materializes the same source credential policy, and reports the effective
source batch offline.

Preflight compares both source rows and rejects every fixture username,
password, PAT, and decoy value in either tool's stdout or stderr. `dv` must
also report zero requests, zero response bytes, and no discovered endpoints.
Core tests separately verify the actual sensitive Basic header and its
same-origin request boundary.

The release executable grew from 4,953,600 to 5,007,872 bytes, a 54,272-byte
(1.10%) cost for zeroization and Windows DPAPI support. A quick SDK-selection
regression check measured `dv` at 3.750 ms median.

Reproduce:

```powershell
cargo bench-all --case nuget_credentials --samples 30 --warmups 3
```
