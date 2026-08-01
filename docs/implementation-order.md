# Feature Parity Implementation Order

This document defines the order in which `dv` should implement the capability
inventory in [feature-parity-map.md](feature-parity-map.md). The parity map owns
scope and completion state. This document owns sequencing. Moving a capability
to a later wave never removes it from the drop-in contract.

Snapshot: working tree on 2026-08-01.

## Scheduling Contract

The implementation target remains full practical parity with the selected
versions of `dotnet`, MSBuild, NuGet, and VSTest. Ordering follows these rules:

1. Close a usable vertical workflow before adding unrelated horizontal breadth.
2. Implement prerequisites before dependants, but only to the depth required by
   the next observable workflow.
3. Add canonical `dv` syntax and executable-token replacement together. Neither
   is a later compatibility pass over a separate execution path.
4. Reuse one typed transform after parsing; do not duplicate evaluators,
   resolvers, schedulers, or reporters per command spelling.
5. Complete correctness and artifact parity before accepting performance wins.
6. Measure cold and warm behavior in the same slice that introduces work or
   caching. Add incremental and no-op cases when the workflow can produce them.
7. Fail unsupported inputs before unrelated process, filesystem, or network
   effects. Rejection keeps the corresponding parity row incomplete.
8. Finish a bounded in-flight slice before changing waves, unless evidence
   disproves its design or shows that it cannot unblock the current gate.
9. Do not claim a phase or unqualified drop-in surface complete while any row
   assigned to its release manifest is missing or partial.

The next wave is not selected by unchecked-row count. Rows differ radically in
cost and dependency value. The active wave is the earliest wave whose exit gate
has not passed.

## Real Data At This Snapshot

The ordering input is one 468-row parity ledger:

- 59 rows are implemented;
- 32 rows have partial foundations;
- 377 rows are missing;
- framework/runtime/pack resolution is substantially present;
- NuGet configuration and source handling is present;
- package resolution, assets, cache, and lock handling have a strong initial
  implementation;
- `dv build --plan` stops before compilation;
- build scheduling, output materialization, incremental state, application run,
  test, pack, publish, and drop-in routing do not yet form complete workflows.

The repository currently owns small project fixtures, package graphs up to 203
resolved packages, Microsoft-oracled comparisons, human/JSON event tests, and
Windows cold/warm timing evidence. Representative private repositories,
test-heavy solutions, numeric release thresholds, and Linux/macOS evidence are
still missing.

`ASSUMPTION: closing a package-bearing console and library build/run slice is
the highest-value dependency for the remaining product - affects the order of
Waves 1 through 6.`

`ASSUMPTION: a 64-byte cache line is common target hardware but is not yet
validated across benchmark machines - affects later hot-layout and worker-state
decisions, not the current sequential vertical slice.`

## Work Unit

Each scheduled unit is a compatibility slice with these owned outputs:

1. A bounded batch of parity IDs and pinned reference-tool versions.
2. Representative valid, boundary, malformed, and unsupported input fixtures.
3. One typed input batch and one typed result/event batch shared by every syntax.
4. The implementation, with explicit filesystem, process, network, allocation,
   cancellation, and invalid-data behavior.
5. Paired oracle checks that replace only the executable token where the
   reference command is in the supported manifest.
6. Artifact, exit-status, stdout/stderr-role, and human/JSON comparisons.
7. Cold/warm and, where applicable, incremental/no-op benchmark evidence.
8. Updated parity states and any unresolved question recorded under `issues/`.

Fixtures and compatibility manifests are repository-owned and retained. Runtime
request/result batches live for one command unless a versioned cache contract
explicitly owns a stable subset. Benchmark samples are cold evidence and must
not enter production hot records.

Before starting the next slice, run the simplification pass: remove work, move
stable work out of the common path, batch repeated operations, and reject
generality unsupported by observed inputs.

## Always-On Rails

These sections advance with every workflow rather than waiting for one late
hardening phase:

