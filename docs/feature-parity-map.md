# Feature Parity Implementation Map

This document maps the work required to satisfy the product goal in `PLAN.md`:
replace the practical C# and .NET workflows normally orchestrated by the .NET
CLI, MSBuild, NuGet, and test tooling, without using those tools as production
fallbacks.

It is a capability map, not a schedule. The dependency-aware work sequence is
maintained in [implementation-order.md](implementation-order.md). A checked
item is present in the repository at the snapshot below. An unchecked item is
required unless it is explicitly identified as a native `dv` addition.
`REJECT` describes safe intermediate behavior for an unfinished compatibility
row, not optional work.

Snapshot: working tree on 2026-08-02.

## Scope Contract

Feature parity has three simultaneous layers:

1. **Drop-in invocation parity:** replacing only the executable token in a
   supported `dotnet`, direct MSBuild, NuGet, or VSTest command must preserve
   the existing command, option, argument, environment, exit, side-effect, and
   machine-consumed output contract.
2. **Workflow and artifact parity:** the workflows named in `PLAN.md` must
   evaluate the same meaningful inputs and produce the same meaningful
   dependency decisions, compiler inputs, artifacts, test results, packages,
   publish trees, and runtime behavior.
3. **Native `dv` experience:** `dv` may additionally organize commands and
   parameters into a smaller or clearer vocabulary, such as `dv sync`, but
   that vocabulary is an alias over the same typed transforms. It never
   replaces or weakens the compatibility spellings.

Drop-in means these substitutions are product contracts:

| Existing invocation | Required `dv` invocation | Contract |
|---|---|---|
| `dotnet build App.sln -c Release` | `dv build App.sln -c Release` | Same accepted arguments and meaningful build result |
| `dotnet restore App.sln --locked-mode` | `dv restore App.sln --locked-mode` | Same restore and locked-mode behavior |
| `dotnet msbuild App.sln -t:Build -p:X=Y` | `dv msbuild App.sln -t:Build -p:X=Y` | Same MSBuild command-line shape |
| `msbuild App.sln /t:Build /p:X=Y` | `dv App.sln /t:Build /p:X=Y` | Direct-MSBuild shape inferred without rewriting arguments |
| `nuget restore App.sln -NonInteractive` | `dv restore App.sln -NonInteractive` | NuGet spelling and behavior accepted |
| `vstest.console Tests.dll /TestCaseFilter:X` | `dv Tests.dll /TestCaseFilter:X` | Direct VSTest container/options shape inferred |
| `dotnet Tests.dll arg` | `dv Tests.dll arg` | Same managed-application host behavior |

The canonical `dv` syntax may differ only as an additional entry point. For
example, both `dv sync` and `dv restore` may reach the same restore transform,
but removing `dv restore` or changing the meaning of a reference option would
break the drop-in contract.

The drop-in target includes the command-line surfaces shipped by the selected
.NET SDK/tool versions. Compatibility is versioned because those surfaces
change. Every release must publish a machine-readable compatibility manifest
that lists the reference tool/version, commands, option spellings, defaults,
environment inputs, exit behavior, output formats, and known unsupported rows.

Explicit non-goals are limited to:

- invoking `dotnet`, the Microsoft MSBuild engine, NuGet, or VSTest as a hidden
  production fallback;
- reproducing Microsoft internal architecture rather than behavior;
- byte-identical incidental prose, timing, nondeterministic identifiers, or
  terminal decoration that no documented or observed automation consumes;
- Visual Studio UI behavior, except for files and design-time command results
  required for repository/tool interoperability;
- reimplementing Roslyn or the Microsoft runtime;
- silently approximating unsupported behavior.

MSBuild project, target, and task semantics required by the compatibility
corpus are in scope. Rust still owns evaluation, target planning, scheduling,
incremental state, and reporting. Managed built-in or custom task assemblies
may eventually run through a versioned typed task-host boundary; invoking the
Microsoft MSBuild engine is not an acceptable implementation.

### Drop-In Definition Of Done

A compatibility row is complete only when:

1. the reference command is copied unchanged except for replacing its
   executable token with `dv`;
2. `dv` accepts the same paths, commands, options, aliases, ordering, quoting,
   response files, environment, stdin, and working directory;
3. it performs the same meaningful restore/build/test/run/package/publish
   decisions and side effects without launching a forbidden fallback;
4. it returns compatible exit behavior and every documented or observed
   machine-consumed output format;
5. its artifacts or observable runtime behavior match the reference oracle;
6. canonical `dv` syntax, when offered, normalizes to the same typed request;
7. unsupported input fails explicitly and keeps that row marked incomplete.

Incidental interactive prose may be clearer in native `dv` mode. Compatibility
mode must retain text layouts that real scripts parse, even when those layouts
are not formally documented.

## Status And Delivery Labels

- `[x]` implemented and covered by repository tests.
- `[~]` partial foundation exists, but the parity contract is incomplete.
- `[ ]` not implemented.
- `P1` fast inner-loop vertical slice.
- `P2` real repository compatibility.
- `P3` testing.
- `P4` distribution and SDK/runtime acquisition.
- `P5` broad compatibility.
- `REJECT` must produce a stable diagnostic rather than approximate behavior.

`REJECT` is an honest intermediate state, not drop-in completion. A command or
project can be called drop-in compatible only when every exercised row in its
versioned compatibility manifest is implemented.

No percentage-complete value is assigned. The items differ radically in cost
and risk, so a count-based percentage would be misleading.

## Real Data At This Snapshot

### Inputs

Repository-owned representative input currently consists of:

- raw OS argument vectors shaped like `dotnet`, MSBuild, NuGet, and VSTest
  invocations, although a checked-in compatibility corpus does not yet exist;
- twelve SDK-style C# projects;
- ten C# source files;
- three project-reference edges;
- 53 exact package references across single-package and real graph fixtures;
- nine single-project fixtures and one three-project acyclic fixture;
- one observed Windows machine with three x64 SDK installations and five
  installed versions in each of the Core, ASP.NET Core, and Windows Desktop
  shared-framework families.

The initial compiler trace in `docs/roslyn-invocation.md` observed SDK
`10.0.100`, 164 reference arguments, eight analyzer/generator arguments, three
analyzer-config arguments, four C# inputs, a 4,608-byte assembly, an
11,340-byte PDB, a 156,160-byte apphost, a 428-byte dependency manifest, and a
268-byte runtime configuration.

The Rust workspace currently has 16 Rust source files, 17,620 nonblank source
lines, and 106 `#[test]` functions. These counts describe the current
repository, not the expected shape of real customer repositories.

### Outputs

Current output is command-local human text or a schema-v8 JSON-lines event
batch. Drop-in modes must also reproduce documented or observed
machine-consumed text/JSON/XML/binary-log formats and tool-specific exit
behavior. Future workflows must additionally own:

- a command-lifetime workspace inventory;
- evaluated project and target records;
- a resolved package graph and persistent lock state;
- immutable compiler input batches;
- build artifacts and incremental state;
- runtime/test process results;
- NuGet packages and publish directories.

Persistent cache entries are tool-owned until eviction. Build artifacts are
workspace-owned until clean or replacement. Reporter events are owned by the
producer only until the reporter call completes.

### Observed Distribution And Missing Facts

The observed common case is currently tiny, mostly package-free, SDK-style C#
on Windows. It cannot justify large-repository layouts or concurrency
thresholds.

`ASSUMPTION: SDK-style C# projects using PackageReference are the dominant
initial customer input - affects language, evaluator, and resolver sequencing.`

`ASSUMPTION: repeated no-op build and test commands are the highest-value
latency paths - affects fingerprint and daemon prioritization.`

`ASSUMPTION: a useful first compatibility boundary can initially reject
arbitrary custom tasks while accepting declarative properties, items, and
imports that only affect known transforms - affects sequencing, but every
observed rejected task remains drop-in parity work.`

Representative large, test-heavy, and authenticated multi-source data remains
missing and is already tracked under `issues/`.

### Stable And Changing Data

Stable within a selected SDK:

- SDK targets, reference packs, compiler binaries, RID graph, and built-in
  analyzer payloads;
- immutable package contents addressed by identity, version, and hash;
- normalized project paths during one command.

Frequently changing:

- project XML, imported props/targets, source and generated files;
- package-source metadata and authentication;
- lock state, build outputs, environment variables, CLI properties;
- source-generator outputs, test discovery, and publish settings.

### Dominant Side Effects

The eventual inner-loop transform performs:

1. one raw argument/environment read and compatibility-mode classification
   with no filesystem or network work;
2. ancestor walks and batched directory enumeration;
3. XML, JSON, editor-config, solution, NuGet, and package-manifest reads;
4. package cache lookups, HTTP requests, downloads, verification, extraction,
   and atomic writes;
5. source/reference hashing and incremental-state reads/writes;
6. Roslyn host launch or reuse and compiler IPC;
7. artifact creation, copy/link, and manifest writes;
8. application or test process launch, stream capture, cancellation, and wait;
9. one ordered reporter write batch.

Every implementation below must remove side effects not required by its output.
A warm no-op must not perform network I/O or launch a managed process merely to
prove that nothing changed.

## Current Capability Inventory

| Capability | Status | Evidence |
|---|---|---|
| Native process startup, help, and version | Implemented | `crates/dv-cli/src/main.rs` |
| Stable unknown/unsupported command failures | Implemented | CLI tests and `DV0001`-`DV0003` |
| Command-lifetime cancellation | Implemented | early Ctrl+C/SIGINT handler, cache-aligned token, bounded child deadline, cancellable package I/O |
| Drop-in command parsing | Initial subset | several `dotnet` command names overlap; no complete tool/version grammar |
| Installed SDK discovery | Implemented | `crates/dv-core/src/sdk.rs` |
| `global.json` SDK selection | Implemented | policy and fixture tests |
| SDK current/list human output | Implemented | CLI integration tests |
| Versioned JSON event stream | Implemented | event/reporter unit tests |
| Structured diagnostics | Foundation only | codes and ordered fields exist |
| Benchmark process harness | Foundation only | startup, SDK, project, package, pack, and framework-plan cases measured |
| Single C# project discovery | Initial subset | explicit/one-directory selection and ambiguity diagnostics |
| Project evaluation | Initial subset | parsed single modern .NET TFM, base-SDK properties/items, and `project inspect` |
| Compiler input planning | Initial subset | target-selected reference pack, Roslyn/analyzers, options, packages, and `build --plan` |
| Package resolution and cache | Initial subset | interval and NuGet floating versions, dependency-only meta-packages, bounded streaming resolution, local and NuGet v2/v3 HTTPS sources, official service-index capability discovery, verified atomic package cache, deterministic dv lock, and identical `restore`/`sync` commands |
| Solution discovery and evaluation | Missing | no production types or commands |
| Restore | Initial subset | exact package restore is implemented; most reference flags and graph cases remain |
| Build execution, run, and test | Missing | build only plans inputs; run/test remain unsupported |
| Pack, publish, SDK/runtime install | Missing | commands return `DV0003` |

## Dependency Spine

The shortest dependency-respecting route to useful parity is:

1. versioned invocation compatibility manifests and captured script corpus;
2. compatibility parsers normalized into one typed command model;
3. compatibility evidence and fixture expansion;
4. workspace selection;
5. project/solution parsing;
6. bounded declarative evaluation;
7. framework, targeting-pack, and runtime-pack resolution;
8. NuGet configuration, source access, package graph, cache, and lock state;
9. target-expanded build graph;
10. generated sources and compiler input batches;
11. native Roslyn hosting and post-compile artifacts;
12. incremental proof and deterministic scheduling;
13. runtime launch;
14. test protocol and adapters;
15. pack and publish;
16. SDK/runtime acquisition;
17. broader SDK, language, framework, and extension compatibility.

A later stage may be prototyped against captured compatibility data, but it
cannot be declared complete before its prerequisites have stable typed
contracts.

## 1. Command And Process Contract

- [x] `CLI-001` Parse help and self-version without project or SDK discovery.
- [x] `CLI-002` Reject non-Unicode command text with a stable diagnostic.
- [x] `CLI-003` Emit stable exit code 2 for current command failures.
- [x] `CLI-004` Offer one `--json` event stream for current commands.
- [x] `CLI-005` Replace string matching with a typed, batch-first command
  request that retains lossless OS arguments where paths require it. `P1`
- [x] `CLI-006` Define global `--help`, `--version`, `--json`, `--verbose`,
  `--quiet`, `--color`, `--no-color`, and diagnostic verbosity behavior. `P1`
- [x] `CLI-007` Preserve the reference tool's documented and observed exit
  behavior in compatibility mode, then map it to stable native `dv` outcome
  classes internally. Explicit `dotnet`, MSBuild, NuGet, and VSTest profiles
  map typed usage, unsupported, and operation failures to the exit behavior
  observed from SDK `10.0.100`; native failures retain stable code 2. `P1`
- [x] `CLI-008` Support `--project`, explicit project/solution paths, and
  unambiguous current-directory defaults. One borrowed typed selector is shared
  by the current inspect, plan, restore, sync, framework, runtime-pack, and
  package-source commands; malformed mixed/repeated selection fails before
  project I/O. Solution selection is typed here while solution evaluation
  remains owned by `SLN-001` through `SLN-011`. The like-for-like named-project
  query measured `328.778 ms` for Microsoft versus `6.204 ms` for `dv`
  (`53.0x`). `P1`
- [ ] `CLI-009` Support repeated CLI property overrides without reparsing
  strings in downstream stages. `P2`
- [ ] `CLI-010` Support configuration, framework, runtime, architecture,
  operating-system, output, artifacts-path, and no-restore selectors
  consistently across applicable commands. `P2`
- [x] `CLI-011` Reject unknown options at the same command boundary as the
  reference tool, before unrelated filesystem or network work. The initial
  linear argument pass rejects unknown global options, and the active help,
  version, SDK, project, build, restore, and sync boundaries reject their own
  unknown options before current-directory, project, SDK, or NuGet discovery.
  SDK parsing produces one borrowed typed request and allocates nothing on its
  successful path. A malformed project/global fixture covers every active
  boundary and compatibility exit code; the benchmark preflight additionally
  requires Microsoft's `MSB1001`, `dv`'s `DV0002`, the exact option spelling,
  and an unchanged workspace. Thirty warm Windows samples measure `146.054 ms`
  for Microsoft versus `4.827 ms` for `dv` (`30.3x`). `P1`
- [x] `CLI-012` Preserve arguments after `--` byte-for-byte for child
  application and test processes. `run` and `test` now receive one typed
  borrowed `OsString` tail from the process-owned argument batch; empty and
  non-Unicode operands, repeated delimiters, and post-delimiter globals remain
  opaque, and no second buffer or parse is introduced. A one-word optional
  nonzero index distinguishes no delimiter from an empty tail without widening
  the hot request; a 64-token test remains one direct slice. The .NET 10 oracle
  proves the same four-argument tail through `dotnet run --`. Thirty warm
  Windows samples measure direct Microsoft host capture at `44.698 ms` and the
  `dv` typed handoff at `5.606 ms` (`8.0x` lower), but this is explicitly not a
  like-for-like execution claim: child launch remains in the ordered run/test
  workflows. `P1`
