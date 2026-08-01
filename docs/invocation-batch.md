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
2. One linear, predictable scan -> global JSON bit and first semantic token.
3. First semantic token -> a typed native command request before SDK,
   current-directory, project, filesystem, process, or network access.
4. Command operands -> borrowed views. Text-only positions reject invalid
   Unicode; path positions construct `PathBuf` directly from the OS string.
5. JSON reporting -> display strings allocated only at the reporting edge.

The raw batch is owned for the process invocation and dropped on exit. Operand
views never outlive it. Empty operands are retained. Unknown commands and
invalid text produce stable diagnostics before discovery. The native parser
does not launch a child process or perform filesystem or network I/O.

## Layout And Access

`InvocationRequest` is 16 bytes and aligned to `usize` on supported 64-bit
targets. Its layout is protected by compile-time assertions. The raw token
array is contiguous; capture and classification are linear. Command dispatch
reads the hot request record once, while cold raw text is read only by the
selected parser and reporter. No cache-line alignment is added: one request is
read-only, never shared between workers, and padding it to the assumed 64-byte
benchmark cache line would waste 48 bytes without reducing contention.

The one-token help/version path does not allocate a token container. A
variable-sized multi-token OS argument batch requires one contiguous
allocation. Command operands are borrowed. Event strings remain a cold,
output-only allocation required by the JSON schema and are not created for
human output.

## Boundaries

- Command names and text-only SDK operands must be valid Unicode.
- Paths remain lossless OS strings until consumed by filesystem APIs.
- The command syntax version is `1` and is independent of the JSON event schema.
- Only native mode exists in this slice. Compatibility modes and precedence
  remain explicitly partial under `DROP-002` and `DROP-003`.
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
