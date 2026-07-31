# Feature Parity Implementation Map

This document maps the work required to satisfy the product goal in `PLAN.md`:
replace the practical C# and .NET workflows normally orchestrated by the .NET
CLI, MSBuild, NuGet, and test tooling, without using those tools as production
fallbacks.

It is a capability map, not a schedule. A checked item is present in the
repository at the snapshot below. An unchecked item is required unless it is
explicitly marked `DEFER` or `REJECT`.

Snapshot: working tree on 2026-07-31, based on commit `e171e92`.

## Scope Contract

Feature parity has three rings:

1. **Required product parity:** the workflows named in `PLAN.md`: create,
   restore/sync, build, run, test, package management, pack, publish, and
   SDK/runtime management for existing SDK-style projects and solutions.
2. **Practical adjacent parity:** clean, solution/reference editing, package
   search and source management, watch, shell integration, and CI-facing
   behavior that real repositories need around the required workflows.
3. **Explicit non-goals:** arbitrary MSBuild execution, identical console
   prose, Visual Studio UI behavior, reimplementing Roslyn or CoreCLR, and
   silently approximating unsupported project behavior.

This is therefore not literal parity with every command shipped under the
`dotnet` driver. Literal parity would include global tools, workloads,
`dotnet msbuild`, build-server control, runtime stores, `dev-certs`,
`user-secrets`, and product-specific tools such as Entity Framework. Those
surfaces are recorded separately at the end so the scope cannot drift
silently.

## Status And Delivery Labels

- `[x]` implemented and covered by repository tests.
- `[~]` partial foundation exists, but the parity contract is incomplete.
- `[ ]` not implemented.
- `P1` fast inner-loop vertical slice.
- `P2` real repository compatibility.
- `P3` testing.
- `P4` distribution and SDK/runtime acquisition.
- `P5` broad compatibility.
- `ADJ` practical adjacent parity.
- `DEFER` not required by the current product contract.
- `REJECT` must produce a stable diagnostic rather than approximate behavior.

No percentage-complete value is assigned. The items differ radically in cost
and risk, so a count-based percentage would be misleading.

## Real Data At This Snapshot

### Inputs

Repository-owned representative input currently consists of:

- five SDK-style C# projects;
- five C# source files;
- three project-reference edges;
- one exact package reference;
- two single-project console fixtures and one three-project acyclic fixture;
- one observed Windows machine with three x64 SDK installations and 15 x64
  shared-runtime directories.

The initial compiler trace in `docs/roslyn-invocation.md` observed SDK
`10.0.100`, 164 reference arguments, eight analyzer/generator arguments, three
analyzer-config arguments, four C# inputs, a 4,608-byte assembly, an
11,340-byte PDB, a 156,160-byte apphost, a 428-byte dependency manifest, and a
268-byte runtime configuration.

The Rust workspace currently has 12 Rust source files, 7,617 nonblank source
lines, and 42 `#[test]` functions. These counts describe the current
repository, not the expected shape of real customer repositories.

### Outputs

Current output is command-local human text or a schema-v1 JSON-lines event
batch. Future workflows must additionally own:

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

The observed common case is currently tiny, package-free, SDK-style C# on
Windows. It cannot justify large-repository layouts or concurrency thresholds.

`ASSUMPTION: SDK-style C# projects using PackageReference are the dominant
initial customer input - affects language, evaluator, and resolver sequencing.`

`ASSUMPTION: repeated no-op build and test commands are the highest-value
latency paths - affects fingerprint and daemon prioritization.`

`ASSUMPTION: a useful first compatibility boundary can reject arbitrary custom
tasks while accepting declarative properties, items, and imports that only
affect known transforms - affects the evaluator and extension model.`

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

