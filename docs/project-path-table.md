# Command-Local Project Path Table

`WS-010` replaces closure-owned `PathBuf` identity vectors and the separate
multi-root merge vector with one compact table shared by every root and
reference in a command.

## Data Contract

Input is an ordered batch of already evaluated `ProjectSpec` roots. Each root
contains a variable ordered batch of relative project references. The observed
common case is one root with zero references. The checked diamond fixture has
four projects and four edges; the benchmark exercises a two- or three-project
physical graph depending on active-filesystem case behavior.

`ProjectClosureBatch` accepts roots incrementally so CLI root loading and each
root's closure retain their previous error and output order. Output is one
root-first `Vec<ProjectSpec>` with every lexical/physical project identity
retained once. It owns projects for the remainder of the restore/build command.
The path table is scratch state and dies immediately after closure construction.

The transform is:

```text
evaluated root batch
  -> append one root
  -> absolute lexical identity bytes
  -> sorted lexical offset lookup
  -> canonical physical identity bytes when references/multiple roots require it
  -> sorted physical offset/project-index lookup
  -> distinct project | no-link alias | link alias
  -> evaluate each distinct reference and append its closure
  -> root-first unique ProjectSpec batch
```

Empty batches produce an empty output. Mixed configurations fail as
`DV0205`. Missing/malformed references fail before insertion. A no-link
physical alias adds only its lexical spelling; a link/reparse alias remains
`DV0207`. One path or the total command table exceeding 4 GiB fails explicitly
before its span is published.

## Layout And Cost

`CommandPathTable` is a 72-byte owner containing three contiguous allocations:

- one byte arena containing opaque `OsStr::as_encoded_bytes()` copies;
- sorted 8-byte lexical `(start, length)` spans;
- sorted 12-byte physical `(start, length, project_index)` rows.

The encoded bytes are compared and discarded in the same process. They are
never decoded, persisted, or used as a wire protocol, so non-Unicode platform
paths do not require an unsafe reconstruction API. Lexical and physical paths
with identical encoded bytes share one span. Compile-time assertions protect
the owner and row layouts. Under the assumed 64-byte benchmark cache line,
eight lexical rows or five physical rows fit per line. The prior Windows rows
retained one 32-byte lexical `PathBuf` plus one 40-byte physical record per
project, each with separately owned external path storage.

Reference access is linear in declared order. Lexical and physical lookups are
logarithmic over contiguous sorted indices; byte comparisons are data-dependent
and usually reject on an early path component. New/already-seen branches are
predictable in ordinary sparse graphs, while diamonds take the colder alias
branch. New records append bytes linearly and insert compact indices
contiguously. The output project vector reserves from the observed root and
immediate-reference counts.

The one-root/zero-reference common case never creates a path table, performs no
canonicalization, and allocates only the required output `Vec<ProjectSpec>`.
Multi-root restore pushes into one batch, removing the CLI's third sorted
`Vec<PathBuf>` and avoiding re-evaluation of shared referenced projects.

`ASSUMPTION: a 64-byte cache line describes the first benchmark machine -
affects stated rows per line, not correctness or persisted layout.`

## Persistence Decision

`dv` currently has no watch loop, daemon, or repeated-command process lifetime.
Retaining this scratch capacity after command completion therefore cannot
reduce a measured process invocation and would increase retained memory plus
state invalidation surface. No persistent cache is added. Capacity is reused
across every root pushed during the real command-local lifetime. `RUN-012`
must remeasure unchanged repeated evaluations before moving the table into a
watch session; that result is **unverified** today.

The traversal is sequential. The observed graph is far below a worker
crossover, and filesystem metadata dependencies make a worker pool strictly
more scheduling and merge work here. The table has no locks, shared mutation,
network work, writes, or false-sharing boundary.

## Simplification

The change removes two closure identity vectors, the CLI merge vector,
per-identity `PathBuf` ownership, duplicate shared-reference evaluation across
roots, and zero-reference identity setup. It adds no hash table, path interner
dependency, daemon, cache invalidation protocol, platform case flag, or
speculative persistence API.

## Verification

Focused tests cover empty/single closure behavior, diamond ordering, incremental
multi-root ordering, shared-reference deduplication, mixed configurations,
compact arena spans, active-filesystem case identity, and link cycle/escape
failures. Existing restore integration tests prove multi-root event order and
physical deduplication.

The retained active-filesystem oracle resets identical package-free project
graphs outside timing and checks Microsoft assets against `dv` events. An
adjacent 30-sample A/B run measured the `WS-009` parent at `5.830 ms` median
versus `5.684 ms` for the compact table, a `2.5%` reduction. In the current
like-for-like run Microsoft measured `507.094 ms` median and `532.656 ms` p95
versus `5.684 ms` and `6.429 ms` for `dv`, an `89.2x` median improvement. No
sample was removed; machine-cold filesystem state remains unverified.
Every process starts with an empty command table, so there is no distinct warm
table state in the current product lifetime. Warm persistent-session coverage
remains TBI with `RUN-012`; repeated samples exercise warm OS caches only.

```powershell
cargo bench-all --dv target/dv-ws009-parent.exe --case filesystem_case_identity --samples 30 --warmups 5 --output target/filesystem-case-ws010-parent-paired.json
cargo bench-all --case filesystem_case_identity --samples 30 --warmups 5 --output target/filesystem-case-ws010-current-paired.json
```