- [x] `CLI-013` Define environment-variable precedence and redact secrets from
  all output modes. The process boundary reads `DV_COLOR`, `DV_VERBOSITY`, and
  `NO_COLOR` exactly once into a five-byte typed policy: explicit command-line
  output options beat `DV_*`, which beats the standard `NO_COLOR` default,
  which beats built-ins. Invalid or non-Unicode values retain no supplied text
  and fail before discovery unless a higher-priority command option replaces
  them. Child-process overlays use ambient, `[env:NAME=VALUE]`, launch-profile,
  then `-e|--environment NAME=VALUE` precedence with stable last-wins ordering.
  The command-lifetime plan borrows four 24-byte edits inline and spills only
  beyond that bound; directives fail explicitly on commands that cannot
  consume them. Launch-profile ingestion and child launch remain in their
  ordered run/test rows. A .NET 10 child oracle proves
  ambient/directive/command-line precedence without printing its secret.
  Human diagnostics and schema-18 JSON argument events redact
  separated/combined API keys, passwords, tokens, credentials, secret MSBuild
  properties, and parsed URL userinfo/query/fragment data before writing.
  Existing NuGet environment credentials remain zeroized and report only
  authentication kind. The identical `build --definitely-unknown` timed oracle
  proves reference failure, no ANSI, no sentinel disclosure, and no workspace
  mutation. Thirty warm Windows samples measure Microsoft at `134.218 ms` and
  `dv` at `5.503 ms` (`24.4x` faster). `P1`
- [x] `CLI-014` Install Ctrl+C/SIGINT cancellation before starting work and
  propagate a bounded cancellation deadline to children. Typed invocation is
  classified before installation so help, version, unknown-command, and
  global-option failures retain their allocation-free fast path; every
  work-bearing command installs one
  handler before SDK, project, filesystem, process, or network work. One
  cache-line-aligned state allocation keeps its hot atomics first and is bounded
  to one or two assumed cache lines across supported hosts. It records the first
  signal against a monotonic epoch, wakes Tokio work, and exposes an absolute
  two-second child
  deadline; a second signal forces immediate termination. Package source,
  retry, response-stream, restore, and credential-provider waits observe the
  same token. The run/test boundary receives the typed policy while child
  launch remains ordered under `RUN-006` and `RUN-009`. Unix
  process tests deliver real SIGINT during stalled HTTP work; all-platform
  tests cover transitions, deadline stability, diagnostics, and the child
  boundary. Thirty warm Windows samples measure cancellation-ready SDK
  selection at `68.400 ms` for Microsoft and `5.302 ms` for `dv` (`12.9x`
  faster); `dv --version`, which deliberately skips installation, remains
  `4.330 ms`. `P1`
- [x] `CLI-015` Preserve child exit codes where the command contract requires
  it and distinguish launch failure from child failure. Reaped children now
  produce one eight-byte typed termination record containing an exact `i32`
  exit, Unix signal, or explicit unknown state; launch and wait errors remain
  cold, separately staged failures. The CLI terminates through a four-byte
  `i32` boundary, so numeric child exits bypass compatibility failure remapping
  without `u8` truncation. A one-byte policy declares application exits
  preserved and test-host failures mapped into the aggregated test result.
  Real cross-platform process tests retain `0`, `37`, and `211`, distinguish a
  nonexistent executable from a nonzero child, and keep Unix `SIGTERM`
  separate from numeric exits. The structural Windows child-boundary case is
  explicitly excluded from like-for-like claims while `run` is TBI; successful
  SDK selection remains `12.6x` faster and like-for-like unknown-option failure
  remains `28.4x` faster after the outer exit change. Application launch,
  process-group ownership, and signal policy remain ordered under `RUN-006`
  and `RUN-009`. `P1`
- [ ] `CLI-016` Support tool-compatible response files, nesting, encoding,
  quoting, comments, default response-file discovery, opt-out, cycles, and
  size/depth bounds. `P2`
- [x] `CLI-017` Version command syntax and JSON compatibility independently so
  a CLI alias does not mutate the event protocol. The 6-byte typed invocation
  request now carries a two-byte syntax-version value while the reporter owns
  schema version 21. Native `version`, `--version`, and `-V` normalize to one
  tool-version request; explicit dotnet `--version` instead normalizes to the
  selected-SDK request required by the reference command. Raw alias arguments
  remain reporter evidence rather than protocol selection. The
  structural query measured `4.479 ms` median and `5.593 ms` p95 on Windows;
  Microsoft has no equivalent dual-version query, so its result is explicitly
  TBI. The like-for-like SDK control remains `13.1x` faster. `P1`
- [x] `CLI-018` Expose the initial evaluator through human and JSON
  `project inspect` output.

## 1A. Drop-In Invocation Routing

- [x] `DROP-001` Generate a versioned compatibility manifest from the selected
  reference SDK/tool set, covering every command, option alias, argument
  position, default, environment input, exit case, and output format. Manifest
  schema/version 1 is generated from .NET SDK `10.0.100`, MSBuild
  `18.0.2.52411`, NuGet `7.0.0.0`, and VSTest `18.0.1`. A bounded recursive
  help walk retains 115 command paths, 769 option records with expanded MSBuild
  prefixes/short forms, 74 argument records, declared environment and output
  contracts, four observed failure exits, and all 468 parity rows. Every
  captured child has a record; support is dimensional and missing work remains
  explicit. The initial `dv compat manifest` query wrote a 270,082-byte artifact
  without discovery, parsing, allocation of a manifest model, filesystem, or
  network work. Thirty Windows samples measured `4.795 ms` median and
  `5.267 ms` p95; Microsoft has no equivalent query and is TBI. The
  like-for-like selected-SDK control remains `14.1x` faster. `P1`
- [x] `DROP-002` Store raw arguments once as lossless OS strings, then normalize
  all accepted tool spellings into one typed command batch. All 19 accepted
  spellings map to 14 exact semantic command kinds; `sync` and `restore` share
  `Restore`, while raw spelling and compatibility provenance remain cold
  reporting/failure-policy data. Moving the raw command index out of the hot
  semantic record reduced it from 16 bytes at machine-word alignment to 6
  bytes at alignment 2. The transform remains one linear scan with no new
  allocation or I/O. Thirty Windows rejection samples measured `121.211 ms`
  for `dotnet restore` and `5.462 ms` for normalized `dv sync`, a `22.2x`
  median improvement; the SDK control remained `13.3x` faster. `P1`
- [x] `DROP-003` Classify invocation mode deterministically before project,
  SDK, filesystem, process, or network work. Native mode and all eight explicit
  `--compat` forms converge during the existing linear OS-argument scan. The
  transient classifier is 5 bytes at byte alignment: three bytes of output
  policy, one mode byte, and one explicitness bitset. It performs no second
  scan, allocation, lookup table, filesystem access, or process launch;
  malformed, non-Unicode, and repeated selectors reject at the same boundary.
  Fifty like-for-like Windows rejection samples measured `152.984 ms` for
  `dotnet` and `5.641 ms` for `dv`, a `27.1x` median improvement. `P1`
- [ ] `DROP-004` Treat a first token matching a `.csproj`, `.fsproj`, `.vbproj`,
  `.sln`, `.slnx`, `.proj`, `.targets`, or `.props` input plus MSBuild switches
  as direct-MSBuild replacement syntax. `P2`
- [ ] `DROP-005` Treat one or more test-container inputs plus VSTest `/`
  switches as direct `vstest.console` replacement syntax. `P3`
- [ ] `DROP-006` Accept both `dv msbuild ...` from `dotnet -> dv` replacement
  and `dv PROJECT ...` from `msbuild -> dv` replacement. `P2`
- [ ] `DROP-007` Accept both `dv nuget COMMAND ...` from `dotnet -> dv`
  replacement and `dv COMMAND ...` where the direct NuGet grammar is
  unambiguous. `P2/P4`
- [ ] `DROP-008` Accept `dv vstest ...` and direct test-container syntax for
  `dotnet vstest` and `vstest.console` replacement. `P3`
- [ ] `DROP-009` Accept `dv APP.dll ...`, `dv exec APP.dll ...`, and runtime
  host options for direct `dotnet` application-host replacement. `P2`
- [x] `DROP-010` Resolve ambiguous command words such as `restore`, `pack`,
  `push`, `list`, `add`, and `update` using an explicit precedence table that
  matches the input shape and never guesses after side effects begin. The
  selected profile and first semantic OS token index a 35-byte read-only
  matrix and map directly to one of 26 exact one-byte command kinds;
  native/dotnet, NuGet, MSBuild-input, and VSTest-input routes cannot cross
  into one another. NuGet-only `push` and `update` remain unknown without
  NuGet evidence. The six-byte request, one linear scan, and allocation/I/O
  counts are unchanged. Fifty Windows pre-I/O rejection samples measured
  `280.174 ms` for `dotnet pack` and `5.242 ms` for the corresponding `dv`
  route, a `53.4x` median improvement.
  `P1`
- [x] `DROP-011` Permit explicit `--compat dotnet|msbuild|nuget|vstest` for
  diagnostics and ambiguous automation without requiring it for ordinary
  executable-token replacement. The typed selector and exit policy are
  complete. Every selected-profile failure now appends exactly one stable
  `compatibility_profile` context field to both human and JSON diagnostics;
  native failures and invalid or repeated selectors omit it. This failure-only
  transform adds no request state or successful-path allocation. Complete
  reference grammars and byte-exact diagnostic layouts remain owned by their
  dedicated rows. Fifty like-for-like Windows rejection samples measured
  `133.281 ms` for `dotnet` and `5.125 ms` for `dv`, a `26.0x` median
  improvement. `P1`
- [ ] `DROP-012` Detect optional executable aliases or shims named `dotnet`,
  `msbuild`, `nuget`, and `vstest.console` through `argv[0]`, while keeping the
  same parser and execution transforms. `P5`
- [x] `DROP-013` Preserve case sensitivity, option-prefix rules, combined
  values, repeated options, separators, quoting, empty arguments, and end-of-
  options behavior for the selected reference tool/platform. The process-owned
  OS token batch is the lossless representation: quoting is resolved once by
  the platform, while token boundaries, empty values, non-Unicode data, and
  post-`--` text remain borrowed without copying. The one-pass scan applies
  exact dotnet command/option case, case-insensitive NuGet command routing, and
  Windows-only slash prefixes for dotnet/MSBuild/VSTest. Implemented Phase 1
  values accept separate, `=`, and `:` forms; singleton repetitions reject
  before project I/O while repeatable sources preserve order. Later command
  rows own their option semantics, but the lexical batch already reaches those
  TBI boundaries intact. Fifty Windows pre-I/O samples of `-c:Release` followed
  by the same invalid sentinel measured `141.461 ms` for `dotnet` and
  `4.912 ms` for `dv`, a `28.8x` median improvement. `P1-P3`
- [ ] `DROP-014` Preserve precedence among command options, response files,
  environment variables, config files, project properties, and defaults. `P2`
- [ ] `DROP-015` Preserve script-consumed stdout/stderr placement, encodings,
  line endings, quiet/verbosity behavior, JSON/XML schemas, result files, and
  binary formats. `P1-P4`
- [~] `DROP-016` Preserve success, usage, build, restore, test-failure,
  no-tests, cancellation, and child-process exit behavior per reference tool.
  Success, usage, unsupported, operation, build, restore, test-failure,
  no-tests, and cancellation results use one 45-byte profile/result matrix;
  inapplicable tool/outcome pairs are explicit sentinels. Reachable Phase 1
  build and restore errors are classified before its allocation-free indexed
  lookup. Normal child exits retain their exact typed `i32` result. Executed
  child, test execution, and signal/cancellation parity remain open with their
  owning workflows. A missing-project restore measured `122.756 ms` for
  `dotnet` and `5.158 ms` for `dv`, a `23.8x` median improvement. `P1-P3`
- [x] `DROP-017` Make help/version/info output expose both accepted
  compatibility syntax and canonical `dv` syntax without changing the result
  of reference `--help`, `/?`, or `help` forms. Profile-aware static help
  accepts the pinned root and Phase 1 command aliases without SDK, project, or
  filesystem discovery. `dotnet --version` and `--info` compatibility syntax
  normalizes to `dv sdk current` and `dv sdk info`; the selected SDK version is
  byte-identical for the version query, and info labels both spellings. Static
  build help measured `135.885 ms` for `dotnet` and `5.518 ms` for `dv`, a
  `24.6x` median improvement. `P1`
- [ ] `DROP-018` Accept deprecated aliases for as long as the pinned reference
  tool does, emit equivalent deprecation diagnostics, and record removals in
  the compatibility manifest. `P5`
- [~] `DROP-019` Prove that each canonical `dv` command and its compatibility
  aliases create identical typed transform batches after parsing. The Phase 1
  `build`, `restore`, `run`, and `test` spellings plus the native `sync` alias
  now dispatch through one borrowed transform view. Equality covers the
  six-byte normalized request, global policy, semantic operands, child tail,
  and environment directives while excluding cold spelling/profile
  provenance. Empty and non-Unicode tokens remain lossless. The view is one
  machine word, allocates nothing, and is used directly by dispatch. Fifty
  Windows pre-I/O samples measured `141.390 ms` for `dotnet` and `5.606 ms`
  for `dv`, a `25.2x` median improvement. SDK query, MSBuild, NuGet, VSTest,
  and later workflow transforms remain bounded by their owning rows rather
  than being falsely claimed by the Phase 1 proof. `P1-P4`
- [~] `DROP-020` Maintain golden argv, environment, stdin, stdout, stderr, exit,
  filesystem, process, and network traces for real CI/build-script
  substitutions. The schema-version-1 corpus covers the repository's real
  selected-SDK and valid offline-restore CI commands. A typed verifier pins
  reference/candidate argv, nine ordered environment overrides, explicit empty
  stdin, normalized stdout/stderr, exit zero, and sorted filesystem deltas.
  The timed restore changes only the executable token and resets its identical
  package-free input outside the interval. Fifty Windows samples measured
  `626.828 ms` for `dotnet` and `7.507 ms` for `dv`, an `83.5x` median
  improvement. Process-tree and network observation remain explicitly `TBI`
  under issue 0009 rather than being inferred as zero. Build/run/test traces
  and cross-platform process/network observers remain open. `P1-P5`
- [x] `DROP-021` Add a `dv compat check` command that scans scripts and project
  inputs, reports unsupported invocation rows without executing them, and
  identifies the exact compatibility manifest version. The bounded path-batch
  transform accepts SDK-style `.csproj`, GitHub Actions YAML literal `run:`
  scalars, and line-oriented PowerShell, POSIX shell, cmd, and batch files.
  It reads each file once, uses a DTD-rejecting XML scan for project shape and
  `Exec` inputs, classifies commands through a build-generated static index
  over the embedded manifest, and reports implemented, partial, missing, or
  uncheckable records in deterministic source order. Dynamic, malformed,
  oversized, and non-UTF-8 input is rejected or retained as uncheckable rather
  than guessed. Human and
  event-schema-21 JSON output consume the same typed report and name embedded
  compatibility manifest version 1. No discovered command, SDK tool, or
  network request is executed. The release benchmark measured `5.791 ms`
  median and `7.314 ms` p95 on Windows across 50 retained samples; Microsoft
  has no equivalent static compatibility command, so its result is explicitly
  TBI rather than a false like-for-like ratio. `P2`
