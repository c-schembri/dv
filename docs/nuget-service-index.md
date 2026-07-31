# NuGet Service-Index Capability Contract

`NUGET-007` resolves the NuGet v3 capabilities advertised by each effective
package source. Production discovery remains native Rust; the Microsoft
`NuGet.Protocol` assembly is used only by the benchmark parity oracle.

## Input And Output Shapes

Input is the ordered package-source batch produced by configuration and CLI
precedence. A v3 source contributes one bounded service-index JSON document.
The measured nuget.org document is 9,272 bytes with 40 resource rows and 31
distinct `@type` values. Both `@type` and `clientVersion` may be a string or an
array. Local and v2 sources remain in the output without v3 capabilities.

Output is one `PackageSourceInventory` per project. Source rows and endpoint
rows are contiguous, immutable arrays; names, locations, and endpoint URLs are
spans into one owned text buffer. A source row is 28 bytes with 4-byte
alignment, and an endpoint row is 12 bytes with 4-byte alignment. The
inventory owns all storage for its lifetime and exposes borrowed batch views.

The five capability ranges are retained in this order:

1. registration;
2. package content (flat container);
3. search;
4. vulnerability information;
5. package publish.

## Transform

```text
ordered effective sources
  -> fetch v3 service indexes concurrently through one bounded Tokio set
  -> parse bounded JSON documents
  -> match exact resource types in official preference order
  -> reject entries incompatible with the NuGet protocol client version
  -> retain every mirror at the best compatible version
  -> compact source and endpoint text into one immutable inventory
  -> report in deterministic source, capability, and resource order
```

`NUGET_PROTOCOL_CLIENT_VERSION` is isolated from SDK selection because it
models the NuGet protocol client's compatibility version, not the selected
.NET SDK version. Updating either therefore does not silently change the
other. Restore consumes the selected package-content endpoint; registration,
search, vulnerability, and publish consumers remain separate features.

Access is a linear scan over tens of cold resource rows followed by linear
compaction. Type and version branches are predictable for known service rows;
unknown rows are discarded outside later restore loops. No pointer graph or
per-resource object hierarchy is retained.

## Boundaries

- The service-index schema major must be `3` and `resources` must be an array.
- Unknown resource types and malformed unrelated rows are ignored.
- Selected endpoints must be absolute HTTPS URLs without embedded credentials.
- Insecure HTTP fails explicitly until `NUGET-012` defines an opt-in contract.
- Resource and response sizes are bounded before allocation or parsing.
- A v3 restore source must advertise package content; inspection may report an
  otherwise valid source with any optional capability absent.
- Offline inspection performs zero network requests and returns configured
  sources with empty endpoint ranges.
- Empty project batches do not create a runtime or perform I/O.

## Verification

The `nuget-service-index` fixture compares every selected endpoint with the
SDK-shipped `NuGet.Protocol` implementation using the same explicit protocol
client version. Each retained timing sample starts both tools with an empty,
isolated HTTP cache and requires one nonempty service-index response.

```powershell
cargo bench-all --case nuget_service_index --samples 30 --warmups 3
```

The protocol shapes are defined by Microsoft's [NuGet service-index
documentation](https://learn.microsoft.com/en-us/nuget/api/service-index) and
[NuGet server API overview](https://learn.microsoft.com/en-us/nuget/api/overview).