| Rail | Parity rows | Required action in every wave |
|---|---|---|
| Command contract | `CLI-001` through `CLI-018` | Extend typed parsing, cancellation, exits, and option boundaries only for the active workflow. |
| Drop-in routing | `DROP-001` through `DROP-022` | Add the active reference syntax to the compatibility manifest and normalize it to the canonical typed request. |
| Driver surfaces | `DNCLI-*`, `MSCLI-*`, `NGCLI-*`, `VSTCLI-*` | Implement the command family alongside its workflow, not as a final parser rewrite. |
| Events and diagnostics | `EVT-001` through `EVT-016` | Feed human and JSON output from the same ordered event batch and preserve causal diagnostics. |
| Reliability and security | `PORT-001` through `PORT-015` | Define malformed input, cancellation, secret handling, atomicity, and platform behavior at each boundary. |
| Evidence gates | `GATE-001` through `GATE-022` | Add oracle, artifact, resource, and executable-token evidence before marking rows complete. |
| Surface inventory | `SURF-001` through `SURF-014` | Extend the pinned manifest continuously; `SURF-014` remains open until the final inventory is closed. |

An always-on rail does not authorize speculative framework work. Implement the
smallest rail segment needed by the active product slice, then extend it in the
next wave.

## Ordered Waves

### Wave 0: Close The In-Flight Signature Boundary

Status: completed on 2026-08-01 by `RES-015`.

**Outcome:** Package signature policy is correct and the tree returns to a
clean, measured baseline before the product focus moves.

Order:

1. Finish `RES-015` author/repository signature and trusted-signer verification.
2. Verify platform roots, timestamp/countersignature cases, malformed archives,
   untrusted signers, and required/accept policy boundaries.
3. Compare signed-package behavior with pinned NuGet oracles.
4. Record cold verification and warm-cache cost, bytes read, and allocations.
5. Update the parity row only after all repository quality gates pass.

Exit gate: signed valid fixtures pass, malformed and untrusted fixtures fail
before cache publication, warm valid cache behavior is defined, and no secret or
certificate material reaches diagnostics.

Dominant cost: ZIP/CMS parsing, certificate-chain work, archive bytes read, and
cache publication. Network work is outside the verification benchmark.

### Wave 1: Invocation And Manifest Spine

Status: in progress; `CLI-005` through `CLI-008` are complete, explicit
compatibility exit profiles and allocation-free named project selection are
available, and the broader foundations of
`DROP-002`, `DROP-003`, `DROP-011`, and `DROP-016` are partial.

**Outcome:** Every subsequent workflow starts from a lossless typed command
batch and can be reached by canonical and supported drop-in spellings.

Order:

1. Implement `CLI-005` through `CLI-008`, `CLI-011` through `CLI-015`, and
   `CLI-017` for lossless arguments, common options, exits, forwarding,
   cancellation, and syntax/schema versioning.
2. Implement `DROP-001` through `DROP-003`, `DROP-010`, `DROP-011`,
   `DROP-013`, `DROP-016`, `DROP-017`, and `DROP-019` through `DROP-022` for
   the Phase 1 restore/build/run surface.
3. Capture the applicable `DNCLI-001` through `DNCLI-003`, `DNCLI-008`, and
   `DNCLI-009` command shapes from pinned SDKs.
4. Begin `GATE-015` through `GATE-018` and `SURF-014`; keep them open while the
   manifest is incomplete.

Exit gate: raw OS arguments are stored once, all accepted Phase 1 spellings
produce an identical typed request batch, and malformed or ambiguous syntax
fails before SDK/project/filesystem/network discovery.

Dominant cost: process startup and argument normalization. The common parse path
must not touch the SDK, workspace, network, or managed runtime.

### Wave 2: Close The Minimal Project Input Contract

**Outcome:** One package-free and one package-bearing C# console/library target
produce complete, deterministic compiler inputs.

Order:

1. Close the Phase 1 portions of `WS-001` through `WS-012`.
2. Close the required Phase 1 evaluator rows in `EVAL-001` through `EVAL-025`.
3. Close the required single-target C# rows in `PROJ-001` through `PROJ-020`.
4. Finish the required partial `PACKS-*`, `RES-016`, `RES-017`, and `RES-023`
   boundaries; defer unrelated P2 package breadth.
5. Make `COMP-007` through `COMP-010` and `COMP-016` complete for the two
   target fixtures.

Exit gate: the normalized evaluator, package plan, reference set, source set,
resources, analyzers, options, and output paths match Microsoft oracle data for
both fixtures, including stable unsupported diagnostics.

Dominant cost: bounded filesystem discovery and XML/manifest reads. Expected
access is linear over compact file, property, item, and reference batches.

### Wave 3: Direct Roslyn Compilation

**Outcome:** `dv` directly produces a managed assembly and diagnostics without
launching `dotnet` or MSBuild.

Order:

1. Implement `COMP-001` through `COMP-004` for framework attributes, assembly
   information, global usings, and analyzer configuration.
2. Implement the remaining compiler option mapping needed by the fixtures.
3. Implement `COMP-014` and `COMP-015`: native `hostfxr` loading and the
   versioned, length-bounded compiler-host protocol.
4. Implement `COMP-017`, `COMP-020`, and the fixture-required portions of
   `COMP-018` and `COMP-019` for diagnostics, analyzers/generators,
   cancellation, and isolation.
5. Implement `COMP-022` normalized compiler-batch and artifact comparison.

Exit gate: package-free and package-bearing fixtures compile through selected
Roslyn, expected failure diagnostics retain their typed locations and IDs, and
no forbidden fallback process is started.

Dominant cost: one managed compiler-host launch, source/reference bytes read,
Roslyn allocations, and assembly/PDB bytes written. Persistent hosting remains
deferred until isolated-host measurements justify it.

### Wave 4: Build Outputs And Deterministic Graph

**Outcome:** `dv build` creates a runnable, atomically published target output.

Order:

1. Implement `GRAPH-001` through `GRAPH-006`, `GRAPH-010`, and `GRAPH-012` as
   a sequential deterministic path first.
2. Implement `OUT-001` through `OUT-005`, `OUT-008`, and `OUT-009` for assembly,
   PDB, copy-local assets, dependency/runtime manifests, and apphost.
3. Add the fixture-required portions of `OUT-006` and `OUT-007`.
4. Implement `OUT-010` artifact and runnable-behavior comparison.
5. Add bounded parallel graph execution only after representative batches show
   a crossover, then complete `GRAPH-007` through `GRAPH-009`.

Exit gate: `dv build` and the supported token-replacement forms produce the
same meaningful outputs and failures as the reference build for both fixtures;
cancelled or failed builds expose no partial successful target.

Dominant cost: compiler process latency, output bytes written/copied, and graph
scheduling. Small graphs retain a straight-line sequential path.

### Wave 5: Incremental And No-Op Build

**Outcome:** Repeated builds prove current state with the smallest correct set
of filesystem touches and rebuild only affected stages.

Order:

1. Implement `INCR-001` and `INCR-002` versioned stage fingerprints.
2. Implement `INCR-003` metadata-first, hash-second no-op proof.
3. Implement `INCR-004` and `INCR-005` invalidation boundaries.
4. Implement `INCR-006` through `INCR-010` commit, concurrency, reuse, and
   explanation behavior.
5. Complete `INCR-011` cold, warm, one-source, one-property, package-change,
   generator-change, and no-op benchmarks.

Exit gate: unchanged builds do not launch Roslyn or rewrite outputs; each
fixture mutation rebuilds exactly its dependent stages; concurrent and crashed
commands cannot publish invalid state.

Dominant cost: metadata reads on no-op, content reads only after uncertain
metadata, and atomic state/output writes after success.