- [x] `DROP-022` Never claim a command is drop-in compatible while any accepted
  option is ignored; every option must affect the typed request or fail
  explicitly. The current Phase 1 global, build, restore, project, run, and test
  option surfaces are covered by effect tests. Build retains `--plan` in its
  32-byte typed request instead of pre-scanning and discarding it. Run/test use
  one linear transform into a 136-byte project/configuration/environment batch;
  malformed, repeated, and unsupported options fail before project, SDK,
  filesystem, child-process, or network work. Help and the compatibility
  manifest continue to label unimplemented Microsoft options as partial or
  missing rather than accepting them as no-ops. A 50-sample Windows oracle for
  `test --definitely-unknown` measured Microsoft at `140.183 ms` median and
  `159.044 ms` p95 versus `6.035 ms` and `7.126 ms` for `dv`, a `23.2x`
  median improvement with identical exit-1, sentinel, and zero-mutation
  preconditions. `P1-P5`

## 1B. `dotnet` Driver Command-Line Surface

- [~] `DNCLI-001` Support global `--info`, `--version`, `--list-sdks`,
  `--list-runtimes`, architecture selection, diagnostics, help, and verbosity
  with compatible text layouts where scripts consume them. The executable
  `--list-sdks` and `--list-runtimes` queries now produce the exact current-
  architecture .NET 10 row order and text layout without launching a managed
  process. Native `sdk runtimes` uses the same typed runtime inventory, while
  the existing `sdk list` remains selection-aware; JSON emits one schema-21
  batch. Incomplete SDK directories are
  excluded using the same `dotnet.dll` completeness boundary. Architecture
  selectors, complete `--info`, diagnostics, and the remaining root grammar
  stay explicit unfinished work. Fifty warm Windows samples measured
  Microsoft at `4.942 ms` median and `5.647 ms` p95 versus `4.901 ms` and
  `5.525 ms` for `dv`, a `1.01x` median improvement at the native process-start
  floor. `P1/P4`
- [ ] `DNCLI-002` Support `build`, `clean`, `new`, `pack`, `publish`,
  `restore`, `run`, `test`, `vstest`, `msbuild`, `sdk check`, `sln`, and
  managed-application execution entry points. `P1-P5`
- [ ] `DNCLI-003` Support `package add/list/remove/search` and both current and
  older `add/list/remove package` orderings for SDK versions that expose them.
  `P2`
- [ ] `DNCLI-004` Support `reference add/list/remove` and older
  `add/list/remove reference` orderings. `P2`
- [ ] `DNCLI-005` Support `nuget delete/push/locals` and source
  add/disable/enable/list/remove/update command trees. `P2/P4`
- [ ] `DNCLI-006` Support workload and tool command trees rather than treating
  them as unknown top-level commands. `P5`
- [ ] `DNCLI-007` Dispatch SDK-bundled and installed extension commands such as
  `dev-certs`, `user-secrets`, `watch`, and `ef` through selected-runtime
  hosting without invoking `dotnet`. `P5`
- [ ] `DNCLI-008` Preserve command-specific short/long aliases, option
  availability by SDK version, implicit restore, default configuration, and
  terminal-logger defaults. `P1-P5`
- [ ] `DNCLI-009` Preserve `--` forwarding and the command-specific boundary
  between `dotnet` options, command options, MSBuild properties, test-runner
  options, and application arguments. `P1-P3`
- [ ] `DNCLI-010` Support `dotnet help` command lookup behavior using bundled
  or local documentation with explicit offline behavior. `P5`
- [ ] `DNCLI-011` Support `build-server shutdown` for every server type `dv`
  actually implements and return compatible success for absent servers. `P5`
- [ ] `DNCLI-012` Support runtime-store commands and outputs for SDK versions
  that still expose `dotnet store`. `P5`

## 1C. Direct MSBuild Command-Line Surface

- [ ] `MSCLI-001` Accept `MSBuild.exe [Switches] [ProjectFile]` with the project
  before, after, or among switches as the reference parser permits. `P2`
- [ ] `MSCLI-002` Accept case-insensitive `-switch`, `/switch`, documented
  `--switch`, full names, short names, `:value`, and separate-value forms per
  platform and reference version. `P2`
- [ ] `MSCLI-003` Support target selection with `-target`/`-t`, ordered target
  lists, default targets, initial targets, and target result semantics. `P2`
- [ ] `MSCLI-004` Support global properties with `-property`/`-p`, multiple
  assignments, escaping, quoting, and immutable global-property precedence.
  `P2`
- [ ] `MSCLI-005` Support `-restore`, `-restoreProperty`, and the distinct
  restore/build property sets in one invocation. `P2`
- [ ] `MSCLI-006` Support `-graphBuild`, `-isolateProjects`,
  `-inputResultsCaches`, and `-outputResultsCache` with deterministic graph
  and cache protocols. `P5`
- [ ] `MSCLI-007` Support `-maxCpuCount`/`-m`, `-nodeReuse`, low-priority,
  interactive, and worker-count behavior through the native scheduler. `P2`
- [ ] `MSCLI-008` Support `-toolsVersion`, `-ignoreProjectExtensions`,
  `-validate`, and schema selection where the selected reference version
  exposes them. `P5`
- [ ] `MSCLI-009` Support `-verbosity`, `-noLogo`, `-noConsoleLogger`,
  `-consoleLoggerParameters`, and detailed summary with compatible routing and
  formatting. `P2`
- [ ] `MSCLI-010` Support repeated file loggers, logger parameters,
  distributed loggers, and forwarding loggers through a versioned `dv` logger
  protocol or a compatible managed logger host. `P5`
- [ ] `MSCLI-011` Produce compatible `.binlog` records for `-binaryLogger`
  consumers, including project evaluation, target/task, message, diagnostic,
  file-embed, and result events. `P5`
- [ ] `MSCLI-012` Support `-preprocess`/`-pp` expanded-project output with import
  provenance and selected global properties. `P2`
- [ ] `MSCLI-013` Support `-getProperty`, `-getItem`, `-getTargetResult`, and
  result JSON/text formats without running unrelated targets. `P2`
- [ ] `MSCLI-014` Support `-targets` target-list output and
  `-profileEvaluation` timing output. `P5`
- [ ] `MSCLI-015` Support automatic `MSBuild.rsp`/`Directory.Build.rsp`
  discovery and `-noAutoResponse`. `P2`
- [ ] `MSCLI-016` Match parse-error, unknown-switch, missing-project,
  ambiguous-project, invalid-property, target-failure, and cancellation exit
  behavior. `P2`
- [ ] `MSCLI-017` Capture the exact supported switch matrix from each reference
  MSBuild version instead of treating a current documentation page as a stable
  protocol. `P2-P5`
- [ ] `MSCLI-018` Interpret MSBuild target/task graphs natively and host
  required managed task assemblies through typed inputs/outputs; never call
  the Microsoft MSBuild engine. `P2-P5`

## 1D. Direct NuGet And VSTest Command-Line Surfaces

- [ ] `NGCLI-001` Accept NuGet option names case-insensitively with compatible
  `-Option`, repeated-option, value, quoting, and `-NonInteractive` behavior.
  `P2`
- [ ] `NGCLI-002` Support consumption commands `config`, `help`, `install`,
  `list`, `locals`, `restore`, `search`, `setapikey`, `sources`, and `update`.
  `P2/P5`
- [ ] `NGCLI-003` Support creation commands `init`, `pack`, and `spec`. `P4/P5`
- [ ] `NGCLI-004` Support publishing commands `add`, `delete`, `push`, and
  source/API-key administration. `P4/P5`
- [ ] `NGCLI-005` Preserve NuGet.Config/environment precedence, culture,
  force-English output, verbosity, prompt, consent, and authentication
  behavior. `P2`
- [ ] `NGCLI-006` Preserve package/source query text formats used by scripts
  and provide native JSON only as an additional mode. `P2`
- [ ] `NGCLI-007` Handle NuGet command/version availability, deprecated
  aliases, `update -self`, and Windows/Mono-only behaviors through the
  versioned manifest. `P5`
- [ ] `NGCLI-008` Keep `dv add/remove/list` canonical package UX equivalent to
  the matching direct NuGet or `dotnet package` typed transforms where their
  contracts overlap. `P2`
- [ ] `VSTCLI-001` Accept one or more positional test containers followed or
  preceded by VSTest options in any reference-supported order. `P3`
- [ ] `VSTCLI-002` Accept case-insensitive `/Settings`, `/Tests`, `/Parallel`,
  `/EnableCodeCoverage`, `/InIsolation`, `/TestAdapterPath`, `/Platform`,
  `/Framework`, and `/TestCaseFilter` forms and mutual exclusions. `P3`
- [ ] `VSTCLI-003` Accept repeated `/Logger`, `/Collect`, logger/data-collector
  parameters, `/ResultsDirectory`, and compatible attachment/result paths.
  `P3`
- [ ] `VSTCLI-004` Support `/ListTests`, `/ListDiscoverers`,
  `/ListExecutors`, `/ListLoggers`, and `/ListSettingsProviders` output. `P3`
- [ ] `VSTCLI-005` Support `/Blame`, `/Diag`, blame crash/hang options, dumps,
  sequence files, and platform availability behavior. `P3`
- [ ] `VSTCLI-006` Support `/ParentProcessId`, `/Port`, design-mode protocol,
  and test-host communication only through a bounded versioned host protocol.
  `P5`
- [ ] `VSTCLI-007` Preserve VSTest no-tests, test-failure, host-crash,
  cancelled, usage, and success exit behavior. `P3`
- [ ] `VSTCLI-008` Keep direct VSTest, `dotnet vstest`, `dotnet test`, and
  canonical `dv test` parser paths distinct while sharing discovery/execution
  batches. `P3`

## 2. Workspace And Input Discovery

- [ ] `WS-001` Batch-enumerate `.csproj`, `.fsproj`, `.vbproj`, `.sln`, and
  `.slnx` candidates with stable normalized path indices. `P1`
- [~] `WS-002` Select an explicit file directly and reject a missing,
  wrong-kind, or unreadable path before evaluation. `P1`
- [~] `WS-003` From a directory, select one project or solution and diagnose
  zero or ambiguous candidates with ordered context. `P1`
- [ ] `WS-004` Define repository root discovery independently from project
  selection. `P1`
- [ ] `WS-005` Discover nearest ancestor `global.json`, `NuGet.Config`,
  `Directory.Build.props`, `Directory.Build.targets`, and
  `Directory.Packages.props` with their distinct precedence rules. `P1`
- [ ] `WS-006` Canonicalize only where required; preserve user spelling for
  diagnostics and avoid turning missing paths into false identities. `P1`
- [ ] `WS-007` Detect symlink/junction cycles and workspace escape attempts.
  `P1`
- [ ] `WS-008` Respect case sensitivity of the active filesystem rather than
  the compile target. `P1`
- [~] `WS-009` Exclude `bin`, `obj`, configured output trees, VCS metadata, and
  tool cache trees from default discovery. `P1`
- [ ] `WS-010` Cache one command-local path table and reuse its capacity across
  watch/repeated-command sessions if measurements justify persistence. `P2`
- [ ] `WS-011` Track file identity, size, timestamp precision, and content hash
  separately so no-op proofs can escalate only when needed. `P1`
- [ ] `WS-012` Add immutable fixtures for ambiguous directories, nested
  projects, symlinks, non-Unicode paths, and case collisions. `P2`

## 3. Solution And Project Graph Inputs

- [ ] `SLN-001` Parse `.sln` project records, GUIDs, nested folders, solution
  configurations, project configurations, and dependency sections. `P2`
- [ ] `SLN-002` Parse `.slnx` XML with equivalent project membership and folder
  semantics. `P2`
- [ ] `SLN-003` Preserve deterministic solution order while using compact
  project indices internally. `P2`
- [ ] `SLN-004` Resolve relative project paths and reject duplicates, missing
  files, unsupported project kinds, and path escapes. `P2`
- [ ] `SLN-005` Map solution configuration/platform pairs to project
  configuration/platform pairs. `P2`
- [ ] `SLN-006` Honor build-disabled projects without removing them from
  reference resolution. `P2`
- [ ] `SLN-007` Detect project-reference and explicit solution-dependency
  cycles before scheduling. `P1`
- [ ] `SLN-008` Diagnose solution/project target-framework incompatibility with
  the exact edge and target pair. `P2`
- [ ] `SLN-009` Implement `dv solution list/add/remove` with loss-minimizing
  edits for `.sln` and structured edits for `.slnx`. `P2`
- [ ] `SLN-010` Support solution-folder placement, root placement, globbed
  adds, duplicate suppression, and `.sln` to `.slnx` migration. `P2`
- [ ] `SLN-011` Compare parsed membership and configuration selection with the
  reference tool on representative solutions. `P2`

## 4. Declarative Project Evaluation

The evaluator is a native MSBuild-compatible evaluator, not a wrapper around
the Microsoft MSBuild engine. It must expand from the initial known-transform
subset toward the properties, items, imports, targets, and tasks exercised by
the compatibility corpus. Any construct that can change an output but is not
yet understood fails explicitly; that failure is an honest intermediate
boundary, not final drop-in parity.

- [~] `EVAL-001` Parse XML securely: BOM/encoding, namespaces, comments, CDATA,
  and entities without DTD or external-entity expansion. `P1`
- [ ] `EVAL-002` Resolve project SDK declarations in `Project@Sdk`, `Sdk`
  elements, version-qualified SDKs, and `global.json` `msbuild-sdks`. `P2`
- [ ] `EVAL-003` Apply implicit `Sdk.props` before and `Sdk.targets` after the
  project body as ordered, typed known inputs. `P1`
- [ ] `EVAL-004` Evaluate properties in document/import order with CLI global
  properties immutable at lower precedence. `P1`
- [ ] `EVAL-005` Supply required reserved/well-known project, directory, SDK,
  OS, architecture, configuration, target, and output properties. `P1`
- [ ] `EVAL-006` Read environment properties once at the boundary and record
  which values influenced the result. `P1`
- [ ] `EVAL-007` Expand `$(Property)` references, escaped characters, empty
  values, and semicolon lists without repeated allocation in item loops. `P1`
- [ ] `EVAL-008` Implement condition grammar for equality, inequality,
  relational and boolean operators, parentheses, `Exists`,
  `HasTrailingSlash`, and version comparisons used by supported inputs. `P1`
- [ ] `EVAL-009` Implement a measured allowlist of property functions required
  by real SDK-style projects; reject other functions with their source span.
  `P2`
- [ ] `EVAL-010` Evaluate `Choose`/`When`/`Otherwise`. `P2`
- [ ] `EVAL-011` Resolve explicit `Import` and `ImportGroup` paths, globs,
  conditions, order, duplicate imports, and cycles. `P2`
