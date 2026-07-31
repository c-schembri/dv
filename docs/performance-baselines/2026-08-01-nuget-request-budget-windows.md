# NuGet Request Budget Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86_64, 24 logical CPUs.
- Samples: 30 retained samples after 3 warm-ups.
- Fixture: one `net10.0` project with six exact, dependency-free package
  references and an empty isolated package cache per iteration.
- Feeds: the harness seeds identical package archives once from NuGet.org, then
  serves them from two loopback V3 flat-container feeds. Every response waits
  25 ms; public-network time is outside the measured region.
- Budgets: both processes receive `NUGET_CONCURRENCY_LIMIT=4`; generated
  `NuGet.Config` sets `maxHttpRequestsPerSource=2` and maps three identities to
  each source.
- Enforcement: every retained sample must contact both sources, observe at
  least two combined active requests without exceeding either configured limit,
  and publish all six package archives byte-for-byte.
- Work: Microsoft issued at most 17 requests and `dv` at most 10. Both
  materialized and byte-compared the same six archives totaling 5,929,224
  bytes; request counts differ because the clients do not make identical
  metadata requests.

## Commands

```text
dotnet restore RequestBudget.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet
dv restore RequestBudget.csproj --packages .packages --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 3109.409 ms | 5249.874 ms | 2967.974 ms | 7358.092 ms |
| `dv` | 247.157 ms | 1178.249 ms | 199.201 ms | 2150.188 ms |

`dv` is 12.6x faster by median. This measures process launch, project and
configuration discovery, bounded service discovery, exact package fetch,
archive verification/extraction, and asset planning against deterministic local
latency. It is not a public-network throughput result.
