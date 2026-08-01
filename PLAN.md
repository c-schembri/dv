# dv Project Plan

## Vision

`dv` is a fast, cohesive toolchain for building, testing, running, packaging,
and publishing C# and .NET projects.

It should do for .NET development what `uv` did for Python: replace a slow,
fragmented collection of routine tools with one dependable executable, one
coherent workflow, and aggressive performance throughout.

Microsoft remains responsible for the two parts that must preserve platform
compatibility:

- Roslyn compiles C#.
- The .NET runtime executes managed programs.

`dv` owns the work around them, including SDK and runtime acquisition, project
evaluation, dependency resolution, caching, build planning, orchestration,
testing, packaging, publishing, diagnostics, and progress reporting.

## Product Goals

1. **Feature parity**

   Support the practical workflows developers currently rely on across the
   .NET CLI, MSBuild, NuGet, and test tooling. Existing projects should work
   without requiring a migration to a new project format.

2. **Exceptional performance**

   Make startup, restore, no-op builds, incremental builds, test discovery,
   and repeated commands feel immediate. Performance is a product feature, not
   a final optimization pass.

3. **Better usability**

   Present a small, predictable command surface with consistent flags,
   defaults, configuration, and output. Common workflows should require less
   ceremony while advanced behavior remains available.

4. **Actionable diagnostics**

   Explain failures in terms of what happened, why it happened, and what the
   developer can do next. Preserve underlying compiler and runtime diagnostics
   while adding useful context instead of burying them in orchestration noise.

5. **Clear reporting**

   Show concise progress by default and detailed evidence on demand. Timing,
   cache behavior, dependency decisions, build steps, and test results should
   be understandable by both humans and automation.

6. **Deterministic operation**

   Resolve dependencies reproducibly, record enough information in a lockfile
   to repeat the result, and make cache keys and invalidation behavior
   explainable.

7. **Cross-platform reliability**

   Treat Windows, macOS, and Linux as first-class environments with consistent
   commands and behavior.

## Priorities

When goals compete, use this order:

1. Correctness and compatibility
2. Performance
3. Diagnostic quality
4. Workflow simplicity
5. Determinism and observability
6. Breadth of feature coverage

Feature parity is the destination, but not the sequencing strategy. We will
first make the highest-frequency inner-loop workflows excellent, then expand
outward without weakening their quality.

## Compatibility Contract

`dv` is intended to be a drop-in replacement for the everyday capabilities
currently exposed through:

- `dotnet restore`
- `dotnet build`
- `dotnet run`
- `dotnet test`
- `dotnet add`, `remove`, and `list`
- `dotnet new`
- `dotnet pack`
- `dotnet publish`
- SDK and runtime installation and selection
- NuGet source, authentication, resolution, download, and cache behavior
- Common SDK-style MSBuild project and solution semantics

Compatibility means accepting existing `.csproj`, `.fsproj` where feasible,
`.sln`, `.slnx`, `global.json`, `NuGet.Config`, and related repository files.
It does not require reproducing Microsoft's internal architecture or output.

Drop-in compatibility is a process contract, not just equivalent final
artifacts. For every supported workflow, a developer or CI script must be able
to replace only the reference executable token (`dotnet`, MSBuild, NuGet, or
VSTest) with `dv` and retain the remaining arguments and their order. Canonical
`dv` commands may organize the same behavior more coherently, but both forms
must normalize into the same typed command batch. Compatibility includes
argument parsing and precedence, environment inputs, stdin/stdout/stderr roles,
exit status, cancellation, and meaningful filesystem and network effects.

Unsupported behavior must fail explicitly. `dv` must never silently produce a
plausible but incompatible build or reinterpret an unknown reference-tool
option after side effects have begun.

## Command Experience

The initial command vocabulary should remain small and composable:

```text
dv init
dv add <package>
dv remove <package>
dv sync
dv build
dv run
dv test
dv pack
dv publish
dv sdk install
```