- [ ] `EVAL-012` Apply `Directory.Build.props` and
  `Directory.Build.targets`, including opt-out and path-override properties.
  `P2`
- [ ] `EVAL-013` Evaluate top-level item `Include`, `Exclude`, `Remove`, and
  `Update` operations in order. `P1`
- [ ] `EVAL-014` Implement filesystem globs with platform-correct separators,
  case behavior, deterministic order, and default excludes. `P1`
- [ ] `EVAL-015` Expand item metadata, item definitions, semicolon lists, and
  the transforms needed by known SDK inputs. `P2`
- [ ] `EVAL-016` Support conditions on groups, properties, items, metadata, and
  imports with correct evaluation timing. `P2`
- [ ] `EVAL-017` Retain unknown XML as source evidence and classify it as
  irrelevant, supported extension input, or blocking unsupported behavior.
  `P1`
- [ ] `EVAL-018` Parse `Target`, task elements, `UsingTask`, and `Exec` into an
  immutable target/task plan without executing them during evaluation. Execute
  them only in the scheduled build stage. `P2`
- [ ] `EVAL-019` Implement built-in SDK/MSBuild task semantics as typed native
  transforms and required managed/custom tasks through a versioned task-host
  protocol after compatibility fixtures establish their contracts. `P2-P5`
- [ ] `EVAL-020` Report unsupported custom targets/imports with file, line,
  condition, affected output, and supported alternative when known. `P1`
- [ ] `EVAL-021` Evaluate outer and inner builds for `TargetFrameworks`,
  including target-specific conditional properties/items. `P2`
- [x] `EVAL-022` Evaluate `RuntimeIdentifier` and `RuntimeIdentifiers` as
  target expansion dimensions rather than repeated project objects. One
  project owns a contiguous unique RID-span batch, a 32-bit plural boundary,
  and a 32-bit selected index; selected/plural overlap reuses one text span.
  The .NET 10 property oracle and 30-sample process benchmark verify the TFM,
  selected RID, ordered plural RIDs, and materialized dimensions. `dv` measures
  `5.687 ms` median versus `321.215 ms` for MSBuild (`56.5x`). `P2`
- [ ] `EVAL-023` Support `Debug` and `Release` defaults plus arbitrary named
  configurations whose values are fully declarative. `P1`
- [ ] `EVAL-024` Parse and apply `MSBuild.rsp` and `Directory.Build.rsp` with
  reference discovery, precedence, encoding, quoting, and opt-out behavior.
  `P2`
- [ ] `EVAL-025` Emit an evaluated-input manifest suitable for oracle
  comparison without exposing secrets. `P1`

## 5. Core SDK-Style Project Semantics

- [~] `PROJ-001` Support `Microsoft.NET.Sdk` C# projects. `P1`
- [~] `PROJ-002` Support `Exe`, `WinExe`, `Library`, and explicitly diagnose
  unsupported output types. `P1`
- [~] `PROJ-003` Parse one `TargetFramework`, validate its moniker, and derive
  framework/platform identifiers and versions. `P1`
- [ ] `PROJ-004` Expand `TargetFrameworks` to deterministic inner-build
  records. `P2`
- [ ] `PROJ-005` Support .NET, .NET Standard, .NET Framework where the selected
  platform has compatible reference assemblies, and platform-qualified TFMs.
  `P5`
- [~] `PROJ-006` Implement default `Compile`, `EmbeddedResource`, and `None`
  inclusion/exclusion with duplicate-item diagnostics. `P1`
- [ ] `PROJ-007` Honor `EnableDefaultItems`, item-specific default switches,
  `DefaultItemExcludes`, and output/intermediate exclusions. `P2`
- [~] `PROJ-008` Support `ProjectReference`, `PackageReference`, `Reference`,
  `FrameworkReference`, `Analyzer`, `AdditionalFiles`, `EditorConfigFiles`,
  `EmbeddedResource`, `Content`, `None`, and `Using`. `P1/P2`
- [ ] `PROJ-009` Support `InternalsVisibleTo`, `AssemblyMetadata`, and generated
  assembly attributes. `P2`
- [~] `PROJ-010` Resolve `AssemblyName`, `RootNamespace`, `OutputPath`,
  `BaseOutputPath`, `IntermediateOutputPath`, `BaseIntermediateOutputPath`,
  `OutDir`, and artifacts-path layout without output collisions. `P1`
- [ ] `PROJ-011` Implement configuration/platform defaults and
  `AppendTargetFrameworkToOutputPath`/`AppendRuntimeIdentifierToOutputPath`.
  `P2`
- [~] `PROJ-012` Support implicit framework references and targeting-pack
  selection for the TFM. `P1`
- [ ] `PROJ-013` Support implicit usings by SDK, language, TFM, and explicit
  `Using` additions/removals. `P1`
- [~] `PROJ-014` Generate framework and platform preprocessor symbols unless
  disabled. `P1`
- [~] `PROJ-015` Parse `.editorconfig`/`.globalconfig` hierarchy and construct
  ordered analyzer-config inputs. `P1`
- [ ] `PROJ-016` Support `global.json` SDK analysis level where it changes
  diagnostics or supported SDK behavior. `P2`
- [ ] `PROJ-017` Support deterministic build, continuous-integration build,
  path mapping, source link, repository metadata, and embedded sources. `P2`
- [ ] `PROJ-018` Support localized resources, resource naming, and satellite
  assembly generation. `P2`
- [ ] `PROJ-019` Support content copy metadata for build and publish:
  `CopyToOutputDirectory`, `CopyToPublishDirectory`, target path, and
  preserve-newest semantics. `P2`
- [ ] `PROJ-020` Support legacy non-SDK projects under a separately versioned
  MSBuild compatibility contract; reject them explicitly only until that P5
  row is implemented. `P5`

## 6. Framework, Runtime, And Pack Resolution

- [~] `PACKS-001` Inventory installed targeting, runtime, host, apphost,
  analyzer, and workload packs from the selected SDK/root. `P1`
- [~] `PACKS-002` Parse pack manifests and versions rather than hard-code the
  observed SDK layout. `P1`
- [~] `PACKS-003` Select the correct reference pack for a TFM and fail on
  missing or unsupported packs before compiler launch. `P1`
- [x] `PACKS-004` Produce an ordered reference-assembly range from the selected
  pack with stable path indices. `P1`
- [x] `PACKS-005` Load the SDK's portable RID graph as data; never infer RID
  compatibility by splitting the RID string. The selected .NET 10 SDK graph
  compiles to 85 sorted 16-byte nodes, 133 contiguous direct edges, and 494
  precomputed breadth-first compatibility indices. Unknown keys remain opaque
  exact-only RIDs. A 30-sample official `NuGet.Packaging` oracle measures
  `36.217 ms` versus `6.049 ms` for `dv` (`6.0x`). `P2`
- [x] `PACKS-006` Select runtime packs, host packs, native assets, and apphost
  templates for requested RID and architecture. SDK manifest identities,
  latest self-contained pack patches, and supported RID batches feed graph-only
  fallback selection; `RuntimeList.xml` yields 172 managed and 15 native
  `win-x64` assets in one compact span batch, and the installed host pack yields
  the exact apphost template. A 30-sample MSBuild oracle measures `376.764 ms`
  versus `8.030 ms` for `dv` (`46.9x`). `P2`
- [x] `PACKS-007` Resolve framework references and shared-framework versions,
  including runtime roll-forward policy. The selected SDK manifest supplies
  implicit/explicit runtime and targeting-pack identities and versions;
  `Disable`, `LatestPatch`, `Minor`, `Major`, `LatestMinor`, and `LatestMajor`
  select installed shared versions without hard-coded .NET generations. The
  .NET 10 Core + ASP.NET fixture matches MSBuild items and an actual host
  launch. A 30-sample MSBuild oracle measures `352.715 ms` versus `5.585 ms`
  for `dv` (`63.2x`). `P2`
- [x] `PACKS-008` Separate compile, runtime, native, resource, analyzer, and
  build assets in the plan. Nine semantic families occupy consecutive ranges
  in one immutable span batch; per-package ranges index that same allocation.
  The 248-byte plan header is 56 bytes smaller than the prior layout and keeps
  compile, runtime, analyzer, resource, content, three build-import families,
  and native assets independently iterable. The 203-package parity fixture
  matches every portable `project.assets.json` family. A 30-sample locked
  restore oracle measures `702.904 ms` versus `107.385 ms` for `dv` (`6.5x`).
  `P1`
- [x] `PACKS-009` Diagnose unavailable TFM/RID/platform combinations with the
  required pack identity and acquisition action. Runtime, host, targeting, and
  shared-framework failures retain one 56-byte typed requirement with one text
  allocation, exact dimensions, and a stable action; successful planning does
  not construct it. The offline `linux-arm`
  fixture requires both tools to name
  `Microsoft.NETCore.App.Runtime.linux-arm`; `dv` also reports version, TFM,
  RID, kind, acquisition, and guidance. A 30-sample restore oracle measures
  `532.652 ms` versus `6.378 ms` for `dv` (`83.5x`). `P1`
- [x] `PACKS-010` Cache immutable SDK pack inventories by selected SDK
  fingerprint and invalidate on installation changes. The schema-2 cache owns
  one text allocation and a contiguous batch of 187 twelve-byte asset records;
  relative paths keep the observed file to 12,405 bytes. SDK/TFM/RID/pack
  selection plus manifest, graph, host-generation, and package-completion
  metadata select immutable entries; SHA-512-invalid, corrupt, or stale
  entries rebuild through atomic publication. Decoded paths must remain inside
  the selected packs. A 30-sample cold-inventory oracle measures `368.322 ms`
  versus `11.118 ms` (`33.1x`), while warm reuse measures `360.550 ms` versus
  `6.403 ms` (`56.3x`). `P2`

## 7. NuGet Configuration, Sources, And Authentication

- [x] `NUGET-001` Discover machine, user, drive, repository, and explicit
  `NuGet.Config` files with platform-correct precedence. Machine and
  additional-user fragments, the main .NET CLI user file, and one
  casing-correct file per ancestor form a deterministic low-to-high batch;
  `--configfile` validates and isolates one file. The six-file locked oracle
  measures `532.948 ms` for Microsoft versus `5.651 ms` for `dv` (`94.3x`)
  across 30 retained samples. `P1`
- [x] `NUGET-002` Merge keyed sections with case-insensitive replacement,
  `<clear>`, add, remove, disabled-source membership, and single-pass `%NAME%`
  environment expansion. Unknown variables remain literal and expansion adds
  no allocation when no marker resolves. The four-level locked oracle
  measures `558.126 ms` for Microsoft versus `9.422 ms` for `dv` (`59.2x`)
  across 30 retained samples. `P1`
- [x] `NUGET-003` Support typed `packageSources`,
  `disabledPackageSources`, `packageSourceMapping`, `auditSources`, and source
  protocol versions. Mapping groups retain contiguous pattern ranges and use
  NuGet's allocation-free longest-pattern selection during package/version
  requests. An official `NuGet.Configuration` oracle validates all effective
  sections; the locked process benchmark measures `527.659 ms` for Microsoft
  versus `5.850 ms` for `dv` (`90.2x`) across 30 retained samples. `P2`
- [x] `NUGET-004` Resolve global-packages, HTTP-cache, scratch, and ordered
  fallback roots with Microsoft precedence; retain typed signature and audit
  policy; and construct a proxy client without retaining or reporting its
  address or credentials. Fallback packages participate in version and
  locked-asset lookup, while downloads stage through scratch and publish
  atomically on the global-cache volume. An official NuGet adapter plus an
  MSBuild property query validates the effective policy. The locked fallback
  oracle measures `523.051 ms` for Microsoft versus `5.370 ms` for `dv`
  (`97.4x`) across 30 retained samples. Conditional HTTP caching and
  vulnerability execution remain in `RES-017/018` and `RES-024`; signature
  verification is implemented by `RES-015`. Encrypted proxy credentials and
  the remaining HTTP transport policy are implemented by `NUGET-011`. Enabled
  auditing fails explicitly until its consumer lands. `P2`
- [x] `NUGET-005` Accept CLI source/config/packages-folder overrides with
  documented precedence. Repeatable source URIs replace configured sources;
  the singleton config and packages paths resolve from the working directory,
  reject duplicates, and beat config/environment values. The locked parity
  oracle measures `524.597 ms` for Microsoft versus `5.103 ms` for `dv`
  (`102.8x`) across 30 retained samples with one resolved package and zero
  timed HTTP requests or downloads. `P1`
- [x] `NUGET-006` Support local folder sources and NuGet v2/v3 HTTP service
  contracts. Configuration-relative, CLI-relative, absolute, and `file://`
  local sources support flat and hierarchical layouts, offline range
  enumeration, nuspec identity checks, hierarchical SHA-512 verification, and
  atomic cache publication. HTTPS v2 range resolution follows bounded
  `FindPackagesById` continuation pages while v3 retains service-index and
  flat-container discovery. The cold two-package local-feed oracle measures
  `670.534 ms` for Microsoft versus `64.522 ms` for `dv` (`10.4x`) across 30
  retained samples with 2,980,145 source bytes and zero HTTP requests. `P1`
- [x] `NUGET-007` Resolve registration, flat-container, search, vulnerability,
  and package-publish endpoints from service-index resources. Selection uses
  NuGet.Client's ordered resource types, string-or-array types, compatible
  `clientVersion` precedence, stable equivalent-endpoint order, URI filtering,
  and HTTPS policy. Independent v3 indexes are fetched through a bounded Tokio
  batch, then compacted into one text allocation, one span batch, and five
  fixed capability ranges. `project package-sources` exposes the effective
  inventory through human output and event schema 8. The live one-request
  oracle measures `344.113 ms` for Microsoft versus `277.336 ms` for `dv`
  (`1.24x`) across 30 retained samples; `dv` also evaluates the project and
  configuration hierarchy inside its timed command. `P2`
- [x] `NUGET-008` Support Basic/PAT credentials from config and environment
  without persisting or reporting plaintext. Exact
  `NuGetPackageSourceCredentials_{name}` values override decoded
  `packageSourceCredentials` groups; malformed environment values fall back
  to configuration like NuGet.Client. Cleartext buffers are zeroed, Windows
  `Password` values use NuGet-compatible user DPAPI entropy, and one sensitive
  Basic header is materialized per effective HTTPS source. Authentication is
  limited to the configured source origin; events and human output expose only
  `none` or `basic` through schema 9. The offline NuGet.Configuration oracle
  measures `73.624 ms` for Microsoft versus `4.615 ms` for `dv` (`16.0x`)
  across 30 retained samples with two sources and zero network requests. `P2`
