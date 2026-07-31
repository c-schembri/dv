# Package Resolution And Cache Contract

## Supported Input

The initial resolver accepts a batch of evaluated SDK-style C# projects with
exact, minimum, bounded NuGet interval, or floating versions in both
`PackageReference` and package dependency metadata. Stable forms (`*`, `1.*`,
`1.2.*`, and `1.2.3.*`) prefer the highest matching stable version, then use
NuGet's nearest in-range fallback when none match. A `-*` or prefixed
prerelease wildcard admits matching prereleases as well as a matching stable
release, with normal NuGet precedence; `*-*` admits every version. The same
typed representation is retained if floating syntax appears in package
metadata, so graph convergence does not approximate it as a plain minimum.
Floating lower bounds may also carry an inclusive or exclusive interval upper
bound; floating upper bounds are rejected like NuGet.Client. The target
framework comes from each `ProjectSpec`; package code does not contain a fixed
current .NET version.
Modern `net5.0` through the latest captured stable generation are evaluated
from the same parsed target descriptor. Recognized legacy families remain
explicitly unsupported until their pack and compiler policies are captured.

NuGet sources are typed records containing URL and protocol generation:

- v3 sources resolve registration, package-content, search, vulnerability,
  and package-publish resources using NuGet's ordered type and compatible
  client-version rules; restore derives exact normalized package URLs from the
  selected `PackageBaseAddress`, while the other endpoint consumers remain
  separate features;
- v2 sources enumerate ranged versions through bounded, cycle-checked
  `FindPackagesById` Atom continuations, then read the exact OData package
  entry and its advertised content URL, SHA-512, and size;
- `protocolVersion="2"` and `"3"` are authoritative; absent values infer v3
  only for a `/v3/index.json` URL and otherwise infer v2;
- HTTPS v2/v3 sources plus local flat or hierarchical folders are accepted;
  local paths may be configuration-relative, CLI-relative, absolute, or
  `file://`, and remain usable in offline mode;
- Basic/PAT source credentials come from merged `packageSourceCredentials` or
  exact `NuGetPackageSourceCredentials_{name}` environment values. Windows
  encrypted passwords use current-user DPAPI with NuGet-compatible entropy;
  cleartext and intermediate buffers are zeroed. One sensitive header is
  reused only for the configured HTTPS origin;
- a 401 from that origin can lazily launch a configured self-contained NuGet
  V2 credential provider. Provider authentication is noninteractive by default,
  bounded by NuGet timeout variables, cancellable, and cached for the command;
- merged `clientCertificates` can attach a bounded PFX identity or a Windows
  store certificate selected by thumbprint. The native TLS client is built
  once, used only for the configured HTTPS origin, and cannot redirect the
  identity to another origin;
- credential-free config or lowercase environment HTTP proxies are applied by
  the native client. Proxy addresses and source credentials are not retained
  in locks, results, diagnostics, or events; source inventory reports only the
  authentication kind.

Configuration discovers machine fragments, additional-user fragments, the
main .NET CLI user file, and one `NuGet.Config` from each drive-to-project
ancestor in precedence order. `--configfile` selects only its validated file.
Repeatable `-s`/`--source` values replace the configured package-source batch
in command-line order, with exact duplicates removed. An exact URL match keeps
the configured source identity and protocol so source mapping remains valid;
otherwise the protocol is inferred from the HTTPS URL. Relative explicit
config and package-directory paths are normalized against the working
directory before configuration discovery.
Keyed `packageSources`, `disabledPackageSources`, and
`globalPackagesFolder` values merge case-insensitively with add, remove, and
clear operations. A disabled-source key disables by presence; both Boolean
spellings are accepted for NuGet compatibility, while clear/remove re-enable.
`%NAME%` environment references expand once in add values, unknown names remain
literal, and MSBuild `$()` syntax remains literal. The explicit
`--packages` path wins over `NUGET_PACKAGES`, which wins over
`globalPackagesFolder`, which wins over the platform default.

`fallbackPackageFolders` uses the same case-insensitive keyed operations.
Higher-precedence configuration rows are searched first after the writable
global cache; `NUGET_FALLBACK_PACKAGES` replaces that merged list.
`NUGET_HTTP_CACHE_PATH` and `NUGET_SCRATCH` select the HTTP metadata-cache and
temporary roots. Conditional HTTP-cache reuse is deliberately tracked by
`RES-017/018`; retaining the path is not treated as implementing revalidation
or corruption policy.