1. argument and environment reads;
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
| Installed SDK discovery | Implemented | `crates/dv-core/src/sdk.rs` |
| `global.json` SDK selection | Implemented | policy and fixture tests |
| SDK current/list human output | Implemented | CLI integration tests |
| Versioned JSON event stream | Implemented | event/reporter unit tests |
| Structured diagnostics | Foundation only | codes and ordered fields exist |
| Benchmark process harness | Foundation only | startup, SDK, and project-evaluation cases measured |
| Single C# project discovery | Initial subset | explicit/one-directory selection and ambiguity diagnostics |
| Project evaluation | Initial subset | parsed single modern .NET TFM, base-SDK properties/items, and `project inspect` |
| Compiler input planning | Initial subset | target-selected reference pack, Roslyn/analyzers, options, packages, and `build --plan` |
| Package resolution and cache | Initial subset | exact versions, NuGet v2/v3 HTTPS, verified atomic package cache, deterministic dv lock, and identical `restore`/`sync` commands |
| Solution discovery and evaluation | Missing | no production types or commands |
| Restore, build, run, test | Missing | commands return `DV0003` |
| Pack, publish, SDK/runtime install | Missing | commands return `DV0003` |

## Dependency Spine

The shortest dependency-respecting route to useful parity is:

1. compatibility evidence and fixture expansion;
2. command model and workspace selection;
3. project/solution parsing;
4. bounded declarative evaluation;
5. framework, targeting-pack, and runtime-pack resolution;
6. NuGet configuration, source access, package graph, cache, and lock state;
7. target-expanded build graph;
8. generated sources and compiler input batches;
9. native Roslyn hosting and post-compile artifacts;
10. incremental proof and deterministic scheduling;
11. runtime launch;
12. test protocol and adapters;
13. pack and publish;
14. SDK/runtime acquisition;
15. broader SDK, language, framework, and extension compatibility.

A later stage may be prototyped against captured compatibility data, but it
cannot be declared complete before its prerequisites have stable typed
contracts.

## 1. Command And Process Contract

- [x] `CLI-001` Parse help and self-version without project or SDK discovery.
- [x] `CLI-002` Reject non-Unicode command text with a stable diagnostic.
- [x] `CLI-003` Emit stable exit code 2 for current command failures.
- [x] `CLI-004` Offer one `--json` event stream for current commands.
- [ ] `CLI-005` Replace string matching with a typed, batch-first command
  request that retains lossless OS arguments where paths require it. `P1`
- [ ] `CLI-006` Define global `--help`, `--version`, `--json`, `--verbose`,
  `--quiet`, `--color`, `--no-color`, and diagnostic verbosity behavior. `P1`
- [ ] `CLI-007` Define stable exit-code classes for usage, compatibility,
  restore, build, test failure, cancellation, and internal failure. `P1`
- [ ] `CLI-008` Support `--project`, explicit project/solution paths, and
  unambiguous current-directory defaults. `P1`
- [ ] `CLI-009` Support repeated CLI property overrides without reparsing
  strings in downstream stages. `P2`
- [ ] `CLI-010` Support configuration, framework, runtime, architecture,
  operating-system, output, artifacts-path, and no-restore selectors
  consistently across applicable commands. `P2`
- [ ] `CLI-011` Reject unknown options before filesystem or network work. `P1`
- [ ] `CLI-012` Preserve arguments after `--` byte-for-byte for child
  application and test processes. `P1`
- [ ] `CLI-013` Define environment-variable precedence and redact secrets from
  all output modes. `P1`
- [ ] `CLI-014` Install Ctrl+C/SIGINT cancellation before starting work and
  propagate a bounded cancellation deadline to children. `P1`
- [ ] `CLI-015` Preserve child exit codes where the command contract requires
  it and distinguish launch failure from child failure. `P1`
- [ ] `CLI-016` Add response-file support only if representative repositories
  require it; otherwise reject `@file` explicitly. `P5`
- [ ] `CLI-017` Version command syntax and JSON compatibility independently so
  a CLI alias does not mutate the event protocol. `P1`
- [x] `CLI-018` Expose the initial evaluator through human and JSON
  `project inspect` output.

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
  edits for `.sln` and structured edits for `.slnx`. `ADJ`
