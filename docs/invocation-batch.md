# Invocation Batch Contract

`CLI-005` establishes the process-entry transform used by canonical and
compatibility grammars. `DROP-002` makes every currently accepted spelling
normalize to one exact semantic command kind while retaining the original OS
tokens only in the owning batch.

## Observed Input

The input is the operating system argument vector after `argv[0]` plus the
three invocation controls `DV_COLOR`, `DV_VERBOSITY`, and `NO_COLOR`. The current
CLI integration corpus contains 31 literal argument batches with 2 to 10 tokens
and a mean of 5.03 tokens; the no-argument and one-argument help/version paths
are covered separately. Token byte lengths are external and unbounded by the
repository corpus.

`ASSUMPTION: real interactive and CI invocations remain small enough that one
contiguous OS-owned token batch is preferable to a fixed maximum - affects the
single process-lifetime allocation, not parsing correctness.`

`ASSUMPTION: representative run/test invocations carry at most four explicit
child-environment edits - affects only the inline allocation crossover; larger
batches preserve behavior through contiguous spill storage.`

## Transform

1. Three exact environment lookups -> a five-byte typed default policy; raw
   values are parsed and discarded immediately.
2. `args_os` -> an inline zero/one-token form or one `Box<[OsString]>` for a
   multi-token batch, retaining the exact platform encoding.
3. One linear, predictable scan -> typed global output policy, invocation
   mode, and first semantic token.
4. Selected profile plus first semantic token -> one of 26 exact routed
   command kinds before SDK,
   current-directory, project, filesystem, process, or network access. The raw
   command index and compatibility provenance stay outside the hot request.
5. Command operands -> borrowed views. Text-only positions reject invalid
   Unicode; path positions construct `PathBuf` directly from the OS string.
6. JSON reporting -> display strings allocated and secret-shaped values
   redacted only at the reporting edge.

The raw batch is owned for the process invocation and dropped on exit. Operand
views never outlive it. Empty operands are retained. Unknown commands and
invalid text produce stable diagnostics before discovery. The native parser
does not launch a child process or perform filesystem or network I/O.

## Global Output Policy

`CLI-006` stores output policy in a three-byte hot record: JSON mode, color
choice, and diagnostic verbosity. Global options may precede the command or be
interspersed with command operands. The normal no-global path borrows the raw
operand slice directly. Only an interspersed global option creates a compact
index batch; it never copies an `OsString`. The observed corpus's maximum of
10 tokens fits in 16 inline `u16` indices. Only a larger batch or a raw index
beyond `u16` promotes to a contiguous heap vector.

`--verbose` selects `detailed`, and `--quiet` selects `quiet`. Explicit
`--verbosity` accepts `quiet`, `minimal`, `normal`, `detailed`, or `diagnostic`.
Repeated verbosity and color selectors use deterministic left-to-right,
last-value precedence. Quiet emits errors only; minimal and normal add warnings;
detailed and diagnostic add informational diagnostics. JSON uses the same
filter before event serialization.

Color defaults to terminal detection, `--color` always emits ANSI diagnostic
severity, and `--no-color` never does. Explicit color with `--json` is rejected
before discovery because color cannot affect a JSON-only invocation. Missing,
non-Unicode, and unsupported verbosity values are rejected at the same boundary.
Top-level help and self-version remain typed command requests and perform no
SDK, current-directory, filesystem, process, or network work.

`CLI-013` applies environment defaults after the argument scan, so the parser
knows which dimensions an explicit command-line option replaced. Precedence is
built-in defaults, then a non-empty `NO_COLOR`, then `DV_COLOR` or
`DV_VERBOSITY`, then command-line output options. `DV_COLOR` accepts
`auto|always|never`; `DV_VERBOSITY` accepts the same five typed levels as
`--verbosity`. A higher-priority command option makes a malformed lower input
irrelevant. Otherwise invalid or non-Unicode environment data produces a
pre-discovery usage failure that names only the variable, never its value.

