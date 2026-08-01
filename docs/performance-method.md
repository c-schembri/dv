# Performance Method

Performance is a product contract. Measurements begin before implementation so
the project cannot quietly redefine success around the result it happens to
produce.

## Benchmark Contract

Input:

- one immutable fixture directory;
- a named tool and exact argument vector;
- a warm-up count and sample count;
- an isolated mutable workspace;
- recorded OS, architecture, logical CPU count, tool version, and repository
  commit when one exists.

Transform:

1. Build `dv` and run the case-specific parity check outside the timed
   interval. SDK selection must return identical version text. Project
   evaluation must return identical requested properties and item identities.
2. Prepare fixture state outside the timed interval.
3. Launch one process and wait for its terminal status.
4. Validate the case-specific terminal status and output. Success cases reject
   non-zero status; diagnostic cases require the expected failure status and
   semantic diagnostic fields.
5. Record elapsed monotonic nanoseconds.
6. Repeat as a batch, discard warm-ups, sort a copy, and calculate min, median,
   p95, and max while preserving raw samples.

Output:

- schema-22 JSON containing every raw sample, statistic, and explicit
  `measured` or `tbi` status;
- a console table for immediate comparison;
- no benchmark files written into an immutable fixture.

Ownership and lifetime:

- checked-in fixtures are immutable and repository-owned;
- `target/benchmark-work/` is disposable harness-owned state;
- ignored `benchmarks/results/` files are machine-local evidence;
- reviewed baselines under `docs/performance-baselines/` are project records.

Invalid input behavior:

- zero samples, unknown options, unsafe work paths, missing tools, setup
  failures, unexpected command statuses, and malformed counts fail the entire
  run;
- measurements from a failed or partially prepared command are never reported.

## Initial Cases

