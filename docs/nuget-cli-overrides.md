# NuGet CLI Override Contract

`NUGET-005` applies restore overrides as one typed boundary transform before
package graph work. The implementation does not invoke or scrape Microsoft
tooling.

## Input And Ownership

`restore` and `sync` accept a batch of zero or more `-s`/`--source` values,
one optional `--configfile`, and one optional `--packages` path. Source order
is preserved and exact duplicate URIs are removed. Variable external text
requires owned strings; their fixed-width records remain contiguous in one
`Vec<String>` owned for the command lifetime. An empty source batch performs
no source-override allocation.

Relative config and package paths are resolved once against the process
working directory. Source matching and deduplication are linear. Branches are
predictable for the usual zero-or-one-source command, and this cold boundary
is outside package graph and asset hot loops.

## Precedence

The effective order is:

1. `--packages`
2. `NUGET_PACKAGES`
3. `globalPackagesFolder` from the selected configuration
4. the platform default global-packages directory

`--configfile` loads only that file. Without it, normal machine, user, drive,
and repository discovery applies. `--source` replaces every configured
package source, while the selected config still supplies non-source policy
such as signature validation and proxy settings.

When an override URI exactly matches a configured source, its source name and
explicit protocol version are retained so package-source mappings remain
usable. Otherwise `.../v3/index.json` selects v3 and other HTTPS URIs select
v2. The first occurrence of an exact duplicate URI wins.

## Boundaries

- Repeated `--packages` and `--configfile` options fail before project I/O.
- Missing or empty option values fail as CLI argument errors.
- A missing explicit config file fails instead of falling back to discovery.
- Only HTTPS v2/v3 sources are currently accepted. Local folders and insecure
  HTTP remain explicit `NUGET-006`/`NUGET-012` work rather than silent guesses.
- Unsupported or malformed policy fails before network access.

The common path performs no filesystem or network work beyond the config and
package operations already required by restore. Its cost is one linear scan
of the small override batch plus unavoidable ownership of caller-provided
source strings.

## Verification

The locked `nuget-cli-overrides` fixture puts conflicting sources and package
folders in the implicit config, selected config, and environment. Preflight
requires both tools to use only the explicit config, CLI source, and CLI
package folder, then compares package identity, version, SHA-512, and zero
timed network/download work.

```powershell
cargo bench-all --case nuget_cli_overrides --samples 30 --warmups 3
```

Reference behavior: [dotnet restore](https://learn.microsoft.com/dotnet/core/tools/dotnet-restore)
and [common NuGet configuration](https://learn.microsoft.com/nuget/consume-packages/configuring-nuget-behavior).