`signatureValidationMode` is retained as `accept` or `require` for the
signature verifier tracked by `RES-015`; `require` fails explicitly until that
verifier exists. Project `NuGetAudit`,
`NuGetAuditMode`, and `NuGetAuditLevel` values are parsed into typed policy for
`RES-024`; .NET 10 defaults to enabled, `all`, and `low`. Enabled auditing also
fails explicitly until `RES-024`, while `NuGetAudit=false` is the supported
opt-out.

`auditSources` uses the same keyed URL/protocol representation and precedence
as package sources; audit execution remains a later policy feature.
`packageSourceMapping` stores source rows with contiguous pattern ranges.
Exact matches outrank prefix wildcards, longer prefixes outrank shorter ones,
and equally specific matches may select multiple sources. Matching is
case-insensitive and allocation-free during graph work. Restore applies the
winning source set before local-feed inspection, v2 endpoint materialization,
or v3 service-index I/O, then lazily activates newly required sources as the
dependency graph expands. Exact and ranged cache hits remain source-independent;
an uncached identity with no enabled winner fails as `DV0412` without source
work.

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
  -> enumerate floating constraints and linearly score NuGet best-match order
  -> seed a bounded queue with direct package constraints
  -> fetch and stage up to twenty-four independent requests with async I/O
  -> parse each completed manifest and immediately enqueue unseen dependencies
  -> merge dependency identities and conflicts through one deterministic owner
  -> retract transitive packages supplied by the selected shared framework
  -> stream each package through SHA-512 into a bounded staging directory
  -> hand completed staging records to bounded blocking archive work
  -> validate ZIP paths, duplicates, links, sizes, and expansion bounds
  -> verify embedded nuspec identity and version before publication
  -> move through same-volume staging and atomically publish the
     NuGet-compatible cache entry
  -> select dependency and asset groups using the parsed target framework
  -> compact graph indices, asset spans, and text into PackageResolution
  -> write deterministic lock data when requested
