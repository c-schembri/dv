# Project Evaluation Contract

## Supported Input

The initial evaluator accepts one UTF-8 SDK-style C# project using
`Microsoft.NET.Sdk`. A caller may pass one `.csproj` path or let `dv` select
exactly one `.csproj` from the current directory.

Restore expands literal `ProjectReference` paths into one deterministic
root-first project batch. Each absolute path is evaluated once through a
sorted command-local seen-path index. Reference order is preserved in the
output batch; cycles and diamonds terminate at the first previously seen path.
Every project uses the root command's selected Debug/Release configuration.

Observed fixture data:

- `small-console`: one project, one source, no references;
- `runtime-project`: one project, no sources, one selected RID, and three
  ordered runtime expansion dimensions;
- `multi-project`: three projects, three sources, three project-reference
  edges;
- `source-links`: three logical sources, two physical source files, and one
  safe in-root directory junction/symlink;
- project files are currently below 1 KiB and use one target framework.

The supported property and item subset is:

- one literal recognized `TargetFramework`; package restore accepts modern,
  .NET Core, .NET Standard, and .NET Framework families, while SDK, pack, and
  compiler planners reject families they do not implement at their own typed
  boundaries;
- one optional literal `RuntimeIdentifier`;
- an optional literal semicolon-delimited `RuntimeIdentifiers` batch;
- `OutputType` equal to `Exe` or `Library`;
- `Debug` or `Release` configuration;
- literal `AssemblyName` and `RootNamespace`, with project-name defaults;
- `Nullable` set to `enable` or omitted;
- `ImplicitUsings` set to `enable`, `disable`, or omitted;
- `Deterministic` set to `true`, `false`, or omitted;
- default recursive `.cs` source discovery excluding `bin`, `obj`, dot-prefixed
  VCS/tool trees, and the five supported configured output roots, while
  following safe in-workspace filesystem links and rejecting cycles/escapes;
- literal C# `ProjectReference` paths;
- `PackageReference` items with exact, interval, or floating literal versions;
- nearest `Directory.Packages.props` central versions, overrides, global
  references, and selected transitive-pin policy;
- explicit `FrameworkReference` items;
- conditions on reference `ItemGroup` elements and individual project,
  package, or framework references, evaluated against `TargetFramework`,
  `RuntimeIdentifier`, and `Configuration`.

Reference conditions support case-insensitive equality and inequality,
`And`/`Or` precedence, `!`, parentheses, boolean literals, and compound values
such as `$(TargetFramework)|$(RuntimeIdentifier)`. Conditions on properties or
other item types, unknown properties, relational operators, functions,
multi-targeting, explicit compile items, custom imports, targets, tasks,
general property expansion, wildcard references, and non-C# projects fail
explicitly. The evaluator does not approximate those behaviors.

## Transform

```text
directory or project path
  -> select exactly one .csproj
  -> read project bytes once
  -> stream XML events through a fixed-depth state machine
  -> evaluate bounded reference conditions against the selected dimensions
  -> discard false branches before reference metadata validation
  -> prune fixed/configured source trees before descent or link resolution
  -> scan retained source directories, resolving physical identity only for links
  -> sort relative source paths
  -> compact text, item, and target-dimension batches
  -> ProjectSpec
  -> for restore, breadth-first literal ProjectReference expansion
  -> root-first unique ProjectSpec batch
```

The XML reader borrows the project bytes. A single scratch string is reused for
property text. Filesystem paths and XML values are dynamically sized external
data, so temporary owned buffers are necessary at this boundary. The completed
`ProjectSpec` compacts all retained UTF-8 text into one immutable buffer.

Source and project-reference batches contain 8-byte `(offset, length)` spans.
Package references contain four spans plus inline asset policy and are 36 bytes
with 4-byte alignment. Framework references are 28 bytes. Raw references retain
an 8-byte pair of condition indexes; the common exact-property comparison
borrows XML and dimension text without allocating evaluation storage.
Runtime target dimensions use the same 8-byte spans in one contiguous batch.
Two 32-bit values mark the plural-property prefix and selected-RID index; the
selected RID reuses its plural span when present, and duplicate plural values
are removed without allocating a hash table. RIDs remain opaque and
case-sensitive here; compatibility traversal belongs to runtime-graph
selection.
The parsed target descriptor is stored once beside its original text and
shared with package and compiler planning. Compile-time assertions protect the
compact layouts.