The transient environment policy is five bytes with byte alignment and compile
time layout checks. Safe values become enums immediately; invalid raw values
are dropped without entering a diagnostic. Unset variables allocate nothing.
The semantic invocation request is 6 bytes at alignment 2. Environment and
output policy do not enlarge it.

At the output edge, JSON argument materialization preserves argument count and
order while replacing separated and combined API keys, passwords, tokens,
credentials, secret property assignments, URL userinfo, and URL query or
fragment data with `<redacted>`. Human rejection paths use the same bounded
classifier before formatting user-controlled option, command, or operand text.
The classifier uses a 64-byte stack normalization buffer; only an actually
redacted JSON string allocates beyond the event schema's existing string batch.
NuGet credentials continue through their narrower zeroizing owners and never
enter this invocation batch.

For child processes, `run` and `test` collect a separate command-lifetime
overlay in increasing precedence order: ambient inheritance, global
`[env:NAME=VALUE]` directives, launch-profile values, and finally
`-e|--environment NAME=VALUE`. Equal-source inputs retain their original order,
so the final occurrence wins without sorting or hashing. Ambient values are
inherited rather than copied. Launch-profile ingestion is reserved in the
typed source order and lands with the runner; until then it contributes no
edits.

The common batch borrows up to four 24-byte edits in a fixed inline array and
allocates only when a command supplies more. Compile-time checks keep each edit
pointer-aligned and the complete plan within two assumed 64-byte cache lines.
Names and values continue to point at the invocation batch, secret
classification is stored as one bit of state, and debug/reporting views never
expose sensitive values. Environment directives are removed from semantic
operands; commands other than `run` and
`test` reject them rather than silently ignoring an unsupported overlay.
Arguments after `--` remain opaque internally and are redacted only when the
event reporter materializes its public string batch. Applying the plan to an
actual child remains part of the pending runner workflow.

Edit capture is linear with predictable option-form branches. Execution can
apply the already precedence-ordered records sequentially, leaving the OS
child-environment map to perform final replacement. Assignments must be valid
Unicode `NAME=VALUE`, names must be non-empty, neither name nor value may
contain NUL, and the split offset must fit `u32`; malformed or out-of-range
inputs reject before SDK, project, filesystem, process, or network work. Values
may be empty. Windows name comparison is ASCII case-insensitive; Unix
comparison is case-sensitive.

`CLI-007` adds a one-byte compatibility mode selected by `--compat
dotnet|msbuild|nuget|vstest`. It follows the same linear scan and indexed-view
rules as output globals, so the selector is removed from semantic operands
without copying tokens. Mode is cold provenance rather than part of the
semantic request; mode plus output policy forms one four-byte options record.
Exit mappings and their pinned oracle evidence are documented in
[exit-behavior.md](exit-behavior.md).

`DROP-003` makes this classification an explicit invariant of the first scan.
Native mode and the four profiles in separated or combined form share one
five-byte transient record: the three-byte global policy, one mode byte, and
one bitset byte recording explicit mode/color/verbosity dimensions. Its size
and byte alignment are compile-time checked. This replaces three independent
boolean locals without adding persistent state, allocation, a second scan, or
external work. The common native branch is predictable; selector parsing is a
rare exact-match branch over one borrowed token. Duplicate, missing,
unsupported, and non-Unicode values reject rather than selecting a fallback.

## Route Precedence

`DROP-010` resolves ambiguous first words with this exhaustive precedence
matrix. The selected profile is authoritative; the parser never probes the
filesystem or launches a candidate tool to decide.

