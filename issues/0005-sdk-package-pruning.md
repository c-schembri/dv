# SDK Package-Pruning Data

## Status

Resolved for .NET 10 package pruning. Legacy generated tables are tracked in
`0006-legacy-sdk-package-pruning.md`.

## Question

Which installed-SDK file is the stable typed source for each target
framework's `packagesToPrune` identity and upper-version table?

## Observed Need

The .NET 10 eShop-derived restore oracle selects 203 packages. Native `dv`
range convergence selects the same identities at the same versions, plus five
framework-provided packages that the SDK prunes:

- `System.Diagnostics.DiagnosticSource` `6.0.1`
- `System.IO.Pipelines` `8.0.0`
- `System.Reflection.Emit.Lightweight` `4.7.0`
- `System.Runtime.CompilerServices.Unsafe` `6.0.0`
- `System.Text.Json` `10.0.5`

`project.assets.json` exposes the evaluated pruning table, but production `dv`
cannot obtain it by invoking MSBuild or by scraping that generated output.

## Required Decision

Trace the selected SDK's framework data to its authoritative source, define a
versioned parser and cache key, and verify the resulting table against .NET 8,
9, and 10 reference restores. Do not hard-code the five observed identities;
the table varies by target framework and SDK version.

## Decision

The selected .NET 10 SDK exposes historical pruning tables under
`sdk/<version>/PrunePackageData/<framework>/<runtime-framework>/` and keeps
the current framework's `PackageOverrides.txt` in the matching targeting
pack. `dv` checks those sources in that order, parses a bounded identity and
upper-version table, and applies the SDK's `major.minor.32767` ceiling for
stable `.NETCoreApp` package versions.

The sorted semantic table is SHA-512 fingerprinted into `dv.lock.json` schema
2, so a changed SDK table invalidates a warm lock without depending on path or
timestamp. Verification against the massive .NET 10 oracle removed all five
framework packages without hard-coding identities: the remaining 203 selected
identities and exact versions match the reference graph.