- [ ] `SLN-010` Support solution-folder placement, root placement, globbed
  adds, duplicate suppression, and `.sln` to `.slnx` migration. `ADJ`
- [ ] `SLN-011` Compare parsed membership and configuration selection with the
  reference tool on representative solutions. `P2`

## 4. Declarative Project Evaluation

The evaluator is not a general MSBuild engine. It must implement the subset
needed to produce known typed transforms, and reject any construct that can
change a supported output but is not understood.

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
- [ ] `EVAL-018` Never execute `Target`, `Task`, `UsingTask`, or `Exec` while
  evaluating a build plan. `REJECT`
- [ ] `EVAL-019` Translate a finite allowlist of SDK targets/tasks into native
  transforms only after compatibility fixtures establish their data contract.
  `P2`
- [ ] `EVAL-020` Report unsupported custom targets/imports with file, line,
  condition, affected output, and supported alternative when known. `P1`
- [ ] `EVAL-021` Evaluate outer and inner builds for `TargetFrameworks`,
  including target-specific conditional properties/items. `P2`
- [ ] `EVAL-022` Evaluate `RuntimeIdentifier` and `RuntimeIdentifiers` as
  target expansion dimensions rather than repeated project objects. `P2`
- [ ] `EVAL-023` Support `Debug` and `Release` defaults plus arbitrary named
  configurations whose values are fully declarative. `P1`
- [ ] `EVAL-024` Parse and apply `Directory.Build.rsp` or reject it explicitly
  based on the selected compatibility boundary. `P5`
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
- [ ] `PROJ-020` Reject legacy non-SDK projects until a separate compatibility
  contract and fixtures exist. `REJECT`

## 6. Framework, Runtime, And Pack Resolution

- [~] `PACKS-001` Inventory installed targeting, runtime, host, apphost,
  analyzer, and workload packs from the selected SDK/root. `P1`
- [~] `PACKS-002` Parse pack manifests and versions rather than hard-code the
  observed SDK layout. `P1`
- [~] `PACKS-003` Select the correct reference pack for a TFM and fail on
  missing or unsupported packs before compiler launch. `P1`
- [x] `PACKS-004` Produce an ordered reference-assembly range from the selected
  pack with stable path indices. `P1`
- [ ] `PACKS-005` Load the SDK's portable RID graph as data; never infer RID
  compatibility by splitting the RID string. `P2`
- [ ] `PACKS-006` Select runtime packs, host packs, native assets, and apphost
  templates for requested RID and architecture. `P2`
- [ ] `PACKS-007` Resolve framework references and shared-framework versions,
  including runtime roll-forward policy. `P2`
- [~] `PACKS-008` Separate compile, runtime, native, resource, analyzer, and
  build assets in the plan. `P1`
- [~] `PACKS-009` Diagnose unavailable TFM/RID/platform combinations with the
  required pack identity and acquisition action. `P1`
- [ ] `PACKS-010` Cache immutable SDK pack inventories by selected SDK
  fingerprint and invalidate on installation changes. `P2`

## 7. NuGet Configuration, Sources, And Authentication

- [~] `NUGET-001` Discover machine, user, drive, repository, and explicit
  `NuGet.Config` files with platform-correct precedence. `P1`
- [~] `NUGET-002` Merge keyed sections with `<clear>`, add, remove, disabled
  sources, and environment-variable expansion. `P1`
- [~] `NUGET-003` Support `packageSources`, `disabledPackageSources`,
  `packageSourceMapping`, `auditSources`, and source protocol version. `P2`
- [~] `NUGET-004` Support global-packages, HTTP cache, temp, fallback folders,
  signature-validation mode, restore audit mode/level, and proxy settings.
  `P2`
- [~] `NUGET-005` Accept CLI source/config/packages-folder overrides with
  documented precedence. `P1`
- [~] `NUGET-006` Support local folder sources and NuGet v2/v3 HTTP service
  contracts. `P1`
- [~] `NUGET-007` Resolve registration, flat-container, search, vulnerability,
  and package-publish endpoints from service-index resources. `P2`