| First word | Native / dotnet | NuGet | MSBuild | VSTest |
|---|---|---|---|---|
| `restore` | `Restore` | `NugetRestore` | `MsbuildInput` | `VstestInput` |
| `pack` | `Pack` | `NugetPack` | `MsbuildInput` | `VstestInput` |
| `push` | `Unknown` | `NugetPush` | `MsbuildInput` | `VstestInput` |
| `list` | `DotnetList` | `NugetList` | `MsbuildInput` | `VstestInput` |
| `add` | `Add` | `NugetAdd` | `MsbuildInput` | `VstestInput` |
| `remove` | `Remove` | `NugetRemove` | `MsbuildInput` | `VstestInput` |
| `update` | `Unknown` | `NugetUpdate` | `MsbuildInput` | `VstestInput` |

Other native/dotnet words use the exact canonical command match; other NuGet
words are unknown. MSBuild and VSTest words remain typed input routes because
their project/container grammars are owned by later rows. NuGet-only direct
replacement without explicit profile evidence remains open under `DROP-007`
and `DROP-012`; guessing it here would make a future canonical command unsafe.

The seven-word `match` produces a row index into one 35-byte read-only matrix;
the already-classified one-byte mode selects its column. This is one indexed
byte read, not a runtime table scan or hash lookup. It adds no state to the
six-byte request and does not allocate. Only native/dotnet `run` and `test` may
activate the child delimiter; a word in another profile cannot cross into
child orchestration. Routed but unimplemented operations return a typed
pre-I/O failure rather than falling through to native restore/build.

## Explicit Profile Diagnostics

`DROP-011` keeps compatibility provenance out of the six-byte semantic
request and reads it only at the shared terminal failure boundary. When an
explicit `dotnet`, `msbuild`, `nuget`, or `vstest` profile has been selected,
that boundary appends one ordered `compatibility_profile` context record to
the existing diagnostic batch. The human and JSON reporters consume the same
record; neither reporter reconstructs the profile from arguments or prose.

Native failures omit the record. Invalid and repeated selectors reject before
a profile is established, so they also omit it and retain native usage exit
policy. The common successful dispatch path performs no new branch,
allocation, copy, filesystem operation, process launch, or network request.
The rare error path allocates the two variable-sized strings required by the
owned diagnostic wire record and grows its context vector by one element.

## Profile Lexical Rules

`DROP-013` keeps the platform-tokenized `OsString` batch as the source of
truth. The scan never reconstructs shell quoting or normalizes case,
separators, empty values, spaces, literal quote bytes, or non-Unicode data.
Native and dotnet command/option matching is exact. Explicit NuGet command
words use ASCII-insensitive comparison only after the seven exact ambiguous
words miss. On Windows, a leading `/` is option-shaped only for the dotnet,
MSBuild, and VSTest profiles; native and NuGet paths keep `/` as operand data.

Implemented Phase 1 value options accept separated, `=`, and `:` forms.
Singleton configuration, project, package-directory, and config-file values
reject mixed repetitions, while repeatable sources append in input order.
Combined values remain suffix slices into the owned token and allocate no
text. Recognized common cases are straight-line matches; case-insensitive
NuGet routing and prefix-shaped error handling remain rare branches.

The first `--` closes global parsing. Native/dotnet run and test expose the
following tokens as one borrowed forwarding slice. For every other command,
the delimiter and complete remaining tail stay in the command-argument view,
so a later `--json` or `--compat` cannot be stolen as a dv global. A leading
delimiter is removed as syntax, then makes the following command token literal.
This uses two previously spare bits in the existing scan bitset and does not
change the five-byte scan record, four-byte options record, or six-byte
semantic request. A direct tail remains one slice; only an invocation that
already removed interspersed globals may need its existing compact index list.

## Layout And Access

`DROP-019` exposes the dispatch input as `TransformBatch<'_>`, one borrowed
pointer to the process-owned `InvocationBatch`. It is 8 bytes at alignment 8
on the measured x86-64 target, protected by target-width compile-time
assertions, so eight views fit in the assumed 64-byte cache line. Copying the
view never copies tokens or policy. Its lifetime cannot exceed the owner, and
no allocation, scan, hash, filesystem access, process launch, or network
request is added when dispatch creates it. A pointer is appropriate here
because operands and child tails are variable-sized external OS data already
owned contiguously for the process lifetime; copying that data into a second
record would add work and split ownership.

