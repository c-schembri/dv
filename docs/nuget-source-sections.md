# NuGet Source Sections

`NUGET-003` extends the configuration fold with typed package sources, disabled
state, audit sources, protocol metadata, and nested package-source mappings.
The parser remains native Rust and does not launch NuGet in production.

## Data Contract

The low-to-high configuration batch produces:

- ordered `(source name, PackageSource)` rows for package sources;
- ordered `(source name, PackageSource)` rows for audit sources;
- one disabled-source key batch applied after the final merge;
- compact mapping-source rows whose `ItemRange` values address one contiguous
  pattern array and one shared text buffer.

Package and audit sources share case-insensitive keyed add/replace, clear, and
remove behavior. Each source retains its expanded URL and typed v2/v3 protocol.
Disabled keys remove matching package sources only; audit sources have their
own independent section.

Within `packageSourceMapping`, each `packageSource` requires a key and at least
one nonempty `package` pattern. Duplicate source keys in one file fail like the
official parser. A higher-precedence source group replaces the lower group's
complete pattern range; clear resets the section. Patterns and source keys are
matched case-insensitively.

## Mapping

Mapping follows NuGet's specificity rule. An exact package ID outranks every
wildcard, a longer wildcard prefix outranks a shorter prefix, and sources tied
at the winning specificity are all eligible. The first `*` terminates a prefix
pattern, matching NuGet's search-tree behavior.

Package/version enumeration and archive acquisition skip ineligible endpoints.
Matching scans the immutable source and pattern arrays without allocation.
On a cache miss, restore now computes the global winning specificity before it
touches a source. Only enabled sources tied at that rank may perform local-feed
discovery, v2 materialization, or v3 service-index I/O. An identity discovered
later in the dependency graph can activate another mapped source without
eagerly discovering every configured feed. Tied v3 sources are discovered
concurrently through the existing bounded 24-task Tokio set.

The global packages and fallback caches remain source-independent, matching
NuGet: an exact cached package or a cached version batch can satisfy an
otherwise unmapped identity without source work. A cache miss with no pattern,
or with a winning pattern attached only to disabled or removed sources, fails
as `DV0412` before URL resolution, DNS, TLS, credentials, or HTTP. With no
`packageSourceMapping` section, all enabled sources retain their previous
behavior.

Audit source rows are parsed, merged, typed, and retained. Audit mode and level
selection are implemented by `NUGET-004`, and vulnerability endpoint discovery
by `NUGET-007`; advisory evaluation remains in `RES-024`.

## Layout And Cost

`SourceMappingEntry` is a 12-byte, four-byte-aligned row: one source index and
one 8-byte pattern range. `SourcePattern` is also 12 bytes: one 8-byte text span,
one prefix/exact flag, and padding. Final mapping text has one allocation;
source and pattern records are contiguous boxed slices. The temporary XML merge
owns external variable strings only until compaction, then drops them before
the graph walk.

`PackageSourceMapping` is 48 bytes with eight-byte alignment. The restore-owned
`LazyServiceEndpoints` state is 40 bytes with eight-byte alignment and owns one
source-indexed `Vec<Option<ServiceEndpoint>>` plus an immutable `Arc` snapshot
borrowed by in-flight tasks. These layouts have compile-time assertions.
`ASSUMPTION: the Windows x64 benchmark host has 64-byte cache lines - affects
the expectation that either hot control record fits in one cache line; neither
record is explicitly over-aligned because workers read snapshots rather than
mutating adjacent records.`

The hot mapping pass reads source rows, their contiguous pattern ranges, and
the shared text buffer linearly. The common matching branches are predictable
for exact IDs and namespace prefixes; source selection then scans only the
small winning source set. Variable source counts require one restore-lifetime
slot allocation. A snapshot allocation and endpoint clone occur only when a
new source becomes reachable, amortized across later identities; an empty or
already-discovered selection does not allocate endpoint work. Network jobs are
created only for selected, undiscovered v3 sources and remain globally bounded.

A configured mapping batch is shared with Tokio tasks through one `Arc`; task
creation clones only the reference count. The common no-mapping path stores
`None`, so it allocates no policy object and performs no atomic reference-count
updates. `ASSUMPTION: ordinary developer configurations contain single-digit
source and pattern counts - affects retaining the compact linear scan instead
of a heap-built trie.` A compiled index should be considered only after
representative configuration measurements cross that threshold.

## Verification

Unit tests cover audit-source protocol replacement, nested mapping replacement
and clear, duplicate/empty group rejection, case-insensitive exact matching,
wildcard prefixes, longest-pattern selection, tied sources, the global fallback
pattern, pre-discovery filtering, pre-network unmapped failure, and cached exact
and ranged identities that require no mapped source.

The process fixture has four machine, additional-user, main-user, and
repository levels. An oracle adapter built outside timing uses the selected
SDK's official `NuGet.Configuration` assembly to assert the final three named
package sources and their enabled state/protocols, the final audit source, and
mapping results for selected, decoy, disabled, and cleared identities. The
enabled v2 decoy is ordered ahead of the selected v3 source in `dv`; package
preflight proves both tools still acquire `Newtonsoft.Json` from the mapped v3
source, then compares the package directory, identity, version, and SHA-512.
Oracle construction and package setup stay outside the timed interval.

Thirty retained Windows samples measured `527.659 ms` for `dotnet restore`
and `5.850 ms` for `dv restore`, a `90.2x` median improvement.

```powershell
cargo bench-all --case nuget_source_sections --samples 30 --warmups 3
```

`NUGET-013` has a separate fresh-workspace failure fixture. Both tools reject
`Unmapped.Package` under an enabled mapping while an unreachable v3 source is
configured, and preflight rejects any source-contact diagnostic. The timed
commands therefore compare project/config discovery, cache-miss proof,
longest-pattern selection, and structured failure with zero HTTP requests:

```powershell
cargo bench-all --case nuget_source_mapping --samples 30 --warmups 3
```

Thirty retained Windows samples measured `639.131 ms` for Microsoft and
`8.445 ms` for `dv`, a `75.7x` median improvement. The full distribution and
commands are retained in the
[source-mapping baseline](performance-baselines/2026-08-01-nuget-source-mapping-windows.md).
