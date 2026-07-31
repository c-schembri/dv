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
| Runtime identifiers | selected SDK portable graph, opaque ordinal RID keys | Loaded into compact breadth-first compatibility ranges and used for manifest-declared pack selection |
| Runtime and host packs | implicit `Microsoft.NETCore.App`, one active RID, restored runtime pack, installed/restored host pack | Managed/native runtime assets and exact apphost template planned and oracle-verified |
| SDK pack inventory cache | selected SDK, TFM/RID/pack dimensions, immutable completed packs | Fingerprinted binary inventory invalidates on SDK/manifest/host/package generation changes and rebuilds corrupt entries |
| Unavailable packs | unsupported TFM/RID or absent runtime, host, targeting, or shared-framework input | Stable diagnostic includes exact requirement dimensions and one concrete acquisition action |
| Framework references | implicit `Microsoft.NETCore.App` plus explicit SDK-known references | Manifest-defined runtime/targeting versions, targeting packs, profiles, and shared-runtime roll-forward planned and oracle-verified |
| Output types | `Exe` and `Library` | Implemented |
| Source items | default `**/*.cs`, excluding generated/output trees | Implemented |
| Nullable | `enable` or omitted SDK default | Implemented |
| Implicit usings | `enable` or `disable` | Implemented |
| Project references | acyclic SDK-style references | Paths captured; graph validation planned |
| Package references | one target, HTTPS NuGet v2/v3 or local flat/hierarchical source, exact versions first | Initial resolution, verified cache, lock, and nine family-partitioned asset ranges implemented |
| NuGet configuration discovery | machine, additional-user, user, drive/repository, or one explicit file | Platform roots, filename casing, precedence, and explicit isolation implemented |
| NuGet configuration merge | keyed sources, disabled sources, package folder, and `%NAME%` values | Case-insensitive add/replace/remove/clear and single-pass environment expansion implemented |
| NuGet source policy | package/audit sources, local/v2/v3 metadata, package-source mappings, and v3 service capabilities | Typed source batches, v2 Atom version enumeration, official registration/package-content/search/vulnerability/publish selection, and longest-pattern package routing implemented; pre-discovery filtering remains planned |
| NuGet storage and restore policy | global packages, HTTP cache, scratch, fallback folders, signature/audit modes, and proxy | Microsoft-compatible precedence, fallback lookup, scratch staging, atomic publication, and proxy construction are implemented; unimplemented HTTP reuse, signature verification, and advisory execution fail or remain explicitly tracked |
| NuGet CLI restore overrides | repeatable HTTPS or local source, one explicit config, and one packages folder | Source replacement, config isolation, working-directory path normalization, and config/environment precedence implemented for `restore` and `sync` |
| NuGet local sources | config/CLI paths and `file://`, flat or hierarchical package layout | One-time layout detection, offline range lookup, nuspec/hash verification, and atomic global-cache publication implemented |
| NuGet source credentials | merged `packageSourceCredentials`, exact per-source environment value, or self-contained cross-platform V2 provider; Basic/PAT over HTTPS | NuGet-compatible static precedence, Windows DPAPI passwords, bounded provider handshake/claims/authentication, noninteractive CI, opt-in interactive output, cancellation/timeouts, zeroized plaintext, same-origin header containment, and redacted reporting implemented; DLL-only providers are rejected without a `dotnet` fallback |
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
