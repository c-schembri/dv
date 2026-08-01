# Repository Root Discovery

`WS-004` separates repository-boundary discovery from project selection. The
public transform accepts one existing file or directory and returns the nearest
ancestor containing a `.git` directory or gitfile. It never enumerates or
parses projects.

## Data Contract

Input:

- one borrowed platform path;
- an existing regular file or directory;
- at most 65,535 ancestor marker probes.

Transform:

1. Make the input absolute without canonicalizing it.
2. For a file input, begin at its parent directory.
3. Probe `<ancestor>/.git` once at each level, nearest first.
4. Accept a directory or gitfile marker and move the successful path buffer
   into the result.

Output is one owned `RepositoryRoot`: an absolute `PathBuf`, a one-byte
`RepositoryKind`, and a `u16` probe count. On 64-bit targets the record is 40
bytes on Windows and 32 bytes elsewhere, aligned to `usize`. The result owns
its path for the command lifetime; the reporter converts it to text only at
the human or JSON edge.

The access pattern is a linear parent walk with one filesystem metadata query
per ancestor. The missing-marker branch is expected until the nearest root;
invalid marker types and I/O failures leave the common path immediately. One
path buffer is reused for every probe, with no per-ancestor dynamic allocation.

## Boundaries

- Missing starts, non-file/non-directory starts, marker I/O failures, invalid
  marker types, absent roots, and the probe-count bound are distinct failures.
- Symlink markers are rejected until `WS-007` defines link and escape policy.
- Path spelling is preserved lexically; canonical identity belongs to
  `WS-006`.
- Only Git is supported in this slice. Other repository systems remain
  unsupported rather than adding unrelated probes to every ancestor.
- `dv project root [PATH]` accepts at most one path and rejects options or
  extra operands before filesystem I/O.

## Cost And Evidence

The representative fixture starts three levels from a gitfile and contains an
invalid project file to prove that discovery performs exactly three marker
metadata queries and no project read. The parity preflight compares the result
with MSBuild's `GetDirectoryNameOfFileAbove(..., '.git')`, verifies the typed
Git/probe fields, and proves zero fixture mutation.

Thirty warm Windows samples measured Microsoft at `137.639 ms` median and
`161.395 ms` p95 versus `5.007 ms` and `5.696 ms` for `dv`, a `27.5x` median
improvement. Cold filesystem-cache behavior is unverified; reproduce it with a
fresh machine rather than treating a fresh process as a cold disk.

```powershell
cargo bench-all --case repository_root --samples 30 --warmups 3
```