- [ ] `NUGET-008` Support Basic/PAT credentials from config and environment
  without persisting or reporting plaintext. `P2`
- [ ] `NUGET-009` Define a credential-provider protocol for private feeds,
  interactive login, cancellation, timeout, and noninteractive CI. `P2`
- [ ] `NUGET-010` Support client certificates and platform certificate stores
  only after authenticated-source fixtures exist. `P5`
- [~] `NUGET-011` Honor proxy, `NO_PROXY`, TLS validation, redirect, retry,
  timeout, rate-limit, and offline behavior. `P2`
- [ ] `NUGET-012` Require explicit opt-in for insecure HTTP or disabled TLS
  validation and surface the security consequence. `P2`
- [ ] `NUGET-013` Apply package source mapping before network requests and
  diagnose unmapped identities. `P2`
- [~] `NUGET-014` Bound concurrent requests per source and globally; implement
  backpressure and deterministic result merge. `P2`
- [~] `NUGET-015` Record request count, bytes, cache outcome, and source timing
  without recording credentials. `P2`

## 8. Package Resolution, Assets, Cache, And Locking

- [x] `RES-001` Parse NuGet package identities case-insensitively while
  preserving display casing. `P1`
- [~] `RES-002` Implement NuGet SemVer 2 precedence, normalized versions,
  prerelease identifiers, build metadata, ranges, and floating versions. `P1`
- [x] `RES-003` Parse `PackageReference` version and metadata from attributes
  or child elements. `P1`
- [ ] `RES-004` Support `IncludeAssets`, `ExcludeAssets`, `PrivateAssets`,
  `NoWarn`, `Aliases`, and `GeneratePathProperty`. `P2`
- [ ] `RES-005` Support conditional references per TFM/RID/configuration. `P2`
- [ ] `RES-006` Read `Directory.Packages.props` and implement central package
  versions, version overrides, global references, and transitive pinning. `P2`
- [~] `RES-007` Implement lowest-applicable-version, floating-version,
  direct-dependency-wins, and cousin-dependency rules. `P1`
- [~] `RES-008` Emit stable downgrade, constraint conflict, cycle, missing
  package/version, and incompatible-framework diagnostics. `P1`
- [ ] `RES-009` Resolve all projects/targets as a batch so shared metadata and
  downloads are deduplicated. `P2`
- [~] `RES-010` Parse `.nuspec` dependency groups and framework assemblies.
  `P1`
- [~] `RES-011` Select `ref`, `lib`, `runtimes`, native, resource,
  `contentFiles`, analyzer, `build`, `buildMultiTargeting`, and
  `buildTransitive` assets by TFM/RID and metadata. `P2`
- [~] `RES-012` Implement compatible-framework reduction and framework
  fallback rules using SDK-owned framework data. `P2`
- [x] `RES-013` Download `.nupkg` and hash metadata concurrently into bounded
  temporary storage. `P1`
- [x] `RES-014` Verify package identity, version, SHA-512, ZIP structure,
  duplicate paths, traversal paths, entry sizes, and total expansion limits
  before cache commit. `P1`
- [ ] `RES-015` Verify author/repository signatures and trusted-signers policy
  with platform-correct certificate roots. `P2`
- [~] `RES-016` Extract atomically into a NuGet-compatible global-packages
  layout with per-package concurrency coordination. `P1`
- [~] `RES-017` Reuse the existing global package and HTTP caches when valid;
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
  outdated, deprecated, and vulnerable package listing. `ADJ`
- [ ] `RES-026` Add cache list/path/clear/prune operations with safe ownership
  checks and concurrent-reader behavior. `ADJ`
- [ ] `RES-027` Reject `packages.config` until a separate legacy restore
  contract exists. `REJECT`

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
  and machine-readable result support. `ADJ`
- [ ] `EDIT-010` Add/list/remove project references with relative path,
  framework condition, duplicate, cycle, and compatibility checks. `ADJ`
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
  servers semantics without delegating. `ADJ`
