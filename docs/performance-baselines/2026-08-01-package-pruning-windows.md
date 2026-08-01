# Package Pruning Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: .NET `10.0.100`; project target: `net9.0` with
  `RestoreEnablePackagePruning=true`.
- Samples: 30 retained samples after 3 warm-ups.
- Input: the implicit `Microsoft.NETCore.App` framework, one direct
  `Microsoft.AspNetCore.App` framework, and `Newtonsoft.Json` `13.0.3`.
- State: both tools start each timed process with matching warm package and
  lock state. Setup and package acquisition are outside timing.
- Parity gate: the selected SDK reports 420 merged pruning identities for the
  .NET 9 Core and ASP.NET frameworks; focused tests compare the same count,
  stable patch ceilings, and representative identities in `dv`.
- Work: `dv` resolved one cache-hit package with zero downloads, zero HTTP
  requests, and zero response bytes in every retained sample.
- Cost: generated compatibility data adds 166,912 bytes to the stripped
  release executable (`6,009,344` to `6,176,256` bytes) and no production
  dependency, startup process, or legacy table I/O.

## Commands

```text
dotnet restore LegacyPruningProject.csproj --locked-mode --packages .packages --nologo --verbosity quiet
dv restore LegacyPruningProject.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 636.670 ms | 1254.781 ms | 468.276 ms | 1411.643 ms |
| `dv` | 9.698 ms | 12.624 ms | 7.785 ms | 17.565 ms |

`dv` is 65.7x faster by median. The timed interval includes process startup,
project evaluation, selected-SDK framework mapping, generated legacy-table
selection and compaction, semantic fingerprinting, and warm lock validation.

Reproduce:

```powershell
cargo bench-all --case package_pruning --samples 30 --warmups 3
```
