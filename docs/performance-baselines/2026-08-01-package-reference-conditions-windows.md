# Conditional Reference Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: .NET `10.0.100`; project target: `net10.0` with RID `win-x64`.
- Samples: 30 retained samples after 3 warm-ups.
- Input: two conditional item groups containing TFM, RID, and Release
  configuration comparisons, `And`/`Or` precedence, parentheses, negation,
  three selected packages, one selected project, and one selected explicit
  framework reference.
- State: warm OS caches; fixture copying and benchmark preflight are outside
  timing. Neither command performs package restore or network I/O.
- Parity gate: the Microsoft MSBuild query and `dv` event must agree on the
  selected TFM, RID, configuration, package identities and versions, project
  path, and explicit framework identity. False branches contain incomplete or
  unsupported references and must not reach metadata validation.

## Commands

```text
dotnet msbuild ConditionalReferences.csproj --nologo -p:Configuration=Release -getProperty:TargetFramework,RuntimeIdentifier,Configuration -getItem:PackageReference,ProjectReference,FrameworkReference
dv project inspect ConditionalReferences.csproj --configuration Release --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 288.983 ms | 321.422 ms | 283.297 ms | 367.518 ms |
| `dv` | 4.765 ms | 6.209 ms | 4.248 ms | 6.319 ms |

`dv` is 60.6x faster by median. The timed interval includes process startup,
project XML loading, condition evaluation, reference filtering, compact text
materialization, and structured output for both tools.

Reproduce:

```powershell
cargo bench-all --case package_reference_conditions --samples 30 --warmups 3
```
