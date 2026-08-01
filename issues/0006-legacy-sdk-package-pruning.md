# Legacy SDK Package-Pruning Data

## Status

Resolved. The generated data contract and parity evidence are described below.

## Question

How should `dv` consume the generated authoritative pruning tables used by
.NET 9 and earlier without invoking MSBuild or copying version-specific data
into production code?

## Observed Constraint

The .NET SDK intentionally uses generated `FrameworkPackages` tables for
.NET 9 and earlier because historical targeting-pack `PackageOverrides.txt`
files are not fully accurate. The .NET 10 pack-data parser therefore cannot be
silently reused for older targets.

## Required Decision

Locate a stable SDK-owned representation or define a generated build-time data
contract, then compare its identities and upper versions against .NET 8 and 9
reference restores. Keep unsupported legacy behavior explicit until that
comparison passes.

## Decision

`tools/generate-legacy-pruning.ps1` reads the authoritative
`FrameworkPackages` sources at the exact `dotnet/dotnet` revision shipped by
SDK `10.0.100`. It resolves inheritance and removals at generation time and
emits sorted effective tables for .NET Standard 2.0/2.1 and .NET Core/.NET
2.0 through 9.0. Production restore therefore performs no managed assembly
load, process launch, network request, or table-file parse for legacy targets.
`AllowMissingPrunePackageData` preserves an explicit empty semantic table when
no compatible source exists.

The generated contract records 4,032 immutable 32-byte rows across all target
and framework combinations. Restore touches only the selected slices, merges
duplicate identities by the greatest upper version, applies the stable patch
ceiling for `.NETCoreApp`, and compacts the result into the resolver's existing
owned text and row buffers. The source generator strips C# comments before
reading initializers so commented package records cannot become live data.

SDK-oracle checks match 280 Core identities for .NET 8, 282 for .NET 9, and
418/420 identities when ASP.NET is merged. The maintained warm benchmark is
documented in `docs/performance-baselines/2026-08-01-package-pruning-windows.md`.