### Wave 6: Application Run And P1 Gate

**Outcome:** The first complete inner loop restores, builds, incrementally
rebuilds, and runs through canonical and token-replacement syntax.

Order:

1. Implement `RUN-001` through `RUN-003`, `RUN-005`, `RUN-006`, and
   `RUN-008` through `RUN-010`.
2. Add `RUN-007` launch profiles only after the plain project path passes.
3. Defer `RUN-004`, `RUN-011`, and `RUN-012` to their self-contained,
   file-based-app, and watch slices.
4. Close the Phase 1 subsets of the always-on rails.
5. Run all P1 phase-completion gates from the parity map.

Exit gate: package-free and package-bearing console/library fixtures pass
restore, build, warm no-op, one-source rebuild, and run on Windows, Linux, and
macOS through `dv` and supported `dotnet -> dv` substitutions, without a
forbidden production fallback.

Dominant cost: implicit no-op proof and one runtime host/process launch. Child
arguments, streams, cancellation, and exit status are compatibility outputs.

### Wave 7: Solutions, Multi-Targeting, And Real Repositories

**Outcome:** The same engine handles representative solution-scale repositories
and target dimensions without duplicating project state.

Order:

1. Implement `SLN-001` through `SLN-011` for `.sln`, `.slnx`, solution
   filters, configurations, and project dependencies.
2. Expand remaining P2 `WS-*`, `EVAL-*`, and `PROJ-*` semantics, including
   imports and multi-targeting.
3. Extend `GRAPH-*`, `PACKS-*`, and `INCR-*` to project x TFM x RID batches.
4. Close `RES-018`, `RES-020` through `RES-026`, cache concurrency, audit,
   listing, and remaining P2 restore behavior.
5. Add sanitized or distribution-derived large solution fixtures and measured
   sequential/parallel crossovers.

Exit gate: representative multi-project and multi-target repositories restore,
build, no-op, incrementally rebuild, and run with deterministic results and
bounded work across supported platforms.

Dominant cost: solution/project discovery, evaluation reuse, graph memory,
package metadata, and coarse worker scheduling.

### Wave 8: Native MSBuild Target And Task Compatibility

**Outcome:** Supported direct MSBuild invocations execute required target/task
graphs natively rather than only mapping SDK-style build properties.

Order:

1. Implement `MSCLI-001` through `MSCLI-005`, `MSCLI-009`, `MSCLI-012`,
   `MSCLI-013`, `MSCLI-015` through `MSCLI-018` for the representative corpus.
2. Implement `DROP-004`, `DROP-006`, and required precedence behavior.
3. Implement `MSTASK-001` through `MSTASK-010` for target semantics and common
   native tasks.
4. Implement `MSTASK-011` through `MSTASK-017` only where real repositories
   require process or managed custom-task hosting.
5. Build `MSTASK-018` compatibility corpora before expanding rare task support.

Exit gate: replacing direct `msbuild` or `dotnet msbuild` with `dv` preserves
the supported argv, target results, artifacts, diagnostics, and exit behavior
for the P2 repository corpus.

Dominant cost: target evaluation, filesystem tasks, process launches from
explicit `Exec`, and isolated managed task-host work. Task inputs/results cross
only versioned typed protocols.

### Wave 9: Package And Reference Operations

**Outcome:** Repositories can be safely mutated and inspected through canonical,
`dotnet`, and direct NuGet-compatible commands.

Order:

1. Implement `EDIT-001` through `EDIT-011` transactionally.
2. Implement `NGCLI-001`, the consumption subset of `NGCLI-002`, and
   `NGCLI-005`, `NGCLI-006`, and `NGCLI-008`.
3. Implement `DROP-007` and the corresponding `DNCLI-*` add/remove/list forms.
4. Complete `RES-027` only in the later legacy `packages.config` slice.

Exit gate: add, remove, list, search, source, cache, and restore operations
preserve project/config formatting and prior state on failure; overlapping
spellings normalize to the same mutation or query batch.

