# SDK Discovery And Selection

`dv --version` and its explicit `dv sdk current` form select a .NET SDK without
launching `dotnet`, MSBuild, or any managed process. `dv sdk list` exposes the
complete installed SDK batch;
`dv sdk runtimes` exposes installed shared frameworks. The compatible
`--list-sdks` and `--list-runtimes` forms render the same rows as the .NET 10
driver and accept its case-insensitive `--arch <arch>` selector.

`dv sdk compatible-rids RID` reuses that selection and loads the installation's
portable runtime graph natively. Its compact graph and breadth-first semantics
are specified in [Portable Runtime Identifier Graph](runtime-identifier-graph.md).

The selection behavior follows Microsoft's
[`global.json` contract](https://learn.microsoft.com/en-us/dotnet/core/tools/global-json)
for standalone CLI use.

## Batch Contract

Inputs:

- current working directory;
- nearest ancestor `global.json`, when present;
- the first `dotnet` host root found on `PATH`;
- architecture-specific `DOTNET_ROOT_<ARCH>` and `DOTNET_ROOT` fallback roots;
- existing platform default roots;
- complete `<root>/sdk/<version>/dotnet.dll` installations;
- `<root>/shared/<family>/<version>` directories for runtime inventory.

Transform:

1. Find the nearest `global.json` with an ancestor walk.
2. Parse its JSON-with-comments SDK policy once.
3. Resolve ordered search roots, including .NET 10 `paths` and `$host$`.
4. Enumerate every SDK directory in each root into one contiguous vector.
5. Parse each valid directory name into numeric major, minor, patch, feature
   band, patch level, and a borrowed range into owned version text.
6. Sort by root index and semantic version.
7. Scan each root in order and select the first compatible result.

Outputs:

- ordered root paths;
- contiguous `SdkInstallation` records using a `u16` root index rather than a
  repeated path;
- one selected installation index;
- the influencing `global.json` path.

The inventory owns all paths and version text. It is immutable after discovery
and lives for one command. Full SDK and runtime paths are constructed only at
the reporting edge. The compatible installed-SDK and runtime transforms do not
read `global.json` or apply selection policy. For a different architecture they
use the .NET host registration before supported platform defaults and return an
empty batch when that architecture is not installed. The selection-aware native
`sdk list` continues to use the full policy above.

## Runtime Inventory Layout

Filesystem names are externally sized, so enumeration briefly owns cold
`String` work rows. After sorting, family and version text are packed into one
arena and each runtime becomes a 16-byte record containing two `u32` offsets,
two `u16` lengths, and one `u16` root index. Four records fit in a 64-byte cache
line; the 64-bit owner is 72 bytes. The batch is bounded to 4,096 installations
and is sorted by root order, family, then semantic version.

`ASSUMPTION: the Windows benchmark machine has 64-byte cache lines - affects
the records-per-line count and is validated under issue 0003.`

The query installs no cancellation handler because it is a bounded, read-only
inventory operation. It launches no process, reads no project or `global.json`,
and performs no network work. Hostfxr, hostpolicy, RID, and provenance
inventory remain explicit follow-on work.

## Supported Policy

- `version` as a complete three-part SDK version;
- `allowPrerelease`, defaulting to `true` for standalone CLI behavior;
- `patch`, `feature`, `minor`, `major`, `latestPatch`, `latestFeature`,
  `latestMinor`, `latestMajor`, and `disable`;
- .NET 10 `paths`, including paths relative to `global.json` and `$host$`;
- .NET 10 `errorMessage`;
- line and block comments in `global.json`.

If no `global.json` controls selection, the highest SDK from the active host
root is selected. Search roots are not merged for selection: the first root
with a compatible SDK wins.

## Boundary Behavior

- malformed JSON, invalid versions, unsupported policy values, filesystem
  errors, missing roots, and no compatible SDK fail explicitly;
- non-directory, incomplete, and non-version entries under `sdk/` are ignored;
- non-Unicode paths are valid for human output but rejected by JSON output,
  which requires lossless text;
- more than 65,535 search roots or 4,096 SDK/runtime installations is rejected;
- no fallback invokes `dotnet`.

## Cost And Access

The dominant work is one ancestor walk plus one linear directory enumeration
per root. Candidate records are traversed linearly with predictable policy
branches. Selection performs no per-candidate allocation.

Path buffers and version text are dynamically allocated because they are
variable-sized external filesystem data that must outlive directory iterators.
They are cold command-lifetime data, not a build hot-loop representation.

Observed on the initial Windows machine:

- one active root: `C:\Program Files\dotnet`;
- three SDK directories;
- selected SDK: `10.0.100`.

The benchmark records whole-process SDK selection latency rather than claiming
the inner enumeration cost from this observation.