- [x] `NUGET-009` Implement the NuGet cross-platform V2 credential-provider
  protocol for private feeds. Self-contained providers use symmetric bounded
  JSON-lines messaging, process monitoring, initialization, authentication
  claims, noninteractive-by-default requests, opt-in interactive login output,
  cancellation, and NuGet-compatible handshake/request timeouts. Provider
  response storage is zeroed, the acquired Basic header is cached only for the
  challenged HTTPS origin, rejected credentials receive one bounded
  `IsRetry=true` refresh, concurrent challenges coalesce per source, and
  DLL-only plugins fail rather than invoking `dotnet`. The official
  `NuGet.Protocol` oracle measures `115.621 ms` for
  Microsoft versus `22.519 ms` for `dv` (`5.1x`) across 30 retained samples
  while both launch the same fixture provider and perform zero network work.
  `P2`
- [x] `NUGET-010` Support NuGet client certificates from relative/absolute PFX
  files and Windows platform certificate stores. File secrets are zeroed,
  certificate files are bounded at 8 MiB, encrypted passwords use NuGet DPAPI,
  and source-specific native TLS clients cannot redirect an identity across an
  origin boundary. Windows supports `CurrentUser`/`LocalMachine`, NuGet's store
  names, and exact thumbprint lookup with an accessible private key; other
  selectors and non-Windows stores fail explicitly. The official
  `NuGet.Configuration` oracle measures `89.254 ms` for Microsoft versus
  `30.003 ms` for `dv` (`3.0x`) across 30 retained offline samples while both
  load one PFX and one `CurrentUser\\My` certificate. Redacted certificate
  authentication is published through event schema 10. `P5`
- [x] `NUGET-011` Honor proxy, `NO_PROXY`, TLS validation, redirect, retry,
  timeout, rate-limit, and offline behavior. Proxy URL credentials are stripped
  into zeroized Basic fields; Windows config credentials use NuGet DPAPI.
  Lower/uppercase proxy environment aliases, bypass lists, bounded per-source
  semaphores, HTTPS-only ten-hop redirects, six-attempt enhanced retry controls,
  `Retry-After`, 100-second requests, 60-second body stalls, and offline
  zero-network behavior are enforced by one 16-byte policy record. Local-server
  tests cover retry, redirect rejection, rate limiting, and stalled bodies. An
  SDK-shipped NuGet.Configuration/Protocol oracle verifies the redacted policy;
  the offline process benchmark measures `78.286 ms` for Microsoft versus
  `6.934 ms` for `dv` (`11.3x`) across 30 retained samples. `P2`
- [x] `NUGET-012` Require explicit per-source opt-in for insecure HTTP or
  disabled TLS validation and surface the security consequence. Missing,
  invalid, and false flags preserve HTTPS validation; exact CLI source matches
  inherit configured policy while arbitrary HTTP remains rejected. Dedicated
  clients contain unsafe policy to one source across redirects, v2 derived
  URLs, and v3 resources without forwarding credentials across origins. Event
  schema 12 reports redacted per-source and aggregate consequences. The
  SDK-shipped NuGet.Configuration oracle measures `71.416 ms` for Microsoft
  versus `5.742 ms` for `dv` (`12.4x`) across 30 retained offline samples. `P2`
- [x] `NUGET-013` Package-source mapping computes the longest effective
  pattern before touching a source. Source-indexed lazy endpoint state activates
  only winning local/v2/v3 sources as new graph identities arrive; tied v3
  indexes are fetched concurrently through the bounded Tokio set and merged in
  deterministic source order. Global/fallback cache hits remain
  source-independent. An uncached identity with no enabled winning source fails
  as typed `DV0412` before URL, credential, DNS, TLS, or HTTP work. A fresh
  expected-failure fixture validates Microsoft's `NU1100` against `DV0412` and
  zero requests; 30 Windows samples measure `531.249 ms` for Microsoft versus
  `9.566 ms` for `dv` (`55.5x`). `P2`
- [x] `NUGET-014` Bound concurrent requests per source and globally. A compact
  command budget honors positive `NUGET_CONCURRENCY_LIMIT` values up to the
  measured 24-task ceiling across service discovery, metadata expansion, and
  acquisition. A smaller selection creates one shared global semaphore;
  `maxHttpRequestsPerSource` creates one independent semaphore per remote source
  only when it is tighter than the global budget. Permits cover response-body
  consumption. The common default allocates no semaphore, queues stay bounded,
  and identity-ordered merges remain deterministic. A
  delayed two-source fixture enforces global `4` and per-source `2` limits for
  both tools. Thirty Windows samples measure `3109.409 ms` for Microsoft versus
  `247.157 ms` for `dv` (`12.6x`). `P2`
- [x] `NUGET-015` Record request count, bytes, cache outcome, and source timing
  without recording credentials. Actual attempts, including successful retry
  and authentication retry paths, source bytes, and cumulative source-work
  microseconds are accumulated in 24-byte task-local records and merged once
  by configured source index. Package rows carry `hit` or `miss`; warm locked
  restores publish zero source work. The source rows introduced in event schema
  13 and retained by schema 14 identify entries only by
  redacted source keys and protocol generations. Source URLs are stripped of
  userinfo, query, and fragment data before inventory, lock, or package-metadata
  publication. No
  atomics, locks, or per-request events were added to the hot path. The cold
  two-source benchmark checks telemetry sums against the loopback servers and
  all six package outcomes on every sample. Thirty Windows samples measure
  `3067.502 ms` for Microsoft versus `232.130 ms` for `dv` (`13.2x`). `P2`

## 8. Package Resolution, Assets, Cache, And Locking

- [x] `RES-001` Parse NuGet package identities case-insensitively while
  preserving display casing. `P1`
- [x] `RES-002` NuGet SemVer 2 precedence, normalized numeric versions,
  prerelease identifiers, ignored build metadata, and inclusive/exclusive
  interval ranges are typed and tested. Numeric, prerelease, and interval
  floating forms in project or package constraints retain NuGet's
  matching-first/highest-float/nearest-fallback ordering; a Microsoft-oracled
  cold benchmark covers exact identity, version, hash, and assets. `P1`
- [x] `RES-003` Parse `PackageReference` version and metadata from attributes
  or child elements. `P1`
- [x] `RES-004` `PackageReference` asset lists are normalized once into an
  eight-bit family mask; effective includes propagate through the dependency
  graph while private assets remain scoped to parent flow. Package-scoped
  `NoWarn`, direct compile `Aliases`, and Microsoft-compatible generated
  `Pkg*` package-root properties occupy a separate identity-ordered 32-byte
  cold policy batch. Compiler aliases are sparse 12-byte reference-index rows,
  so the common 168-reference framework batch is not widened. Lock schema 4
  invalidates selected assets when the effective mask changes, and event schema
  14 exposes the same structured policy to human/JSON consumers. Parser tests
  cover attribute and child-element forms; the checked Microsoft oracle covers
  compile-only selection, private-all flow, two warning codes, the direct
  compiler alias, and the exact `PkgNewtonsoft_Json` root. Thirty warm Windows
  samples measure `456.722 ms` for
  Microsoft versus `6.611 ms` for `dv` (`69.1x`) with one cache hit and zero
  timed network work. `P2`
- [x] `RES-005` Item-group and item conditions filter project, package, and
  framework reference batches by `TargetFramework`, `RuntimeIdentifier`, and
  `Configuration` before path or metadata validation. The bounded linear
  evaluator supports case-insensitive equality/inequality, `And`/`Or`
  precedence, parentheses, boolean negation, empty RID values, and compound
  property interpolation without building an expression tree. Exact-property
  comparisons allocate no evaluation storage; variable-sized compound
  expansion is isolated to the uncommon path. Conditions are limited to 1,024
  bytes, 32 comparisons, and eight nested expressions. Each parsed reference
  carries two sentinel-backed `u32` indices in an asserted eight-byte row
  rather than two 16-byte pointer-width options; selected conditions and raw
  strings die with project materialization. Unsupported properties and
  malformed or over-limit expressions fail explicitly, while conditions on
  property groups remain outside the compatibility contract. Restore, sync,
  package-source inspection, and project inspection accept the selected
  Debug/Release configuration. Microsoft-oracled preflight compares the same
  TFM, RID, configuration, three-package batch, project path, and explicit
  framework row. Thirty warm Windows samples measure `288.983 ms` for
  Microsoft versus `4.765 ms` for `dv` (`60.6x`). `P2`
- [x] `RES-006` The nearest bounded `Directory.Packages.props` is parsed into a
  case-insensitive identity-ordered 16-byte central-version row batch.
  Conditional `PackageVersion`, `VersionOverride`, fixed-policy
  `GlobalPackageReference`, and exact transitive pin promotion are integrated
  with project evaluation, graph convergence, lock fingerprinting, and schema
  15 structured roles. Malformed, duplicate, missing, dynamic, unsupported,
  and downgrade inputs fail explicitly before unrelated package work. A
  Microsoft-oracled 54-package warm lock verifies every identity, version,
  SHA-512, asset family, and `CentralTransitive` role. Thirty Windows samples
  measure `461.826 ms` for Microsoft versus `29.864 ms` for `dv` (`15.5x`).
  `P2`
- [x] `RES-007` Lowest-applicable-version, direct-dependency-wins, and cousin
  convergence use an identity-ordered constraint table with stale-edge
  retraction and bounded non-convergence failure. Direct wins is applied at
  every package subgraph by suppressing only constraints whose parent is
  dominated by a nearer constraining parent; alternate project-root paths keep
  shared diamond nodes as cousins. Unrelated cousins still combine across
  different absolute depths. Edge changes invalidate affected descendants
  deterministically. Microsoft-oracled local graphs verify the nested downgrade
  and cousin selections, while the eShop-derived acceptance graph retains exact
  identity/version parity across 203 packages. Thirty warm-cache Windows
  samples measure `604.023 ms` for Microsoft versus `16.971 ms` for `dv`
  (`35.6x`). `P1`
- [x] `RES-008` Package failures retain typed, ordered diagnostic fields behind
  one optional cold-path allocation. Successful direct-wins downgrades compact
  into 32-byte rows and persist in lock schema 7, so cold and warm restores emit
  the same `DV0413` warning without reopening manifests. Constraint conflicts,
  cycles, missing identities, missing versions, and incompatible frameworks emit
  `DV0414`, `DV0415`, `DV0416`, `DV0417`, and `DV0402` respectively. Microsoft
  preflight proves the matching `NU1605`, `NU1107`, `NU1108`, `NU1101`,
  `NU1102`, and `NU1202` categories. Thirty cold local-source samples measure
  `569.423 ms` for Microsoft versus `13.797 ms` for `dv` (`41.3x`). `P1`
- [x] `RES-009` Restore evaluates literal project-reference closures once in
  deterministic root-first order and resolves the resulting `ProjectSpec`
  batch through one command-scoped Tokio runtime. Exact package dependency
  metadata is retained in a sorted 40-byte-row cache keyed by storage scope,
  target, identity, and version, with identity/version text stored as compact
  spans into one scope buffer. Each project keeps an independently mutable
  graph while immutable archives publish once into the shared cache. A
  Microsoft-oracled two-project cold local graph verifies both eight-package
  selections, three ordered `dv` project events, 16 resolved rows, eight total
  publications, and zero HTTP work. Thirty Windows samples measure
  `700.911 ms` for Microsoft versus `51.502 ms` for `dv` (`13.6x`). `P2`
- [x] `RES-010` One bounded XML pass tracks `dependencies`,
  `frameworkReferences`, and `frameworkAssemblies` as distinct containers, so
  later framework groups cannot become dependency edges. Modern shared
  frameworks and legacy .NET Framework assemblies use independent nearest-TFM
  selection; unscoped assemblies are fallback-only, modern targets ignore
  legacy assemblies, and runtime asset exclusion removes only legacy
  assemblies. Selected names occupy sparse 20-byte package rows plus one
  contiguous text-span batch, persist in lock schema 8, and publish through
  event schema 17 and human output. Microsoft-oracled `net10.0` and `net48`
  preflight verifies dependency, shared-framework, legacy-assembly, cold, and
  warm-lock parity. Thirty cold local-source Windows samples measure
  `558.832 ms` for Microsoft versus `15.989 ms` for `dv` (`35.0x`). `P1`
- [x] `RES-011` Select `ref`, `lib`, `runtimes`, native, resource,
  `contentFiles`, analyzer, `build`, `buildMultiTargeting`, and
  `buildTransitive` assets by compatible TFM and NuGet include/exclude
  propagation. Concrete RID restores traverse the selected SDK's compatibility
  graph and choose the nearest runtime/resource and native groups independently;
  portable restores retain typed runtime targets. One bounded nuspec pass
  records ordered `contentFiles` rules, with later attributes overriding earlier
  matches and unmatched files retaining NuGet defaults. Content metadata uses a
  parallel 12-byte row batch. Lock schema 8 fingerprints the semantic RID chain,
  and event schema 17 preserves runtime identity and content metadata across
  cold and warm restores. Microsoft-oracled portable, exact Windows/Linux, and
  Windows-fallback projects verify all asset families. Thirty Windows samples
  measure cold local restore at `600.782 ms` versus `23.186 ms` (`25.9x`) and
  warm locked restore at `456.098 ms` versus `7.589 ms` (`60.1x`). `P2`
- [x] `RES-012` Package pruning evaluates the SDK properties and direct
  framework profiles, reads .NET 10-and-later data from the selected SDK or
  highest matching reference pack, and uses generated authoritative effective
  tables for .NET Standard and .NET 9-and-earlier targets. Core, ASP.NET, and
  WindowsDesktop batches merge by greatest upper version; WindowsDesktop uses
  the SDK's nearest compatible generated fallback. Stable package versions get
  the SDK's patch ceiling, pruned graph edges retract, and the compact semantic
  table fingerprints warm locks. The generated .NET 8/9 Core and ASP.NET
  counts match MSBuild at 418/420. Thirty warm samples measure `492.588 ms`
  for Microsoft restore versus `7.339 ms` for `dv` (`67.1x`). `P2`
- [x] `RES-013` Stream exact `.nupkg` content through SHA-512 into bounded
  temporary storage. `P1`
- [x] `RES-014` Verify package identity, version, v2 source hash/size, ZIP
  structure, duplicate paths, traversal paths, entry sizes, and total
  expansion limits before cache commit. `P1`
- [x] `RES-015` Verify author and repository CMS signatures, repository
  countersignatures, signing-certificate attributes, RFC 3161 timestamps,
  archive content hashes, certificate chains, and hierarchical NuGet
  `trustedSigners` policy. SHA-256/384/512 fingerprints, case-sensitive owners,
  conflicting `allowUntrustedRoot` records, unsigned packages, tampering,
  cache hits, and warm locks follow NuGet's security boundary. Windows uses
  native roots; Linux and macOS use platform-correct system or selected-SDK
  certificate bundles. A one-package local-feed oracle holds network work at
  zero. Thirty cold samples measure `664.903 ms` for Microsoft versus
  `30.841 ms` for `dv` (`21.6x`); warm locked validation measures `467.897 ms`
  versus `11.463 ms` (`40.8x`). The
  [transform contract](package-signature-contract.md) records layout and cost;
  online revocation remains a focused
  [compatibility follow-up](../issues/signature-revocation.md). `P2`
- [~] `RES-016` Extract atomically into a NuGet-compatible global-packages
  layout with per-package concurrency coordination. `P1`
