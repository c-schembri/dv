# Runtime Pack Planning

`PACKS-006` selects the runtime and host inputs for one evaluated runtime
dimension without launching `dotnet`, MSBuild, or NuGet.

## Contract

Input:

- one evaluated project with a literal `RuntimeIdentifier`;
- the already selected SDK inventory;
- an optional explicit global-packages directory;
- the selected SDK's bundled-versions manifest and portable RID graph;
- restored runtime packs and installed or restored host packs.

Transform:

1. Match the implicit `Microsoft.NETCore.App` framework and apphost records for
   the evaluated TFM in `Microsoft.NETCoreSdk.BundledVersions.props`.
2. Read pack patterns, latest runtime-pack patch, apphost-pack patch, and each
   supported RID batch from those records. No SDK, pack, or package version is
   embedded in `dv`.
3. Traverse the SDK portable RID graph in breadth-first nearest-first order and
   choose the first RID present in each pack's declared batch. Unknown RIDs
   remain exact-only; text splitting never invents compatibility.
4. Expand the manifest's literal `**RID**` placeholder once.
5. Resolve each pack from the selected dotnet root first, matching SDK task
   precedence, then from the configured global-packages directory.
6. Resolve a fingerprinted immutable pack inventory. A miss parses
   `data/RuntimeList.xml`, validates every selected path, separates managed and
   native files, and locates exactly one `apphost` or `apphost.exe`; a hit
   decodes the validated compact inventory.
7. Materialize absolute output paths directly from the cached relative spans
   and selected pack root.

For default self-contained acquisition, the runtime pack uses
`LatestRuntimeFrameworkVersion`. Explicit per-reference or project
`RuntimeFrameworkVersion` and `TargetLatestRuntimePatch` values override that
default using the framework plan's version precedence. The requested runtime
framework version may remain the default while a future self-contained
deployment acquires the latest pack. This mirrors the SDK's
[`ProcessFrameworkReferences` version precedence](https://github.com/dotnet/sdk/blob/main/src/Tasks/Microsoft.NET.Build.Tasks/ProcessFrameworkReferences.cs).

Output:

- project, SDK, manifest, TFM, and requested RID;
- selected runtime RID, pack identity, version, and root;
- ordered managed and native runtime assets;
- selected host RID, pack identity, version, root, and apphost template.

## Layout

`RuntimePackPlan` owns all variable text in one contiguous allocation. Managed
and native assets share one contiguous batch of eight-byte text spans and are
addressed by two eight-byte ranges. The retained plan therefore has no string
or object allocation per asset; reporters materialize owned strings only at
the JSON boundary.

Transient XML records are discarded after planning. Asset order is the runtime
manifest order, which is also the downstream copy order.

The [SDK pack inventory cache](sdk-pack-inventory-cache.md) decodes to a
40-byte in-memory header, one text allocation, and contiguous 12-byte asset
records. It is
fingerprinted by selected SDK, target/RID/pack selection, source manifests,
host generation, and package completion metadata. Cache construction and
publication never occur in downstream copy loops.

## Failure Boundaries

Planning fails before output work when:

- no active runtime identifier exists;
- the selected SDK lacks a matching framework or apphost definition;
- no graph-compatible RID appears in a declared pack batch;
- a pattern is ambiguous or lacks one RID placeholder;
- NuGet configuration cannot provide a cache directory;
- a required pack, runtime manifest, runtime asset, or apphost is missing;
- a manifest path is absolute, escapes its pack, or is not valid Unicode;
- compact storage exceeds its 32-bit index space.

Missing or incompatible pack diagnostics retain the exact manifest-derived
identity, version, TFM, requested RID, pack kind, and one acquisition action.
See the [unavailable pack diagnostic contract](pack-diagnostics.md).

## Command

```powershell
dv project runtime-packs path\to\App.csproj
dv project runtime-packs path\to\App.csproj --packages path\to\packages --json
```

This command plans the implicit `Microsoft.NETCore.App` runtime pack for one
project RID. `dv project frameworks` separately materializes explicit
framework references, targeting packs, and installed shared-framework
roll-forward; runtime-pack planning consumes Core version overrides from the
same evaluated project data.

## Parity Gate

The `runtime_pack_plan` preflight restores its fixture outside timing, executes
Microsoft's `ProcessFrameworkReferences`, `ResolveRuntimePackAssets`, and
apphost targets, and compares:

- requested and selected RIDs;
- runtime identity, version, and package root;
- all 172 managed and 15 native runtime assets in order;
- host root and selected RID;
- the resolved apphost path and `AppHostSourcePath`.

The current 30-sample warm-cache Windows baseline measured `360.550 ms` for
the MSBuild query and `6.403 ms` for `dv`, a `56.3x` median improvement. With
the inventory removed before every sample, construction measured `368.322 ms`
versus `11.118 ms`, a `33.1x` improvement.
