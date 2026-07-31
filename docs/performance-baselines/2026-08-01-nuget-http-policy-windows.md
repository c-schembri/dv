# NuGet HTTP Policy Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86_64, 24 logical CPUs.
- Samples: 30 retained samples after 3 warm-ups.
- Fixture: one HTTPS v3 source, explicit proxy and bypass list, per-source limit
  7, and custom values for all five enhanced retry environment controls.
- Oracle: the selected .NET SDK's `NuGet.Configuration` and `NuGet.Protocol`
  assemblies.
- Timed state: network disabled; oracle build is outside the timed region.
- Preflight: both tools select the same eleven retry, timeout, rate-limit, and
  redacted proxy fields. `dv` additionally proves TLS validation, secure
  ten-hop redirects, offline state, and zero HTTP requests.

## Commands

```text
dotnet oracle/bin/Release/HttpPolicyOracle.dll .
dv project package-sources HttpPolicyProject.csproj --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 78.286 ms | 81.730 ms | 73.587 ms | 91.066 ms |
| `dv` | 6.934 ms | 8.059 ms | 5.430 ms | 8.063 ms |

`dv` is 11.3x faster by median. This isolates policy discovery, proxy/client
construction, and structured output; it does not claim a network transfer or
TLS-handshake speedup.
