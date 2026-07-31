# Package Sync Baseline: Windows, 2026-07-31

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- `Newtonsoft.Json` `13.0.3`, 2,441,966-byte package
- release `dv` binary from the working tree based on `e171e92`

## Like-For-Like Warm Locked Result

Both commands used an isolated populated global-packages directory, an
unchanged project, and a lock created outside the timed interval. Neither
command required a network request. Preflight compared target framework,
package identity, exact version, archive SHA-512, and compile assets.

| Tool | Exact command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| `dotnet` | `dotnet restore PackageConsole.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | 446.029 ms | 459.179 ms | 440.757 ms | 459.179 ms |
| `dv` | `dv sync PackageConsole.csproj --packages .packages --offline --json` | 4.459 ms | 5.255 ms | 4.162 ms | 5.255 ms |

Ten samples were retained after three warm-ups. The median ratio is `100.0x`.
The `dv` timing includes process startup and JSON event reporting.

Reproduce:

```powershell
cargo bench-all --case package_sync_warm --samples 10 --warmups 3
```

## Cold Package-Cache Observation

A quick cold-cache run retained three samples after one warm-up:

| Tool | Median | P95 |
|---|---:|---:|
| `dotnet` | 728.739 ms | 767.345 ms |
| `dv` | 799.022 ms | 1,512.216 ms |

This result is not promoted to the README comparison. Each iteration removed
the isolated global-packages directory, but `dotnet` could reuse its separate
user HTTP cache while `dv` deliberately has no HTTP metadata/payload cache yet.
The network states are therefore not like-for-like. The measurement still
covers the cold `dv` path: v3 discovery, metadata, a 2.4 MB streamed download,
SHA-512 verification, bounded ZIP validation/extraction, and atomic publish.

Reproduce the observation:

```powershell
cargo bench-all --case package_sync_cold --quick
```