Dominant cost: bounded XML/config reads, package queries, transactional file
replacement, and optional restore. Dry-run/query paths perform no writes.

### Wave 10: Test Discovery And Execution

**Outcome:** Supported test repositories discover and execute without VSTest
orchestration while retaining VSTest and MTP contracts.

Order:

1. Implement `TEST-001` through `TEST-008` for fixtures, project detection,
   adapter-host protocol, stable identity, cached discovery, and list-tests.
2. Implement `TEST-009` through `TEST-017` for filters, execution, isolation,
   output, result states, and result files.
3. Implement `TEST-018` through `TEST-022` for diagnostics, attachments,
   retries, cancellation, and exits.
4. Implement `DROP-005`, `DROP-008`, `VSTCLI-001` through `VSTCLI-005`,
   `VSTCLI-007`, `VSTCLI-008`, and relevant `DNCLI-*` forms.
5. Complete `TEST-023` benchmarks and the P3 phase gate.

Exit gate: xUnit, NUnit, MSTest, VSTest-adapter, and MTP fixtures have equivalent
discovery, filtering, execution, attachments/results, cancellation, and exits
through every supported command shape.

Dominant cost: build/no-op proof, adapter and test-host process startup, test
runtime, captured output bytes, result files, and bounded module scheduling.

### Wave 11: Project And Template Creation

**Outcome:** New projects enter the already-compatible build/test path without
special execution machinery.

Order:

1. Implement `NEW-001` through `NEW-005` for built-in offline templates and
   solution insertion.
2. Implement `NEW-006` through `NEW-010` for template discovery, package
   lifecycle, constraints, and reviewed post-actions.
3. Add current and legacy `dotnet new` command orderings from `SURF-009` and
   `SURF-010`.

Exit gate: generated projects match selected template artifacts and immediately
pass sync/build/run or test through token-replacement commands.

Dominant cost: template/package reads and atomic file creation. Dry-run performs
no writes or post-actions.

### Wave 12: Pack And Package Publishing

**Outcome:** Build outputs become deterministic NuGet packages and can be
published safely.

Order:

1. Implement `PACK-001` through `PACK-012` for standard and explicit nuspec
   packages.
2. Implement `PACK-013` and the evidence-backed validation subset of `PACK-014`.
3. Implement `PUB-017`, `PUB-018`, `NGCLI-003`, `NGCLI-004`, and corresponding
   `dotnet nuget`/direct NuGet command forms.
4. Close NuGet sign/verify and trusted-signer operations in `SURF-008` after
   packaging and signature primitives are shared.

Exit gate: nupkg/snupkg contents, dependency groups, metadata, signatures,
push/delete behavior, diagnostics, and exits match the supported reference
commands without exposing secrets or partial publication state.

Dominant cost: input/output bytes, deterministic ZIP work, signature work, and
authenticated network uploads. Package construction is CPU/file work, not async
executor work; independent uploads may use bounded async I/O.

### Wave 13: Application Publish

**Outcome:** Applications publish in increasing order of transform complexity.

Order:

1. Implement `PUB-001` and `PUB-002` framework-dependent portable and
   RID-specific output.
2. Implement `PUB-003` through `PUB-006`, `PUB-011` through `PUB-014` for
   self-contained assets, profiles, atomic output, manifests, and parity.
3. Implement ReadyToRun, trimming, single-file, and Native AOT in the order
   `PUB-007` through `PUB-010`, each through selected Microsoft tools directly.
4. Implement web/container specializations `PUB-015` and `PUB-016` only after
   the base publish manifest and collision rules are stable.

Exit gate: supported `dotnet publish -> dv publish` substitutions produce
equivalent runnable behavior and meaningful file roles for every declared mode.

Dominant cost: runtime-pack bytes, copy/link work, Microsoft optimization-tool
processes, and output storage. Each advanced transform has its own cold/warm
evidence rather than inheriting the base publish claim.

