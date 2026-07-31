# Legacy SDK Package-Pruning Data

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
