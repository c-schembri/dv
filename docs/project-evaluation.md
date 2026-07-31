# Project Evaluation Contract

## Supported Input

The initial evaluator accepts one UTF-8 SDK-style C# project using
`Microsoft.NET.Sdk`. A caller may pass one `.csproj` path or let `dv` select
exactly one `.csproj` from the current directory.

Observed fixture data:

- `small-console`: one project, one source, no references;
- `multi-project`: three projects, three sources, three project-reference
  edges;
- project files are currently below 1 KiB and use one target framework.

The supported property and item subset is:

- one literal modern unified .NET `TargetFramework` (`net5.0` or later);
- `OutputType` equal to `Exe` or `Library`;
- `Debug` or `Release` configuration;
- literal `AssemblyName` and `RootNamespace`, with project-name defaults;
- `Nullable` set to `enable` or omitted;
- `ImplicitUsings` set to `enable`, `disable`, or omitted;
- `Deterministic` set to `true`, `false`, or omitted;
- default recursive `.cs` source discovery excluding `bin` and `obj`;
- literal C# `ProjectReference` paths;
- `PackageReference` items with exact literal versions.

Conditions, multi-targeting, explicit compile items, custom imports, targets,
tasks, property expansion, wildcard references, and non-C# projects fail
explicitly. The evaluator does not approximate those behaviors.

## Transform

```text
directory or project path
  -> select exactly one .csproj
  -> read project bytes once
  -> stream XML events through a fixed-depth state machine
  -> scan source directories once
  -> sort relative source paths
  -> compact text and item batches
  -> ProjectSpec
```

The XML reader borrows the project bytes. A single scratch string is reused for
property text. Filesystem paths and XML values are dynamically sized external
data, so temporary owned buffers are necessary at this boundary. The completed
`ProjectSpec` compacts all retained UTF-8 text into one immutable buffer.

Source and project-reference batches contain 8-byte `(offset, length)` spans.
Package references contain two spans and are 16 bytes with 4-byte alignment.
The parsed target descriptor is stored once beside its original text and
shared with package and compiler planning. Compile-time assertions protect the
compact layouts.

`ASSUMPTION: the first benchmark machine has 64-byte cache lines - affects the
expected eight text spans or four package records per line; this is not a
persisted or FFI layout.`

The final arrays are traversed linearly. XML element dispatch is branchy but
predictable for repeated property/item groups. Filesystem entry type and
extension checks are data-dependent. Parallel traversal is deliberately
absent: thread creation and merging cost more than the observed one-project
batch.

## Output And Lifetime

`ProjectSpec` owns:

- one compact UTF-8 text buffer;
- ordered source spans;
- ordered project-reference spans;
- ordered exact package-reference records;
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
