# dv Agent Operating Rules

These rules apply to every change in this workspace. They turn data-oriented
design into required engineering behavior for `dv`; they are not optional
performance advice.

## Precedence And Scope

When instructions conflict, use this order:

1. The user's explicit instruction for the current task.
2. This file.
3. Existing repository convention.

Choose a change tier before designing:

- **Tier 0:** mechanical edits and one-line fixes. Apply these rules silently.
- **Tier 1:** behavior, interface, contract, or data-layout changes. Write a
  short data-first plan, run the simplification pass, and self-check.
- **Tier 2:** a new or substantially changed subsystem, pipeline, or tool.
  Everything in Tier 1 plus the enforceable deliverables below.

When unsure, choose the higher tier. Never use tiering to avoid measurement or
correctness work.

## Start With Real Data

Before proposing machinery, state:

1. The concrete input shape, source, volume, and valid ranges.
2. The concrete output shape, destination, ownership, and lifetime.
3. The common values and observed distribution.
4. What changes frequently and what remains stable.
5. Every read, write, allocation, process launch, filesystem touch, and network
   request in the dominant transform.

Inspect representative projects, files, call sites, traces, and benchmark
samples before assuming. When live data exists, instrument it temporarily,
analyze the distribution, then remove the probe before measuring.

Never invent measurements or distributions. Record missing facts exactly as:

`ASSUMPTION: <fact> - affects <decision>`

Ask the user only when the answer materially changes the design. Prefer a
design that is cheap to revise when an assumption is uncertain.

## Design The Transform

- The problem is a transformation of data, not a taxonomy of objects.
- Describe each stage as input -> transform -> output with explicit layout,
  meaning, ownership, lifetime, and out-of-range behavior.
- Treat one item as a batch of one. Public transforms are plural/batch-first
  unless the data is a genuine singleton and the exception is documented.
- Use contiguous storage and indices by default. A pointer-, reference-, or
  handle-heavy hot path requires a written reason that indices are unsuitable.
- Organize hot data by access pattern. Split cold fields that the dominant loop
  does not read.
- State expected access as linear, strided, or random and branch behavior as
  predictable or unpredictable for every hot path.
- Partition high-entropy cases into straight-line passes instead of repeatedly
  branching per item.
- Put rare and error handling outside the common path.
- Make boundary behavior explicit: reject, drop, clamp, or fail. Never leave it
  implicit.
- Use explicit, versioned, flat data protocols between subsystems. Do not hide
  data movement behind object hierarchies or opaque interfaces.
- Do not add parameters, options, extension points, or abstractions for
  hypothetical future data.

Rust is the implementation tool. The real platform is the developer's hardware
and filesystem: process startup, cache hierarchy, memory bandwidth, storage,
network latency, and the selected Microsoft SDK/runtime constrain the design.

## Speed Is Paramount

Once correctness and compatibility are preserved, speed is the governing
product constraint. It is designed into data layout and work scheduling from
the first implementation; it is not a later cleanup phase. Rust gives us the
necessary control only when we use it deliberately.

- Do no work unless the requested output depends on it. Prove no-op state from
  the smallest stable fingerprint that can establish correctness.
- Batch filesystem discovery, hashing, parsing, graph updates, cache lookups,
  downloads, compiler inputs, diagnostics, and reporter writes.
- Run independent CPU work across a bounded worker pool. Partition batches into
  coarse contiguous ranges so scheduling overhead and false sharing do not
  dominate useful work.
- Use async I/O when many operations spend most of their lifetime waiting, such
  as concurrent package downloads. Do not put CPU-bound transforms on an async
  executor; hand them to the worker pool.
- Keep queues bounded. Backpressure is explicit; unbounded task creation,
  channels, and result buffering are forbidden.
- Do not dynamically allocate in production hot paths unless variable-sized
  external data makes it necessary and a fixed-capacity, stack, pooled, arena,
  or reused buffer is demonstrably unsuitable. Document that necessity at the
  allocation site or subsystem contract.
- Allocate stable workspace data in arenas or contiguous owned buffers, size
  from observed counts, and reuse capacity across repeated commands.
- Prefer slices, compact indices, offsets, bitsets, and dense tables over
  per-item `Box`, `String`, `Vec`, reference-counted pointers, hash maps, trait
  objects, or linked structures.
- Treat struct layout as part of the transform contract. Order fields to keep
  the dominant working set compact, split hot and cold records, and avoid
  carrying metadata through loops that do not read it.
- Record `size_of`, `align_of`, field count, expected elements per cache line,
  and total working-set bytes for hot records. Protect intentional layouts with
  compile-time assertions and focused tests.
- Do not assume a cache-line size silently. Record the target hardware value as
  an `ASSUMPTION`, isolate it behind one platform constant, and validate it on
  benchmark machines where the OS exposes the value.
- Align data when it enables required SIMD loads, direct I/O, atomics, or keeps
  independently written worker state off the same cache line. Alignment and
  padding consume memory and cache capacity; state and measure that cost.
- Prevent false sharing by cache-line-isolating per-worker counters, queue
  cursors, and other independently mutated state. Keep read-mostly shared data
  packed and immutable.
- Use `#[repr(C)]`, explicit integer widths, and explicit padding only for a
  wire/FFI/persisted protocol or a measured layout requirement. Default Rust
  layout is not a stable protocol.
