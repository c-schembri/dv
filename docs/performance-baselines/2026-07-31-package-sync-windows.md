# Package Sync Baseline: Windows, 2026-07-31

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- `Newtonsoft.Json` `13.0.3`, 2,441,966-byte package
- release `dv` binary from the working tree based on `95a02db`

## Like-For-Like Warm Locked Result

Both commands used an isolated populated global-packages directory, an
unchanged project, and a lock created outside the timed interval. Neither
command required a network request. Preflight compared target framework,
package identity, exact version, archive SHA-512, and compile assets.

| Tool | Exact command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| `dotnet` | `dotnet restore PackageConsole.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | 518.249 ms | 547.722 ms | 472.478 ms | 547.722 ms |
| `dv` | `dv restore PackageConsole.csproj --packages .packages --offline --json` | 5.190 ms | 5.903 ms | 4.929 ms | 5.903 ms |

Ten samples were retained after three warm-ups. The median ratio is `99.9x`.
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
| `dotnet` | `dotnet restore PackageConsole.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | 910.720 ms | 963.904 ms | 892.961 ms | 963.904 ms |
| `dv` | `dv restore PackageConsole.csproj --packages .packages --json` | 366.890 ms | 482.027 ms | 353.201 ms | 482.027 ms |

Ten samples were retained after two warm-ups. The median ratio is `2.5x`.
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
cargo bench-all --case package_sync_cold --samples 10 --warmups 2
```