Transform equality reads the six-byte request and then linearly compares the
borrowed semantic operand, forwarded-child, and environment-directive batches.
The usual equal path has predictable presence branches; mismatches return at
the first differing item. Compatibility mode and original command spelling
remain cold provenance and are deliberately excluded. Unit coverage proves
the Phase 1 `build`, `restore`, `run`, and `test` compatibility spellings plus
native `sync`, global options on either side of the command, empty child
arguments, and non-Unicode tokens. It also proves that a different global,
command kind, operand, environment value, or child token is unequal. Invalid
or unsupported syntax continues to produce the existing typed pre-discovery
failure; later MSBuild, NuGet, and VSTest grammars are not claimed equivalent.

`InvocationRequest` is 6 bytes and aligned to 2 on every supported target: a
two-byte syntax version, one-byte semantic command, and three-byte output
policy. Its layout is protected by compile-time assertions. Ten requests fit
in an assumed 64-byte cache line with four bytes unused, although production
owns only one request per process. The raw command index and one-byte
compatibility provenance remain in the cold owning batch. The raw token array
is contiguous; capture and classification are linear. Command dispatch reads
the hot request once, while cold raw text is read only by the selected parser
and reporter. Explicit cache-line alignment would waste 58 bytes and cannot
prevent contention because the request is read-only and not shared.

The borrowed command-argument view is 32 bytes and aligned to `usize`, allowing
two views per assumed 64-byte cache line. Its tagged storage packs either one
slice or the raw/index pair into the same 16-byte payload. It is copied by value,
read linearly, and never shared or independently mutated, so explicit alignment
would add cost without preventing false sharing.

The one-token help/version path does not allocate a token container. A
variable-sized multi-token OS argument batch requires one contiguous
allocation. Command operands are borrowed. Event strings remain a cold,
output-only allocation required by the JSON schema and are not created for
human output.

The forwarded-argument view is one 16-byte slice, aligned to `usize`, so four
views fit in an assumed 64-byte cache line. Only one read-only view exists per
invocation, so explicit cache-line alignment would waste space without
preventing contention.

## Unknown Option Boundary

`CLI-011` rejects an unrecognized global option during the initial linear
argument scan. Leaf, SDK, project, build-plan, restore, and sync parsers reject
their own unrecognized options before current-directory, SDK, project,
filesystem, or network discovery. A valid global option may still be
interspersed because the invocation batch removes its index without copying
the underlying OS token.

The successful SDK path normalizes into one borrowed `SdkRequest` singleton.
It performs no dynamic allocation and reads only the command and required RID
operand. Diagnostic strings and context allocate only on rejection. Build
option parsing now precedes its explicit unimplemented-operation boundary, so
malformed syntax cannot be hidden by `DV0003`.

The retained oracle keeps `build --definitely-unknown` identical after the
executable token. .NET 10 reports `MSB1001`; `dv` reports `DV0002`. Before and
after snapshots prove that neither command creates, removes, or changes a file
or directory in the input workspace. Integration tests repeat the
no-discovery assertion with malformed `global.json` and project inputs across
every active command family.

## Child Argument Boundary

`CLI-012` recognizes the first `--` after native `run` or `test`. One optional
nonzero index, compile-time protected to one machine word, splits the
already-owned raw batch: command options retain their existing
borrowed view and the complete tail becomes a contiguous
`ForwardedArguments<'_>` slice. The delimiter is not forwarded. Empty strings,
a second literal `--`, option-looking text, and platform-native non-Unicode
operands are never decoded, copied, normalized, or reordered. An empty tail is
distinct from an invocation without a delimiter. A 64-token test remains one
direct slice rather than allocating a semantic index batch.