### Wave 14: SDK, Runtime, Tool, And Workload Management

**Outcome:** `dv` manages the selected Microsoft platform installations and
global command families without using `dotnet` as the orchestrator.

Order:

1. Complete `SDK-006` through `SDK-012` inventory, acquisition, verification,
   installation, and removal.
2. Complete `SDK-013` through `SDK-019` update checks, channels, architectures,
   cache, concurrency, and self-management behavior.
3. Implement `SURF-001`, `SURF-003` through `SURF-006`, and `SURF-011` in
   measured command-family slices.
4. Complete remaining `DNCLI-*` and `NGCLI-*` management command surfaces.

Exit gate: every declared SDK/runtime/tool/workload management command has a
versioned manifest, verified downloads, interruption-safe state, compatible
script output/exits, and cross-platform evidence.

Dominant cost: network bytes, signatures/hashes, archive extraction, disk
retention, and atomic installation. Downloads use bounded async I/O; hashing and
extraction use measured CPU worker batches.

### Wave 15: Broad Project And Legacy Compatibility

**Outcome:** Close language, project-system, design-time, legacy, and rare
command gaps using observed repositories rather than speculative frameworks.

Order:

1. Implement `BROAD-001` through `BROAD-015` in frequency and dependency order
   derived from the compatibility corpus.
2. Complete advanced `MSCLI-*`, `MSTASK-*`, `DROP-*`, and `DNCLI-*` rows left
   after the earlier workflows.
3. Complete `SURF-002`, `SURF-007`, `SURF-009`, `SURF-012`, and `SURF-013` for
   all declared SDK/tool versions.
4. Add each newly observed behavior to the manifest before implementation so
   unsupported behavior remains visible rather than silently approximated.

Exit gate: each supported language/workload/tool/version row has representative
inputs, typed protocols, artifacts or observable results, diagnostics, exits,
and performance evidence.

Dominant cost: varies by observed workflow and must be stated per slice. No
cross-language or extension abstraction is accepted without a real consumer.

### Wave 16: Full Parity Closure

**Outcome:** Convert scoped compatibility claims into the intended unqualified
drop-in claim only when the evidence permits it.

Order:

1. Enumerate the complete selected-version command surfaces using `GATE-015`.
2. Fail the build for manifest omissions through `SURF-014` and `GATE-022`.
3. Close every remaining partial or missing parity row, or explicitly narrow
   the published product claim instead of marking it complete.
4. Run Windows, Linux, and macOS compatibility and release gates for every
   declared workflow.
5. Establish numeric regression budgets from retained representative samples,
   then enforce correctness, latency, throughput, memory, allocation, process,
   filesystem, and network budgets.

Exit gate: no required row in the published manifests is missing or partial;
paired executable-token tests and meaningful artifacts/results pass for every
declared `dotnet`, MSBuild, NuGet, and VSTest surface.

## Reordering Rules

This order may change only when recorded evidence shows one of the following:

- an earlier wave depends on a capability currently scheduled later;
- representative repositories rank a missing workflow above the current one;
- compatibility or benchmark evidence disproves the active transform;
- a security/correctness defect requires immediate closure;
- selected Microsoft tool versions add, remove, or materially change a surface.

Record a reorder in this document with the affected parity IDs, evidence, cost,
and replacement gate. Do not reorder because a later feature is easier, more
interesting, or increases the checked-row count faster.

## Completion Accounting

The parity map remains the only feature-completion ledger. This order document
answers "what next," not "how complete are we." Report progress using:

- the earliest open wave and its exit-gate evidence;
- implemented/partial/missing parity states without converting them into a
  percentage estimate;
- completed end-to-end workflows and supported reference-tool versions;
- cold/warm/incremental/no-op correctness and cost evidence;
- unresolved assumptions and issue files.

Full feature parity remains the destination after every wave. The ordering
exists to make each step usable, testable, measurable, and cumulative on the
way there.