- Design linear traversal and prefetch-friendly batches first. Pointer chasing,
  random hashing, strided scans, and gather/scatter access require measured
  justification in hot paths.
- Store text as offsets into owned byte buffers when it crosses hot subsystem
  boundaries. Decode or format at the user-facing edge.
- Avoid copying. Make ownership and lifetime permit borrowed batch views, then
  move or reference the underlying contiguous storage.
- Avoid locks in dominant loops. Partition ownership first; merge deterministic
  batch results once. When sharing is unavoidable, state contention and cache
  line behavior and measure it.
- Preserve deterministic ordering independent of thread completion order.
- Build release binaries with startup, binary size, peak memory, allocation
  count, throughput, and latency evidence for each major workflow.

Concurrency is not automatically faster. Thread creation, task scheduling,
wakeups, synchronization, cache contention, and async state machines are costs.
Use multithreading or async wherever the work is genuinely independent or
waiting concurrently and the representative benchmark shows a net win. Keep a
straight-line sequential path when the batch is below the measured crossover.

## State The Cost

Every recommendation must name where its cost is paid:

- elapsed latency and throughput;
- bytes read, written, retained, copied, or allocated;
- process, filesystem, and network operations;
- additional states, branches, dependencies, and maintenance surface.

Distinguish latency from throughput. Do not claim a change is faster without a
measurement. If it cannot be measured yet, label the result **unverified**,
state the hypothesis, and name the exact benchmark that would verify it.

Measure cold and warm states separately. A no-op command must prove that work
is current while touching only the state needed for that proof.

## Simplification Pass

Run these questions recursively before implementing Tier 1 and Tier 2 work:

1. Can this work be removed?
2. Can it happen once, be precomputed, cached, or amortized?
3. Can it happen fewer times?
4. Can a result be approximated without violating the contract?
5. Does a small lookup table fit the observed range?
6. Does a large lookup table fit better?
7. Can a bounded buffer decouple producer and consumer?
8. Can another real constraint make the machine simpler?
9. Has the current approach plateaued, and does the data indicate a different
   algorithm or representation?

State what this pass removed. Complexity requires observed evidence.

## Tier 1 Plan

Before editing, write a short plan covering:

1. Problem value, scope limit, and fallback.
2. Real input/output data and labeled assumptions.
3. Dominant cost on the actual platform.
4. Transform stages and boundary contracts.
5. Simplification decisions.
6. Done criteria and evidence that would disprove the design.
7. Verification against representative data.

## Tier 2 Deliverables

Every new or reworked subsystem must include:

- a batch transform contract with layouts, owner, lifetime, and valid ranges;
- batch-first APIs, with documented true-singleton exceptions;
- access-pattern and branch-behavior notes for hot paths;
- justification for pointer/reference/handle-heavy hot paths;
- explicit malformed and out-of-range input behavior;
- correctness tests and cold/warm benchmark coverage;
- unresolved design questions as focused files under `issues/`, never inline
  TODOs or remote issue placeholders.

## dv-Specific Constraints

- `dv` production code must never invoke `dotnet`, MSBuild, NuGet, or VSTest as
  a fallback. Reference invocations belong only in benchmark and compatibility
  tooling.
- Roslyn compiles C# and the selected Microsoft runtime executes managed code.
  Rust owns discovery, evaluation, resolution, caching, planning, scheduling,
  diagnostics, and reporting.
- Subsystems communicate through typed data, never scraped console prose.
- Human and JSON output consume the same structured event batch.
- Stable diagnostics include a code, severity, short message, ordered context,
  causal chain, and an action when known.
- Unsupported compatibility behavior fails explicitly rather than producing a
  plausible result.
- Keep production dependencies few and justified. Dependency cost includes
  compile time, binary size, startup work, audit surface, and maintenance.
- Preserve deterministic output, stable ordering, cancellation boundaries, and
  cross-platform behavior in every design.

## Editing And Verification

- Read nearby code and `git status --short` before editing.
- Keep edits scoped. Do not revert user changes or perform unrelated refactors.
- Prefer `rg` for search and structured parsers for structured data.
- Use `apply_patch` for manual edits.
- Add comments only for invariants, ownership, lifetime, layout, or non-obvious
  ordering.
- Format, lint, test, and build release output before delivery:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

Run affected benchmarks and report exact results when a performance path
changes.

Every push must finish green in GitHub Actions. Before pushing, run the exact
local format, lint, workspace-test, release-build, and affected benchmark-smoke
gates. After pushing, wait for the workflow attached to that exact commit SHA
and verify every Windows, Linux, macOS, and benchmark job succeeds. Do not start
or push the next feature on top of a red run; inspect the failed job, correct it,
repeat the local gates, push the correction, and wait for green.

## Final Self-Check

Before delivering Tier 1 or Tier 2 work, verify:

- The plan used real data or labeled every missing fact as an assumption.
- The common case is straight-line and rare cases are outside it.
- The simplification pass removed unnecessary work and generality.
- Every boundary has explicit invalid-data behavior.
- Transforms are batch-first or document a true singleton.
- Hot access and branch patterns are stated.
- No performance claim lacks measurement or an explicit unverified label.
- Done criteria were checked against representative inputs.
- Tier 2 contracts and unresolved issue files exist.