| Case | State before timed interval | Timed work |
|---|---|---|
| `sdk_current` | no project state | process launch, SDK discovery, and selection |
| `cli_cancellation` | no project state; typed run-boundary deadline preflight outside timing | process launch, Ctrl+C/SIGINT handler installation, SDK discovery, selection, and output |
| `cli_version` | none | `dv` process launch and self-version output |
| `cli_protocol_version` | immutable `small-console` directory; three-native-alias schema preflight outside timing | `dv` process launch and one validated schema-19 protocol-version event batch; Microsoft has no equivalent command and reports TBI |
| `cli_compat_manifest` | embedded manifest; byte-identity and schema/completeness preflight outside timing | `dv` process launch and one 270,150-byte static manifest write; Microsoft has no equivalent query and reports TBI |
| `cli_compat_help` | immutable `small-console` fixture; successful output-shape and zero-mutation preflight outside timing | process launch, profile-aware static help dispatch, output validation, and no SDK/project/filesystem/network discovery |
| `cli_command_normalization` | immutable `small-console` fixture; tree snapshot and typed invalid-option parity preflight outside timing | process launch, lossless command capture, `dotnet restore`/`dv sync` normalization boundary, pre-I/O rejection, and output validation |
| `cli_mode_classification` | immutable `small-console` fixture; tree snapshot and compatibility-profile rejection parity preflight outside timing | process launch, one-pass explicit mode classification, `dotnet`-profile exit selection, stable profile diagnostic context, pre-I/O rejection, and output validation |
| `cli_exit_policy` | immutable `small-console` fixture; tree snapshot and missing-project restore parity preflight outside timing | process launch, project-path discovery failure, typed restore-result classification, one indexed profile-policy lookup, diagnostic output, and status propagation |
| `cli_lexical_preservation` | immutable `small-console` fixture; tree snapshot and combined-value rejection parity preflight outside timing | process launch, exact `-c:Release` recognition, selected profile/platform lexical policy, stable profile diagnostic context, pre-I/O sentinel rejection, and output validation |
| `cli_route_precedence` | immutable `small-console` fixture; tree snapshot and typed `pack` rejection parity preflight outside timing | process launch, profile-aware ambiguous-word routing, pre-I/O pack-option rejection, and output validation |
| `cli_environment` | immutable `small-console` fixture; identical `NO_COLOR`, `DV_COLOR`, and `DV_VERBOSITY` values | process launch, typed environment precedence, pre-I/O unknown-option rejection, ANSI suppression, and secret-free failure output |
| `cli_child_exit` | prebuilt `argument-forwarding` fixture; child-exit parity and typed TBI-boundary preflight outside timing | Microsoft launches the managed child; `dv` captures the declared exit policy and emits its TBI boundary |
| `rid_graph` | selected SDK graph; prebuilt official NuGet oracle adapter | process launch, SDK selection, graph read/parse, breadth-first RID expansion, and text output |
| `project_evaluate` | immutable `small-console` fixture | process launch, project parsing, source discovery, evaluation, and JSON output |
| `runtime_evaluate` | immutable `runtime-project` fixture | process launch, project parsing, compact RID target-dimension materialization, and JSON output |
| `runtime_pack_plan` | restored isolated runtime pack and one validated immutable inventory built during warm-up | process launch, SDK/graph/manifest selection, inventory fingerprint/decode, compact path materialization, and JSON output |
| `runtime_pack_inventory_cold` | restored isolated runtime pack; only the `dv` inventory removed before every iteration | process launch, SDK/graph/manifest selection, validation of 187 runtime assets, apphost selection, binary inventory publication, and JSON output |
| `framework_reference_plan` | restored immutable `framework-reference-project` fixture and installed targeting/shared packs | process launch, project/SDK manifest parsing, two framework and targeting-pack resolutions, installed shared-framework roll-forward, and JSON output |
| `pack_diagnostic` | fresh `unavailable-pack-project` copy, empty local source, empty isolated packages | process launch, SDK/manifest/RID evaluation, missing-pack proof, and actionable failure output |
| `restore_cold` | fresh fixture copy | restore |
| `package_sync_cold` | fresh `package-console` copy, empty isolated packages, reference HTTP cache bypassed | process launch, graph resolution, package download, verification, extraction, and dependency output |
| `package_graph_cold` | fresh `large-package-graph` copy, empty isolated packages, reference HTTP cache bypassed | the same cold transform across a real 50-package closure |
| `package_graph_massive` | fresh `massive-package-graph` copy, empty isolated packages, reference HTTP and audit queries bypassed | a 51-direct-reference, 203-selected-package real-solution workload with package and portable-asset oracle comparison |
| `package_asset_plan` | unchanged `massive-package-graph`, populated isolated packages, matching tool-native lock | process launch, 203-package locked validation, family-partitioned asset-plan materialization, and output |
| `package_rid_content_cold` | four projects and one deterministic local RID/content package; isolated packages, outputs, and locks removed per sample | process launch, SDK RID-graph loading, nearest runtime/resource/native selection, ordered content-rule application, one archive publication, lock write, and output |
| `package_rid_content_warm` | the same four-project oracle with populated isolated packages and matching locks | process launch, semantic RID-fingerprint validation, locked asset/content materialization, and output |
| `package_sync_warm` | unchanged project, populated isolated packages, matching lock | process launch, locked dependency validation, and output |
| `nuspec_framework_metadata` | two deterministic local archives, empty isolated package cache, outputs, and locks | process launch, one-pass manifest parsing, independent nearest dependency/shared-framework/legacy-assembly selection, two-package publication, lock write, and output |
| `package_conflict_resolution` | fifteen deterministic local archives and a populated package cache; restore outputs and locks removed per sample | process launch, nested direct-wins, cousin and diamond convergence, stale-edge retraction, eleven-package materialization, and output |
| `package_diagnostics` | eight deterministic local archives; isolated packages, outputs, and locks removed per sample | process launch, project/config parsing, local discovery and publication, cousin constraint convergence, expected-failure classification, and output |
| `package_batch_resolution` | one root plus two package-bearing references, fifteen deterministic local archives, empty isolated package cache, outputs, and locks | process launch, root-first closure evaluation, two eight-package graphs, one shared metadata session, eight archive publications, three events, and output |
| `nuget_config_hierarchy` | six machine/user/repository configs, populated isolated package cache, matching native lock | process launch, platform config discovery/merge, project evaluation, one-package locked validation, and output |
| `nuget_config_merge` | four machine/user/repository configs with keyed overrides and environment values, populated isolated package cache, matching native lock | process launch, config discovery, keyed merge/expansion, one-package locked validation, and output |
| `nuget_source_sections` | four config levels with package/audit sources, protocols, disabled state, and nested mappings; populated isolated package cache and matching native lock | process launch, typed source-policy merge, mapping construction, one-package locked validation, and output |
| `nuget_source_mapping` | fresh project/config copy, empty isolated package cache, one unreachable v3 source whose only mapping does not match the requested identity | process launch, project/config discovery, cache-miss proof, longest-pattern selection, zero-source proof, and the expected typed failure; source contact is forbidden |
| `nuget_request_budget` | six seeded exact packages, empty isolated cache, two delayed loopback v3 feeds, global limit 4, per-source limit 2 | process launch, project/config discovery, bounded service discovery and archive fetch, integrity/extraction, and asset planning; both peak bounds and six byte-identical published packages are verified |
| `nuget_source_telemetry` | the same cold two-source fixture with an empty cache | the same restore plus source-indexed request, response-byte, duration, and package cache-outcome reporting; aggregate requests/bytes are checked against server observations and reporter output is checked for source locations |
| `nuget_storage_policy` | machine/user/repository policy, fallback-only package, empty global cache, matching native lock, reference HTTP cache bypassed | process launch, typed storage/signature/audit/proxy merge, fallback lookup, one-package locked validation, and output |
| `nuget_cli_overrides` | conflicting implicit/config/environment values, explicit config/source/packages, populated CLI cache, matching native lock | process launch, explicit-config parse, CLI precedence transform, one-package locked validation, and output |
| `nuget_local_sources` | mapped flat and hierarchical local feeds, empty global cache and restore outputs | process launch, local layout discovery, two-package graph resolution, 2,980,145 source bytes, hash/ZIP validation, extraction, atomic publication, and output |
| `nuget_service_index` | one v3 source, prebuilt official NuGet.Protocol oracle, fresh isolated HTTP cache | process launch, project/config discovery, one live HTTPS request, bounded index parse, five capability selections, and output |
| `nuget_credentials` | two v3 sources, one environment override and one config-only credential, prebuilt official NuGet.Configuration oracle | process launch, project/config discovery, credential precedence, sensitive-header materialization, redacted source output, and zero network work |
| `nuget_credential_provider` | one private v3 source, one prebuilt self-contained provider, prebuilt official NuGet.Protocol oracle | process launch, provider launch, symmetric handshake, monitor/initialize/claims/authentication requests, secret-free result, provider close, and zero network work |
| `nuget_client_certificates` | two v3 sources, one PFX and one Windows-store certificate, prebuilt official NuGet.Configuration oracle | process launch, bounded PFX/store selection, source-specific native TLS-client construction, redacted output, and zero network work |
| `nuget_http_policy` | one v3 source, explicit proxy/bypass/rate limit, five enhanced-retry environment values, prebuilt official NuGet.Configuration/Protocol oracle | process launch, project/config discovery, proxy/client construction, compact policy materialization, redacted output, and zero network work |
| `nuget_source_security` | one opted-in HTTP source, one TLS-validation-disabled HTTPS source, one secure HTTPS source, prebuilt official NuGet.Configuration oracle | process launch, project/config discovery, per-source policy materialization, exceptional client construction, redacted output, and zero network work |
| `build_clean` | fresh restored fixture | build |
| `build_noop` | already built fixture | no-op build proof |
| `run_warm` | already built fixture | orchestration and application run |