Link-free source traversal retains the original single `Vec<PathBuf>`
depth-first stack. Directory and link entries first pass an allocation-free
leading-dot/`bin`/`obj` classifier. Projects with configured output paths add
one sorted fixed-capacity batch of at most five absolute lexical paths; literal
values remain borrowed during expansion. Exact matches do not canonicalize,
while a case-only configured match resolves physical identity on its cold
branch. Excluded entries never pay link-target metadata or descent. A retained
link first queries its target type, then canonicalizes the target and lazily
canonicalizes the project root.
Directory links use a cold enter/leave work stack plus active physical ancestry,
so arbitrary graph cycles fail as `DV0207`, not only direct ancestor links.
Physical targets outside the root also fail. Project closure lazily creates one
command-local encoded-byte arena with sorted lexical and physical offset
indices only after its first reference or second explicit root. The active
filesystem decides whether case variants canonicalize to one identity. No-link
aliases enter the lexical index and are skipped; link aliases still fail,
without changing ordinary `..` reference semantics. The zero-reference path
allocates no identity state, and every root pushed by restore shares the same
table. See [project-path-table.md](project-path-table.md).

The exclusion batch is 168 bytes on 64-bit Windows and 128 bytes on other
64-bit targets, aligned to `usize`; its five `PathBuf` slots are 32 or 24 bytes,
or two records per assumed 64-byte cache line. Compile-time assertions protect
the intentional layout. Full exclusion behavior and benchmark evidence are in
[source-exclusion-discovery.md](source-exclusion-discovery.md).

`ASSUMPTION: filesystem links are rare in ordinary project trees - affects the
cold-branch linear identity checks, not correctness.`

`ASSUMPTION: the first benchmark machine has 64-byte cache lines - affects the
expected eight text spans or four package records per line; this is not a
persisted or FFI layout.`

The final arrays are traversed linearly. XML element dispatch is branchy but
predictable for repeated property/item groups. Filesystem entry type and
extension checks are data-dependent. Parallel traversal is deliberately
absent: thread creation and merging cost more than the observed one-project
batch.

Closure expansion reads project-reference spans linearly. The sorted path
index uses logarithmic lookup and contiguous insertion because project counts
are externally sized but typically small; it avoids a hash table and produces
deterministic duplicate removal. Each referenced project owns its immutable
`ProjectSpec` for the lifetime of the downstream package batch.

## Output And Lifetime

`ProjectSpec` owns:

- one compact UTF-8 text buffer;
- ordered source spans;
- ordered project-reference spans;
- ordered exact package-reference records;
- one ordered unique runtime-dimension span batch and a selected index;
- fixed enums and flags for configuration, output type, nullable mode,
  implicit usings, and deterministic output.

The spec remains immutable from evaluation through future build-plan
construction. Human and JSON reporters materialize owned strings only at the
output edge.

## Failure Contract

| Boundary | Behavior |
|---|---|
| No project | `DV0200` |
| Multiple implicit candidates | `DV0201` |
| Filesystem failure | `DV0202` |
| Malformed XML | `DV0203` |
| Unsupported MSBuild behavior | `DV0204` |
| Invalid supported property | `DV0205` |
| Non-Unicode retained path | `DV0206` |

Unsupported input never falls back to `dotnet` or MSBuild.

## Verification

The benchmark preflight evaluates `small-console` with both:

```text
dotnet msbuild SmallConsole.csproj --nologo -getProperty:TargetFramework,OutputType,Nullable,ImplicitUsings,AssemblyName,RootNamespace,Configuration,Deterministic -getItem:Compile,ProjectReference,PackageReference
dv project inspect SmallConsole.csproj --json
```

It compares every requested property and the ordered compile item identities
before retaining timing samples.

Runtime expansion has its own like-for-like parity gate and timed case:

```text
dotnet msbuild RuntimeProject.csproj --nologo -getProperty:TargetFramework,RuntimeIdentifier,RuntimeIdentifiers
dv project inspect RuntimeProject.csproj --json
```

The gate compares the TFM, selected RID, ordered plural RID property, and the
unique target-dimension batch. The maintained 30-sample Windows baseline is
`321.215 ms` for the Microsoft query and `5.687 ms` for `dv`, a `56.5x`
median improvement.

Conditional references have a separate parity gate and timed case:

```text
dotnet msbuild ConditionalReferences.csproj --nologo -p:Configuration=Release -getProperty:TargetFramework,RuntimeIdentifier,Configuration -getItem:PackageReference,ProjectReference,FrameworkReference
dv project inspect ConditionalReferences.csproj --configuration Release --json
```

It compares all three dimensions plus the selected package identities and
versions, normalized project path, and framework identity before sampling. The
30-sample Windows medians are `288.983 ms` for Microsoft and `4.765 ms` for
`dv`, a `60.6x` improvement.
