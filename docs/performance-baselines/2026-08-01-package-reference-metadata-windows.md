# PackageReference Metadata Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: .NET `10.0.100`; project target: `net10.0`.
- Samples: 30 retained samples after 3 warm-ups.
- Input: one direct `Newtonsoft.Json` `13.0.3` reference with
  `IncludeAssets=compile;runtime`, `ExcludeAssets=runtime`,
  `PrivateAssets=all`, two `NoWarn` codes, `Aliases=JsonAlias`, and
  `GeneratePathProperty=true`.
- State: both tools start each timed process with matching warm package and
  lock state. Setup and package acquisition are outside timing.
- Parity gate: Microsoft `project.assets.json`, an MSBuild property query, and
  `dv` restore/compiler-plan events must agree on the effective compile-only
  mask, private-all propagation, warning codes, direct compile alias, excluded
  runtime family, and exact `PkgNewtonsoft_Json` package root.
- Work: `dv` resolved one cache-hit package with zero downloads, zero HTTP
  requests, and zero response bytes in every retained sample.

## Commands

```text
dotnet restore MetadataProject.csproj --locked-mode --packages .packages --nologo --verbosity quiet
dv restore MetadataProject.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 456.722 ms | 460.724 ms | 450.203 ms | 462.601 ms |
| `dv` | 6.611 ms | 7.714 ms | 5.756 ms | 7.935 ms |

`dv` is 69.1x faster by median. The timed interval includes project evaluation,
lock validation, direct policy materialization, asset-family reporting, and
process startup for both tools.

Reproduce:

```powershell
cargo bench-all --case package_reference_metadata --samples 30 --warmups 3
```