Command naming is provisional until each workflow is designed. Every command
should follow these rules:

- Fast startup with no unnecessary network or filesystem work.
- Useful defaults for the common case.
- Stable, scriptable exit codes and machine-readable output.
- Quiet success output with progressive detail for warnings and failures.
- A consistent `--verbose` path for investigation.
- A consistent `--json` path for tools and CI.
- No hidden fallback to `dotnet`, MSBuild, NuGet, or VSTest.

## Technical Direction

`dv` will be implemented in Rust as a native executable.

Core architectural boundaries:

- Rust owns discovery, parsing, evaluation, resolution, downloads, caching,
  graph construction, incremental state, scheduling, and reporting.
- Roslyn is invoked directly for compilation.
- Applications and test processes execute on the selected Microsoft .NET
  runtime.
- Components communicate through explicit typed data rather than scraping
  console output.
- Internal APIs must support concurrency, cancellation, structured
  diagnostics, tracing, and deterministic testing.

Major subsystems are expected to include:

- Workspace and project model
- SDK and runtime manager
- Project evaluator
- Package resolver and content-addressed cache
- Lockfile
- Build graph and incremental engine
- Roslyn compiler driver
- Process and application runner
- Test discovery and execution
- Pack and publish pipeline
- Human and machine-readable reporters

## Performance Standards

Every major workflow needs a benchmark before it is considered complete.
Measurements should cover cold and warm states separately.

Initial performance targets:

- CLI help and version output feel instantaneous.
- No-op operations avoid launching managed processes.
- Warm dependency resolution performs no unnecessary network access.
- No-op builds inspect only the state needed to prove that work is current.
- Independent downloads, projects, and tests run concurrently by default.
- Cache hits are cheaper than recomputing or revalidating their contents.
- Memory usage remains predictable on large repositories.

We will maintain representative repositories for:

- A single small console project
- A multi-project application
- A large solution with shared dependencies
- A test-heavy repository
- A repository with private and multiple package sources

Benchmarks will compare `dv` with the corresponding Microsoft toolchain
workflow. Results must include command startup, restore, clean build,
incremental build, no-op build, test discovery, and test execution overhead.

## Diagnostics and Reporting

Diagnostics are part of the core architecture. Each failure should carry:

- A stable diagnostic code
- A short description
- Relevant file, project, package, source, and command context
- The causal chain without duplicate wrapper errors
- A suggested next action when one is known
- Optional detailed evidence for debugging

Default output should optimize for comprehension, not log volume. CI output
should identify the failing unit of work and retain useful timing information.
Structured events should make it possible to build JSON reports, IDE
integration, and richer terminal interfaces without parsing prose.

## Delivery Phases

### Phase 0: Foundations

- Establish the Rust workspace, contribution standards, and CI.
- Define performance benchmark fixtures and a repeatable measurement harness.
- Define structured events, diagnostics, and command output conventions.
- Document the compatibility boundary and direct Roslyn invocation strategy.

### Phase 1: Fast Inner Loop

- Discover SDK-style projects and select an installed SDK/runtime.
- Evaluate the minimum project properties needed by simple C# projects.
- Resolve and cache package dependencies with an initial lockfile.
- Route the supported `dotnet` and direct MSBuild argument shapes into the same
  typed commands as canonical `dv` syntax.
- Construct the minimal deterministic project build graph.
- Generate the SDK-owned C# inputs required by representative console and
  library projects.
- Compile directly with Roslyn.
- Materialize compatible assemblies, symbols, dependency manifests, runtime
  configuration, copy-local assets, and apphosts.
- Implement `dv build` and `dv run`, including their supported executable-token
  replacement forms.
- Support accurate incremental and no-op builds.

The phase succeeds when representative console and library projects can be
restored, built, and run without invoking the .NET CLI or MSBuild. Paired
compatibility tests must preserve each supported reference command's complete
argument vector while changing only its executable token to `dv`, then compare
exit status and meaningful outputs.

