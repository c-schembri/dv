# SDK pack inventory cache baseline - Windows - 2026-08-01

This baseline promotes `PACKS-010` after exact runtime-pack/apphost parity in
both cache-construction and cache-hit states.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after three warm-ups
- warm OS caches; background Windows scheduling was not controlled
- restore and isolated-package preparation outside timed intervals

## Commands

```text
dotnet msbuild RuntimePackProject.csproj --nologo -p:SelfContained=true -p:UseAppHost=true "-t:ProcessFrameworkReferences;ResolveFrameworkReferences;ResolveRuntimePackAssets;_GetAppHostPaths" -getProperty:RuntimeIdentifier,AppHostSourcePath -getItem:ResolvedRuntimePack,RuntimePackAsset,ResolvedAppHostPack
C:\Projects\dv\target\release\dv.exe project runtime-packs RuntimePackProject.csproj --packages .packages --json
```

## Results

| State and tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Cold inventory, `dotnet` | 368.322 ms | 380.190 ms | 357.701 ms | 395.331 ms |
| Cold inventory, `dv` | 11.118 ms | 12.636 ms | 10.240 ms | 13.130 ms |
| Warm inventory, `dotnet` | 360.550 ms | 370.695 ms | 354.459 ms | 381.070 ms |
| Warm inventory, `dv` | 6.403 ms | 8.218 ms | 5.651 ms | 8.464 ms |

Cold `dv` construction was 33.1x faster at the median. Warm reuse was 56.3x
faster and reduced the prior `dv` runtime-pack median from 8.030 ms to
6.403 ms, a 20.3% reduction. Even the slowest retained `dv` sample was 27.2x
faster than the fastest Microsoft sample in the matching cold state.

## State Boundaries

Both tools use the same restored isolated package directory. Restore is never
timed. Before every cold `dv` iteration, the harness removes only
`.packages/.dv/sdk-pack-inventories/v2`; package contents remain populated.
The measured command must repopulate exactly one validated binary entry.

The warm case allows the first warm-up to publish the inventory. All retained
samples reuse it, and the harness verifies after every timed process that the
cache contains exactly one immutable binary entry. Validation is outside the
timed interval.

The preflight compares requested/selected RIDs, runtime and host identities,
versions and roots, all 172 managed and 15 native assets in order, and the
exact apphost template.

Reproduce:

```powershell
cargo bench-all --case runtime_pack_inventory_cold --samples 30 --warmups 3
cargo bench-all --case runtime_pack_plan --samples 30 --warmups 3
```