Global parsing stops at the delimiter. Consequently `--json`, color, verbosity,
and compatibility spellings in the tail remain application/test data. Other
current commands do not accept child arguments and continue to reject `--` as
an unknown command option before discovery. The run and test boundaries consume
the typed batch today; process launch remains deliberately ordered after
environment, cancellation, and child-exit contracts.

## Boundaries

- Command names and text-only SDK operands must be valid Unicode.
- Paths remain lossless OS strings until consumed by filesystem APIs.
- The command syntax version is `2` and is independent of the JSON event schema.
- Every currently accepted alias normalizes to one semantic command kind.
  Executable-token inference remains explicitly open under `DROP-012`; future
  compatibility aliases remain unsupported in their owning `DROP-*` rows.
- Command-specific options remain explicit in their owning driver rows and
  never become silent no-ops at a lexical boundary.

The design is disproved if a supported path cannot round-trip through
`PathBuf`, classification performs external I/O, or process-level startup
regresses beyond benchmark noise relative to the prior release baseline.

## Independent Protocol Versions

`CLI-017` stores command syntax version 2 as a two-byte transparent value in
the 6-byte invocation request. Event schema version 20 remains a
reporter constant. Native `version`, `--version`, and `-V` produce the same
typed tool-version request. `--compat dotnet --version` produces the typed SDK
selection request required by the Microsoft spelling. Original tokens remain
only in the reporter-safe argument batch.

Human `dv --version` retains its single-line output and does not allocate an
event batch. JSON version output materializes three cold events:
`command_started` with the syntax version, `tool_version` with the executable
and both protocol versions, and `command_finished`. The common non-JSON parser
adds no filesystem access, network request, managed process, dynamic
allocation, or branch selected by alias spelling. Syntax versions are
build-owned rather than inferred from argv; unsupported event schemas are
rejected by the reporter boundary rather than guessed from an alias.

## Windows Evidence

Thirty retained samples after three warm-ups on the repository benchmark
machine measured the isolated `dv --version` path at `6.095 ms` median and
`8.818 ms` p95. It has no like-for-like Microsoft result because the two
commands report different product versions.

The like-for-like SDK selection case measured `dotnet --version` at
`69.660 ms` median and `72.407 ms` p95, and `dv sdk current` at `6.102 ms`
median and `7.247 ms` p95. That is an `11.4x` median improvement.

A detached build of pre-change commit `6c757e4` measured `5.740 ms` for CLI
self-version and `5.958 ms` for SDK selection. The new medians differ by
`0.355 ms` and `0.144 ms` respectively, both smaller than the uncontrolled
sample spread, so no parser speedup or regression is claimed. Raw current
samples are retained as `benchmarks/results/baseline-1785559342.json` and
`benchmarks/results/baseline-1785559345.json`; pre-change samples are
`benchmarks/results/baseline-1785559199.json` and
`benchmarks/results/baseline-1785559202.json`.

The `CLI-006` like-for-like case intersperses `--quiet --no-color` in the `dv` SDK
selection command. Thirty retained samples after three warm-ups measured
`dotnet --version` at `74.362 ms` median and `78.493 ms` p95, while
`dv sdk --quiet --no-color current` measured `6.986 ms` median and `7.957 ms`
p95, a `10.6x` median improvement. The preceding boxed-index run measured
`7.033 ms`; the `0.047 ms` difference is noise, so no speedup is
claimed for the structural allocation removal. The retained inline-index raw
samples are `benchmarks/results/baseline-1785567618.json`.

The explicit compatibility case compares `dotnet --version` with the exact
`dv --compat dotnet --version` spelling after preflight proves identical
selected-SDK output. Fifty retained samples after ten warm-ups measured
`63.402 ms` median and `65.472 ms` p95 for `dotnet`, and `5.088 ms` median and
`5.718 ms` p95 for `dv`, a `12.5x` median improvement. The raw samples are
`benchmarks/results/2026-08-02-sdk-current-compat-v2-windows.json`.

