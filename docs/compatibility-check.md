# Compatibility Check

`dv compat check PATH...` statically classifies explicitly supplied scripts
and project inputs against the compatibility artifact embedded in the running
binary. It never executes a discovered command.

## Batch Contract

Input is an ordered batch of one or more regular files or directories.
Directory discovery is deterministic, skips VCS/build output trees and accepts
up to 4,096 files through 4,096 directories. The corpus is bounded to 32 MiB,
one MiB per UTF-8 file and 4,096 discovered invocations. Script candidates
include YAML, PowerShell, POSIX shell, cmd/batch, `Dockerfile`, and `Makefile`;
MSBuild XML candidates include C#, F#, VB, generic project, props, and targets
files. Explicit files are inspected regardless of extension.

The line tokenizer recognizes literal Microsoft tool positions after common
script prefixes, environment assignments and command boundaries. It retains
up to 64 stack-backed tokens per line and redacts secret option values before
they enter the report. Shell expansion, computed command selection, quoting,
piping, and redirection that change observable behavior are `uncheckable`:
static analysis neither runs nor guesses them. MSBuild XML is parsed with DTDs
rejected; the scanner classifies literal SDK/target-framework shapes and scans
`Exec Command` values through the same script transform. Malformed or
out-of-bounds input fails with a stable diagnostic instead of being ignored.

The transform is:

```text
ordered paths
  -> bounded deterministic discovery and one read per file
  -> project-shape / Exec XML scan or literal script scan
  -> literal tool-token extraction
  -> indexed embedded-manifest classification
  -> one ordered compatibility report
```

The report owns contiguous input and invocation vectors until reporting ends.
Each invocation stores its input index plus one-based line/column instead of a
path copy. Human and JSON output render the same batch. JSON uses event schema 21;
the report separately identifies compatibility manifest version 1. A check
returns exit 0 only when every retained record is `implemented`; `partial`,
`missing`, and `uncheckable` records return the native unsupported exit 2.

## Cost And Layout

Access is linear over paths, file bytes, XML events, lines, and tokens. Common
literal text takes predictable whitespace/tool branches. Rare malformed and
dynamic syntax exits into cold diagnostic/report paths. The build script
validates the manifest, removes already-implemented parity rows and emits
tool-grouped static command ranges, so production performs no manifest JSON
parse and each lookup scans only the chosen tool's bounded command batch.

`CompatibilitySupport` and the internal tool key are one byte, each command
range is four bytes, and each stack token is 12 bytes. The 64-token line batch
therefore occupies 768 stack bytes. The 270,192-byte release manifest is
projected at build time into immutable static command slices, ranges, and only
unresolved row IDs. On 64-bit builds each three-field command record is 40 bytes:
the 115-record table occupies 4,600 bytes before its borrowed path/row slices,
and a lookup scans at most the 77 dotnet records. Static slice references avoid
duplicating manifest strings; the bounded linear lookup is cheaper than a
runtime-built hash index at this scale. Externally sized paths, redacted
command text, and result
batches require dynamic storage; capacities are reserved from discovered
counts and die with the command. There is no shared mutable worker state,
cache-line isolation requirement, process launch, SDK discovery, command
execution, or network request. Sequential work avoids scheduling overhead for
the benchmark corpus of one 195-byte script and one 278-byte project.

The design must be revisited if representative repositories exceed the
file/directory/corpus bounds, if static parsing time becomes material beside
process startup, or if supported automation depends predominantly on dynamic
or multiline command construction.

The retained Windows measurement is in the
[static compatibility-check baseline](performance-baselines/2026-08-02-compatibility-check-windows.md).
