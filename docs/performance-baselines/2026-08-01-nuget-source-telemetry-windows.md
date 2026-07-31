# NuGet Source Telemetry Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86_64, 24 logical CPUs.
- Samples: 30 retained samples after 3 warm-ups.
- Fixture: one `net10.0` project with six exact, dependency-free references,
  an empty isolated cache, and two loopback V3 feeds with a fixed 25 ms delay.
- Work parity: both tools publish the same six byte-identical archives totaling
  5,929,224 bytes. Microsoft made at most 17 HTTP requests; `dv` made at most
  10 because their metadata request shapes differ.
- Telemetry gate: each `dv` sample must report two configuration-ordered source
  rows, nonzero request/byte/duration values, aggregate sums equal to the
  loopback servers' observations, and six package cache misses. Source URLs
  and query credentials are forbidden from reporter output.
- Warm coverage: the existing `package_sync_warm` case requires zero HTTP work,
  zeroed source rows, and package cache hits from a matching lock.

## Commands

```text
dotnet restore RequestBudget.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet
dv restore RequestBudget.csproj --packages .packages --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 3067.502 ms | 5257.469 ms | 2952.403 ms | 7152.263 ms |
| `dv` | 232.130 ms | 1179.197 ms | 178.792 ms | 2146.441 ms |

`dv` is 13.2x faster by median. This measures cold package readiness plus
source telemetry against deterministic local latency, not public-network
throughput.
