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

- schema-17 JSON containing every raw sample, statistic, and explicit
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
| `cli_version` | none | `dv` process launch and self-version output |
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
| `package_sync_warm` | unchanged project, populated isolated packages, matching lock | process launch, locked dependency validation, and output |
| `nuget_config_hierarchy` | six machine/user/repository configs, populated isolated package cache, matching native lock | process launch, platform config discovery/merge, project evaluation, one-package locked validation, and output |
| `nuget_config_merge` | four machine/user/repository configs with keyed overrides and environment values, populated isolated package cache, matching native lock | process launch, config discovery, keyed merge/expansion, one-package locked validation, and output |
| `nuget_source_sections` | four config levels with package/audit sources, protocols, disabled state, and nested mappings; populated isolated package cache and matching native lock | process launch, typed source-policy merge, mapping construction, one-package locked validation, and output |
| `nuget_source_mapping` | fresh project/config copy, empty isolated package cache, one unreachable v3 source whose only mapping does not match the requested identity | process launch, project/config discovery, cache-miss proof, longest-pattern selection, zero-source proof, and the expected typed failure; source contact is forbidden |
| `nuget_request_budget` | six seeded exact packages, empty isolated cache, two delayed loopback v3 feeds, global limit 4, per-source limit 2 | process launch, project/config discovery, bounded service discovery and archive fetch, integrity/extraction, and asset planning; both peak bounds and six byte-identical published packages are verified |
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
| `rid-graph-oracle` | selected SDK graph, official `NuGet.Packaging` parser/expander, `linux-musl-x64` query | graph compatibility parity and one-shot latency | executable for both tools with exact sequence preflight |
| `runtime-project` | 1 project, 1 selected RID, 3 ordered RID expansion values | compact target expansion and selected-index lookup | executable with property parity preflight |
| `runtime-pack-project` | 1 `net10.0` executable, `win-x64`, 172 managed runtime assets, 15 native assets, and 1 apphost template | manifest-driven pack/RID/asset selection | executable for both tools with complete pack and asset parity preflight |
| `framework-reference-project` | 1 `net10.0` executable, implicit Core plus explicit ASP.NET Core, `LatestPatch` | framework/targeting-pack parity and actual host shared-runtime selection | executable for both tools with item and host-launch parity preflight |
| `unavailable-pack-project` | 1 self-contained `net10.0` executable, SDK-known `linux-arm`, empty local source and isolated package cache | deterministic missing runtime-pack identity and acquisition guidance | executable for both tools with expected-failure preflight |
| `multi-project` | 3 projects, 3 edges, shared dependency | discovery, graph ordering, invalidation | checked in |
| `large-package-graph` | 1 project, 1 direct reference, 50 resolved packages, 3,241,550 payload bytes | streaming dependency scheduling and many-small-archive publication | executable |
| `massive-package-graph` | union of 51 direct eShop references, 203 selected packages, 272 reference archives, 197,860,237 reference payload bytes | real-solution restore scale, range convergence, asset diversity, and network throughput | executable for both tools with package/asset parity preflight |
| `nuget-config-merge` | 1 project, 4 config levels, 1 enabled and 1 disabled final source, 1 package | keyed precedence, clear/remove, disabled membership, environment expansion | executable for both tools with source/cache/package parity preflight |
| `nuget-source-sections` | 1 project, 4 config levels, 2 package sources, 1 audit source, 2 final mapping groups, 1 package | typed source/protocol precedence and nested longest-pattern mapping | executable for both tools with official `NuGet.Configuration` and package parity preflight |
| `nuget-source-mapping` | 1 `net10.0` project, 1 exact package request, empty cache, 1 unreachable v3 source, and 1 nonmatching mapping pattern | mapping-before-discovery behavior and typed unmapped failure | executable for both tools with expected-failure, diagnostic, and zero-request preflight |
| `nuget-request-budget` | 1 `net10.0` project, 6 dependency-free exact packages, 2 delayed loopback v3 feeds, 4 global and 2 per-source active requests | deterministic request backpressure and cold restore throughput | executable for both tools with upper-bound, package-count, and source-contact validation |
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
