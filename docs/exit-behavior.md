# Exit Behavior Contract

`CLI-007` separates command results from process exit policy. Native `dv`
retains its stable current failure code, while an explicitly selected
compatibility profile maps the same typed failure to the reference tool's
observed process code.

## Reference Snapshot

The Windows oracle was captured on 2026-08-02 from .NET SDK `10.0.100`, MSBuild
`18.0.2.52411`, and NuGet `7.0.0.0`. VSTest is the copy bundled with that SDK.
The retained probes produced this matrix:

| Reference invocation | Observed exit |
|---|---:|
| `dotnet --version` | 0 |
| `dotnet frobnicate` | 1 |
| `dotnet restore DefinitelyMissing.csproj` | 1 |
| `dotnet build DefinitelyMissing.csproj` | 1 |
| `dotnet run --project DefinitelyMissing.csproj` | 1 |
| `dotnet test DefinitelyMissing.csproj` | 1 |
| `dotnet test SmallConsole.csproj --no-restore --nologo` (no tests) | 0 |
| failing `dotnet test` xUnit fixture | 1 |
| same xUnit fixture with an unmatched filter | 0 |
| `dotnet msbuild --definitely-invalid` | 1 |
| `dotnet nuget frobnicate` | 1 |
| `dotnet vstest DefinitelyMissing.dll` | 1 |
| failing direct `vstest.console` xUnit assembly | 1 |
| same direct VSTest assembly with an unmatched filter | 0 |
| `dotnet run --project ArgumentForwarding.csproj --no-build --no-restore -- exit 23` | 23 |

The P1 compatibility policy therefore maps every currently reachable typed
failure class to `1` for `dotnet`, `msbuild`, `nuget`, and `vstest` profiles.
Success and the observed no-tests result remain `0`. Native mode maps usage,
unsupported, operation, build, restore, test, and cancellation failures to `2`,
preserving `CLI-003` and existing automation. The xUnit probes used a generated
`net10.0` project with `Microsoft.NET.Test.Sdk` 17.14.1, xUnit 2.9.3, and the
Visual Studio runner 3.1.4. These test probes are retained as oracles, not
execution claims; the `dv test` workflow does not exist yet.

## Typed Boundary

`InvocationMode` is a one-byte enum selected by
`--compat dotnet|msbuild|nuget|vstest`. The process-lifetime invocation owner
retains it as cold provenance and combines it with the three-byte global output
record only when producing a four-byte `InvocationOptions` value. The semantic
`InvocationRequest` no longer carries provenance or the raw command index: it
is 6 bytes at alignment 2, so compatibility does not enlarge the hot dispatch
record or add an allocation.

The parser removes the selector from semantic command operands during its one
linear argument scan. Selection therefore completes before current-directory,
SDK, project, filesystem, process, or network discovery. A missing, unknown,
non-Unicode, or repeated selector is rejected at that boundary. A selector may
precede the command or be interspersed with operands, matching the existing
global-option policy.

`DROP-003` stores scan-only policy in a five-byte, byte-aligned record: three
global-option bytes, the mode byte, and one explicitness bitset. All eight
separated/combined selector forms use one selection function, while the common
native path remains the zero-valued default. The record is transient and adds
no allocation or persistent request bytes. Executable-name inference is a
separate source of mode evidence reserved for `DROP-012`.

The policy terminal set is success, usage, unsupported surface, operation,
build failure, restore failure, test failure, no tests, and cancellation. Its
one-byte class and the one-byte invocation profile index a 45-byte immutable
matrix. Inapplicable tool/outcome pairs contain an explicit sentinel rather
than a plausible exit code. A reachable lookup is one indexed byte read and one
sentinel comparison. It is allocation-free and performs no formatting,
filesystem access, process launch, or network work. Build planning and restore
classify operational errors before the lookup rather than collapsing them into
a generic failure. The matrix is read-only and densely traversed by tests;
there is no mutable worker state or cache-alignment requirement.

`CLI-014` gives cancellation a distinct matrix column, `DV0005`, and cancelled
event outcome.

`ASSUMPTION: interrupted dotnet, MSBuild, NuGet, and VSTest work uses the
current generic compatibility failure code 1 on Windows - affects the
provisional cancellation cells and must be replaced by signal-driven oracle
evidence in RUN-009 and TEST-021.`

`CLI-015` separates launch/wait failure from a reaped child and retains the
latter's exact `i32` exit without compatibility remapping. Reference-specific
executed-child, executed-test, signal, and forced-cancellation behavior remains
partial under `DROP-016`, `RUN-009`, and their owning workflows.

`--compat` selects both this exit policy and the `DROP-010` command-route
precedence. The profile prevents an ambiguous NuGet, MSBuild, or VSTest word
from entering a native workflow, but it does not claim that the routed grammar
is implemented. Full reference grammar, diagnostic prose, stdout/stderr
formats, and automatic executable-token inference remain open in the
corresponding `DROP-*` rows. Compatibility manifest version 1 records those
surfaces and their partial/missing states instead of implying that capture
equals execution support.

`DROP-011` also makes an explicitly selected profile observable without
scraping diagnostic prose. Every selected-profile failure carries exactly one
ordered `compatibility_profile` context field in the shared structured
diagnostic batch, so human and JSON output expose the same value. Native
failures and malformed, unsupported, or repeated selectors omit the field.
This reporting transform does not alter failure classification or exit-code
selection.

## Evidence

Integration tests cover every explicit profile, native and compatibility
failure codes, typed build/restore failures, early invalid-selector rejection,
and selector removal from a successful SDK command. The like-for-like benchmark
compares `dotnet --version` with `dv --compat dotnet sdk current`; preflight
requires both to print the same selected SDK before samples are retained.
Thirty Windows samples after three warm-ups measured `65.901 ms` median and
`67.752 ms` p95
for `dotnet`, versus `5.225 ms` median and `6.202 ms` p95 for `dv`, a `12.6x`
median improvement. Raw samples are retained in
`benchmarks/results/baseline-1785569009.json`. The later `DROP-003`
like-for-like rejection benchmark measured `dotnet build
--definitely-unknown` at `152.984 ms` median and `193.102 ms` p95, versus `dv
--compat dotnet build --definitely-unknown` at `5.641 ms` median and `6.630 ms`
p95, a `27.1x` median improvement. Its raw samples are retained in
`benchmarks/results/2026-08-01-invocation-mode-windows.json`.
The `DROP-011` diagnostic-context run strengthened the same benchmark
preflight to require `compatibility_profile: dotnet`. Fifty Windows samples
after ten warm-ups measured `133.281 ms` median and `147.033 ms` p95 for
`dotnet`, versus `5.125 ms` median and `6.256 ms` p95 for `dv`, a `26.0x`
median improvement. Raw samples are retained in
`benchmarks/results/2026-08-02-cli-compat-diagnostics-windows.json`.
The Phase 1 workflow result benchmark compares missing-project restore after
both commands accept their syntax. Fifty samples after ten warm-ups measured
`122.756 ms` median and `134.338 ms` p95 for `dotnet`, versus `5.158 ms` and
`6.073 ms` for `dv`, a `23.8x` median improvement. The fixture remains
unchanged, both processes return `1`, and raw samples are retained in
`benchmarks/results/2026-08-02-cli-exit-policy-windows.json`.