Cold OS page-cache state is not currently controlled. `restore_cold` means
fresh project state, not a cold machine cache. `package_sync_cold` additionally
removes the isolated package directory for every iteration and passes
`--no-http-cache` to `dotnet restore`. It is the reproducible cold-dependency
boundary. A machine-cold measurement requires a newly provisioned environment;
the harness does not pretend to flush Windows page cache, DNS, TLS, or CDN
state.

`ASSUMPTION: repeated local samples are representative enough to expose gross
startup and orchestration regressions - affects use of this harness for early
directional decisions.`

## Fixture Shapes

| Fixture | Concrete data | Primary question | Status |
|---|---|---|---|
| `small-console` | 1 project, 1 source, 0 packages | fixed startup and no-op cost | executable |
| `argument-forwarding` | 1 `net10.0` executable that reports either exact arguments or one public environment selection plus secret presence | lossless child arguments and ambient/directive/command-line environment precedence | executable through .NET 10; `dv` pre-launch boundaries are typed while child execution is TBI |
| `rid-graph-oracle` | selected SDK graph, official `NuGet.Packaging` parser/expander, `linux-musl-x64` query | graph compatibility parity and one-shot latency | executable for both tools with exact sequence preflight |
| `runtime-project` | 1 project, 1 selected RID, 3 ordered RID expansion values | compact target expansion and selected-index lookup | executable with property parity preflight |
| `runtime-pack-project` | 1 `net10.0` executable, `win-x64`, 172 managed runtime assets, 15 native assets, and 1 apphost template | manifest-driven pack/RID/asset selection | executable for both tools with complete pack and asset parity preflight |
| `framework-reference-project` | 1 `net10.0` executable, implicit Core plus explicit ASP.NET Core, `LatestPatch` | framework/targeting-pack parity and actual host shared-runtime selection | executable for both tools with item and host-launch parity preflight |
| `unavailable-pack-project` | 1 self-contained `net10.0` executable, SDK-known `linux-arm`, empty local source and isolated package cache | deterministic missing runtime-pack identity and acquisition guidance | executable for both tools with expected-failure preflight |
| `multi-project` | 3 projects, 3 edges, shared dependency | discovery, graph ordering, invalidation | checked in |
| `large-package-graph` | 1 project, 1 direct reference, 50 resolved packages, 3,241,550 payload bytes | streaming dependency scheduling and many-small-archive publication | executable |
| `massive-package-graph` | union of 51 direct eShop references, 203 selected packages, 272 reference archives, 197,860,237 reference payload bytes | real-solution restore scale, range convergence, asset diversity, and network throughput | executable for both tools with package/asset parity preflight |
| `package-rid-content` | 4 `net10.0` projects covering portable, exact Windows/Linux, and Windows fallback; 1 generated archive with portable and RID runtime/resource/native assets plus ordered content rules | concrete RID fallback, runtime-target retention, content metadata, and cold/warm lock parity | executable for both tools with complete `project.assets.json` family/metadata preflight |
| `package-conflict-resolution` | 1 project, 15 local archives, 11 selected identities, one nested downgrade, different-depth and alternate-root cousin convergence, and one retracted stale edge | advanced NuGet graph conflict selection without network variance | executable for both tools with exact version-batch preflight |
| `package-conflict-resolution` diagnostic projects | 5 projects, 8 local archives, exact cousin conflict, cycle, absent identity, absent version, and incompatible TFM | stable structured failure categories and cold diagnostic latency without network variance | executable for both tools with six Microsoft/`dv` diagnostic-category pairs and cold/warm warning preflight |
| `package-conflict-resolution` batch projects | 1 root, 2 project-reference children, 15 available local archives, 8 selected identities per child | project-closure ordering plus command-local metadata/download deduplication | executable for both tools with exact child graph parity, 16-row/eight-publication evidence, and zero-request preflight |
| `nuspec-framework-metadata` | 1 `net10.0` timed project, 1 `net48` legacy oracle project, 2 local archives, conflicting dependency/framework groups, 4 legacy assembly rows | isolated nuspec-container parsing, nearest target selection, legacy `Any` fallback, and warm-lock preservation | executable for both tools with Microsoft `project.assets.json`, cold/warm `dv` event, schema-8 lock, two-publication, and zero-request preflight |
| `nuget-config-merge` | 1 project, 4 config levels, 1 enabled and 1 disabled final source, 1 package | keyed precedence, clear/remove, disabled membership, environment expansion | executable for both tools with source/cache/package parity preflight |
| `nuget-source-sections` | 1 project, 4 config levels, 2 package sources, 1 audit source, 2 final mapping groups, 1 package | typed source/protocol precedence and nested longest-pattern mapping | executable for both tools with official `NuGet.Configuration` and package parity preflight |
| `nuget-source-mapping` | 1 `net10.0` project, 1 exact package request, empty cache, 1 unreachable v3 source, and 1 nonmatching mapping pattern | mapping-before-discovery behavior and typed unmapped failure | executable for both tools with expected-failure, diagnostic, and zero-request preflight |
| `nuget-request-budget` | 1 `net10.0` project, 6 dependency-free exact packages, 2 delayed loopback v3 feeds, 4 global and 2 per-source active requests | deterministic request backpressure, cold restore throughput, and source telemetry | executable for both tools with upper-bound, package-count, source-contact, server-observed telemetry, and credential-free output validation |
| `nuget-storage-policy` | 1 project, 3 config levels, 1 fallback-only package, isolated global/HTTP/scratch roots | storage precedence, fallback consumption, typed signature/audit policy, and proxy redaction | executable for both tools with official NuGet/MSBuild and package parity preflight |
| `nuget-local-sources` | 1 project, 2 mapped local feeds, 1 flat package, 1 hierarchical package, 2,980,145 archive bytes | offline layout detection, local range/exact lookup, integrity validation, and cold cache publication | executable for both tools with source/package/hash parity preflight |
| `nuget-service-index` | 1 project, 1 v3 source, 40 resource rows, 31 distinct types, 5 capability families, 9,272 response bytes | official resource preference, client-version compatibility, mirror retention, and live request latency | executable for both tools with exact SDK-shipped NuGet.Protocol endpoint parity preflight |
| `nuget-credentials` | 1 project, 2 HTTPS v3 sources, 1 environment credential override, 1 config-only PAT, 6 secret/decoy strings | NuGet-compatible credential selection, Basic policy, redacted output, and offline setup latency | executable for both tools with official NuGet.Configuration selection and plaintext-containment preflight |
| `nuget-credential-provider` | 1 project, 1 HTTPS v3 source, 1 self-contained provider, 1 Basic credential, 2 secret strings | NuGet V2 handshake/lifecycle, authentication claim, noninteractive flags, timeout cancellation, redacted output, and offline probe latency | executable for both tools with official NuGet.Protocol plugin manager, trace-policy, timeout, and plaintext-containment preflight |
| `nuget-client-certificates` | 1 project, 2 HTTPS v3 sources, 1 relative PFX, 1 `CurrentUser\\My` thumbprint binding | bounded certificate loading, private-key selection, native TLS-client construction, source containment, and redacted output | executable for both tools with official NuGet.Configuration selection and zero-network preflight |
| `nuget-http-policy` | 1 project, 1 HTTPS v3 source, proxy/bypass configuration, per-source limit 7, custom enhanced retry values | proxy and policy selection, secure transport invariants, offline suppression, and setup latency | executable for both tools with exact SDK-shipped NuGet.Configuration/Protocol policy parity preflight |
| `nuget-source-security` | 1 project, 3 v3 sources covering opted-in HTTP, disabled TLS validation, and secure defaults | source-local exception containment, explicit risk reporting, and offline setup latency | executable for both tools with exact SDK-shipped NuGet.Configuration source-policy parity preflight |
| `large-solution` | many projects with shared dependency layers | memory scaling and parallel scheduling | package workload captured; project-graph shape still pending |
| `test-heavy` | many test cases and adapter metadata | discovery and execution overhead | specification pending real sample |
| `multiple-sources` | public, private, and local package sources | auth, concurrency, cache behavior | specification pending sanitized sample |

