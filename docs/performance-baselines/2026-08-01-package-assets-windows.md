# Package asset baseline - Windows - 2026-08-01

This baseline promotes the massive eShop-derived restore after `dv` matched
the .NET SDK's selected package graph and portable package-asset output.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- five retained samples after one warm-up
- fresh fixture and empty isolated package directory before every sample
- warm OS, DNS, TLS, and CDN state not controlled

## Commands

```text
dotnet restore MassivePackageGraph.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet
C:\Projects\dv\target\release\dv.exe restore MassivePackageGraph.csproj --packages .packages --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 9977.524 ms | 10416.603 ms | 9102.256 ms | 10416.603 ms |
| `dv` | 4325.957 ms | 4852.974 ms | 4065.283 ms | 4852.974 ms |

`dv` was 2.3x faster at the median. The reference populated 272 archives and
197,860,237 payload bytes. `dv` retained 203 packages and observed a maximum
of 208 HTTP requests and 164,964,741 payload bytes across retained samples.
The maximum is reported because eager streaming can issue a small, variable
amount of speculative metadata or package work while constraints converge.

## Parity gate

Before timing, the harness performs isolated restores and rejects any mismatch
in target framework; package identity, version, or SHA-512; compile, runtime,
resource, content, analyzer, build, build-multitargeting, or native asset paths; and
RID-specific runtime-target path, RID, or asset type. This gate compares all
203 selected packages against `project.assets.json` and cannot be skipped for
the massive case.

Reproduce:

```powershell
cargo bench-all --case package_graph_massive --samples 5 --warmups 1
```
