# Invocation Batch Contract

`CLI-005` establishes the process-entry transform used by later canonical and
compatibility grammars. Compatibility aliases and their generated manifest are
separate open rows in the parity map.

## Observed Input

The input is the operating system argument vector after `argv[0]`. The current
CLI integration corpus contains 31 literal argument batches with 2 to 10 tokens
and a mean of 5.03 tokens; the no-argument and one-argument help/version paths
are covered separately. Token byte lengths are external and unbounded by the
repository corpus.

`ASSUMPTION: real interactive and CI invocations remain small enough that one
contiguous OS-owned token batch is preferable to a fixed maximum - affects the
single process-lifetime allocation, not parsing correctness.`

## Transform

1. `args_os` -> an inline zero/one-token form or one `Box<[OsString]>` for a
   multi-token batch, retaining the exact platform encoding.
2. One linear, predictable scan -> typed global output policy and first
   semantic token.
3. First semantic token -> a typed native or explicit compatibility request before SDK,
   current-directory, project, filesystem, process, or network access.
4. Command operands -> borrowed views. Text-only positions reject invalid
   Unicode; path positions construct `PathBuf` directly from the OS string.
5. JSON reporting -> display strings allocated only at the reporting edge.

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

## Boundaries

- Command names and text-only SDK operands must be valid Unicode.
- Paths remain lossless OS strings until consumed by filesystem APIs.
- The command syntax version is `1` and is independent of the JSON event schema.
- Native and explicit compatibility modes exist. Automatic grammar inference,
  aliases, and precedence remain partial under `DROP-002` and `DROP-003`.
- End-of-options and child forwarding remain open under `CLI-012` and
  `DROP-013`; no unsupported syntax is silently accepted by this contract.

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
