# Package Sync Baseline: Windows, 2026-07-31

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- `Newtonsoft.Json` `13.0.3`, 2,441,966-byte package
- `Humanizer` `2.14.1`, 50-package closure, 3,241,550 payload bytes
- release `dv` binary with the bounded Tokio package scheduler following
  scoped-worker baseline commit `4060eaf`

## Like-For-Like Warm Locked Result

Both commands used an isolated populated global-packages directory, an
unchanged project, and a lock created outside the timed interval. Neither
command required a network request. Preflight compared target framework,
package identity, exact version, archive SHA-512, and compile assets.

| Tool | Exact command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| `dotnet` | `dotnet restore PackageConsole.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | 498.177 ms | 508.203 ms | 491.961 ms | 508.203 ms |
| `dv` | `dv restore PackageConsole.csproj --packages .packages --offline --json` | 5.881 ms | 7.601 ms | 5.030 ms | 7.601 ms |

Ten samples were retained after three warm-ups. The median ratio is `84.7x`.
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
| `dotnet` | `dotnet restore PackageConsole.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | 994.180 ms | 1048.203 ms | 971.725 ms | 1053.043 ms |
| `dv` | `dv restore PackageConsole.csproj --packages .packages --json` | 400.814 ms | 634.719 ms | 375.208 ms | 732.690 ms |

Thirty samples were retained after three warm-ups. The median ratio is `2.5x`.
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
| `dotnet` | `dotnet restore LargePackageGraph.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | 1350.559 ms | 1401.944 ms | 1318.197 ms | 1401.944 ms |
| `dv` | `dv restore LargePackageGraph.csproj --packages .packages --json` | 581.450 ms | 641.654 ms | 546.617 ms | 641.654 ms |

Ten samples were retained after two warm-ups. The median ratio is `2.3x`.
`dv` reported 50 downloaded packages, 51 HTTP requests, and 3,241,550 payload
bytes in every retained sample. The reference command does not expose typed
work counters.

The modest payload spread across many small archives makes this a
dependency-wave, request-scheduling, validation, extraction, and filesystem
publication case rather than a raw download-throughput test. Network, DNS,
TLS, CDN, Windows page-cache, and endpoint-security variance remain visible.
The earlier scoped-worker implementation replaced global dependency-wave
barriers with a bounded streaming queue and removed redundant staging I/O. It
reduced the `dv` median by 341.298 ms, or 37.7%, from 904.097 ms to 562.799 ms.

Five-sample crossover trials after streaming was enabled measured 636.038 ms
at eight workers, 546.024 ms at sixteen, and 546.823 ms at twenty-four.
Sixteen retained the full throughput gain without the eight additional
threads. The final ten-sample sixteen-worker result above is the promoted
baseline; the shorter trials are directional crossover evidence only.

### Tokio scheduler comparison

Commit `4060eaf` preserves the scoped-thread baseline. The current
implementation replaces blocking HTTP workers with Reqwest on a two-thread
Tokio runtime, caps active package tasks at twenty-four, and sends ZIP work to
`spawn_blocking`.

Under the earlier low-latency network state, the final ten-sample Tokio graph
median was 567.682 ms versus the 562.799 ms scoped median. This is a 4.883 ms
or 0.9% regression and is within network variance. The promoted Tokio run
above later measured 581.450 ms under another low-latency network window. The
required async-file handoff flush was present in both runs.

When the CDN later became congested, sequential runs were not comparable.
An alternating A/B used the exact `4060eaf` executable and the Tokio
executable, fresh project and package directories for every invocation, two
warm-up pairs, six retained pairs, and reversed executable order each pair.
The 50-package scoped median was 2076.227 ms and the Tokio median was
1318.214 ms: Tokio reduced the median by 758.013 ms or 36.5%. An identical
one-package A/B measured 959.830 ms scoped and 951.363 ms Tokio, effectively
equal at this variance.

The Tokio release executable is 3,991,040 bytes versus 2,607,616 bytes for
`4060eaf`, an increase of 1,383,424 bytes or 53.1%. The experiment therefore
shows a real high-wait, many-download throughput benefit but no low-latency
win, plus a substantial dependency and binary-size cost. These results are
comparison evidence, not a replacement for the promoted README baseline.

## Massive eShop-derived acceptance graph

The `massive-package-graph` fixture unions 51 direct package references from
Microsoft eShop commit
`9b4f9434f46fdc5c1a6e9e936af2868340cdbc48` into one `net10.0` project. It
includes Aspire hosting and integrations, ASP.NET Core, EF Core and Npgsql,
resilience and service discovery, OpenTelemetry, Duende IdentityServer, gRPC,
validation, mediation, and test infrastructure. A single union project
isolates package resolution because `dv` cannot yet batch a complete
solution/project-reference closure.

The reference command was:

```text
dotnet restore MassivePackageGraph.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet
```

Every iteration used a fresh project copy and empty isolated package
directory. Audit queries were disabled so vulnerability-service access was
not mixed into package-graph timing.

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 10079.053 ms | 11241.686 ms | 9502.920 ms | 11241.686 ms |
| `dv` | TBI | - | - | - |

Five samples were retained after one warm-up. `project.assets.json` contained
203 selected packages. The fresh global-packages directory contained 272
downloaded package archives totaling 197,860,237 bytes; the reference command
does not expose HTTP request count.

`dv` currently stops on two correctness boundaries before this graph can be
timed honestly:

- minimum dependency ranges such as `Microsoft.Extensions.Http >= 10.0.0`
  and `>= 10.0.5` are treated as conflicting exact versions instead of
  converging under NuGet's resolution rules;
- packages containing `build`, `buildTransitive`, `buildMultiTargeting`, or
  RID-specific `runtimes` assets remain outside the initial asset contract.

The harness therefore prints the intended `dv restore` command and `TBI`
rather than weakening the fixture or reporting partial-download time.

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
