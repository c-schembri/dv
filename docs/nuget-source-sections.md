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
Service indexes are still discovered as one batch before that filter; moving
mapping ahead of service-index requests and emitting a dedicated unmapped-ID
diagnostic is the remaining `NUGET-013` work.

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

A configured mapping batch is shared with Tokio tasks through one `Arc`; task
creation clones only the reference count. The common no-mapping path stores
`None`, so it allocates no policy object and performs no atomic reference-count
updates. The expected configured case has single-digit source and pattern
counts, so a predictable linear scan is smaller and faster than a heap-built
trie. A compiled index should be considered only after a measured real
configuration crosses that threshold.

## Verification

Unit tests cover audit-source protocol replacement, nested mapping replacement
and clear, duplicate/empty group rejection, case-insensitive exact matching,
wildcard prefixes, longest-pattern selection, tied sources, and the global
fallback pattern.

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
