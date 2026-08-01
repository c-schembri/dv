# Invocation Batch Contract

`CLI-005` establishes the process-entry transform used by later canonical and
compatibility grammars. Compatibility aliases and their generated manifest are
separate open rows in the parity map.

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
3. One linear, predictable scan -> typed global output policy and first
   semantic token.
4. First semantic token -> a typed native or explicit compatibility request before SDK,
   current-directory, project, filesystem, process, or network access.
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
The 16-byte invocation request and its common dispatch/cache behavior remain
unchanged.

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
without copying tokens. Mode plus output policy forms one four-byte options
record. Exit mappings and their pinned oracle evidence are documented in
[exit-behavior.md](exit-behavior.md).

## Layout And Access

`InvocationRequest` is 16 bytes and aligned to `usize` on supported 64-bit
targets. Its layout is protected by compile-time assertions. The raw token
array is contiguous; capture and classification are linear. Command dispatch
reads the hot request record once, while cold raw text is read only by the
selected parser and reporter. No cache-line alignment is added: one request is
read-only, never shared between workers, and padding it to the assumed 64-byte
benchmark cache line would waste 48 bytes without reducing contention.

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
- The command syntax version is `1` and is independent of the JSON event schema.
- Native and explicit compatibility modes exist. Automatic grammar inference,
  aliases, and precedence remain partial under `DROP-002` and `DROP-003`.
- Automatic drop-in forwarding aliases remain open under `DROP-013`; no
  unsupported syntax is silently accepted by this contract.

The design is disproved if a supported path cannot round-trip through
`PathBuf`, classification performs external I/O, or process-level startup
regresses beyond benchmark noise relative to the prior release baseline.

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

The explicit compatibility case compares `dotnet --version` with `dv --compat
dotnet sdk current` after preflight proves identical selected-SDK output.
Thirty retained samples after three warm-ups measured `65.901 ms` median and
`67.752 ms` p95 for `dotnet`, and `5.225 ms` median and `6.202 ms` p95 for
`dv`, a `12.6x` median improvement. The raw samples are
`benchmarks/results/baseline-1785569009.json`.

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
reference-specific exit propagation remain in `RUN-006`, `RUN-009`, and
`CLI-015`.

The common no-environment SDK control measured `dotnet --version` at
`68.493 ms` median and `70.539 ms` p95, and `dv sdk current` at `5.596 ms`
median and `5.980 ms` p95 (`12.2x`). This remains below the earlier published
`6.102 ms` `dv` median, so the three exact environment lookups show no practical
startup regression. Raw samples are
`benchmarks/results/sdk-current-cli013-control.json`.
