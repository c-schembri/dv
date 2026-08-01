# Workspace Candidate Discovery

`WS-001` is the bounded directory-to-candidate transform used before project
evaluation. It recognizes immediate `.csproj`, `.fsproj`, `.vbproj`, `.sln`,
and `.slnx` files in one filesystem enumeration and does not read their
contents.

## Contract

Input is one existing directory. Relative input spelling is made absolute
without canonicalizing it. The command opens the directory once, reads each
entry once, and queries each entry's file type once. Subdirectories, symlinks,
and unrelated extensions are dropped. A missing root returns `NotFound`; a
non-directory root, more than 65,535 candidates, a candidate name longer than
65,535 UTF-8 bytes, or an arena beyond 4 GiB returns an explicit error.
Recognized non-Unicode candidate names return `NonUnicodePath` rather than
silently disappearing.

Output owns the normalized absolute root, one contiguous UTF-8 filename arena,
and candidates sorted by their preserved relative spelling. Each
`WorkspaceCandidate` has three fields:
a `u32` text offset, `u16` byte length, and one-byte kind. The record is 8 bytes
at alignment 4, so eight records occupy one assumed 64-byte cache line. The
owner is 80 bytes on 64-bit Windows and 72 bytes on other current 64-bit
targets because Windows `PathBuf` is larger.

The observed benchmark corpus contains 58 project/solution files across 47
directories, with 0 to 7 immediate candidates per directory. Successful
inventories reserve eight records and 256 path bytes on the first recognized
candidate; an empty directory allocates neither buffer. Variable path storage
is necessary because filenames are external data. Irrelevant entries do not
materialize a full `PathBuf`; full paths are constructed only at error or
consumer boundaries. The common scan is linear; the file-versus-directory and
recognized-extension branches are predictable in ordinary source roots, while
kind selection is cold. Sorting is over compact records and candidate counts
observed here are tiny.

Implicit C# evaluation consumes the same batch. One C# project proceeds; zero
candidates fails; one unsupported project/solution kind fails explicitly; and
ambiguity reports up to the first 16 sorted candidates plus the remaining
count. Full paths are constructed only for the selected or diagnostic row.

`ASSUMPTION: the benchmark machine has 64-byte cache lines - affects the
records-per-line statement, not record correctness or layout assertions.`

## Deferred Boundaries

Recursive repository discovery, root markers, symlink/junction traversal,
filesystem identity, platform case behavior, configured output exclusions,
and solution parsing remain separate transforms. Their evidence requirements
are recorded in `issues/0013-recursive-workspace-discovery.md`; the immediate
selection path does not speculate about them.

## Verification

Focused tests cover every candidate kind, stable ordering, ignored files and
nested directories, compact working-set bytes, one unsupported candidate,
Linux non-Unicode rejection, and ordered ambiguity. The retained benchmark
compares Microsoft implicit project selection/evaluation with
`dv project inspect --json` on the identical
immutable `small-console` directory and validates the same evaluated property
and item batch before timing. Thirty retained samples after three warm-ups
measured Microsoft at `290.493 ms` median and `305.210 ms` p95 versus
`5.287 ms` and `6.048 ms` for `dv`, a `55.0x` median improvement.
