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
6. Parse `data/RuntimeList.xml`, validate every selected path, and separate its
   managed and native files without directory guessing.
7. Locate exactly one `apphost` or `apphost.exe` template beneath the selected
   host RID.

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

The 30-sample Windows baseline measured `376.764 ms` for the MSBuild query and
`8.030 ms` for `dv`, a `46.9x` median improvement.