- [~] `RES-017` Reuse the existing global package and HTTP caches when valid,
  including conditionally revalidated service-index entries keyed by source;
  never rewrite a valid immutable entry. `P1`
- [ ] `RES-018` Implement conditional HTTP caching, negative-result policy, and
  corruption quarantine. `P2`
- [x] `RES-019` Generate a versioned deterministic `dv` lockfile containing
  source decision, full dependency closure, content hash, selected assets, and
  relevant compatibility inputs. `P1`
- [ ] `RES-020` Read/write NuGet `packages.lock.json`, including locked mode,
  force evaluation, custom path, pruning, and multi-project rules. `P2`
- [ ] `RES-021` Decide and document whether `project.assets.json` and generated
  NuGet props/targets are compatibility outputs; produce them if IDE or package
  target compatibility requires them. `P2`
- [x] `RES-022` Make warm locked resolution a linear validation over the
  smallest stable fingerprints with zero network requests. `P1`
- [~] `RES-023` Support force, no-cache, no-HTTP-cache, ignore-failed-sources,
  disable-parallel, interactive, and offline restore modes. `P2`
- [ ] `RES-024` Audit direct and transitive dependencies for known
  vulnerabilities with configurable severity and source policy. `P2`
- [ ] `RES-025` Produce deterministic dependency trees for direct, transitive,
  outdated, deprecated, and vulnerable package listing. `P2`
- [ ] `RES-026` Add cache list/path/clear/prune operations with safe ownership
  checks and concurrent-reader behavior. `P2`
- [ ] `RES-027` Support `packages.config` restore/install/update semantics,
  repository layout, project mutations, and NuGet version-specific resolution
  under the direct NuGet compatibility contract. `P5`

## 9. Package And Reference Editing

- [ ] `EDIT-001` `dv add <package>` selects one project or requires an explicit
  path when ambiguous. `P2`
- [ ] `EDIT-002` Resolve an omitted version according to explicit stable or
  prerelease policy and show the selected source/version before commit. `P2`
- [ ] `EDIT-003` Add or update `PackageReference` while preserving unrelated
  XML, encoding, comments, indentation, newline style, and element order. `P2`
- [ ] `EDIT-004` Write a central `PackageVersion` instead of an inline version
  when central package management controls the project. `P2`
- [ ] `EDIT-005` Support version, framework, source, prerelease, no-restore,
  interactive, package directory, and asset metadata options. `P2`
- [ ] `EDIT-006` Restore/resolve before committing when needed, and make the
  project plus lockfile update transactional. `P2`
- [ ] `EDIT-007` `dv remove <package>` removes the correct conditional or
  central reference and updates lock state. `P2`
- [ ] `EDIT-008` `dv list` reports direct/transitive packages in human and JSON
  forms with stable ordering. `P2`
- [ ] `EDIT-009` Add package search with source, prerelease, skip, take, exact,
  and machine-readable result support. `P2`
- [ ] `EDIT-010` Add/list/remove project references with relative path,
  framework condition, duplicate, cycle, and compatibility checks. `P2`
- [ ] `EDIT-011` Use temp-file plus atomic replacement and preserve the
  original on parse, resolution, or write failure. `P2`

## 10. Build Graph And Scheduling

- [ ] `GRAPH-001` Represent each build unit as project x TFM x RID x
  configuration using compact indices into shared tables. `P1`
- [ ] `GRAPH-002` Convert project and package edges into contiguous adjacency
  ranges and record hot-record size/alignment/working-set bytes. `P1`
- [ ] `GRAPH-003` Validate all indices, missing nodes, duplicate outputs, and
  cycles before starting work. `P1`
- [ ] `GRAPH-004` Topologically order deterministically independent of
  filesystem enumeration and worker completion. `P1`
- [ ] `GRAPH-005` Distinguish build-order-only, compile-reference, runtime,
  analyzer, generator, and content edges. `P1`
- [ ] `GRAPH-006` Support whole workspace, one project plus dependencies, and
  no-dependencies selection. `P2`
- [ ] `GRAPH-007` Batch independent project nodes into coarse contiguous worker
  ranges with a measured sequential/parallel crossover. `P2`
- [ ] `GRAPH-008` Keep worker queues and result buffers bounded. `P2`
- [ ] `GRAPH-009` Partition mutable state by worker and merge diagnostics,
  events, and outputs deterministically without hot-path locks. `P2`
- [ ] `GRAPH-010` Stop launching dependent work after failure while allowing
  already-running independent work to reach a defined cancellation boundary.
  `P2`
- [ ] `GRAPH-011` Support clean, rebuild, no-incremental, and disable-build-
  servers semantics without delegating. `P2`
- [ ] `GRAPH-012` Record per-stage input/output byte counts, process count,
  allocations, CPU work, and elapsed time at batch granularity. `P1`

## 10A. MSBuild Target And Task Compatibility

- [ ] `MSTASK-001` Parse project `InitialTargets`, `DefaultTargets`, and named
  command-line targets into the native target graph. `P2`
- [ ] `MSTASK-002` Implement `DependsOnTargets`, `BeforeTargets`,
  `AfterTargets`, declaration-order tie breaking, duplicate suppression, and
  the rule that a target executes at most once per build context. `P2`
- [ ] `MSTASK-003` Implement target `Condition`, `Inputs`, `Outputs`,
  `Returns`, `KeepDuplicateOutputs`, and partial/full incremental skipping.
  `P2`
- [ ] `MSTASK-004` Implement property groups, item groups, item definitions,
  metadata, include/remove/update operations, and output capture inside
  targets at execution time. `P2`
- [ ] `MSTASK-005` Implement MSBuild batching by item lists and metadata,
  including bucket construction, scoped property/item views, and deterministic
  merge. `P5`
- [ ] `MSTASK-006` Implement task parameter conversion, required/output
  parameters, scalar/item arrays, escaped values, conditions, and output
  property/item bindings. `P2`
- [ ] `MSTASK-007` Implement `ContinueOnError`, `OnError`, warning/error
  conversion, cancellation, yielded work, and target result propagation. `P2`
- [ ] `MSTASK-008` Implement native equivalents for common data/filesystem
  tasks such as `Message`, `Warning`, `Error`, `Copy`, `Delete`, `MakeDir`,
  `RemoveDir`, `Touch`, `ReadLinesFromFile`, and `WriteLinesToFile`. `P2`
- [ ] `MSTASK-009` Implement native graph/control tasks such as `CallTarget`
  and `MSBuild` without recursive Microsoft MSBuild process launches. `P2`
- [ ] `MSTASK-010` Implement compile/resource/reference/apphost/package tasks
  as the typed transforms defined elsewhere in this map rather than generic
  reflection calls. `P1-P5`
- [ ] `MSTASK-011` Implement `Exec` with reference-compatible command,
  working-directory, environment, encoding, timeout, exit-code, console-line,
  logging, and cancellation behavior. `P2`
- [ ] `MSTASK-012` Parse `UsingTask` assembly/task-factory declarations,
  conditions, runtime/architecture requirements, parameter groups, and inline
  task bodies. `P5`
- [ ] `MSTASK-013` Define a length-bounded task-host protocol that loads
  compatible managed task assemblies under the selected Microsoft runtime and
  returns typed outputs/events without console scraping. `P5`
- [ ] `MSTASK-014` Provide compatible task build-engine services for logging,
  project builds, task objects, cancellation, yield/reacquire, and node
  affinity without exposing `dv` hot-state pointers. `P5`
- [ ] `MSTASK-015` Isolate task assembly load contexts, crashes, hangs, static
  state, architecture, and runtime conflicts; recycle hosts at deterministic
  boundaries. `P5`
- [ ] `MSTASK-016` Require explicit trust policy for repository/package custom
  tasks and make every external process, filesystem write, and network request
  observable in compatibility evidence. `P5`
- [ ] `MSTASK-017` Preserve target/task events and results sufficiently for
  loggers, binary logs, `-getTargetResult`, IDE/design-time consumers, and
  failure diagnostics. `P2-P5`
- [ ] `MSTASK-018` Build a compatibility corpus of Microsoft.Common, .NET SDK,
  NuGet, Web/Razor, desktop, test, and representative custom targets/tasks,
  recording every unsupported task by frequency and affected workflow. `P2-P5`

## 11. Generated Inputs And Roslyn Compilation

- [ ] `COMP-001` Generate target-framework and platform assembly attributes.
  `P1`
- [ ] `COMP-002` Generate project assembly info from version, company,
  product, title, description, copyright, neutral language, and repository
  properties with per-attribute opt-outs. `P1`
- [ ] `COMP-003` Generate SDK-specific implicit global usings. `P1`
- [ ] `COMP-004` Generate ordered analyzer config and editor-config inputs.
  `P1`
- [ ] `COMP-005` Generate source-link and embedded-source inputs when enabled.
  `P2`
- [ ] `COMP-006` Transform `.resx` and other supported resources into compiler
  inputs with deterministic logical names. `P2`
- [~] `COMP-007` Build one immutable compiler batch with ordered ranges for
  sources, generated sources, references, analyzers, generators, resources,
  additional files, and configs. `P1`
- [~] `COMP-008` Map C# language options: language version, nullable,
  constants, unsafe, overflow checking, features, and checked context. `P1`
- [~] `COMP-009` Map output/codegen options: target kind, platform target,
  optimization, debug type, PDB path, deterministic, reference assembly,
  main type, subsystem, high entropy VA, and preferred base address as
  supported. `P1/P2`
- [~] `COMP-010` Map warning/diagnostic options: warning level, no-warn,
  warnings-as-errors, warnings-not-as-errors, rulesets, analyzer severity, and
  error log. `P1`
- [ ] `COMP-011` Map signing options: key file/container, public signing, delay
  signing, and checksum algorithm. `P2`
- [ ] `COMP-012` Map advanced inputs: modules, Win32 icon/manifest/resource,
  application configuration, link resources, embedded files, and reference
  aliases. `P2`
- [x] `COMP-013` Select the Roslyn compiler and built-in analyzers/generators
  from the selected SDK, never from ambient PATH. `P1`
- [ ] `COMP-014` Load `hostfxr` through its native hosting interface and launch
  a build-pinned managed compiler host without `dotnet exec`. `P1`
- [ ] `COMP-015` Define a versioned length-bounded binary compiler-host
  protocol over typed batches and typed diagnostic/result batches. `P1`
- [~] `COMP-016` Reject missing inputs, bad indices, unsupported compiler
  properties, and output collisions before process/host invocation. `P1`
- [ ] `COMP-017` Preserve Roslyn diagnostic ID, severity, warning level, file
  span, message, arguments where exposed, and help link as structured data.
  `P1`
- [ ] `COMP-018` Support analyzers and source generators, generated-tree
  identity, additional files, analyzer configs, and generator diagnostics.
  `P2`
- [ ] `COMP-019` Make analyzer/generator failure or nondeterminism visible in
  cache evidence. `P2`
- [ ] `COMP-020` Implement cancellation and crash isolation for the compiler
  host. `P1`
- [ ] `COMP-021` Start with an isolated compiler host; add safe persistent reuse
  only if cold/warm measurements prove the added protocol/state worthwhile.
  `P1/P2`
- [ ] `COMP-022` Compare normalized compiler batches and observable artifacts
  with captured Microsoft-oracle invocations. `P1`

## 12. Post-Compile Build Outputs

- [ ] `OUT-001` Produce managed assembly, portable/embedded/Windows PDB as
  configured, and reference assembly when required. `P1`
- [ ] `OUT-002` Copy project and package runtime assemblies using conflict and
  copy-local rules. `P1`
- [ ] `OUT-003` Generate `.deps.json` with compile/runtime/resource/native
  libraries, hashes, target, and runtime fallback data. `P1`
- [ ] `OUT-004` Generate `.runtimeconfig.json` and merge
  `runtimeconfig.template.json` plus runtime configuration properties. `P1`
- [ ] `OUT-005` Generate or patch the correct platform apphost and honor
  `UseAppHost`. `P1`
- [ ] `OUT-006` Copy native assets, satellite resources, documentation, symbols,
  content, and configuration files according to metadata. `P2`
- [ ] `OUT-007` Support runtime config dev file and preserve-compilation-context
  compatibility where requested. `P2`
- [ ] `OUT-008` Write outputs through temporary files and atomically expose a
  complete successful target. `P1`
- [ ] `OUT-009` Remove or quarantine partial outputs after compiler, copy, or
  cancellation failure. `P1`
- [ ] `OUT-010` Compare assembly metadata, manifests, apphost behavior, and
  program output with the reference workflow rather than requiring byte-for-
  byte binaries where nondeterministic fields are understood. `P1`

## 13. Incremental State And Build Cache

- [ ] `INCR-001` Define a versioned build-state protocol keyed by selected SDK,
  evaluator version, target identity, normalized properties, and input
  fingerprints. `P1`
- [ ] `INCR-002` Separate evaluation, resolution, generation, compilation, and
  output-materialization fingerprints so invalidation is stage-local. `P1`
- [ ] `INCR-003` Prove a no-op from metadata first, content hashes second, and
  full recomputation only when earlier evidence is insufficient. `P1`
- [ ] `INCR-004` Detect timestamp rollback, coarse timestamp filesystems,
  replaced files with equal size/time, and clock changes. `P2`
- [ ] `INCR-005` Invalidate exactly affected downstream targets after source,
  property, import, reference, package, analyzer, generator, SDK, or tool
  change. `P1/P2`
- [ ] `INCR-006` Preserve deterministic state independent of worker completion
  order. `P1`
- [ ] `INCR-007` Commit state only after all declared outputs are verified.
  `P1`
- [ ] `INCR-008` Coordinate concurrent `dv` commands with bounded waits,
  ownership records, stale-owner recovery, and no corrupt partial state. `P2`
- [ ] `INCR-009` Reuse compilation or artifact outputs across workspaces only
  after path mapping and content-addressed correctness are proven. `P5`
- [ ] `INCR-010` Expose a structured explanation for each hit, miss, and
  invalidation without storing one reporter event per hot item. `P1`
- [ ] `INCR-011` Benchmark cold, warm, one-source incremental, one-property
  incremental, package-change, generator-change, and no-op cases. `P1`

## 14. Application Run

- [ ] `RUN-001` Select one runnable target and diagnose zero/multiple runnable
  projects or target frameworks. `P1`
- [ ] `RUN-002` Perform implicit sync/build unless `--no-restore`/`--no-build`
  proves prerequisites exist. `P1`
- [ ] `RUN-003` Launch framework-dependent apps through native hostfxr/runtime
  hosting with the generated deps/runtime config. `P1`
- [ ] `RUN-004` Launch self-contained/native executable outputs directly. `P2`
- [ ] `RUN-005` Resolve runtime framework version and roll-forward policy. `P1`
- [ ] `RUN-006` Apply project run arguments, CLI arguments, working directory,
  environment, and executable path with documented precedence. `P1`
- [ ] `RUN-007` Parse `launchSettings.json`, select profiles, expand URLs and
  environment, and support `--no-launch-profile`. `P2`
