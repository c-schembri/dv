# Explicit Project Selection

`WS-002` is the direct path-to-project boundary shared by positional project
arguments and `--project`. The argument parser retains one borrowed OS path in
the 24-byte `ProjectSelection` value; selecting a project does not copy path
text or allocate a path container.

## Contract

Recognized `.csproj`, `.fsproj`, `.vbproj`, `.sln`, and `.slnx` extensions are
classified case-insensitively before filesystem access. A C# project proceeds
directly to file validation. Other recognized kinds fail as unsupported without
a metadata query because the current evaluator cannot consume them. Paths with
an unrecognized extension retain explicit-directory behavior only when they
name a directory.

The C# path performs one metadata query. A missing path returns `NotFound`, a
metadata failure returns `Io`, and a non-regular file returns `Unsupported`.
Only a validated regular file is made absolute and read. Read failures return
`Io` before XML parsing; malformed readable content returns `InvalidXml`.
This ordering keeps filesystem identity, XML evaluation, and later project
work outside every rejected boundary.

The earlier CLI path first queried whether every explicit path was a directory
and then queried the same C# path again in the evaluator. Candidate-kind
classification now removes that redundant metadata operation from the common
explicit-file path. Wrong-kind candidate paths perform no filesystem I/O.

Symlink identity, canonicalization, workspace escape, and filesystem case
semantics remain owned by `WS-006` through `WS-008`; this transform does not
silently choose those policies.

## Verification

Focused tests cover positional and named selectors, missing C# paths,
wrong-kind paths, candidate-shaped directories, regular-file validation, and
Unix unreadable-file errors before XML parsing. Existing parser tests prove
that repeated, mixed, empty, and non-Unicode selectors retain their typed
pre-I/O behavior.

The retained like-for-like benchmark evaluates the same explicit
`SmallConsole.csproj` and compares the complete requested MSBuild property/item
batch before timing. Thirty Windows samples after three warm-ups measured
Microsoft at `289.046 ms` median and `302.358 ms` p95 versus `5.326 ms` and
`6.219 ms` for `dv`, a `54.3x` median improvement.