```

Version selection scans each source/cache version batch linearly in ascending
SemVer order. Non-floating constraints return on the first accepted version;
floating constraints keep one candidate and apply NuGet's matching-first,
highest-float, nearest-fallback ordering. Stable feeds make the prerelease
branch predictable; prefixed prerelease floats take the rarer prefix path.
`PackageVersion` is 48 bytes with 8-byte alignment. The 100,000-version input
limit therefore caps its contiguous record working set at 4.8 MB, excluding
the externally sized normalized text allocations. Float behavior and a
prerelease-prefix length occupy existing `VersionBound` padding: the bound
remains 56 bytes and `VersionRange` remains 112 bytes, both 8-byte aligned.
No worker mutates these records, so extra cache-line alignment would consume
memory without preventing false sharing. `ASSUMPTION: the benchmark host has
64-byte data cache lines - affects layout analysis only; no correctness or
alignment decision depends on it.`

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
span allocation. Actual per-package roots and ordered fallback roots are cold
parallel path-span batches, so fallback support does not enlarge the hot
package scan. `PackageAssetRanges` is 72 bytes with 4-byte alignment; every
path is an 8-byte offset/length span into one owned UTF-8 buffer. The
pointer-aligned `PackageResolution` header is 328 bytes after adding the two
cold root batches, typed policy fields, and source-work batch. Assuming the benchmark machine's
observed 64-byte cache line, eight spans fit per line. Reporters and compiler
planning scan only the ranges they consume.

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
records remain contiguous and identity-sorted. Tasks read immutable service
endpoints and one borrowed storage-policy view, own one request and staging
directory at a time, and stream HTTP response chunks through SHA-512 into
`tokio::fs::File` under the configured scratch root. Network waits are
concurrent; ZIP validation, extraction, nuspec parsing, and atomic publication
run through Tokio's blocking pool rather than occupying an executor thread.
Publication hard-links into same-volume global-cache staging when possible and
falls back to one asynchronous cross-volume copy when required.
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

- target framework, global/HTTP/temp/fallback roots, lock path, selected
  source, and protocol;
- signature and audit policy plus a redacted proxy-presence bit;
- package records sorted by case-insensitive identity;
- dependency indices and one contiguous, explicitly partitioned package-asset
  span batch;
- computed archive hashes, with source-advertised v2 hashes verified;
- cache-hit, download, request, and payload-byte counters;
- one 32-byte, eight-byte-aligned immutable row per configured source with its
  configuration key, protocol, actual request attempts, source bytes,
  and cumulative source-work microseconds.

Remote tasks account into 24-byte, eight-byte-aligned `HttpWork` and
`SourceWork` values that travel with existing task results. The deterministic
scheduler owner merges them by contiguous source index, so instrumentation
adds no shared atomic, lock, channel, task, or per-request allocation. Source
duration includes retry and authentication waits plus body consumption;
concurrent source durations may therefore sum above command wall time. Warm
locked work materializes one zeroed row per configured source and classifies
every validated package as a cache hit.

The successful single-source path carries one inline source record. Only the
rare path where a source fails and a later source succeeds allocates a small
failure batch, preserving those attempted requests without charging the common
path.

The selected source and source-work rows retain configuration keys rather than
locations. Reported inventory locations, persisted lock locations, and package
metadata strip URL userinfo, query, and fragment components. Raw URLs remain
only in command-local transport state where requests require them.

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
| Credential-provider discovery or protocol failure | `DV0410` |
| Package authentication or provider work cancelled | `DV0411` |
| Uncached identity has no enabled winning source mapping | `DV0412` |

Unsupported package build-target execution, signature enforcement, and
advanced conflict rules fail instead of being approximated.

## Verification

Unit and CLI tests cover typed floating and interval selection, malformed
floating rejection, transitive typed retention, cold/warm cache behavior,
local/v2/v3
configuration, v2 Atom metadata and
continuations, flat and hierarchical discovery, flat archive identity checks,
hierarchical hash rejection, target-dependent asset selection, zero-request
warm locking, cache reuse, and archive traversal rejection. Live verification
downloads the same public package through both NuGet v2 and v3 and compares
identity, archive SHA-512, size, and selected compile asset.

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

The source-telemetry case uses the same six-package, two-source cold fixture,
then checks each `dv` source row and aggregate against the loopback servers'
actual request and response-byte counters. It also requires six cache misses
and credential-free output:

```powershell
cargo bench-all --case nuget_source_telemetry --samples 30 --warmups 3
```

The curated distribution is retained in the
[source-telemetry baseline](performance-baselines/2026-08-01-nuget-source-telemetry-windows.md).

The floating-version case prepares a local feed containing two real `13.*`
archives outside timing. It starts each process with empty isolated package
state, asks both tools to resolve `Newtonsoft.Json` `13.*`, and requires the
same exact identity, version, archive SHA-512, target, asset batches, and zero
HTTP work before timing:

```powershell
cargo bench-all --case nuget_floating_version --samples 30 --warmups 3
```

The curated distribution is retained in the
[floating-version baseline](performance-baselines/2026-08-01-nuget-floating-version-windows.md).

The storage-policy case builds an adapter against the selected SDK's official
`NuGet.Common` and `NuGet.Configuration` assemblies, queries audit properties
through MSBuild, and compares global/fallback/HTTP/temp paths, signature and
proxy policy, package folders, identity/version/hash, and the compile asset
selected from a fallback-only locked state.

The local-source case maps two public packages across flat and hierarchical
feeds, clears the global cache and restore outputs before every sample, and
compares configured source paths, identities, versions, and SHA-512 values.
`dv` must publish both entries while reporting zero HTTP requests.

The service-index case performs one live, uncached HTTPS request in each tool
and compares every selected registration, package-content, search,
vulnerability, and publish endpoint with the SDK-shipped `NuGet.Protocol`
implementation. The protocol client version is explicit and independent of
the selected .NET SDK version.

The next cold-path optimization is a persistent conditional service-index
cache keyed by normalized source URL and validators such as ETag or
Last-Modified. A dedicated package-cold/metadata-warm benchmark must measure
that real repeated-project state; the existing HTTP-cold benchmark must keep
the cache disabled so it remains an honest first-machine boundary.