The last three fixtures are not fabricated. Their exact counts and
distributions remain unresolved until representative repositories can be
sampled; see `issues/`.

## Measurement Rules

- Compare tools and revisions on the same machine, power mode, fixture, command,
  sample count, and cache state.
- Measure debug correctness separately from optimized release performance.
- Record latency and throughput separately.
- Record peak memory, bytes read/written, process count, network requests,
  allocation count, and CPU utilization as soon as those workflows exist.
- Record typed request and downloaded-payload counts when the measured command
  exposes them. Do not infer missing reference-tool counters from console text.
- Never time fixture copying or prerequisite restore when the named case is
  build latency.
- Keep warm-up output out of the raw sample batch.
- Investigate distributions and outliers; do not select the most flattering
  statistic.
- Re-sample live inputs when optimization plateaus. A different distribution
  may require a different representation or algorithm.

## Commands

Quick smoke run:

```text
cargo bench-all --quick
```

Standard reference run:

```text
cargo bench-all
```

This builds the release `dv` executable outside the timed interval, measures
all implemented `dotnet` and `dv` cases, and prints `TBI` for `dv` cases that
are not implemented. Supply a different `dv` executable when needed:

```text
cargo bench-all --dv target/release/dv
```

Measure only SDK selection:

```text
cargo bench-all --case sdk_current --dv target/release/dv
```

