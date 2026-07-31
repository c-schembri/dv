# Compiler Input Planning Contract

## Supported Input

The initial planner accepts a batch of already evaluated single-target
`Microsoft.NET.Sdk` C# projects and one selected SDK inventory. .NET 10 is the
current stable fixture baseline, not a production constant. Framework family
and major/minor version are parsed once from the project. The newest stable
installed matching `Microsoft.NETCore.App.Ref` patch is selected; the SDK
patch remains controlled by normal SDK discovery and `global.json`.

The local reference fixture has one user source. SDK `10.0.100` with reference
pack `10.0.0` contributes 167 managed references, 6 pack analyzers, 2 SDK
analyzers, 3 analyzer-config paths, and 3 planned generated source paths.
These are observed values, not fixed counts.

## Transform

```text
ProjectSpec batch + selected SdkInventory
  -> locate selected SDK compiler and analyzer assets once
  -> enumerate compatible reference-pack versions once
  -> select highest stable patch matching the evaluated target
  -> stream FrameworkList.xml
  -> enumerate references once and validate manifest membership
  -> retain managed references and C# analyzers in manifest order
  -> derive source, generated-source, option, define, and output ranges
  -> compact each project into one immutable CompilerPlan
```

The pack manifest is structured XML, never scraped command output. The
reference directory is enumerated once into a temporary membership set instead
of issuing one metadata probe per reference. Every manifest asset retained by
the plan must exist. Absolute paths and external manifest text require
temporary dynamic storage; final variable text is copied once into one owned
UTF-8 buffer.

Each ordered batch contains 8-byte `(offset, length)` spans with 4-byte
alignment, protected by compile-time assertions. Reads by reporters and the
future compiler host are linear and branches are predictable.

`ASSUMPTION: the benchmark machine has 64-byte cache lines - affects the
expected eight spans per line; this is not a persisted or FFI layout.`

The observed pack is about 175 compiler assets. Planning is deliberately
single-threaded because directory, XML, and validation work at this size is
below a useful scheduling crossover. Pack discovery is shared across every
project in an input batch.

## Output And Lifetime

Each `CompilerPlan` owns:

- selected SDK, `csc.dll`, reference-pack identity, and pack root;
- ordered user and planned generated source paths;
- ordered reference assemblies, analyzers, analyzer configs, and defines;
- language version, warning level, configuration, output kind, nullable, and
  deterministic flags;
- intermediate assembly, PDB, and reference-assembly paths.

The plan remains immutable through reporting and future compilation. Generated
source paths are planned but their contents are not materialized yet.
`dv build --plan` is therefore inspection, not build execution.

## Failure Contract

| Boundary | Behavior |
|---|---|
| Compatible target reference pack absent | `DV0300` |
| Framework manifest malformed or targets another TFM | `DV0301` |
| Manifest, compiler, analyzer, or config asset absent | `DV0302` |
| Selected SDK cannot compile the evaluated target | `DV0303` |
| Pack or manifest filesystem failure | `DV0304` |
| Non-Unicode retained path | `DV0305` |
| Compact text exceeds 4 GiB | `DV0306` |

No error path invokes `dotnet`, MSBuild, or an ambient compiler.

## Verification

The benchmark preflight restores a mutable fixture copy outside the timed
interval and compares language version, define order, user sources, all
reference paths, and all analyzer paths against:

```text
dotnet msbuild SmallConsole.csproj --nologo -t:ResolveReferences -getProperty:LangVersion,DefineConstants -getItem:ReferencePath,Analyzer,Compile
dv build --plan SmallConsole.csproj --json
```

The process benchmark includes JSON reporting in both timed commands.
