# Minimal C# Compatibility Matrix

This matrix defines the first supported input. `Planned` means the behavior is
explicitly in scope but is not implemented; `Reject` means Phase 1 must emit a
diagnostic rather than approximate Microsoft behavior.

| Input or behavior | Initial contract | Phase 0 |
|---|---|---|
| Project type | SDK-style C# `.csproj` | Planned |
| SDK discovery | Native root enumeration and `global.json` selection | Implemented |
| SDK declaration | `Microsoft.NET.Sdk` | Planned |
| Target frameworks | one installed `net9.0` target | Planned |
| Output types | `Exe` and `Library` | Planned |
| Source items | default `**/*.cs`, excluding generated/output trees | Planned |
| Nullable | `enable` or omitted SDK default | Planned |
| Implicit usings | `enable` or `disable` | Planned |
| Project references | acyclic SDK-style references | Planned |
| Package references | one target, public source, exact versions first | Planned |
| Configuration | `Debug` and `Release` | Planned |
| Generated inputs | global usings, assembly attributes, editor config | Planned |
| Outputs | managed assembly, portable PDB, deps/runtime config as required | Planned |
| Custom imports/targets/tasks | no execution | Reject |
| Legacy project format | unsupported | Reject |
| Multi-targeting | unsupported initially | Reject |
| F# and Visual Basic | unsupported initially | Reject |
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