Measure cancellation-ready startup and SDK selection:

```text
cargo bench-all --case cli_cancellation --samples 30 --warmups 5
```

Measure invocation environment precedence and redaction through identical
reference/dv process inputs:

```text
cargo bench-all --case cli_environment --samples 30 --warmups 3
```

Measure accepted command-spelling normalization through the restore/sync
pre-I/O boundary:

```text
cargo bench-all --case cli_command_normalization --samples 30 --warmups 5
cargo bench-all --case cli_compat_help --samples 50 --warmups 10
cargo bench-all --case cli_mode_classification --samples 50 --warmups 10
cargo bench-all --case cli_exit_policy --samples 50 --warmups 10
cargo bench-all --case cli_lexical_preservation --samples 50 --warmups 10
cargo bench-all --case cli_route_precedence --samples 50 --warmups 10
```

Measure the independently versioned command-syntax and JSON protocol query:

```text
cargo bench-all --case cli_protocol_version --samples 30 --warmups 5
```

Measure the structural child-exit boundary. This is not like-for-like until
`dv run` launches the managed application:

```text
cargo bench-all --case cli_child_exit --samples 30 --warmups 5
```

Measure SDK-owned portable RID expansion:

```text
cargo bench-all --case rid_graph --samples 30 --warmups 3
```

