# Framework Reference Planning

`PACKS-007` resolves project framework references, targeting packs, requested
runtime versions, and installed shared-framework versions without launching
`dotnet`, MSBuild, or NuGet.

## Contract

Input:

- a batch of evaluated SDK-style projects;
- the already selected SDK inventory and dotnet root;
- an optional explicit global-packages directory;
- `KnownFrameworkReference` rows from the selected SDK's
  `Microsoft.NETCoreSdk.BundledVersions.props`;
- installed `shared/<framework>/<version>` directories.

Transform:

1. Add the SDK-defined implicit `Microsoft.NETCore.App` reference first, then
   retain explicit `FrameworkReference` order. An explicit Core reference
   supplies metadata to the implicit row rather than creating a duplicate.
2. Match each reference and TFM against the selected SDK manifest. Framework,
   pack, and runtime versions are data; no .NET generation is embedded in
   production code.
3. Apply Microsoft's runtime-version precedence: per-reference
   `RuntimeFrameworkVersion`, project `RuntimeFrameworkVersion`, then the
   manifest default/latest version selected by per-reference or project
   `TargetLatestRuntimePatch`.
4. Resolve per-reference `TargetingPackVersion` overrides and find packs under
   the selected dotnet root first, then the configured global package cache.
5. Enumerate installed shared-framework version directories and apply
   `Disable`, `LatestPatch`, `Minor`, `Major`, `LatestMinor`, or `LatestMajor`.
   Stable requests prefer stable installations; prerelease installations are
   considered only when stable candidates cannot satisfy the policy.
6. Skip installed shared-framework binding for self-contained projects; their
   runtime pack remains the deployment input.

The version precedence follows the SDK's
[`ProcessFrameworkReferences`](https://github.com/dotnet/sdk/blob/main/src/Tasks/Microsoft.NET.Build.Tasks/ProcessFrameworkReferences.cs),
and the policies follow the official
[framework-dependent roll-forward contract](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet#options-for-running-an-application).

Output:

- one plan per input project, preserving project order;
- project, selected SDK, manifest, TFM, roll-forward, and deployment kind;
- one implicit/explicit framework batch with reference identity, runtime name,
  requested and selected versions, shared root, targeting-pack identity,
  version, root, and optional profile.

## Layout And Cost

`FrameworkReferencePlan` is a 72-byte header on 64-bit targets. It owns one
text allocation and one contiguous record allocation. Each immutable
`ResolvedFrameworkReference` is 72 bytes with nine eight-byte text spans. The
dominant downstream access is a predictable linear scan; no hash table,
pointer graph, lock, worker pool, or async runtime is retained.

`ASSUMPTION: the benchmark machine has 64-byte cache lines - affects only the
layout interpretation, not correctness or alignment.` Each row contributes
72 working-set bytes. Depending on allocator placement it normally touches two
cache lines and may touch three without stronger alignment. Padding every
read-only row to 128 bytes would waste 56 bytes per framework and provides no
false-sharing benefit, so the natural four-byte alignment is retained.

One project normally has one or two rows. The measured transform performs one
SDK XML read, one targeting-pack existence check per row, one shared-directory
enumeration per framework row, and reporter-edge allocations only.
NuGet configuration is not read unless an installed targeting-pack lookup
misses and the package cache is actually needed.

## Failure Boundaries

Planning fails before runtime-config or compiler work when:

- the SDK manifest is missing, malformed, or lacks the requested TFM/reference;
- a runtime or targeting-pack version is malformed;
- the targeting pack is neither installed nor restored;
- no installed shared version satisfies the requested policy;
- NuGet configuration cannot supply the package cache;
- a required path is not Unicode or compact text exceeds 4 GiB.

Unsupported project XML still fails in project evaluation rather than being
silently approximated.

## Command

```powershell
dv project frameworks path\to\App.csproj
dv project frameworks path\to\App.csproj --packages path\to\packages --json
```

The fixture targets the latest stable baseline, .NET 10, and explicitly
references `Microsoft.AspNetCore.App`. Production selection remains manifest
driven so older and newer supported SDK data can use the same transform.

## Parity Gate

Before timing, `framework_reference_plan` compares the complete MSBuild
`RuntimeFramework` and `ResolvedFrameworkReference` batches: identity, runtime
name, requested version, profile, targeting-pack identity/version/root,
project TFM, `RollForward`, and `SelfContained`. It then builds and launches the
fixture outside timing and compares the actual Core and ASP.NET shared-runtime
versions chosen by the Microsoft host with `dv`'s installed selections.

The 30-sample Windows baseline measured `352.715 ms` for the MSBuild query and
`5.585 ms` for `dv`, a `63.2x` median improvement.
