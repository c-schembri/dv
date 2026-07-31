# Portable Runtime Identifier Graph

`dv sdk compatible-rids RID` selects the same SDK as `dv sdk current`, reads
that installation's `PortableRuntimeIdentifierGraph.json`, and emits compatible
RIDs in nearest-first order. RID text is opaque and compared ordinally. An
unknown RID expands only to itself; no code splits a RID into guessed operating
system or architecture components.

## Transform

```text
selected SDK installation
  -> read PortableRuntimeIdentifierGraph.json once
  -> parse ordered #import batches
  -> add imported opaque leaves
  -> sort runtime nodes by ordinal identifier
  -> resolve imports to contiguous u32 node indices
  -> breadth-first expansion for every node
  -> immutable RuntimeIdentifierGraph
```

The breadth-first rule matches NuGet's official
[`RuntimeGraph.ExpandRuntime`](https://github.com/NuGet/NuGet.Client/blob/4a1790c46bdc107e89a2cbd9c7d3337cff649adf/src/NuGet.Core/NuGet.Packaging/RuntimeModel/RuntimeGraph.cs)
implementation: the requested RID is first, direct imports retain source order,
and each unique fallback is visited at its nearest depth.

## Layout

- one immutable UTF-8 text table containing each RID once;
- sorted 16-byte nodes containing an 8-byte text span and 8-byte edge range;
- one contiguous `u32` direct-edge batch;
- one 8-byte compatibility range per node;
- one contiguous `u32` precomputed compatibility batch.

The selected .NET 10.0.100 graph contains 85 nodes, 133 direct edges, and 494
precomputed compatibility indices. All retained graph arrays together remain
small enough for cache-resident repeated package selection. Precomputation uses
one queue and generation-mark batch; cycles and diamond imports cannot repeat
or recurse indefinitely. Queries perform one binary search and a linear range
walk with no allocation.

JSON parsing uses temporary dynamically sized maps and strings because the SDK
file is external variable-sized input. Those objects are discarded after the
compact graph is built.

## Boundary Behavior

- a missing selected-SDK graph is `DV0110`;
- graph I/O failure is `DV0111`;
- malformed JSON or an empty RID is `DV0112`;
- compact range overflow is `DV0113`;
- imported names absent from `runtimes` become opaque leaf nodes, matching
  NuGet expansion behavior;
- unknown query RIDs are exact-only.

No failure path launches `dotnet`, NuGet, or MSBuild.

## Verification

The benchmark fixture compiles a minimal adapter over the selected SDK's own
`NuGet.Packaging.dll` outside the timed interval. Before sampling, the harness
requires its complete `linux-musl-x64` expansion to equal `dv` exactly:

```text
dotnet bin/Release/RidGraphOracle.dll linux-musl-x64
dv sdk compatible-rids linux-musl-x64
```

The maintained 30-sample Windows result is `36.217 ms` for the official NuGet
implementation and `6.049 ms` for `dv`, a `6.0x` median improvement.