- [ ] `RUN-008` Inherit or capture stdin/stdout/stderr according to TTY and JSON
  mode without treating application output as orchestration data. `P1`
- [ ] `RUN-009` Forward termination, handle Ctrl+C, wait for child cleanup, and
  return the defined child exit status. `P1`
- [ ] `RUN-010` Support framework, runtime, architecture, OS, configuration,
  project, property, verbosity, and interactive selectors. `P2`
- [ ] `RUN-011` Add file-based C# app run/build/publish only after the project
  vertical slice is complete. `P5`
- [ ] `RUN-012` Add `dv watch` with debounced filesystem batches, rebuild
  invalidation, restart, and hot-reload protocol. `P5`

## 15. Test Discovery And Execution

- [ ] `TEST-001` Expand real fixtures for xUnit, NUnit, MSTest, MTP, VSTest
  adapter, multi-target, data-driven, failing, skipped, and high-output tests.
  `P3`
- [ ] `TEST-002` Identify test projects and runner/adapters from evaluated
  project/package data, not filename heuristics alone. `P3`
- [ ] `TEST-003` Read `global.json` test-runner selection and validate that all
  selected projects use a compatible runner. `P3`
- [ ] `TEST-004` Build test targets with the same sync/build graph and no-build/
  no-restore contracts. `P3`
- [ ] `TEST-005` Define a versioned adapter-host protocol for discovery requests,
  test cases, execution requests, results, output, attachments, and
  cancellation. `P3`
- [ ] `TEST-006` Preserve stable test identity: executor URI, source assembly,
  fully qualified name, display name, traits, location, and target. `P3`
- [ ] `TEST-007` Batch discovery by compatible adapter/runtime and cache it by
  assembly, deps, adapter, and settings fingerprints. `P3`
- [ ] `TEST-008` Implement list-tests without executing tests. `P3`
- [ ] `TEST-009` Implement VSTest filter grammar for name, traits/categories,
  equality, inequality, contains, boolean operators, and escaping. `P3`
- [ ] `TEST-010` Pass runner-specific MTP arguments and extension options
  without pretending VSTest and MTP have one option set. `P3`
- [ ] `TEST-011` Support runsettings, adapter paths, environment, results
  directory, target framework/runtime/architecture, and project/module
  selection. `P3`
- [ ] `TEST-012` Execute modules/tests in bounded parallel batches with a
  measured sequential crossover and deterministic report order. `P3`
- [ ] `TEST-013` Define process isolation, apartment/working-directory,
  environment, timeout, crash, and orphan cleanup behavior. `P3`
- [ ] `TEST-014` Stream bounded stdout/stderr and spill large output without
  unbounded memory retention. `P3`
- [ ] `TEST-015` Represent passed, failed, skipped, not-found, timeout,
  cancelled, and infrastructure-error results distinctly. `P3`
- [ ] `TEST-016` Preserve failure message, stack, expected/actual where exposed,
  duration, traits, output, attachments, and retry attempts. `P3`
- [ ] `TEST-017` Support TRX and one documented open result format or a stable
  JSON result schema; retain deterministic naming and paths. `P3`
- [ ] `TEST-018` Support diagnostic logs, blame crash/hang, sequence files, dump
  collection hooks, and platform-specific availability diagnostics. `P3`
- [ ] `TEST-019` Provide coverage integration points and attachment transport;
  do not build a coverage engine into the scheduler. `P3`
- [ ] `TEST-020` Implement retries only as an explicit policy that records every
  attempt and never turns an initial failure invisible. `P3`
- [ ] `TEST-021` Stop scheduling after cancellation/fail-fast while draining
  bounded in-flight work. `P3`
- [ ] `TEST-022` Return stable command outcomes for test failures versus test
  infrastructure failures. `P3`
- [ ] `TEST-023` Benchmark discovery-only, warm filtered run, many fast tests,
  few slow tests, failure-heavy, and output-heavy workloads. `P3`

## 16. Project And Template Creation

- [ ] `NEW-001` Define `dv init` defaults for console, class library, test,
  web, worker, and solution templates according to supported SDKs. `P2/P5`
- [ ] `NEW-002` Implement name, output, language, framework, force, dry-run,
  no-restore, and template-specific typed parameters. `P2`
- [ ] `NEW-003` Render built-in template file/content transforms
  deterministically and reject path traversal or overwrite without force. `P2`
- [ ] `NEW-004` Run sync only after all files commit successfully and preserve
  created files if restore fails with a clear partial-success result. `P2`
- [ ] `NEW-005` Add project to an explicitly selected solution/folder when
  requested. `P2`
- [ ] `NEW-006` List and search installed/remote templates. `P5`
- [ ] `NEW-007` Install, update, and uninstall template packages with source,
  authentication, prerelease, and version policy. `P5`
- [ ] `NEW-008` Implement template constraints, baselines, symbols, choices,
  conditional content, file renames, localization, and primary outputs. `P5`
- [ ] `NEW-009` Execute only a reviewed allowlist of post-actions; report all
  other actions for explicit manual execution. `P5`
- [ ] `NEW-010` Consume existing .NET template packages only after a
  compatibility corpus proves the supported template-engine subset. `P5`

## 17. Pack

- [ ] `PACK-001` Select project/solution/nuspec input and pack all packable
  projects in deterministic order. `P4`
- [ ] `PACK-002` Build and sync implicitly unless no-build/no-restore is
  requested and validated. `P4`
- [ ] `PACK-003` Evaluate package identity/version, authors, company,
  description, title, summary, tags, release notes, copyright, language,
  project/repository URLs, repository commit/type, and serviceable metadata.
  `P4`
- [ ] `PACK-004` Validate license expression/file, icon, readme, and package
  metadata rules. `P4`
- [ ] `PACK-005` Map target outputs into `lib`/`ref` TFM folders and runtime/
  native/resource assets into valid NuGet paths. `P4`
- [ ] `PACK-006` Generate dependency groups from resolved direct references,
  private/include/exclude assets, project-reference policy, and central
  transitive pinning. `P4`
- [ ] `PACK-007` Apply item `Pack`, `PackagePath`, build action, copy-to-output,
  flatten, and content-file metadata. `P4`
- [ ] `PACK-008` Include package `build`, `buildMultiTargeting`,
  `buildTransitive`, analyzers, tools, and framework assemblies only when the
  compatibility contract supports their consumer behavior. `P4`
- [ ] `PACK-009` Support `IsPackable`, package-on-build, symbols package,
  include-source, symbol format, output path, configuration, framework,
  runtime, version suffix, and no-build options. `P4`
- [ ] `PACK-010` Parse explicit `.nuspec` inputs, replacement tokens,
  dependency/reference groups, and file globs for the supported subset. `P4`
- [ ] `PACK-011` Create deterministic ZIP entries, timestamps, ordering,
  permissions, nuspec, relationships, and content-types metadata. `P4`
- [ ] `PACK-012` Validate the generated package by reopening it and comparing
  identity, manifest, file hashes, and consumer restore behavior. `P4`
- [ ] `PACK-013` Generate `.snupkg` where requested and compare package contents
  with the reference oracle. `P4`
- [ ] `PACK-014` Support package validation/API compatibility only through a
  defined native transform or explicit external opt-in, never an implicit
  MSBuild task. `P5`

## 18. Publish And Distribution

- [ ] `PUB-001` Publish framework-dependent portable output. `P4`
- [ ] `PUB-002` Publish framework-dependent RID-specific executable output.
  `P4`
- [ ] `PUB-003` Publish self-contained output with selected runtime/host packs.
  `P4`
- [ ] `PUB-004` Select runtime assets, native libraries, satellite resources,
  apphost, deps, runtime config, symbols, docs, and content without duplicate
  output paths. `P4`
- [ ] `PUB-005` Support configuration, framework, runtime, architecture, OS,
  self-contained/no-self-contained, output/artifacts path, no-build,
  no-restore, version suffix, and property overrides. `P4`
- [ ] `PUB-006` Parse folder publish profiles and merge CLI/project/profile
  values with documented precedence. `P4`
- [ ] `PUB-007` Support ReadyToRun with composite/crossgen inputs and explicit
  platform prerequisite diagnostics. `P5`
- [ ] `PUB-008` Support trimming, trim analyzers, descriptors, roots, feature
  switches, and trim warning policy. `P5`
- [ ] `PUB-009` Support single-file bundling for framework-dependent and
  self-contained output, extraction options, symbols, and native libraries.
  `P5`
- [ ] `PUB-010` Support Native AOT only through the selected Microsoft AOT
  compiler/toolchain with explicit native prerequisite checks. `P5`
- [ ] `PUB-011` Support satellite/resource language filtering and
  documentation/symbol copy controls. `P4`
- [ ] `PUB-012` Detect duplicate publish outputs and preserve the prior
  successful directory until a complete new publish is ready. `P4`
- [ ] `PUB-013` Produce a publish manifest containing all files, sizes, hashes,
  origins, and deployment mode for verification. `P4`
- [ ] `PUB-014` Compare runnable behavior and file roles with `dotnet publish`
  for each supported deployment mode. `P4`
- [ ] `PUB-015` Add Web/Razor static web assets, web.config transforms, and
  related publish behavior only with Web SDK compatibility. `P5`
- [ ] `PUB-016` Add container-image publishing as a later distribution target,
  with registry auth, deterministic layers, and signed metadata. `P5`
- [ ] `PUB-017` Add NuGet package push with API key/credential provider,
  duplicate policy, timeout, retries, symbols source, and secret redaction.
  `P4`
- [ ] `PUB-018` Add package delete/unlist only as an explicit destructive
  command with source capability checks. `P4`

## 19. SDK And Runtime Management

- [x] `SDK-001` Discover SDKs from the active host root and fallbacks.
- [x] `SDK-002` Parse stable/prerelease SDK versions and semantic ordering.
- [x] `SDK-003` Apply documented `global.json` version, roll-forward, and
  prerelease policy.
- [x] `SDK-004` Apply .NET 10 `paths`, `$host$`, comments, and custom error
  message.
- [x] `SDK-005` List and select installed SDKs without a managed process.
- [ ] `SDK-006` Match `--list-sdks --arch` and architecture-specific roots.
  `P2`
- [~] `SDK-007` Inventory shared frameworks, hostfxr, hostpolicy, architecture,
  RID, and install provenance. Shared frameworks are now enumerated once into
  16-byte records backed by one contiguous text arena, sorted by host-root,
  family, and semantic version, and bounded to 4,096 installations. Four
  records fit in an assumed 64-byte cache line; issue 0003 owns validation of
  that platform value. Hostfxr, hostpolicy, explicit
  architecture/RID classification, and provenance remain open. `P1`
- [ ] `SDK-008` Select runtime framework versions using runtimeconfig and
  roll-forward rules independently from SDK selection. `P1`
- [ ] `SDK-009` Query official release metadata by channel, version, quality,
  architecture, and OS with cache/offline behavior. `P4`
- [ ] `SDK-010` Plan exact SDK/runtime/ASP.NET/runtime-desktop payloads and disk
  effects before download. `P4`
- [ ] `SDK-011` Download concurrently with bounded streams, resume, retries,
  proxy, and progress. `P4`
- [ ] `SDK-012` Verify release metadata, content hash, archive paths, entry
  sizes, total expansion, and platform signing/notarization where available.
  `P4`
- [ ] `SDK-013` Install atomically to user or explicit roots without requiring
  elevation by default. `P4`
- [ ] `SDK-014` Handle PATH/shell activation separately from payload install
  and never edit global shell state silently. `P4`
- [ ] `SDK-015` Support side-by-side versions and architectures, idempotent
  reinstall, repair, uninstall, and rollback after interruption. `P4`
- [ ] `SDK-016` Implement current/list/check/install/update/remove for SDKs and
  runtimes with human and JSON parity. `P4`
- [ ] `SDK-017` Respect `global.json` acquisition intent and diagnose an exact
  install command when no compatible SDK exists. `P4`
- [ ] `SDK-018` Coordinate concurrent installs and active readers without
  exposing partial roots. `P4`
- [ ] `SDK-019` Measure cold metadata query, cached query, download throughput,
  verification CPU, install writes, startup, and selection latency. `P4`

## 20. Events, Diagnostics, And User Experience

- [x] `EVT-001` Validate schema version, contiguous sequence, and monotonic
  elapsed time.
- [x] `EVT-002` Serialize one JSON object per event from a borrowed event batch.
- [x] `EVT-003` Carry stable diagnostic code, severity, message, ordered
  context, causal chain, and optional action.
- [~] `EVT-004` Human and JSON views share current SDK/command event data, but
  there is no general reporter abstraction or full workflow coverage.
- [ ] `EVT-005` Define command, stage, cache, download, build target, compiler,
  run, test, package, publish, and cancellation event payloads from real
  consumers. `P1-P4`
- [ ] `EVT-006` Version event additions and breaking changes with reader
  compatibility tests and golden streams. `P1`
- [ ] `EVT-007` Keep one event per meaningful batch/state transition, never per
  source/package/graph node in hot state. `P1`
- [ ] `EVT-008` Make human output quiet on success, progressively detailed on
  warnings/failure, and noninteractive in redirected/CI output. `P1`
- [ ] `EVT-009` Add TTY progress that does not corrupt child output and is
  disabled or converted to events in JSON mode. `P2`
- [ ] `EVT-010` Implement stable warning suppression/promotion and deduplicate
  wrapper causes without losing original Roslyn/NuGet/test IDs. `P2`
- [ ] `EVT-011` Attach project, target, package/source, file/span, command,
  causal edge, and action context as typed ordered fields. `P1`
- [ ] `EVT-012` Redact credentials, API keys, authorization headers, query
  secrets, environment secrets, and credential-provider output. `P1`
- [ ] `EVT-013` Handle broken stdout/stderr pipes and reporter write failure
  without panic. `P1`
- [ ] `EVT-014` Add optional trace/evidence files containing evaluation,
  resolution, cache, process, and timing data with an explicit schema. `P2`
- [ ] `EVT-015` Generate shell completions and command documentation from the
  typed command model. `P5`
- [ ] `EVT-016` Define a stable IDE/editor integration protocol only after
  CLI/JSON consumers prove which data is needed. `P5`

## 21. Cross-Platform, Reliability, And Security

- [ ] `PORT-001` Run every supported workflow on Windows x64/arm64, Linux
  x64/arm64, and macOS x64/arm64 where the selected SDK supports it. `P1-P5`
- [ ] `PORT-002` Handle Windows drive/UNC/long paths, Unix roots, separators,
  case sensitivity, symlinks, junctions, and executable permissions. `P1`
- [ ] `PORT-003` Preserve non-Unicode filesystem paths internally and reject
  only text protocols that cannot represent them losslessly. `P1`
- [ ] `PORT-004` Quote Windows command lines and Unix argv/env without
  round-tripping through shell text. `P1`
- [ ] `PORT-005` Normalize only protocol-defined paths; never corrupt case or
  separator-sensitive application data. `P1`
- [ ] `PORT-006` Define atomic replacement and file-lock behavior per
  filesystem, including network filesystems and antivirus interference. `P2`
