# Golden Compatibility Trace Contract

`DROP-020` stores reviewed substitution evidence under `compatibility/traces`.
Schema version 1 contains two commands from the checked-in Phase 1 GitHub
Actions fixture: selected-SDK discovery and a valid zero-package offline
restore.

## Input And Output

The JSON document is one ordered case batch. Each case owns its UTF-8 ID,
source path, reference and candidate argv, nine ordered environment overrides,
explicit stdin, input-filesystem contract, and separate expected observations.
Each observation records stdout, stderr, exit code, a sorted filesystem delta,
and mandatory process/network fields.

The verifier parses the cold artifact once outside timing into contiguous boxed
slices. Variable-sized JSON requires allocation in this benchmark-only path;
it adds no allocation or tracing branch to `dv`. Commands receive borrowed
arguments, piped stdin, the same environment policy, and separate fresh copies
of the same fixture.

## Transform

1. Parse the JSON bytes into one typed, versioned case batch and reject unknown
   fields or contract drift.
2. Reset separate reference/candidate trees and prove their sorted snapshots
   are identical.
3. Run each root command with its recorded argv, environment, stdin, and
   working directory.
4. Normalize only the workspace prefix and path separators, then compare exact
   streams, exit code, and sorted created/modified/deleted entries.
5. Admit benchmark samples only after the whole two-case preflight succeeds.

The selected SDK is captured from the reference and substituted into both SDK
expectations. The offline restore uses an empty local source and unreachable
proxy overrides, so it is deterministic without network availability. Missing
executables, malformed JSON, unknown schemas, nonzero exits, stream drift, or
filesystem drift fail the complete run.

## Layout And Cost

The schema is test data, not a persisted production protocol. Its records use
default Rust layout rather than `repr(C)`. Case and override traversal is
linear; snapshots and deltas stay in deterministic path order. Preflight pays
for one JSON parse, four process launches, and tree snapshots. Each timed
restore pays only fixture-independent process launch, controlled environment,
zero-package resolution, and output capture; fixture reset is outside timing.

`ASSUMPTION: benchmark hosts use a 64-byte cache line - affects only the shared
repository layout target; this cold two-case verifier is dominated by process
and filesystem latency, so explicit record alignment would waste memory
without changing the dominant cost.`

Process and network fields currently contain `TBI`. An unreachable proxy and
empty source do not prove that no socket or child process existed. Issue
[0009](../issues/0009-process-network-golden-tracing.md) owns event-driven,
cross-platform observation and positive controls. Missing instrumentation is
never converted to a zero count.

## Boundary

This slice intentionally reuses the benchmark harness, selected-SDK oracle,
lossless command capture, fixture reset, and tree snapshotter. It adds no
recorder to `dv`, runtime trace flag, second protocol, daemon, or polling loop.
Build, run, and test traces remain open until their owning workflows are
meaningful. Microsoft restore-artifact compatibility remains `RES-021`.