The unknown-option case compares the identical argument vector `build
--definitely-unknown` after replacing only the executable token. Thirty
retained samples after three warm-ups measured `dotnet` at `146.054 ms` median
and `152.690 ms` p95, and `dv` at `4.827 ms` median and `6.424 ms` p95. `dv`
was `30.3x` faster at the median. Raw samples are retained as
`benchmarks/results/baseline-1785575360.json`.

The forwarding preflight builds a .NET 10 application outside timing and
requires `dotnet run --` to deliver `alpha`, an empty string, `--color`, and
`two words` in exact order. `dv` reports the same four-item typed tail, while
cross-platform unit tests additionally retain an invalid-Unicode OS token.
Thirty retained samples after five warm-ups compare direct Microsoft-host
capture/reporting with the `dv` forwarding boundary: `44.698 ms` versus
`5.606 ms` median, and `51.902 ms` versus `6.184 ms` p95 (`8.0x` median).
Because `dv run` execution is still TBI, this structural result is not promoted
as a like-for-like run benchmark. Raw samples are
`benchmarks/results/cli-forwarding-final.json`.

The no-delimiter control measured `dv sdk current` at `6.102 ms` median and
`6.892 ms` p95, matching the earlier published `6.102 ms` median. The added
predictable delimiter check therefore shows no practical common-path
regression. `dotnet --version` measured `77.833 ms`, so `dv` remained `12.8x`
faster on the like-for-like SDK result. Raw control samples are
`benchmarks/results/sdk-current-cli012-control.json`.

The `CLI-013` oracle gives both executables the identical argument vector
`build --definitely-unknown` and environment batch `NO_COLOR` (with a secret
sentinel), `DV_COLOR=never`, and `DV_VERBOSITY=normal`. .NET 10 retains its
`MSB1001` failure and `dv` retains `DV0002`; preflight rejects ANSI output,
sentinel disclosure, and any workspace mutation. Thirty retained samples after
three warm-ups measured Microsoft at `134.218 ms` median and `150.374 ms` p95,
and `dv` at `5.503 ms` median and `6.314 ms` p95. `dv` was `24.4x` faster at
the median. The complete stable batch spans `130.490-153.938 ms` for Microsoft
and `4.542-6.896 ms` for `dv`; no retained sample was removed. Raw samples are
`benchmarks/results/cli-environment-cli013-final.json`.

Preflight separately builds the .NET 10 child oracle and proves that ambient
`DV_CLI013_ORACLE=ambient`, `[env:...=directive]`, and
`--environment ...=command-line` select `command-line`. The application sees
the secret sentinel but reports only its presence. `dv` reaches the typed run
boundary with three edits, one sensitive edit, and no
secret text in schema-18 output.

`CLI-014` consumes the classified request before work begins. SDK, project,
build, restore/sync, run, and test commands install one command-lifetime
Ctrl+C/SIGINT token; help, version, malformed, and unknown invocations do not
pay that allocation or handler-thread cost. The run/test boundary receives the
same typed token and its fixed two-second child grace. The absolute deadline is
anchored to the first signal rather than restarted by each child, and a second
signal changes the policy to immediate termination. Actual child creation and
process-group ownership remain in `RUN-006` and `RUN-009`. `CLI-015` now owns
typed numeric termination and the declared run/test mapping; signal and
cancellation exit profiles remain with their workflow-specific slices.

The common no-environment SDK control measured `dotnet --version` at
`68.493 ms` median and `70.539 ms` p95, and `dv sdk current` at `5.596 ms`
median and `5.980 ms` p95 (`12.2x`). This remains below the earlier published
`6.102 ms` `dv` median, so the three exact environment lookups show no practical
startup regression. Raw samples are
`benchmarks/results/sdk-current-cli013-control.json`.

