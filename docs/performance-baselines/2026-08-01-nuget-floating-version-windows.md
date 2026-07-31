# NuGet Floating-Version Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: .NET `10.0.100`; project target: `net10.0`.
- Samples: 30 retained samples after 3 warm-ups.
- Input: one direct `Newtonsoft.Json` `13.*` reference, a generated local feed
  containing real `13.0.3` and `13.0.4` archives, and empty isolated
  project/package state before every timed process.
- Parity gate: Microsoft and `dv` must select the same exact package identity,
  version, archive SHA-512, target framework, and compile/runtime asset batches.
  This run selected `Newtonsoft.Json` `13.0.4`.
- Work: `dv` made zero HTTP requests, copied and published one 2,484,726-byte
  archive, and resolved one package in every retained sample. Feed seeding is
  outside timing; package copy, SHA-512, extraction, publication, and asset
  planning are inside it.

## Commands

```text
dotnet restore FloatingVersion.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet
dv restore FloatingVersion.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 667.568 ms | 714.356 ms | 642.102 ms | 723.083 ms |
| `dv` | 60.007 ms | 74.028 ms | 53.419 ms | 84.574 ms |

`dv` is 11.1x faster by median. Preflight asks the installed stable Microsoft
SDK for the highest `13.*` result in the same two-version feed before retaining
samples.

Reproduce:

```powershell
cargo bench-all --case nuget_floating_version --samples 30 --warmups 3
```