### Phase 2: Real Repositories

- Add solution and multi-target project support.
- Expand project evaluation and build graph compatibility.
- Close remaining package source, signature, cache, and conflict-compatibility
  gaps exposed by representative repositories.
- Implement `dv add`, `dv remove`, and `dv sync`.
- Expand direct MSBuild command and target compatibility for the supported
  repository workflows.
- Harden cross-platform behavior and cache concurrency.

### Phase 3: Testing

- Discover test projects and adapters.
- Build and execute tests without VSTest orchestration.
- Support the corresponding `dotnet test` and direct VSTest executable-token
  argument forms without invoking either reference tool.
- Add filtering, parallel execution, cancellation, retries where appropriate,
  result files, coverage integration points, and clear failure summaries.

### Phase 4: Distribution

- Implement `dv pack` and `dv publish`.
- Manage SDK and runtime downloads, verification, selection, and updates.
- Add shell completion, self-update, installation workflows, and CI caching.

### Phase 5: Broad Compatibility

- Close high-value compatibility gaps discovered in real repositories.
- Support extensibility scenarios without adopting MSBuild as the execution
  engine.
- Publish a compatibility matrix and migration guidance for unsupported
  custom build behavior.

## Definition of Done

A workflow is complete only when:

- Its supported compatibility behavior is documented and tested.
- It produces the same meaningful artifact or result as the reference
  Microsoft workflow.
- It has cold, warm, incremental, and no-op benchmarks where applicable.
- Failure cases produce actionable diagnostics.
- Human and JSON reporting are covered by tests.
- It works on supported Windows, macOS, and Linux environments.
- It does not invoke `dotnet`, MSBuild, NuGet, or VSTest behind the scenes.

## Non-Goals

- Reimplementing the C# compiler or the .NET runtime.
- Preserving accidental quirks or console formatting from Microsoft tools.
- Running arbitrary MSBuild tasks as the foundation of the build engine.
- Claiming universal project compatibility before it has been measured.
- Trading correctness for a favorable benchmark.

## Immediate Next Steps

As of 2026-08-01, the committed implementation has native SDK selection,
bounded project evaluation, framework/runtime/apphost planning, structured
events, exact package resolution, a verified NuGet-compatible cache, an initial
lockfile, source configuration and authentication, deterministic package-asset
planning, compatibility fixtures, and cold/warm benchmarks. `dv build --plan`
produces compiler inputs but does not compile. Build outputs, execution,
incremental state, and executable-token routing remain open.

Work the next steps in this order. Do not deepen unrelated compatibility areas
until the end-to-end checkpoint passes unless new evidence shows they block it.
The dependency-aware order beyond this checkpoint is maintained in
`docs/implementation-order.md`; `docs/feature-parity-map.md` remains the scope
and completion ledger.

1. Create the Phase 1 compatibility manifest and normalize supported canonical,
   `dotnet`, and direct MSBuild build forms into one typed command batch before
   project or SDK I/O.
2. Materialize target-framework attributes, assembly information, and implicit
   global usings, then finalize the immutable compiler batch for one console
   and one library fixture.
3. Define the versioned, length-bounded compiler-host protocol and invoke the
   selected Roslyn through native `hostfxr`, preserving typed diagnostics and
   never invoking `dotnet exec`.
4. Atomically materialize the minimal compatible output set: assemblies,
   symbols, copy-local assets, `.deps.json`, `.runtimeconfig.json`, and apphost.
5. Implement the smallest deterministic build graph and stage-local
   fingerprints needed for cold, warm, incremental, and proven no-op builds.
6. Run the console fixture through the selected Microsoft runtime with correct
   arguments, environment, standard streams, cancellation, and exit status.
7. Gate the vertical slice with paired executable-token tests, artifact
   comparisons, human/JSON diagnostics, and cold/warm/incremental/no-op
   benchmarks on representative console and library projects.
