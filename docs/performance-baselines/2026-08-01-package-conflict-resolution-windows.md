# Package Conflict Resolution Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: .NET `10.0.100`; project target: `net10.0`.
- Samples: 30 retained samples after 3 warm-ups.
- Input: fifteen deterministic local archives forming one nested direct-wins
  downgrade and one unrelated cousin graph at different absolute depths. The
  cousin leaf versions point at distinct children; a diamond graph gives one
  constraining parent both nested and project-direct paths.
- Output: eleven selected identities. `Direct.Leaf` must be `1.0.0`,
  `Cousin.Leaf` must be `2.0.0`, `Cousin.Current` must remain, and
  `Cousin.Stale` must be retracted. `Diamond.Leaf` must be `2.0.0`.
- State: populated isolated package caches; `obj`, `packages.lock.json`, and
  `dv.lock.json` are removed before every sample.
- Parity gate: Microsoft restore and `dv` produce the same exact ordered
  identity/version batch before timing.
- Work: zero timed network requests and zero package downloads.

Microsoft restore receives `NoWarn=NU1605` because the SDK promotes its valid
direct-wins downgrade warning to an error by default. This changes diagnostic
severity only; the selected package graph is verified independently.

## Commands

```text
dotnet restore ConflictResolution.csproj --packages .packages -p:NoWarn=NU1605 --nologo --verbosity quiet
dv restore ConflictResolution.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 604.023 ms | 689.544 ms | 565.602 ms | 779.330 ms |
| `dv` | 16.971 ms | 19.661 ms | 15.574 ms | 38.140 ms |

`dv` is 35.6x faster by median. The timed interval includes process startup,
project/config parsing, local cache discovery, ancestry-aware constraint
convergence, alternate-root detection, stale-edge retraction, eleven-package materialization, and
structured output.

Reproduce:

```powershell
cargo bench-all --case package_conflict_resolution --samples 30 --warmups 3
```