Measure only like-for-like project evaluation:

```text
cargo bench-all --case project_evaluate --samples 30 --warmups 3
```

Measure runtime target-dimension evaluation:

```text
cargo bench-all --case runtime_evaluate --samples 30 --warmups 3
```

Measure runtime, host, native-asset, and apphost planning:

```text
cargo bench-all --case runtime_pack_plan --samples 30 --warmups 3
```

Measure cold immutable inventory construction while keeping restored package
contents outside timing:

```text
cargo bench-all --case runtime_pack_inventory_cold --samples 30 --warmups 3
```

Measure framework references, targeting packs, and shared-runtime roll-forward:

```text
cargo bench-all --case framework_reference_plan --samples 30 --warmups 3
```

Measure deterministic unavailable-pack diagnosis without network variance:

```text
cargo bench-all --case pack_diagnostic --samples 30 --warmups 3
```

Measure platform NuGet configuration discovery and merge through a warm,
one-package locked restore:

```text
cargo bench-all --case nuget_config_hierarchy --samples 30 --warmups 3
```

Measure keyed NuGet config merge and environment expansion through the same
warm, one-package locked boundary:

```text
cargo bench-all --case nuget_config_merge --samples 30 --warmups 3
```

Measure package/audit source sections, protocol metadata, and mapping-policy
construction through the warm locked boundary:

