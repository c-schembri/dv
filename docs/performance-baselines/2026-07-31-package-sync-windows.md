# Package Sync Baseline: Windows, 2026-07-31

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- `Newtonsoft.Json` `13.0.3`, 2,441,966-byte package
- release `dv` binary from the working tree based on `c23f2b3`

## Like-For-Like Warm Locked Result

Both commands used an isolated populated global-packages directory, an
unchanged project, and a lock created outside the timed interval. Neither
command required a network request. Preflight compared target framework,
package identity, exact version, archive SHA-512, and compile assets.

| Tool | Exact command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| `dotnet` | `dotnet restore PackageConsole.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | 454.249 ms | 456.167 ms | 451.008 ms | 456.167 ms |
| `dv` | `dv restore PackageConsole.csproj --packages .packages --offline --json` | 4.469 ms | 4.954 ms | 4.300 ms | 4.954 ms |

Ten samples were retained after three warm-ups. The median ratio is `101.6x`.
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
| `dotnet` | `dotnet restore PackageConsole.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | 910.292 ms | 952.869 ms | 885.639 ms | 952.869 ms |
| `dv` | `dv restore PackageConsole.csproj --packages .packages --json` | 803.122 ms | 1,926.573 ms | 793.164 ms | 1,926.573 ms |

Five samples were retained after one warm-up. The median ratio is `1.1x`.
`dv` reported four HTTP requests and 2,441,966 downloaded payload bytes for
every retained sample. The reference command does not expose typed request or
byte counters.

The result covers v3 discovery, metadata, a streamed download, SHA-512
verification, bounded ZIP validation/extraction, and atomic publish. It is
deliberately retained despite a 1,926.573 ms `dv` outlier: first-restore
latency is network-sensitive, and hiding its tail would make the record less
useful. The benchmark does not claim to reset Windows page cache, DNS, TLS, or
CDN state.

Reproduce:

```powershell
cargo bench-all --case package_sync_cold --samples 5 --warmups 1
```
