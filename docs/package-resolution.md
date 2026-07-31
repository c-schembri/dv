# Package Resolution And Cache Contract

## Supported Input

The initial resolver accepts a batch of evaluated SDK-style C# projects with
exact, minimum, or bounded NuGet interval `PackageReference` versions. The
target framework comes from each `ProjectSpec`; package code does not contain
a fixed current .NET version.
Modern `net5.0` through the latest captured stable generation are evaluated
from the same parsed target descriptor. Recognized legacy families remain
explicitly unsupported until their pack and compiler policies are captured.

NuGet sources are typed records containing URL and protocol generation:

- v3 sources discover `PackageBaseAddress` from the service index and derive
  exact normalized package-content URLs from that advertised base;
- v2 sources read the exact OData package entry and its advertised content
  URL, SHA-512, and size;
- `protocolVersion="2"` and `"3"` are authoritative; absent values infer v3
  only for a `/v3/index.json` URL and otherwise infer v2;
- only HTTPS HTTP sources are accepted initially. Local folders,
  authentication, source mapping, and proxies fail or remain outside the
  supported subset.

Configuration discovers machine fragments, additional-user fragments, the
main .NET CLI user file, and one `NuGet.Config` from each drive-to-project
ancestor in precedence order. `--configfile` selects only its validated file.
Keyed `packageSources`, `disabledPackageSources`, and
`globalPackagesFolder` values merge case-insensitively with add, remove, and
clear operations. A disabled-source key disables by presence; both Boolean
spellings are accepted for NuGet compatibility, while clear/remove re-enable.
`%NAME%` environment references expand once in add values, unknown names remain
literal, and MSBuild `$()` syntax remains literal. The explicit
`--packages` path wins over `NUGET_PACKAGES`, which wins over
`globalPackagesFolder`, which wins over the platform default.

`dv restore` and `dv sync` dispatch to the same package transform with
identical options, cache behavior, lock behavior, diagnostics, and output
payload. Structured command lifecycle events retain the spelling invoked by
the caller.

Exact `Newtonsoft.Json` `13.0.3` is the representative package. Its 2,441,966
byte archive selects `lib/net6.0/Newtonsoft.Json.dll` for the `net10.0`
fixture. A v3 miss performs two requests: service index and package content.
A v2 miss performs metadata and package requests.

The large-graph fixture references `Humanizer` `2.14.1`. Its dependency-only
root is retained as a valid graph node and resolves to 50 packages totaling
3,241,550 downloaded bytes. Dependency-only meta-packages need no compile,
runtime, or analyzer asset of their own; packages with neither a compatible
asset nor a dependency remain incompatible.

## Transform

```text
ProjectSpec batch + resolve options
  -> merge typed source and cache configuration
  -> select SDK-owned package-pruning data for the target framework
  -> validate a matching dv.lock.json and immutable cache entries
  -> otherwise normalize direct version constraints
  -> seed a bounded queue with direct package constraints
  -> fetch and stage up to twenty-four independent requests with async I/O
  -> parse each completed manifest and immediately enqueue unseen dependencies
  -> merge dependency identities and conflicts through one deterministic owner
  -> retract transitive packages supplied by the selected shared framework
  -> stream each package through SHA-512 into a bounded staging directory
  -> hand completed staging records to bounded blocking archive work
  -> validate ZIP paths, duplicates, links, sizes, and expansion bounds
  -> verify embedded nuspec identity and version before publication
  -> extract and atomically publish the NuGet-compatible cache entry
  -> select dependency and asset groups using the parsed target framework
  -> compact graph indices, asset spans, and text into PackageResolution
  -> write deterministic lock data when requested
```

For .NET 10 and later, pruning is driven by the selected SDK's versioned
`PrunePackageData` first and the matching `Microsoft.NETCore.App.Ref`
`PackageOverrides.txt` otherwise. The parsed identity table is sorted once,
queried by binary search, and hashed canonically into lock schema 2. Its
32-byte, 4-byte-aligned records contain text spans and fixed numeric version
fields; the observed 272-entry table occupies 8,704 record bytes, or two
records per assumed 64-byte cache line on the benchmark machine. Identity and
prerelease text live once in a contiguous backing buffer. Stable package
versions use the SDK's `major.minor.32767` upper bound. Direct package
references remain explicit; only transitive nodes and their outgoing edges
are pruned.

The warm locked path reads configuration, selected SDK pruning data, one lock,
and one immutable-cache completion marker per package. The lock carries the
already verified archive hash and every selected relative asset path. Producing
the plan deliberately does not stat every asset: concrete compiler, copy, and
runtime consumers diagnose a missing file when they open it. This removes
thousands of redundant Windows metadata requests from large warm graphs while
retaining traversal-path validation. The path performs zero HTTP requests and
never launches `dotnet`, MSBuild, or NuGet.

