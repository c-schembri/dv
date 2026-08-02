# Ancestor Build Input Discovery

`WS-005` discovers the files which establish SDK, NuGet, MSBuild, and central
package context before project evaluation. `dv project inputs [PATH]` exposes
the same command-local batch used by SDK selection, package configuration, and
central package discovery.

## Precedence

One nearest-to-root walk applies each input family's actual rule:

- the nearest `global.json` wins;
- every ancestor `NuGet.Config` is retained from filesystem root to the start
  directory, so later files have higher merge precedence;
- the nearest `Directory.Build.props` wins independently;
- the nearest `Directory.Build.targets` wins independently;
- the nearest `Directory.Packages.props` wins independently.

The start may be a file or directory. A file starts discovery at its parent.
Paths become absolute and lexically normalized without canonicalization. Input
contents are not opened or parsed by discovery. Missing markers produce empty
views; a known marker with a non-file type fails instead of falling through to
a parent file.

On case-sensitive systems NuGet's three recognized ancestor spellings are
probed in precedence order: `nuget.config`, `NuGet.config`, then
`NuGet.Config`. Windows needs one case-insensitive probe. macOS performs a
directory enumeration only after a successful probe to preserve the actual
entry spelling. Regular files reached through filesystem links retain the link
spelling. This bounded marker lookup does not recurse through a target
directory; consumers treat the selected marker as one explicit file input.

## Data Layout

Consumers request input families through the one-byte `AncestorInputRequest`
mask. Each successful marker becomes an eight-byte `AncestorInput` row:

```text
u32 file length | u16 ancestor depth | u8 kind | u8 spelling
```

Eight rows occupy one assumed 64-byte benchmark-host cache line. The batch
keeps eight rows inline and spills into one contiguous vector only for larger
hierarchies. It stores one start `PathBuf`, not one allocated path per result;
consumers materialize paths into a reusable buffer or at an ownership boundary.
The complete batch is 160 bytes on 64-bit Windows and 152 bytes on other
64-bit targets, aligned to `usize`.

Singleton requests stop as soon as their nearest match is found. SDK selection
therefore probes only `global.json`; central package discovery probes only
`Directory.Packages.props`. NuGet configuration alone walks to the root because
its merge semantics require the full hierarchy. The diagnostic command requests
all five families in one walk. Metadata probes, visited ancestors, macOS casing
enumerations, and retained row bytes are explicit bounded evidence.

## Verification

Core tests cover independent nearest matches, root-to-leaf NuGet order,
single-family early exit, file starts, absent inputs, invalid marker types, and
inline row storage. Existing SDK, central package, and NuGet hierarchy tests
exercise the shared implementation through production consumers. CLI tests
prove option validation happens before filesystem work and malformed marker
contents are not parsed.

The benchmark fixture has three directory levels, two NuGet configurations,
and independently located singleton inputs. Preflight compares the typed `dv`
event with MSBuild property-function queries and proves neither command mutates
the fixture. Thirty warm Windows samples measured Microsoft at `139.917 ms`
median and `145.349 ms` p95 versus `4.845 ms` and `6.203 ms` for `dv`, a
`28.9x` median improvement. No retained sample was removed.

```powershell
cargo bench-all --case workspace_inputs --samples 30 --warmups 5
```

Raw samples: `target/workspace-inputs-final-2.json`.
