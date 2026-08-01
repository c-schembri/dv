# Package Diagnostic Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: .NET `10.0.100`; project target: `net10.0`.
- Samples: 30 retained samples after 3 warm-ups.
- Input: two project-direct local packages request exact `Diagnostic.Leaf`
  versions `1.0.0` and `2.0.0`. The local feed contains eight deterministic
  archives and the isolated package cache starts empty for every sample.
- Output: expected failure. Microsoft must emit `NU1107`; `dv` must emit
  structured `DV0414` with `Diagnostic.Leaf`, both ordered ranges, error
  severity, and an action.
- State: `.packages`, `obj`, `packages.lock.json`, and `dv.lock.json` are
  removed before every sample. Network access is impossible because the only
  source is the local fixture feed.
- Parity gate: additional deterministic projects require Microsoft/dv pairs
  `NU1605`/`DV0413`, `NU1108`/`DV0415`, `NU1101`/`DV0416`,
  `NU1102`/`DV0417`, and `NU1202`/`DV0402`. Successful nested direct-wins
  restore must emit the same `DV0413` before and after reading the native warm
  lock.

## Commands

```text
dotnet restore ConflictFailure.csproj --packages .packages --nologo --verbosity minimal
dv restore ConflictFailure.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 569.423 ms | 581.503 ms | 560.573 ms | 586.356 ms |
| `dv` | 13.797 ms | 17.209 ms | 10.850 ms | 18.270 ms |

`dv` is 41.3x faster by median. The timed interval includes process startup,
project and configuration parsing, local-feed discovery, cold package-cache
publication, cousin constraint convergence, conflict classification, and
diagnostic serialization.

Reproduce:

```powershell
cargo bench-all --case package_diagnostics --samples 30 --warmups 3
```
