# Central Package Management Contract

## Input And Output

`dv` walks from the evaluated project directory toward the filesystem root and
reads the nearest regular `Directory.Packages.props`. The file is limited to
4 MiB, XML depth is limited to eight, and each `PackageVersion` or
`GlobalPackageReference` batch is limited to 100,000 rows. The supported input
is:

- literal Boolean values for `ManagePackageVersionsCentrally`,
  `CentralPackageTransitivePinningEnabled`, and
  `CentralPackageVersionOverrideEnabled`;
- literal `PackageVersion` identities and version/range values;
- literal `GlobalPackageReference` identities and versions;
- the bounded TFM, RID, and configuration condition grammar shared with
  project reference evaluation;
- project `PackageReference` values supplied centrally or by
  `VersionOverride`.

The transform produces the existing ordered `PackageReference` batch plus one
case-insensitive identity-ordered central version batch. Each retained
`CentralPackageVersion` is 16 bytes, aligned to 4 bytes, and contains two spans
into the project-owned immutable text buffer. Four rows occupy 64 bytes on the
benchmark host. The batch lives for the `ProjectSpec` lifetime and is borrowed
by package resolution.

`ASSUMPTION: the benchmark host has 64-byte data cache lines - affects the
elements-per-line analysis only; no correctness or alignment decision depends
on it.`

## Transform

```text
project directory
  -> scan ancestors linearly for the nearest Directory.Packages.props
  -> bound and parse one forward-only XML document
  -> filter item groups and items by the selected project dimensions
  -> sort selected central rows once by case-insensitive identity
  -> reject duplicate selected identities
  -> apply VersionOverride, otherwise binary-search the central table
  -> append global references with Microsoft asset/private policy
  -> hash the effective policy and version table into the lock fingerprint
  -> seed transitive resolution with an identity-ordered central pin table
  -> promote a matching transitive node to its exact selected central version
  -> materialize deterministic direct, central-transitive, and transitive roles
```

Ancestor access is linear and predictable. XML parsing is a forward scan;
selected version lookup is logarithmic over a contiguous table. Dependency
discovery performs a binary search only when transitive pinning is enabled.
The common non-central path retains an empty batch and skips pin lookup.

The simplification pass removed general MSBuild import evaluation, an
expression tree, per-reference hash maps, and injection of unused central
packages into the graph. Global references use the fixed Microsoft policy;
there is no speculative metadata extension surface.

## Precedence And Boundaries

- The nearest `Directory.Packages.props` wins; parent files are not merged.
- `VersionOverride` wins over both inline and central versions. When central
  overrides are disabled, using it fails explicitly.
- With central management active, an inline `Version` without an override is
  rejected and a versionless reference without a selected central row fails.
- Without central management, `VersionOverride` still wins, matching NuGet.
- Duplicate selected central identities, missing versions, dynamic values,
  unsupported imports/elements, malformed XML, and exceeded limits fail before
  package or network work.
- A central transitive version below or outside a dependency requirement fails
  as stable downgrade diagnostic `DV0413` instead of silently upgrading or
  downgrading it.
- Central-transitive packages remain distinct from project-direct packages in
  results, JSON events, human output, and lock schema 7.

The central policy and sorted version batch are SHA-256 fingerprinted into the
lock. A changed props file that has the same effective selected data retains
the warm lock; an effective version or policy change invalidates it.

## Evidence

The representative `net10.0` fixture resolves 54 exact packages. It exercises
a versionless `Humanizer` reference, a `Newtonsoft.Json` override,
`Microsoft.SourceLink.GitHub` as a global reference, and promotion of
`Humanizer.Core` to Microsoft's `CentralTransitive` role. Preflight compares
all 54 package identities, versions, SHA-512 values, and asset families, then
checks Microsoft's lock role and `dv`'s structured central batch.

On Windows x64 with 24 logical CPUs, 30 samples after 3 warm-ups measured a
warm locked median of 461.826 ms for Microsoft and 29.864 ms for `dv`, a 15.5x
improvement. Both timed commands validate their populated package and lock
state with zero network work.

See the [central package management baseline](performance-baselines/2026-08-01-central-package-management-windows.md).
