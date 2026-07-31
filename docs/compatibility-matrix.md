# Minimal C# Compatibility Matrix

This matrix defines the first supported input. `Planned` means the behavior is
explicitly in scope but is not implemented; `Reject` means Phase 1 must emit a
diagnostic rather than approximate Microsoft behavior.

| Input or behavior | Initial contract | Current status |
|---|---|---|
| Project type | SDK-style C# `.csproj` | Implemented |
| SDK discovery | Native root enumeration and `global.json` selection | Implemented |
| SDK declaration | `Microsoft.NET.Sdk` | Implemented |
| Target frameworks | one installed modern unified .NET target; .NET 10 fixture baseline | Parsed dynamically and matching reference pack validated |
| Output types | `Exe` and `Library` | Implemented |
| Source items | default `**/*.cs`, excluding generated/output trees | Implemented |
| Nullable | `enable` or omitted SDK default | Implemented |
| Implicit usings | `enable` or `disable` | Implemented |
| Project references | acyclic SDK-style references | Paths captured; graph validation planned |
| Package references | one target, HTTPS NuGet v2/v3 source, exact versions first | Initial resolution, verified cache, lock, and compiler assets implemented |
| Configuration | `Debug` and `Release` | Implemented |
| Generated inputs | global usings, assembly attributes, editor config | Paths planned; content generation planned |
| Compiler inputs | framework references, SDK/pack analyzers, defines and core options | Initial immutable plan implemented |
| Outputs | managed assembly, portable PDB, deps/runtime config as required | Planned |
| Custom imports/targets/tasks | no execution | Rejected |
| Legacy project format | unsupported | Rejected |
| Multi-targeting | unsupported initially | Rejected |
| F# and Visual Basic | unsupported initially | Rejected |
| Native/AOT workloads | unsupported initially | Reject |

## Correctness Oracle

For a supported fixture, compare:

- evaluated source and reference input sets;
- normalized compiler argument data, excluding path-only differences;
- output assembly metadata and observable program behavior;
- incremental invalidation after changing one source, property, reference, and
  generated input;
- clean, warm, incremental, and no-op command results.

Reference `dotnet`/MSBuild invocations exist only in compatibility tests and
benchmarks. Production `dv` code must not call them.

## Boundary Behavior

Unknown XML required to determine compiler inputs is an error. Unknown XML that
is provably irrelevant to the selected initial contract may be retained as
unconsumed evidence, but it is never silently interpreted.

The evaluator must report the project path, unsupported element or condition,
and the smallest supported alternative when known.