- [ ] `GRAPH-012` Record per-stage input/output byte counts, process count,
  allocations, CPU work, and elapsed time at batch granularity. `P1`

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
  invalidation, restart, and hot-reload protocol as adjacent parity. `ADJ`

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
  with registry auth, deterministic layers, and signed metadata. `DEFER`
- [ ] `PUB-017` Add NuGet package push with API key/credential provider,
  duplicate policy, timeout, retries, symbols source, and secret redaction.
  `P4`
- [ ] `PUB-018` Add package delete/unlist only as an explicit destructive
  command with source capability checks. `ADJ`

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
- [ ] `SDK-007` Inventory shared frameworks, hostfxr, hostpolicy, architecture,
  RID, and install provenance. `P1`
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
  typed command model. `ADJ`
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
  programs. `DEFER`
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
- [ ] `GATE-004` Add package-bearing small fixture, conditional project,
  analyzer/generator fixture, resources/content fixture, and failure corpus.
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

## Simplification Pass

This mapping deliberately removes the following machinery:

- no general MSBuild interpreter or arbitrary task runner;
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

The primary fallback is explicit rejection with a stable diagnostic, captured
unsupported evidence, and a supported alternative when one exists.

## Phase Completion Gates

### P1: Fast Inner Loop

Complete only when the package-free and package-bearing console/library
fixtures can sync, build, no-op build, incrementally rebuild, and run without a
production `dotnet`, MSBuild, NuGet, or VSTest process. Evaluated inputs,
compiler batches, artifacts, and observable execution must match the oracle
contract.

### P2: Real Repositories

Complete only when `.sln`/`.slnx`, multi-target projects, package sources,
central package management, private authentication, package editing, concurrent
cache use, and cross-platform output pass representative fixtures and failure
injection.

### P3: Testing

Complete only when supported VSTest-adapter and MTP repositories have equivalent
discovery, filtering, execution, result, attachment, cancellation, and exit
behavior, with bounded memory and measured scheduling.

### P4: Distribution

Complete only when pack, app publish, package push, and SDK/runtime acquisition
produce verified contents, survive interruption, redact secrets, and have
cold/warm resource evidence.

### P5: Broad Compatibility

Complete per SDK/language/workload row, never as a blanket claim. Each row needs
its own input contract, rejection boundary, oracle corpus, artifacts, failures,
and benchmark evidence.

## Evidence That Would Disprove This Map

- Real target repositories depend predominantly on an omitted workflow.
- Common projects require arbitrary target/task execution that cannot be
  represented as typed native transforms.
- IDE/tool interoperability requires NuGet/MSBuild intermediate files that the
  current boundaries treat as optional.
- Direct Roslyn/runtime hosting cannot preserve compiler/runtime compatibility
  without unacceptable process or protocol cost.
- The isolated compiler host, content-addressed cache, or fingerprint layers
  cost more than the work they avoid on representative batches.

Any of these requires revising the scope or transform, not silently adding a
fallback.

## Adjacent `dotnet` Surface Not Required By The Current Goal

The official .NET CLI also exposes these command families. They are not
required for the scope defined at the top, but must be reconsidered if
"feature parity" is later changed to mean literal `dotnet` driver parity:

- `dotnet build-server`;
- `dotnet msbuild`;
- `dotnet store`;
- global/local/tool-path tool install, update, uninstall, list, and search;
- workload clean, config, history, install, list, repair, restore, search,
  uninstall, and update;
- SDK-bundled `dev-certs`, `user-secrets`, `watch`, and product-specific tools
  such as `dotnet ef`;
- generic execution options for arbitrary managed applications;
- NuGet signing, verification, trusted-signer editing, and server
  administration beyond package push/delete;
- Visual Studio project-system/design-time build and IDE UI behavior.

`dv watch` is already retained as practical adjacent parity because it directly
serves the inner loop. The others need explicit product value, representative
data, and a separate subsystem contract before entering required scope.

## Primary Compatibility References

- [The `dotnet` command and command families](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet)
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
