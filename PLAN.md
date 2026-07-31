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

`dv` should eventually provide replacements for the everyday capabilities
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

Unsupported behavior must fail explicitly. `dv` must never silently produce a
plausible but incompatible build.

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
- Compile directly with Roslyn.
- Implement `dv build` and `dv run`.
- Support accurate incremental and no-op builds.

The phase succeeds when representative console and library projects can be
restored, built, and run without invoking the .NET CLI or MSBuild.

### Phase 2: Real Repositories

- Add solution and multi-target project support.
- Expand project evaluation and build graph compatibility.
- Add package source configuration, authentication, and conflict diagnostics.
- Implement `dv add`, `dv remove`, and `dv sync`.
- Harden cross-platform behavior and cache concurrency.

### Phase 3: Testing

- Discover test projects and adapters.
- Build and execute tests without VSTest orchestration.
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

Completed foundations now include the compatibility matrix, Roslyn trace,
benchmark fixtures, Rust workspace, structured events, native SDK selection,
and strict evaluation of the initial SDK-style project subset.

1. Discover the selected framework reference pack and build the compiler input
   batch for the small console fixture.
2. Resolve exact package references into a content-addressed cache and initial
   lockfile.
3. Invoke Roslyn through the selected native runtime host.
4. Build the smallest end-to-end path: resolve, compile, cache, and run one
   console application.
