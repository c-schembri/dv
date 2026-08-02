# Filesystem Link Safety

`WS-007` applies one fail-closed physical-identity policy where workspace work
can actually traverse links: recursive default-source discovery, linked
immediate project candidates, and project-reference closure.

## Data Contract

Source input is one existing project directory containing a variable batch of
platform directory entries. Output is the same sorted batch of logical,
portable `.cs` paths consumed by `ProjectSpec`; link spelling is preserved and
physical target spelling is cold diagnostic data only.

The source transform is:

1. Traverse normal directories with the existing single `Vec<PathBuf>` DFS
   stack and one file-type query per entry.
2. Drop `bin`, `obj`, dot-prefixed metadata/cache trees, and configured output
   roots before inspecting a link target.
3. For a remaining link, query the target type and establish its physical path.
4. Lazily establish the physical project root and reject targets outside it.
5. On the first directory link, seed a cold active-identity vector from the
   physical root through the current ordinary directory ancestry.
6. Traverse that linked subtree with explicit enter/leave work records. Reject
   any link target already in the active physical ancestry, including cycles
   between sibling directories rather than only links to lexical ancestors.
7. Follow safe directories or retain safe `.cs` file links under their logical
   relative spelling, then sort the completed batch.

Project-reference input is the evaluated literal reference batch. Missing
paths fail before identity creation. On the first existing reference, closure
lazily creates one shared encoded-byte table with sorted lexical and physical
offset indices seeded by the root project. Each successfully evaluated physical
identity is inserted once; a different logical spelling that resolves to an
existing identity fails as `DV0207`.
Lexically explicit `..` references remain valid. A lexically in-directory
reference whose physical target leaves that directory is an escape.

## Cost And Layout

The link-free source path adds no allocation or filesystem operation inside the
normal directory/file arms. It remains a linear scan over one contiguous path
stack followed by an unstable sort of source strings. A followed directory
link pays one target metadata query, target canonicalization, one lazy root
canonicalization, and cold contiguous work/active vectors. Ordinary descendants
derive their physical identity by appending the observed entry name; nested
links alone pay another canonicalization.

Project closure pays physical canonicalization only when references exist. Its
compact sorted offset indices use logarithmic lookup and contiguous insertion.
Source link ancestry uses linear lookup because observed link depths are tiny
and the batch must preserve stack order. These pointer-bearing cold records are
justified because platform paths are variable-sized external data; project
identities now use the `WS-010` shared byte arena instead. No mutable worker
state, alignment boundary, or false-sharing risk is introduced. `ProjectError`
remains 64 bytes on 64-bit Windows and 56 bytes on other 64-bit targets.

`ASSUMPTION: filesystem links are rare in ordinary project trees - affects
keeping canonicalization and physical-path allocation on a cold branch.`

The traversal is deliberately sequential. Each directory reveals the next
batch and the measured three-source link case is far below any plausible worker
crossover; scheduling and deterministic merge work would dominate its metadata
queries.

## Boundaries

- Safe file and directory links inside the selected source root are followed.
- Every active physical-ancestry cycle, broken link identity,
  non-file/non-directory target used for traversal, and physical root escape
  fails closed.
- Fixed and configured excluded links are dropped without target resolution
  because the transform cannot traverse them.
- Project-reference aliases cannot re-enter an evaluated physical project.
- Ordinary explicit `../Library/Library.csproj` references remain valid.
- Active-filesystem case equivalence reuses physical project identities through
  `WS-008`; physical file metadata beyond path identity belongs to `WS-011`.

Failures use `DV0207` with ordered `path`, `workspace_root`, and
`resolved_target` context plus a corrective action.

## Verification

Focused cross-platform tests create real Unix symlinks or Windows junctions
for safe logical-source retention, ancestor and sibling-directory cycles,
source escapes, and project-reference cycles/escapes hidden under excluded
output trees. Benchmark preflight repeats every control, compares the safe
three-source batch with MSBuild, and proves all fixture trees remain unchanged.

Thirty warm Windows samples measured Microsoft at `301.738 ms` median and
`305.440 ms` p95 versus `4.473 ms` and `5.252 ms` for `dv`, a `67.5x` median
improvement. No sample was removed. Cold filesystem-cache behavior is
unverified because the harness cannot flush the host cache portably.

```powershell
cargo bench-all --case source_link_traversal --samples 30 --warmups 5 --output target/source-links-ws007-final.json
```
