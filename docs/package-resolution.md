# Package Resolution And Cache Contract

## Supported Input

The initial resolver accepts a batch of evaluated SDK-style C# projects with
exact `PackageReference` versions. The target framework comes from each
`ProjectSpec`; package code does not contain a fixed current .NET version.
Modern `net5.0` through the latest captured stable generation are evaluated
from the same parsed target descriptor. Recognized legacy families remain
explicitly unsupported until their pack and compiler policies are captured.

NuGet sources are typed records containing URL and protocol generation:

- v3 sources discover registration and flat-container resources from the
  service index;
- v2 sources read the exact OData package entry and its advertised content
  URL, SHA-512, and size;
- `protocolVersion="2"` and `"3"` are authoritative; absent values infer v3
  only for a `/v3/index.json` URL and otherwise infer v2;
- only HTTPS HTTP sources are accepted initially. Local folders,
  authentication, source mapping, proxies, and environment expansion fail or
  remain outside the supported subset.

Configuration currently merges the user file and ancestor `NuGet.Config`
files in precedence order. `packageSources` supports add, remove, and clear;
`disabledPackageSources` supports clear and Boolean add values. The explicit
`--packages` path wins over `NUGET_PACKAGES`, which wins over
`globalPackagesFolder`, which wins over the platform default.

Exact `Newtonsoft.Json` `13.0.3` is the representative package. Its 2,441,966
byte archive selects `lib/net6.0/Newtonsoft.Json.dll` for the `net10.0`
fixture. A v3 miss performs four requests: service index, registration leaf,
catalog entry, and package. A v2 miss performs metadata and package requests.

## Transform

```text
ProjectSpec batch + resolve options
  -> merge typed source and cache configuration
  -> validate a matching dv.lock.json and immutable cache entries
  -> otherwise normalize exact direct references
  -> resolve dependency waves in stable identity order
  -> fetch up to four independent misses concurrently
  -> stream each package through SHA-512 into a bounded staging directory
  -> validate ZIP paths, duplicates, links, sizes, and expansion bounds
  -> extract and atomically publish the NuGet-compatible cache entry
  -> select dependency and asset groups using the parsed target framework
  -> compact graph indices, asset spans, and text into PackageResolution
  -> write deterministic lock data when requested
```

The warm locked path reads configuration, one lock, package markers and
hashes, and selected asset metadata. It performs zero HTTP requests and never
launches `dotnet`, MSBuild, or NuGet.

Network and archive data are externally sized, so staging paths, XML/JSON
documents, graph work maps, and extraction buffers allocate dynamically.
Final package records are contiguous. `ResolvedPackage` is 28 bytes with
4-byte alignment; its asset record is 32 bytes with 4-byte alignment. Text and
paths cross the subsystem boundary as 8-byte offset/length spans into one
owned UTF-8 buffer. Reporters and compiler planning traverse final records and
asset ranges linearly.

Independent misses use at most four scoped workers. Small waves remain
sequential. Workers claim indices atomically, retain immutable shared input,
and merge results by original index, so completion order cannot affect the
graph. Downloads use a reused 64 KiB buffer. Package size, entry size, expanded
size, entry count, and worker count are bounded constants.

## Output And Lifetime

Each `PackageResolution` owns:

- target framework, cache root, lock path, selected source, and protocol;
- package records sorted by case-insensitive identity;
- dependency indices and compile, runtime, and analyzer asset spans;
- verified archive hashes;
- cache-hit, download, request, and payload-byte counters.

The graph is immutable through compiler planning and reporting. Published
package entries are immutable until an explicit future cache operation removes
them. `dv.lock.json` is project-owned persistent state.

## Failure Contract

| Boundary | Diagnostic |
|---|---|
| Unsupported configuration or source | `DV0400` |
| Invalid identity/version, conflict, or cycle | `DV0401` |
| No compatible supported assets | `DV0402` |
| Offline cache miss | `DV0403` |
| HTTP or source metadata failure | `DV0404` |
| Identity, size, or SHA-512 mismatch | `DV0405` |
| Malformed or unsafe ZIP archive | `DV0406` |
| Cache or lock I/O failure | `DV0407` |
| Non-Unicode retained path | `DV0408` |
| Compact range or text overflow | `DV0409` |

Unsupported build targets, runtime-specific assets, signatures, general NuGet
ranges, and advanced conflict rules fail instead of being approximated.

## Verification

Unit and CLI tests cover typed v2/v3 configuration, real v2 Atom parsing,
target-dependent asset selection, zero-request warm locking, cache reuse, and
archive traversal rejection. Live verification downloads the same public
package through both NuGet v2 and v3 and compares identity, archive SHA-512,
size, and selected compile asset.

The benchmark preflight compares `dotnet restore` and `dv sync` package
identity, exact version, archive SHA-512, target framework, and compile assets
before retaining samples. Cold package-cache and warm locked states are
reported separately.
