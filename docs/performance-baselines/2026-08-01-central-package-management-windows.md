# Central Package Management Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: .NET `10.0.100`; project target: `net10.0`.
- Samples: 30 retained samples after 3 warm-ups.
- Input: nearest `Directory.Packages.props` with four selected central rows,
  two project references, one version override, one global reference, and
  transitive pinning enabled.
- Graph: 54 exact packages; `Humanizer.Core` is centrally promoted.
- State: both tools start each timed process with populated package caches and
  matching lock state. Setup and acquisition are outside timing.
- Parity gate: all package identities, versions, SHA-512 values, and selected
  asset families agree. Microsoft's lock reports `Humanizer.Core` as
  `CentralTransitive`; `dv` reports the same distinct role.
- Work: zero timed network requests or package downloads.

## Commands

```text
dotnet restore CentralPackages.csproj --locked-mode --packages .packages --nologo --verbosity quiet
dv restore CentralPackages.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 461.826 ms | 490.623 ms | 446.370 ms | 492.309 ms |
| `dv` | 29.864 ms | 34.461 ms | 23.544 ms | 36.410 ms |

`dv` is 15.5x faster by median. The timed interval includes process startup,
nearest-file discovery, central evaluation, 54-package lock/cache validation,
role materialization, asset reporting, and deterministic structured output.

Reproduce:

```powershell
cargo bench-all --case central_package_management --samples 30 --warmups 3
```
