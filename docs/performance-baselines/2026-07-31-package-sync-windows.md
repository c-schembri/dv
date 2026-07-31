# Package Sync Baseline: Windows, 2026-07-31

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- `Newtonsoft.Json` `13.0.3`, 2,441,966-byte package
- `Humanizer` `2.14.1`, 50-package closure, 3,241,550 payload bytes
- release `dv` binary from the working tree based on `0105cd0`

## Like-For-Like Warm Locked Result

Both commands used an isolated populated global-packages directory, an
unchanged project, and a lock created outside the timed interval. Neither
command required a network request. Preflight compared target framework,
package identity, exact version, archive SHA-512, and compile assets.

| Tool | Exact command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| `dotnet` | `dotnet restore PackageConsole.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | 456.544 ms | 481.635 ms | 452.422 ms | 481.635 ms |
| `dv` | `dv restore PackageConsole.csproj --packages .packages --offline --json` | 5.155 ms | 7.208 ms | 4.531 ms | 7.208 ms |

Ten samples were retained after three warm-ups. The median ratio is `88.6x`.
The `dv` timing includes process startup and JSON event reporting.

Reproduce:

```powershell
cargo bench-all --case package_sync_warm --samples 10 --warmups 3
```

## Cold Dependency Readiness

Each timed iteration used a fresh project copy and an empty isolated
global-packages directory. The reference command included `--no-http-cache`, so
neither tool reused package payloads or metadata from an HTTP cache. Preflight
established the same exact dependency graph and selected compile assets.

| Tool | Exact command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| `dotnet` | `dotnet restore PackageConsole.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | 916.034 ms | 955.827 ms | 883.243 ms | 965.211 ms |
| `dv` | `dv restore PackageConsole.csproj --packages .packages --json` | 353.981 ms | 443.317 ms | 343.843 ms | 530.086 ms |

Thirty samples were retained after three warm-ups. The median ratio is `2.6x`.
`dv` reported two HTTP requests and 2,441,966 downloaded payload bytes for
every retained sample. The reference command does not expose typed request or
byte counters.

The result covers v3 service-index discovery, direct exact-version package
download, SHA-512 calculation, embedded nuspec identity validation, bounded
ZIP validation/extraction, and atomic publish. First-restore latency remains
network-sensitive. The benchmark does not claim to reset Windows page cache,
DNS, TLS, or CDN state.

Local stage profiling used ten fresh package directories. Bounded extraction
reduced ZIP validation/extraction from 36.3 ms to 26.5 ms median. Service-index
fetching remained the dominant stage at roughly 250 ms; a persistent,
conditionally revalidated service-index cache is the next recommendation for
the package-cold/metadata-warm state, not for this HTTP-cold result.

Reproduce:

```powershell
cargo bench-all --case package_sync_cold --samples 30 --warmups 3
```

## Cold Large Dependency Graph

The immutable `large-package-graph` fixture has one direct `Humanizer`
`2.14.1` reference. Both preflight restores resolved the same 50 package
identities, exact versions, archive SHA-512 values, target framework, and
selected compile assets. Each timed iteration used a fresh project copy and
empty isolated packages directory; the reference HTTP cache was disabled.

| Tool | Exact command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| `dotnet` | `dotnet restore LargePackageGraph.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | 1243.644 ms | 1433.290 ms | 1223.437 ms | 1433.290 ms |
| `dv` | `dv restore LargePackageGraph.csproj --packages .packages --json` | 562.799 ms | 660.337 ms | 512.396 ms | 660.337 ms |

Ten samples were retained after two warm-ups. The median ratio is `2.2x`.
`dv` reported 50 downloaded packages, 51 HTTP requests, and 3,241,550 payload
bytes in every retained sample. The reference command does not expose typed
work counters.

The modest payload spread across many small archives makes this a
dependency-wave, request-scheduling, validation, extraction, and filesystem
publication case rather than a raw download-throughput test. Network, DNS,
TLS, CDN, Windows page-cache, and endpoint-security variance remain visible.
Replacing global dependency-wave barriers with a bounded streaming queue,
raising concurrency at the measured crossover, and removing redundant staging
I/O reduced the `dv` median by 341.298 ms, or 37.7%, from the previous
904.097 ms result.

Five-sample crossover trials after streaming was enabled measured 636.038 ms
at eight workers, 546.024 ms at sixteen, and 546.823 ms at twenty-four.
Sixteen retained the full throughput gain without the eight additional
threads. The final ten-sample sixteen-worker result above is the promoted
baseline; the shorter trials are directional crossover evidence only.

Before that scheduler change, temporary release-build instrumentation captured
a 920.026 ms cold process sample close to the then-current 904.097 ms median.
Its additive wall-time critical path was:

| Stage | Wall time |
|---|---:|
| Process entry, argument parsing, and project evaluation | 1.092 ms |
| Resolver setup and loop overhead | 1.670 ms |
| NuGet v3 service-index discovery | 255.814 ms |
| Wave 0: download, verify, extract, and publish the root meta-package | 37.318 ms |
| Parse the root manifest and expand dependencies | 0.197 ms |
| Wave 1: process 48 independent packages with four workers | 531.790 ms |
| Parse the 48 manifests and merge graph edges | 16.007 ms |
| Wave 2: process the final discovered package | 61.535 ms |
| Parse the final manifest | 0.956 ms |
| Validate/materialize the graph and write the lock | 3.612 ms |
| JSON reporting and process exit | 10.016 ms |

The package workers overlap download, extraction, and publication. Across the
three waves they accumulated 2,139.697 ms of worker time inside 630.643 ms of
wall time: 1,036.939 ms downloading, hashing, writing, and flushing archives;
897.780 ms validating and extracting ZIPs; and 204.978 ms validating metadata
and atomically publishing cache entries. These worker totals describe work
composition and must not be added to the wall-time table.

Reproduce:

```powershell
cargo bench-all --case package_graph_cold --samples 10 --warmups 2
```
