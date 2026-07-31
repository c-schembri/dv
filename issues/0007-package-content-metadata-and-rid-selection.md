# Package Content Metadata And RID Selection

## Status

Open. Portable package asset selection is implemented; these are the remaining
`RES-011` inputs.

## Observed Need

Restore now matches the massive .NET 10 oracle for portable compile, runtime,
resource, content, build, build-multitargeting, native, and RID runtime-target
paths. It also preserves runtime-target RID and asset-type metadata.

Two remaining decisions need additional selection or metadata inputs:

- The evaluated runtime dimensions must select one compatible RID through the
  SDK's portable RID graph rather than by string inference.
- `contentFiles` entries need their nuspec `buildAction`, `copyToOutput`, and
  `flatten` metadata compiled into the lock and downstream build plan.

## Acceptance

- Use the compact `EVAL-022` runtime dimensions and implement `PACKS-005`
  before selecting RID-specific assets.
- Parse bounded content-file patterns and metadata without general glob or XML
  interpretation in the restore hot path.
- Extend lock and event schemas deliberately if their contracts change.
- Compare selected paths and metadata against `project.assets.json` fixtures
  covering portable, Windows, Linux, and fallback RID cases.
- Add cold and warm benchmarks and keep malformed metadata behavior explicit.