Network and archive data are externally sized, so staging paths, XML/JSON
documents, graph work maps, and extraction buffers allocate dynamically.
Final package records are contiguous. `ResolvedPackage` is 28 bytes with
4-byte alignment; its hot asset record is 32 bytes with 4-byte alignment. The
compile, runtime, analyzer, resource, content, inner-build, outer-build,
transitive-build, and native families occupy nine consecutive ranges in one
span allocation. `PackageAssetRanges` is 72 bytes with 4-byte alignment; every
path is an 8-byte offset/length span into one owned UTF-8 buffer. The 248-byte,
pointer-aligned `PackageResolution` header is 56 bytes smaller than the former
nine-allocation layout. Assuming the benchmark machine's observed 64-byte
cache line, eight spans fit per line. Reporters and compiler planning scan only
the ranges they consume.

The cold scheduler uses a two-thread Tokio runtime and one `JoinSet` capped at
twenty-four active package tasks. The runtime exists only after the warm-lock
path misses. One scheduler owner merges completed manifests into
identity-ordered maps and immediately submits newly discovered dependencies
when capacity exists. Completion order affects which eligible transfer starts
next but cannot affect final package order, version-conflict text, graph
indices, or lock output. Active tasks and retained results share the same
twenty-four-item bound; there is no unbounded task creation or result
retention.

Scheduling maps contain cold variable-sized external identities. Final graph
records remain contiguous and identity-sorted. Tasks read immutable shared
service endpoints, own one request and staging directory at a time, and stream
HTTP response chunks through SHA-512 into `tokio::fs::File`. Network waits are
concurrent; ZIP validation, extraction, nuspec parsing, and atomic publication
run through Tokio's blocking pool rather than occupying an executor thread.
Package size, entry size, expanded size, entry count, active tasks, and
runtime threads are bounded constants.

A lone package with at least eight archive entries uses up to four
contiguous-range extraction workers. When multiple package requests are
already in flight, each package extracts sequentially instead of nesting
worker pools. On the representative 24-entry archive this reduced ZIP
validation/extraction from 36.3 ms to 26.5 ms median.

The async file is flushed and closed before the blocking ZIP reader opens it.
It is not forced to stable storage while still in a disposable staging
directory. A successful atomic rename uses the already validated in-memory
hash and identity rather than rescanning the entry. If another publisher wins
the race, the winner is fully revalidated before use. Transient Windows
permission failures during the atomic rename receive three bounded retries
totaling at most 21 ms.

## Output And Lifetime

Each `PackageResolution` owns:

- target framework, cache root, lock path, selected source, and protocol;
- package records sorted by case-insensitive identity;
- dependency indices and one contiguous, explicitly partitioned package-asset
  span batch;
- computed archive hashes, with source-advertised v2 hashes verified;
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
| Identity, v2 source size/SHA-512 mismatch, or invalid hash | `DV0405` |
| Malformed or unsafe ZIP archive | `DV0406` |
| Cache or lock I/O failure | `DV0407` |
| Non-Unicode retained path | `DV0408` |
| Compact range or text overflow | `DV0409` |

Unsupported package build-target execution, signatures, floating versions,
and advanced conflict rules fail instead of being approximated.

## Verification

Unit and CLI tests cover typed v2/v3 configuration, real v2 Atom parsing,
target-dependent asset selection, SDK package-pruning bounds and edge
retraction, zero-request warm locking, cache reuse, and archive traversal
rejection. Live verification downloads the same public package through both
NuGet v2 and v3 and compares identity, archive SHA-512, size, and selected
compile asset.

The benchmark preflight compares `dotnet restore` and `dv restore` complete
package identity, exact-version, archive-SHA-512, target-framework, compile,
runtime, native, resource, analyzer, content, build, build-multitargeting,
build-transitive, and RID-specific runtime-target batches before retaining
samples. Cold dependency readiness uses
a fresh isolated package directory per iteration and disables the reference
tool's HTTP cache. The 50-package case applies the same boundary to streaming
dependency discovery and many small archives. Warm locked restore is reported
separately for both the single-package startup boundary and the 203-package
asset-plan boundary.
`dv` package, request, and payload counts are recorded as typed benchmark
evidence.

The next cold-path optimization is a persistent conditional service-index
cache keyed by normalized source URL and validators such as ETag or
Last-Modified. A dedicated package-cold/metadata-warm benchmark must measure
that real repeated-project state; the existing HTTP-cold benchmark must keep
the cache disabled so it remains an honest first-machine boundary.
