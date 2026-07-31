# NuGet service-index baseline - Windows - 2026-08-01

This baseline promotes `NUGET-007` after registration, flat-container, search,
vulnerability, and package-publish resource-selection parity.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- warm OS caches; oracle compilation occurs outside timed intervals
- one fresh live HTTPS service-index request in every timed process

## Commands

```text
dotnet oracle/bin/Release/ServiceIndexOracle.dll https://api.nuget.org/v3/index.json
C:\Projects\dv\target\release\dv.exe project package-sources ServiceIndex.csproj --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 344.113 ms | 868.499 ms | 332.405 ms | 879.515 ms |
| `dv` | 277.336 ms | 289.483 ms | 262.994 ms | 295.994 ms |

`dv` was 1.24x faster at the median. Its slowest retained sample remained
faster than the fastest retained Microsoft sample.

## State Boundary

The reference adapter performs one uncached `HttpClient` request, parses the
response with `Newtonsoft.Json`, and selects endpoints through the selected
SDK's official `NuGet.Protocol` implementation. The `dv` command performs the
same request and endpoint selection, plus project evaluation and the effective
`NuGet.Config` hierarchy. Its retained samples reported one request and at
most 9,272 response bytes.

Preflight compares every selected URI for all five capabilities. It exercises
NuGet.Client's ordered service types and explicit client version `7.0.0`; core
fixtures additionally cover array-valued resource types, equivalent endpoints,
future and prerelease client versions, invalid URIs, unsupported index schemas,
and rejected insecure resources.

This is a live-network comparison. DNS, TLS, CDN, and route conditions are
shared only approximately, so the claim applies to the paired run rather than
unrelated runs on different network conditions.

Reproduce:

```powershell
cargo bench-all --case nuget_service_index --samples 30 --warmups 3
```