```text
cargo bench-all --case nuget_source_sections --samples 30 --warmups 3
```

Measure unmapped-identity diagnosis before any service-index work. Both tools
must reject the same package, and the unreachable feed turns an accidental
request into a failed preflight rather than a misleading timing:

```text
cargo bench-all --case nuget_source_mapping --samples 30 --warmups 3
```

Measure global and per-source request backpressure against two deterministic
delayed feeds. Package seeding and public network work remain outside timing:

```text
cargo bench-all --case nuget_request_budget --samples 30 --warmups 3
```

Measure global/HTTP/scratch path precedence, fallback consumption, signature
and audit policy, and proxy construction with secret-free reporting through
the warm locked boundary:

```text
cargo bench-all --case nuget_storage_policy --samples 30 --warmups 3
```

Measure source replacement, explicit-config isolation, and CLI package-folder
precedence through the same warm locked boundary:

```text
cargo bench-all --case nuget_cli_overrides --samples 30 --warmups 3
```

Measure cold local-feed discovery and publication from flat and hierarchical
layouts with all network work disabled:

```text
cargo bench-all --case nuget_local_sources --samples 30 --warmups 3
```

Measure stable package conflict diagnosis. Both tools begin every sample with
empty package and output state, fail on the same local exact-version conflict,
and must pass the category/field parity gate before timing:

```text
cargo bench-all --case package_diagnostics --samples 30 --warmups 3
```

Measure a cold project-reference package batch. Both tools evaluate the same
root and two children; the harness requires exact child graphs and one archive
publication per unique package before timing:

```text
cargo bench-all --case package_batch_resolution --samples 30 --warmups 3
```

Measure isolated nuspec dependency, shared-framework, and legacy-assembly
group selection. Timed samples use the latest stable target; preflight also
compares `net48` behavior and a warm native lock with Microsoft:

```text
cargo bench-all --case nuspec_framework_metadata --samples 30 --warmups 3
```

Measure one live, uncached v3 service-index request and exact capability
selection against the official NuGet.Protocol implementation:

```text
cargo bench-all --case nuget_service_index --samples 30 --warmups 3
```

Measure config/environment Basic credential selection and secret-free source
reporting without network variance:

```text
cargo bench-all --case nuget_credentials --samples 30 --warmups 3
```

Measure the same self-contained V2 provider lifecycle through Microsoft's
official plugin manager and `dv`, without network variance:

```text
cargo bench-all --case nuget_credential_provider --samples 30 --warmups 3
```

Measure explicit HTTP and TLS-validation source flags against the selected
SDK's official configuration implementation, with network work disabled:

```text
cargo bench-all --case nuget_source_security --samples 30 --warmups 3
```

Measure first dependency readiness with a fresh package cache and no NuGet HTTP
cache reuse:

```text
cargo bench-all --case package_sync_cold --samples 30 --warmups 3
```

Measure the real 50-package graph under the same cold boundary:

```text
cargo bench-all --case package_graph_cold --samples 10 --warmups 2
```

Measure the massive eShop-derived acceptance graph:

```text
cargo bench-all --case package_graph_massive --samples 5 --warmups 1
```

Measure family-partitioned planning over that graph from matching locks and
populated isolated caches:

```text
cargo bench-all --case package_asset_plan --samples 30 --warmups 3
```

Measure concrete RID selection and content metadata from a cold local package,
then from matching native locks:

```text
cargo bench-all --case package_rid_content_cold --samples 30 --warmups 3
cargo bench-all --case package_rid_content_warm --samples 30 --warmups 3
```