- [ ] `PORT-007` Bound every queue, download, archive expansion, output capture,
  diagnostic batch, test result batch, and retained trace. `P1`
- [ ] `PORT-008` Reject XML external entities, ZIP traversal, cache poisoning,
  source spoofing, insecure redirects, and untrusted executable extensions.
  `P1`
- [ ] `PORT-009` Make all writes crash-consistent and all persistent protocols
  versioned with migration or explicit invalidation. `P1`
- [ ] `PORT-010` Reproduce output ordering and meaningful artifact content
  across worker counts and repeated builds. `P1`
- [ ] `PORT-011` Support offline mode with a complete missing-input report and
  zero attempted network requests. `P2`
- [ ] `PORT-012` Define telemetry policy; default to no undisclosed network
  requests, including workload advertising behavior. `P1`
- [ ] `PORT-013` Emit software-bill-of-material and provenance data for release
  binaries and verify third-party dependency licenses. `P4`
- [ ] `PORT-014` Fuzz XML/JSON/solution/nuspec/lock/event parsers and package
  archive boundaries. `P2`
- [ ] `PORT-015` Test disk-full, permission, interrupted write, process crash,
  corrupt cache, unavailable source, TLS, and cancellation failures. `P2`

## 22. Broader Project Compatibility

These are required for broad practical parity, but only after the C# base SDK
vertical slice is correct and measured.

- [ ] `BROAD-001` `Microsoft.NET.Sdk.Web`: framework reference defaults,
  analyzers, generated inputs, content, static web assets, apphost/run, and
  publish. `P5`
- [ ] `BROAD-002` `Microsoft.NET.Sdk.Razor`: Razor item discovery, code
  generation, analyzers, component/tag-helper inputs, incremental state, and
  publish assets. `P5`
- [ ] `BROAD-003` `Microsoft.NET.Sdk.BlazorWebAssembly`: workload/runtime packs,
  linking, boot manifest, compression, globalization, service-worker, and
  publish layout. `P5`
- [ ] `BROAD-004` `Microsoft.NET.Sdk.Worker`: implicit usings/framework
  references, run, and publish defaults. `P5`
- [ ] `BROAD-005` Windows Forms and WPF: Windows targeting, desktop reference
  packs, XAML/resources, generated code, WinExe, apphost metadata, and Windows-
  only build boundaries. `P5`
- [ ] `BROAD-006` `MSTest.Sdk` and Microsoft.Testing.Platform project SDK
  behavior. `P3/P5`
- [ ] `BROAD-007` Aspire AppHost SDK project graph, orchestration metadata, and
  run behavior only under a separate typed subsystem contract. `P5`
- [ ] `BROAD-008` F# project evaluation and direct F# compiler hosting where
  feasible. `P5`
- [ ] `BROAD-009` Visual Basic project evaluation and direct VB compiler
  hosting where feasible. `P5`
- [ ] `BROAD-010` .NET Framework reference assemblies, binding redirects,
  app.config, Windows-only execution, and legacy resource/signing behavior.
  `P5`
- [ ] `BROAD-011` Platform TFMs and workloads for Android, iOS, Mac Catalyst,
  tvOS, browser, and WASI only as individually measured compatibility
  programs. `P5`
- [ ] `BROAD-012` Native AOT and workload-specific custom tasks remain explicit
  failures until their Microsoft tool inputs can be invoked through typed
  protocols without MSBuild fallback. `REJECT until P5`
- [ ] `BROAD-013` Package-provided build props/targets may contribute
  declarative known inputs; arbitrary package tasks or executables never run
  implicitly. `P5`
- [ ] `BROAD-014` Create an extension compatibility registry mapping each known
  SDK/package target to native transform version, supported inputs, outputs,
  side effects, and rejection behavior. `P5`
- [ ] `BROAD-015` Publish migration guidance that names unsupported constructs
  and gives the smallest supported alternative. `P5`

## 23. Benchmark And Compatibility Gates

- [~] `GATE-001` Process-level harness records raw wall-time samples, median,
  p95, min, max, environment, command, and tool version.
- [x] `GATE-002` SDK-selection benchmark verifies identical selected version
  before timing.
- [~] `GATE-003` Add exact oracle comparisons for evaluated items/properties,
  resolved graph/assets, compiler batch, artifacts, program behavior, test
  results, package contents, and publish output. `P1-P4`
- [~] `GATE-004` Add package-bearing small and real large-graph fixtures,
  conditional project, analyzer/generator fixture, resources/content fixture,
  and failure corpus.
  `P1/P2`
- [ ] `GATE-005` Add sanitized or distribution-derived large solution,
  test-heavy repository, and authenticated multi-source fixture. `P2/P3`
- [ ] `GATE-006` Measure cold, warm, incremental, and no-op states separately
  for sync, build, run, test, pack, and publish. `P1-P4`
- [ ] `GATE-007` Capture wall latency, CPU time, peak memory, allocation count,
  bytes read/written/copied, filesystem operations, processes, network
  requests/bytes, cache outcomes, and worker utilization. `P1-P4`
- [ ] `GATE-008` Establish numeric regression budgets only after at least 30
  controlled release samples on representative Windows and Linux machines.
  `P1`
- [ ] `GATE-009` Measure sequential versus bounded parallel crossovers for
  parsing, hashing, resolution, graph work, compilation, and tests. `P2/P3`
- [ ] `GATE-010` Record `size_of`, `align_of`, field count, elements per
  assumed cache line, and working-set bytes for hot records with compile-time
  assertions. `P1/P2`
- [ ] `GATE-011` Validate cache-line assumptions on benchmark hardware before
  applying alignment/padding. `P2`
- [ ] `GATE-012` Run compatibility and release checks on Windows, Linux, and
  macOS for every declared supported workflow. `P1-P5`
- [ ] `GATE-013` Fail parity claims when meaningful artifacts/results differ,
  even if `dv` is faster. `P1-P5`
- [ ] `GATE-014` Label every unmeasured performance hypothesis
  **unverified** and name its deciding benchmark. `P1-P5`
- [ ] `GATE-015` Enumerate commands/options from pinned `dotnet`, MSBuild,
  NuGet, and VSTest help/version output and fail when the published
  compatibility manifest omits a row. `P1-P5`
- [ ] `GATE-016` Run paired executable-token tests that keep the complete argv,
  environment, stdin, working directory, and input filesystem identical while
  changing only the reference executable to `dv`. `P1-P5`
- [ ] `GATE-017` Compare exit code, stdout/stderr roles, documented
  machine-readable output, created/changed/deleted files, processes, network
  requests, and meaningful artifacts for every paired invocation. `P1-P5`
- [ ] `GATE-018` Verify canonical `dv` syntax and every compatibility alias
  normalize to an identical typed command batch before execution. `P1-P5`
- [ ] `GATE-019` Maintain real CI-script fixtures from GitHub Actions, Azure
  Pipelines, build scripts, Dockerfiles, and local shell workflows with only
  the executable token substituted. `P2-P5`
- [ ] `GATE-020` Test output consumers, not only snapshots: parse generated
  JSON, XML, TRX, lock files, assets files, binary logs, and list/info text
  using representative downstream tools. `P2-P5`
- [ ] `GATE-021` Record compatibility separately for each supported SDK,
  MSBuild, NuGet, VSTest, OS, architecture, and shell combination. `P1-P5`
- [ ] `GATE-022` Block unqualified drop-in release claims when any required
  manifest row is missing, rejected, ignored, or only tested through canonical
  `dv` syntax. `P1-P5`

## Simplification Pass

This mapping deliberately removes the following machinery:

- no invocation of the Microsoft MSBuild engine and no separate evaluator or
  scheduler per compatibility syntax;
- no duplicated execution paths for `dotnet`, direct MSBuild, NuGet, VSTest,
  and canonical `dv` commands after they become typed requests;
- no plugin framework before a real extension protocol consumer exists;
- no daemon before isolated compiler-host and whole-process benchmarks prove
  persistence is required;
- no async runtime for CPU-bound evaluation, graph, hashing, or compilation
  planning;
- no one-task-per-file scheduling;
- no duplicated human and JSON execution paths;
- no hot execution records built from event/diagnostic wire objects;
- no fabricated large fixtures, cache-line values, concurrency thresholds, or
  performance budgets;
- no broad language/workload implementation before the minimal C# vertical
  slice is artifact-compatible.

The temporary implementation boundary is explicit rejection with a stable
diagnostic, captured unsupported evidence, and a supported alternative when
one exists. Rejection prevents incorrect artifacts, but the rejected row
remains missing drop-in parity work.

## Phase Completion Gates

### P1: Fast Inner Loop

Complete only when the package-free and package-bearing console/library
fixtures can sync, build, no-op build, incrementally rebuild, and run without a
production `dotnet`, MSBuild, NuGet, or VSTest process. Evaluated inputs,
compiler batches, artifacts, and observable execution must match the oracle
contract. The supported vertical slice must work through both canonical `dv`
syntax and direct executable-token replacements such as replacing
`dotnet restore` with `dv restore`, `dotnet build` with `dv build`, and
`dotnet run` with `dv run`.

### P2: Real Repositories

Complete only when `.sln`/`.slnx`, multi-target projects, package sources,
central package management, private authentication, package editing, concurrent
cache use, and cross-platform output pass representative fixtures and failure
injection. Replacing direct `msbuild` with `dv`, `dotnet msbuild` with
`dv msbuild`, and direct `nuget` with `dv` must also pass the versioned
invocation manifests for the supported project/restore surface.

### P3: Testing

Complete only when supported VSTest-adapter and MTP repositories have equivalent
discovery, filtering, execution, result, attachment, cancellation, and exit
behavior, with bounded memory and measured scheduling. The same test batches
must be reachable through `dotnet test`, `dotnet vstest`,
`vstest.console`, and canonical `dv test` replacement shapes.

### P4: Distribution

Complete only when pack, app publish, package push, and SDK/runtime acquisition
produce verified contents, survive interruption, redact secrets, and have
cold/warm resource evidence. Reference `dotnet` and NuGet command names,
options, defaults, outputs, and exits must pass executable-token replacement
tests.

### P5: Broad Compatibility

Complete per SDK/language/workload/tool row. A scoped compatibility claim may
name completed rows, but an unqualified "drop-in replacement for dotnet,
MSBuild, NuGet, and VSTest" claim is allowed only when every required row in
the published compatibility manifests passes. Each row needs its own input
contract, oracle corpus, artifacts, failures, output/exit evidence, and
benchmarks.

## Evidence That Would Disprove This Map

- Real target repositories depend predominantly on an omitted workflow.
- Real scripts contain invocation ambiguities that cannot be classified from
  executable-token replacement plus argument shape.
- Required MSBuild task assemblies cannot be hosted compatibly without using
  the Microsoft MSBuild engine.
- IDE/tool interoperability requires undocumented binary or design-time
  behavior that cannot be captured as a versioned compatibility protocol.
- Direct Roslyn/runtime hosting cannot preserve compiler/runtime compatibility
  without unacceptable process or protocol cost.
- The isolated compiler host, content-addressed cache, or fingerprint layers
  cost more than the work they avoid on representative batches.

Any of these requires revising the scope or transform, not silently adding a
fallback.

## Full Drop-In Surface Ledger

The official command families below are required eventual compatibility work,
even when sequenced after the inner loop. They cannot be omitted from an
unqualified drop-in claim.

- [ ] `SURF-001` `dotnet build-server` server discovery/shutdown and compatible
  absent-server behavior. `P5`
- [ ] `SURF-002` `dotnet msbuild` plus direct `msbuild` command-line,
  evaluation, target, task, logger, and result behavior. `P2-P5`
- [ ] `SURF-003` `dotnet store` runtime package-store generation and manifests
  for SDK versions that expose it. `P5`
- [ ] `SURF-004` global, local, and tool-path `dotnet tool`
  install/update/uninstall/list/search plus manifest and restore behavior. `P5`
- [ ] `SURF-005` `dotnet workload` clean/config/history/install/list/repair/
  restore/search/uninstall/update, workload sets, manifests, rollback, and
  advertising behavior. `P5`
- [ ] `SURF-006` SDK command dispatch for `dev-certs`, `user-secrets`, `watch`,
  Entity Framework, and installed extension commands without a `dotnet`
  process. `P5`
- [ ] `SURF-007` generic managed application execution with additional probing
  paths/deps, runtime config, deps file, framework version, roll-forward,
  architecture, arguments, environment, signals, and exit behavior. `P2`
- [ ] `SURF-008` NuGet sign/verify, trusted-signer editing, API-key/source
  administration, install/update/spec/init, and server operations exposed by
  supported NuGet versions. `P5`
- [ ] `SURF-009` project/reference/package/solution command orderings from old
  and current SDK versions, including renamed noun-first forms. `P2-P5`
- [ ] `SURF-010` `dotnet new` template search/list/details/install/update/
  uninstall and every selected built-in template parameter. `P5`
- [ ] `SURF-011` `dotnet sdk check`, runtime inventory, architecture-specific
  list commands, and script-compatible output. `P4`
- [ ] `SURF-012` design-time/evaluation targets and intermediate files consumed
  by editors, IDEs, language servers, and CI integrations. Visual Studio UI
  rendering itself remains outside the command-line drop-in contract. `P5`
- [ ] `SURF-013` compatibility aliases that have been removed from current
  documentation but remain in supported LTS SDK/tool versions. `P5`
- [ ] `SURF-014` a release gate that compares the published manifest against
  the complete command inventories reported by the selected `dotnet`,
  MSBuild, NuGet, and VSTest reference executables. `P1-P5`

## Primary Compatibility References

- [The `dotnet` command and command families](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet)
- [MSBuild command-line reference](https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-command-line-reference)
- [NuGet CLI reference](https://learn.microsoft.com/en-us/nuget/reference/nuget-exe-cli-reference)
- [VSTest.Console command-line options](https://learn.microsoft.com/en-us/visualstudio/test/vstest-console-options)
- [.NET project SDK overview](https://learn.microsoft.com/en-us/dotnet/core/project-sdk/overview)
- [.NET SDK MSBuild properties and items](https://learn.microsoft.com/en-us/dotnet/core/project-sdk/msbuild-props)
- [MSBuild conditions](https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-conditions)
- [NuGet PackageReference](https://learn.microsoft.com/en-us/nuget/consume-packages/package-references-in-project-files)
- [NuGet dependency resolution](https://learn.microsoft.com/en-us/nuget/concepts/dependency-resolution)
- [NuGet.Config reference](https://learn.microsoft.com/en-us/nuget/reference/nuget-config-file)
- [NuGet package restore](https://learn.microsoft.com/en-us/nuget/consume-packages/package-restore)
- [`dotnet test`](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-test)
- [`dotnet pack`](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-pack)
- [`dotnet publish`](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-publish)
- [.NET Runtime Identifier catalog](https://learn.microsoft.com/en-us/dotnet/core/rid-catalog)

These references describe the moving Microsoft compatibility surface. Each
implemented capability must pin its supported SDK/tool versions in tests and
must not assume this map alone is an executable specification.
