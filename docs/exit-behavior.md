# Exit Behavior Contract

`CLI-007` separates command results from process exit policy. Native `dv`
retains its stable current failure code, while an explicitly selected
compatibility profile maps the same typed failure to the reference tool's
observed process code.

## Reference Snapshot

The Windows oracle was captured on 2026-08-01 from .NET SDK `10.0.100`, MSBuild
`18.0.2.52411`, and NuGet `7.0.0.0`. VSTest is the copy bundled with that SDK.
The retained probes produced this matrix:

| Reference invocation | Observed exit |
|---|---:|
| `dotnet --version` | 0 |
| `dotnet frobnicate` | 1 |
| `dotnet restore DefinitelyMissing.csproj` | 1 |
| `dotnet build --definitely-invalid` | 1 |
| `dotnet msbuild --definitely-invalid` | 1 |
| `dotnet nuget frobnicate` | 1 |
| `dotnet vstest DefinitelyMissing.dll` | 1 |

The P1 compatibility policy therefore maps every currently reachable typed
failure class to `1` for `dotnet`, `msbuild`, `nuget`, and `vstest` profiles.
Success remains `0`. Native mode continues to map current usage, unsupported,
and operation failures to `2`, preserving `CLI-003` and existing automation.

## Typed Boundary

`InvocationMode` is a one-byte enum selected by
`--compat dotnet|msbuild|nuget|vstest`. It lives beside the three-byte global
output record in a four-byte `InvocationOptions` value. `InvocationRequest`
remains 16 bytes and pointer-aligned, so compatibility does not enlarge the hot
dispatch record or add an allocation.

The parser removes the selector from semantic command operands during its one
linear argument scan. Selection therefore completes before current-directory,
SDK, project, filesystem, process, or network discovery. A missing, unknown,
non-Unicode, or repeated selector is rejected at that boundary. A selector may
precede the command or be interspersed with operands, matching the existing
global-option policy.

Failures are classified internally as usage, unsupported surface, or operation
failure before an exit profile is applied. These classes intentionally do not
guess at test, no-tests, cancellation, or child-process semantics before those
workflows exist. Their reference-specific policies remain partial under
`DROP-016`, `CLI-014`, and `CLI-015`.

`--compat` currently changes exit policy only. Full reference grammar,
diagnostic prose, stdout/stderr formats, automatic executable-token inference,
and the generated compatibility manifest remain open in the corresponding
`DROP-*` rows.

## Evidence

Integration tests cover every explicit profile, native and compatibility
failure codes, early invalid-selector rejection, and selector removal from a
successful SDK command. The like-for-like benchmark compares `dotnet
--version` with `dv --compat dotnet sdk current`; preflight requires both to
print the same selected SDK before samples are retained. Thirty Windows
samples after three warm-ups measured `65.901 ms` median and `67.752 ms` p95
for `dotnet`, versus `5.225 ms` median and `6.202 ms` p95 for `dv`, a `12.6x`
median improvement. Raw samples are retained in
`benchmarks/results/baseline-1785569009.json`.
