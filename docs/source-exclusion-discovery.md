# Default Source Exclusion Discovery

`WS-009` removes generated, metadata, and tool-owned directory trees before
default C# source traversal performs work inside them.

## Data Contract

Input is one evaluated SDK-style project directory plus its recursive batch of
filesystem entries. The fixed exclusion protocol is:

- `bin` and `obj`, compared with protocol-defined ASCII-insensitive spelling;
- every directory whose first encoded filename byte is `.`, including VCS
  metadata and dot-prefixed tool caches;
- the resolved `BaseOutputPath`, `BaseIntermediateOutputPath`, `OutputPath`,
  `IntermediateOutputPath`, and `ArtifactsPath` values when present.

Configured values may be absolute or project-relative. The current evaluator
expands only its known selected-project dimensions: project directory/name,
configuration, target framework, runtime identifier, assembly name, artifacts
path, and base output/intermediate paths. An unknown expansion fails rather
than guessing. Wildcards, lists, and unterminated expansions are invalid at
this boundary. General MSBuild property/import evaluation remains owned by the
`EVAL-*` rows, and final compiler output layout remains owned by `PROJ-010`.

Output is the existing sorted, portable source-path batch owned by
`ProjectSpec` for the downstream command lifetime. Exclusions are command-local
and discarded after evaluation. A configured path equal to or above the
project root produces an empty default source batch.

The repository snapshot contains 63 `.csproj` files and 47 tracked `.cs`
files. Only the retained oracle fixture configures one of the five path
properties. Its concrete distribution is two retained source files and 13
excluded directories: `bin`, `obj`, `.git`, eight other dot directories, and
two configured generated trees.

## Transform And Cost

```text
raw configured path properties
  -> bounded known-property expansion
  -> lexical absolute normalization
  -> sorted fixed-capacity path batch
directory entry
  -> file type
  -> fixed-name rejection
  -> configured-prefix rejection
  -> descend, retain .cs, or validate a link
  -> sorted portable source batch
```

The common project has no configured paths. It retains a zero-length
`SourceExclusionBatch`, does no exclusion heap allocation or canonicalization,
and performs the leading-dot/`bin`/`obj` classifier only for directory and link
entries. Rejection happens before link-target metadata, identity resolution,
or descent, so every pruned tree removes all work proportional to its contents.

The configured cold path stores at most five sorted `PathBuf` records in an
inline array. Literal property text remains borrowed; property substitution and
separator conversion allocate only when needed, while retained variable-sized
OS paths necessarily own their buffers. The batch is 168 bytes on 64-bit
Windows and 128 bytes on other 64-bit targets, aligned to `usize`. A `PathBuf`
record is 32 or 24 bytes respectively, so two records fit the assumed 64-byte
benchmark cache line. Compile-time assertions protect these layouts.

Traversal access is linear over directory batches and the configured array.
The fixed-name branches are predictable for ordinary source directories; entry
types and configured prefix matches are data-dependent. An ASCII case-only
configured match uses physical identity on that cold branch so behavior comes
from the active filesystem. Exact paths and sensitive-filesystem mismatches do
not canonicalize.

`ASSUMPTION: a 64-byte cache line describes the first benchmark machine -
affects the stated records per line, not correctness or persisted layout.`

Traversal remains sequential. The observed two-source fixture is below any
credible worker crossover, and pruning eliminates work more cheaply than
scheduling it. There are no locks, worker counters, alignment padding, network
requests, writes, or child processes in production discovery.

## Simplification

The design removes traversal rather than optimizing work after discovery. It
uses no glob engine, regex, hash table, dynamic exclusion vector, platform case
flag, recursive task queue, or speculative visible cache names. `target`,
`packages`, `node_modules`, and other ordinary visible directories remain
discoverable unless one is an explicitly configured output path. This matches
the observed .NET 10 default-item boundary instead of inventing policy.

## Verification

Focused tests cover fixed and configured trees, selected-dimension expansion,
project-root coverage, unknown dynamic paths, active-filesystem casing, and
excluded junctions/symlinks that would otherwise escape the workspace. The
benchmark preflight creates ignored `bin`, `obj`, and `.git` controls outside
timing, requires both tools to return exactly `Program.cs` and
`Source/Feature.cs`, and rejects any workspace mutation.

Thirty warm Windows samples on 2026-08-02 measured Microsoft at `264.677 ms`
median and `268.625 ms` p95 versus `4.139 ms` and `4.639 ms` for `dv`, a
`63.9x` median improvement. No sample was removed. Cold filesystem-cache state
is unverified because the harness cannot portably flush it.

The affected ordinary project-evaluation path measured `dv` at `3.998 ms`
median and `4.230 ms` p95. Safe linked-source traversal measured `4.101 ms`
median and `4.649 ms` p95, below its retained `WS-007` median of `4.473 ms`.

```powershell
cargo bench-all --case source_exclusions --samples 30 --warmups 5 --output target/source-exclusions-ws009-final.json
cargo bench-all --case project_evaluate --samples 30 --warmups 5 --output target/project-evaluate-ws009-final.json
cargo bench-all --case source_link_traversal --samples 30 --warmups 5 --output target/source-links-ws009-final.json
```