The `CLI-017` structural query validates all three native version aliases before
timing `dv --json --version`. Thirty retained samples after five warm-ups
measured `4.479 ms` median and `5.593 ms` p95. Microsoft has no command that
reports both its command grammar and dv's JSON event contract, so the harness
prints TBI rather than manufacturing a comparison. The separate like-for-like
SDK control measured `dotnet --version` at `63.022 ms` median and `64.736 ms`
p95, while `dv sdk current` measured `4.828 ms` median and `5.252 ms` p95,
leaving `dv` `13.1x` faster at the median. Raw samples are retained as
`benchmarks/results/2026-08-01-cli-protocol-version-windows.json` and
`benchmarks/results/2026-08-01-cli-protocol-version-sdk-control-windows.json`.

`DROP-002` validates all 19 accepted spellings in unit coverage and measures
`dotnet restore --definitely-unknown` against normalized `dv sync
--definitely-unknown`. Thirty retained samples after five warm-ups measured
`121.211 ms` versus `5.462 ms` median and `128.378 ms` versus `6.337 ms` p95,
so `dv` is `22.2x` faster at this like-for-like pre-I/O boundary. The SDK
control remained `13.3x` faster. Full evidence is retained in the
[command-normalization baseline](performance-baselines/2026-08-01-cli-command-normalization-windows.md).

`DROP-003` compares `dotnet build --definitely-unknown` with `dv --compat
dotnet build --definitely-unknown`. Both reject the same invalid option and a
before/after fixture snapshot proves neither reaches project or SDK discovery.
Fifty retained samples after ten warm-ups measured `152.984 ms` versus
`5.641 ms` median and `193.102 ms` versus `6.630 ms` p95, so `dv` is `27.1x`
faster at this like-for-like boundary. The selected-SDK control remained
`12.0x` faster. Full evidence is retained in the
[invocation-mode baseline](performance-baselines/2026-08-01-invocation-mode-windows.md).

`DROP-011` remeasures that same rejection after requiring stable selected-
profile context in human and JSON diagnostics. Fifty retained samples after
ten warm-ups measured `133.281 ms` for Microsoft and `5.125 ms` for `dv`, a
`26.0x` median improvement; p95 was `147.033 ms` and `6.256 ms`. Full evidence
is retained in the [compatibility-diagnostics baseline](performance-baselines/2026-08-02-cli-compat-diagnostics-windows.md).

`DROP-013` compares `dotnet build -c:Release --definitely-unknown` with the
exact `dv --compat dotnet` replacement. Both accept the colon-joined
configuration token, reject only the sentinel, and leave the fixture unchanged.
Fifty retained samples after ten warm-ups measured `141.461 ms` versus
`4.912 ms` median and `176.976 ms` versus `6.003 ms` p95, so `dv` is `28.8x`
faster. Full evidence is retained in the
[lexical-preservation baseline](performance-baselines/2026-08-02-cli-lexical-preservation-windows.md).

`DROP-022` closes the accepted-option boundary for the current Phase 1
global, build, restore, project, run, and test surfaces. Each accepted option
now changes typed state; unsupported tokens fail before discovery rather than
being silently ignored. The like-for-like `dotnet test
--definitely-unknown` comparison measured `140.183 ms` versus `6.035 ms`
median and `159.044 ms` versus `7.126 ms` p95 across 50 retained samples, so
`dv` is `23.2x` faster. Both commands return exit 1 and preserve the fixture
exactly. Full evidence is retained in the
[option-effect baseline](performance-baselines/2026-08-02-cli-option-effects-windows.md).

`DROP-010` compares `dotnet pack --definitely-unknown` with `dv --compat
dotnet pack --definitely-unknown`. Both reject the same invalid pack option;
fixture snapshots prove the route does not reach SDK, project, or filesystem
work. Fifty retained samples after ten warm-ups measured `280.174 ms` versus
`5.242 ms` median and `306.891 ms` versus `5.841 ms` p95, so `dv` is `53.4x`
faster. The SDK control remained `12.8x` faster. Full evidence is retained in
the [route-precedence baseline](performance-baselines/2026-08-01-cli-route-precedence-windows.md).
