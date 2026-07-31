# SDK Package-Pruning Data

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

