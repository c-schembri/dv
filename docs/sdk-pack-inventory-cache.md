# SDK Pack Inventory Cache

`PACKS-010` retains validated runtime-pack and apphost inventory data across
commands. Production code does not launch or scrape `dotnet`, MSBuild, or
NuGet to create or validate the cache.

## Data Contract

Input:

- selected SDK installation path and semantic version;
- evaluated TFM and requested RID;
- manifest-selected runtime/host RIDs, identities, and versions;
- selected SDK bundled-versions manifest and portable RID graph;
- one runtime-pack `RuntimeList.xml`, one runtime-pack root, and one host-pack
  native directory;
- NuGet completion markers when either selected pack came from the global
  package cache.

Output:

- one schema-2, SHA-512-checksummed binary file under
  `<global-packages>/.dv/sdk-pack-inventories/v2/`;
- one 156-byte little-endian persistent header followed by UTF-8 text and
  nine-byte `{ kind, start, length }` records;
- one immutable 40-byte in-memory header;
- one contiguous batch of 12-byte `{ kind, relative_path_span }` records;
- one text allocation containing relative runtime-asset and apphost paths.

The observed .NET 10 `win-x64` inventory has 172 managed assets, 15 native
assets, and one apphost. Its persistent file is 12,405 bytes. Relative paths
avoid repeating the cache-root prefix 187 times; final absolute paths are
written directly into the plan's existing text table without per-asset path
allocation.

The cache file owns its bytes until invalidation or manual removal. A decoded
inventory owns its text and asset batch for one planning call; the resulting
plan owns a separate compact text table and asset spans. Variable external
path counts require those two inventory allocations, but no asset owns a
string, path buffer, reference-counted pointer, or object. The selected active
runtime dimension is a genuine singleton today; its asset transform remains
batch-first.

`RuntimePackInventory` is 40 bytes with pointer alignment. `RuntimeAsset` is
12 bytes with four-byte alignment, so five records fit in an expected cache
line with four bytes unused. The persistent nine-byte form has no padding.

`ASSUMPTION: the benchmark CPU has 64-byte cache lines - affects the expected
five records per line, not correctness or the serialized layout.`

## Fingerprint And Invalidation

The SHA-512 key covers:

- cache schema and selected SDK version;
- TFM, framework version, requested/selected RIDs, pack identities, and pack
  versions;
- normalized selected-SDK, manifest, RID-graph, runtime-pack, runtime-manifest,
  host-pack, and host-native paths;
- size and nanosecond modification time for the SDK manifest, RID graph,
  runtime manifest, host-native directory, and optional NuGet completion
  markers for both selected packs.

Changing the selected SDK, pack selection, manifest, RID graph, installed
host generation, or completed package generation therefore selects a new
immutable entry. Pack version directories and completed NuGet packages are
treated as immutable; content repair replaces their manifest or generation
boundary rather than editing a cached generation in place.

`ASSUMPTION: SDK-installed version directories and NuGet packages carrying a
completion marker are immutable until their recorded generation metadata
changes - affects whether warm reuse may skip 187 per-asset file probes.`

Malformed, truncated, checksum-invalid, wrong-schema, wrong-fingerprint,
unsafe-path, invalid-UTF-8-span, or missing-apphost entries are removed and
rebuilt from source data. A cache read, creation, or atomic publication
failure falls back to normal validated planning and never turns a cache
failure into a build failure.

## Transform Cost

Cold inventory construction reads and parses `RuntimeList.xml`, validates all
187 asset paths, selects the apphost, builds the compact batches, encodes one
buffer, and publishes through a process-unique temporary file plus rename.
Source parsing, cache decode, and publication are linear. Materialization uses
two linear, strongly biased kind-filter passes to preserve managed/native
batch order; there is no random access or pointer chasing.

Warm reuse still performs project/SDK selection and manifest/RID selection.
It replaces the runtime-list parse, 187 asset existence probes, and apphost
directory search with bounded fingerprint metadata reads, one 12,405-byte
cache read, linear decode, and one apphost existence check. Cache validation
and benchmark assertions happen outside downstream asset loops.

Concurrent publishers use immutable content-addressed filenames. A winner is
never overwritten; a losing temporary file is removed. Readers never observe
a partially published entry.

The simplification pass excluded the small SDK-definition and RID-graph
transforms from this cache: measured probes placed them around 0.15 ms and
0.18 ms, so caching them would add format and invalidation work without
addressing the dominant inventory scan. Entries are not evicted automatically;
each observed retained generation costs 12,405 bytes, and explicit cache
removal is the bounded recovery path.

## Verification

Unit coverage proves warm hits, corrupted-entry rebuild, runtime-manifest
invalidation, selected-SDK invalidation, missing-apphost rejection, compact
layout, and path-escape rejection.

The process oracle compares the same 172 managed assets, 15 native assets,
runtime/host identities and versions, RIDs, roots, and apphost as Microsoft
MSBuild. The cold case removes only the `dv` inventory before every timed
sample; restored package contents remain outside timing. The warm case builds
the inventory during warm-up and verifies exactly one binary entry after each
sample.

On the initial Windows machine, 30 retained samples measured:

| State | Microsoft median | `dv` median | Ratio |
|---|---:|---:|---:|
| Cold inventory construction | 368.322 ms | 11.118 ms | 33.1x |
| Warm inventory reuse | 360.550 ms | 6.403 ms | 56.3x |

```powershell
cargo bench-all --case runtime_pack_inventory_cold --samples 30 --warmups 3
cargo bench-all --case runtime_pack_plan --samples 30 --warmups 3
```
