# dv

[![CI](https://github.com/c-schembri/dv/actions/workflows/ci.yml/badge.svg)](https://github.com/c-schembri/dv/actions/workflows/ci.yml)

`dv` is a native, data-oriented toolchain for C# and .NET development, written
in Rust.

The goal is one fast executable for SDK selection, dependency resolution,
project evaluation, building, running, testing, packaging, and publishing.
Roslyn remains the compiler and Microsoft .NET remains the runtime; `dv` owns
the expensive orchestration around them.

```text
dotnet --version               63.347 ms median
dv sdk current                  4.501 ms median

dotnet NuGet RID expansion     36.217 ms median
dv sdk compatible-rids          6.049 ms median

dotnet msbuild project query  282.186 ms median
dv project inspect              3.846 ms median

dotnet named project query    328.778 ms median
dv project --project            6.204 ms median

dotnet reject unknown option  146.054 ms median
dv reject unknown option         4.827 ms median

dotnet conditional references 288.983 ms median
dv conditional references       4.765 ms median

dotnet msbuild runtime query  321.215 ms median
dv runtime project inspect      5.687 ms median

dotnet msbuild runtime packs  360.550 ms median
dv runtime pack plan             6.403 ms median

dotnet msbuild framework plan 352.715 ms median
dv framework reference plan     5.585 ms median

dotnet msbuild compiler plan  368.952 ms median
dv build --plan                 4.979 ms median

dotnet restore (cold deps)     966.711 ms median
dv restore (cold deps)         411.275 ms median

dotnet restore (50 packages)  1364.792 ms median
dv restore (50 packages)       598.220 ms median

dotnet restore (warm locked)   552.265 ms median
dv restore (warm locked)         7.019 ms median

dotnet RID/content (cold)      600.782 ms median
dv RID/content (cold)           23.186 ms median

dotnet RID/content (warm)      456.098 ms median
dv RID/content (warm)            7.589 ms median

dotnet PackageReference policy 456.722 ms median
dv PackageReference policy       6.611 ms median

dotnet legacy package pruning  492.588 ms median
dv legacy package pruning        7.339 ms median

dotnet signed restore (cold)    664.903 ms median
dv signed restore (cold)         30.841 ms median

dotnet signed restore (warm)    467.897 ms median
dv signed restore (warm)         11.463 ms median

dotnet central package restore 461.826 ms median
dv central package restore      29.864 ms median

dotnet package conflict error  569.423 ms median
dv package conflict error       13.797 ms median

dotnet two-project restore     700.911 ms median
dv two-project restore          51.502 ms median

dotnet restore (config stack) 532.948 ms median
dv restore (config stack)       5.651 ms median

dotnet restore (config merge) 558.126 ms median
dv restore (config merge)       9.422 ms median

dotnet restore (source policy) 527.659 ms median
dv restore (source policy)       5.850 ms median

dotnet restore (unmapped source) 531.249 ms median
dv restore (unmapped source)       9.566 ms median

dotnet restore (request budget) 3109.409 ms median
dv restore (request budget)      247.157 ms median

dotnet restore (source telemetry) 3067.502 ms median
dv restore (source telemetry)      232.130 ms median

dotnet restore (storage policy) 523.051 ms median
dv restore (storage policy)       5.370 ms median

dotnet restore (CLI overrides)  524.597 ms median
dv restore (CLI overrides)        5.103 ms median

dotnet restore (local sources)  670.534 ms median
dv restore (local sources)       64.522 ms median

dotnet NuGet service index      344.113 ms median
dv NuGet service index          277.336 ms median

dotnet NuGet credentials         73.624 ms median
dv NuGet credentials              4.615 ms median

dotnet credential provider      115.621 ms median
dv credential provider           22.519 ms median

dotnet client certificates       89.254 ms median
dv client certificates           30.003 ms median

dotnet NuGet HTTP policy          78.286 ms median
dv NuGet HTTP policy               6.934 ms median

dotnet NuGet source security      71.416 ms median
dv NuGet source security            5.742 ms median

dotnet locked asset plan       702.904 ms median
dv locked asset plan           107.385 ms median
```

The benchmark preflight verifies the same selected SDK, evaluated project
properties, source items, framework/runtime versions, targeting packs, runtime
packs, runtime assets, and apphost before retaining samples. Framework
roll-forward is also checked against an actual Microsoft host launch.

## Why dv

The current .NET development path crosses several tools and managed startup
boundaries before useful work begins. `dv` replaces that orchestration with
native, explicit data transforms:

- no hidden fallback to `dotnet`, MSBuild, NuGet, or VSTest;
- no managed process for SDK discovery, project discovery, or no-op proofs;
- batch-first work over contiguous indexed records;
- bounded parallelism for independent CPU work and async only for waiting I/O;
- deterministic ordering, stable diagnostics, and versioned JSON events;
- cold, warm, incremental, and no-op benchmarks designed alongside features.

Unsupported behavior fails explicitly. A favorable timing never outranks a
correct artifact.

## Current Status

The project is in the first implementation phase.

| Capability | Status |
|---|---|
| Lossless typed CLI, profile/platform lexical rules, command-spelling normalization, environment precedence, secret-safe reporting, child-argument forwarding, early option rejection, global output policy, compatibility exit profiles, child termination classification, and independent command/event protocol versions | Implemented |
| Installed SDK discovery | Implemented |
| `global.json` SDK selection | Implemented |
| Initial SDK-style project evaluation | Implemented |
| TFM/RID/configuration conditional references | Implemented |
| Target-aware framework and compiler input planning | Implemented |
| Framework references and shared-runtime roll-forward | Implemented |
| Runtime, host, native asset, and apphost planning | Implemented |
| Fingerprinted immutable SDK pack inventory cache | Implemented |
| Actionable unavailable-pack diagnostics | Implemented |
| Machine/user/repository/explicit NuGet config discovery | Implemented |
| Keyed NuGet config merge and environment expansion | Implemented |
| NuGet package/audit sources, protocols, and pre-discovery source mapping | Implemented |
| Bounded global and per-source NuGet request scheduling | Implemented |
| NuGet storage, fallback, signature, proxy, and audit policy | Implemented |
| Author/repository package signatures and trusted signers | Implemented |
| NuGet CLI source, config, and package-folder overrides | Implemented |
| NuGet flat and hierarchical local sources | Implemented |
| NuGet interval and floating version selection | Implemented |
| PackageReference asset, warning, alias, and path-property policy | Implemented |
| Central versions, overrides, global references, and transitive pinning | Implemented |
| Lowest-applicable, nested direct-wins, and cousin package convergence | Implemented |
| Stable package downgrade, conflict, cycle, missing, and compatibility diagnostics | Implemented |
| Project-reference closure restore with shared package metadata and downloads | Implemented |
| Nuspec framework references and legacy framework assemblies | Implemented |
| Concrete package RID selection and `contentFiles` metadata | Implemented |
| NuGet v3 service-index capability discovery | Implemented |
| NuGet Basic/PAT source credentials | Implemented |
| NuGet V2 credential-provider authentication | Implemented |
| NuGet PFX and Windows-store client certificates | Implemented |
| NuGet proxy, retry, timeout, redirect, rate-limit, and offline policy | Implemented |
| Explicit per-source HTTP and TLS-validation security policy | Implemented |
| Family-partitioned package asset planning | Implemented |
| Human and JSON diagnostics/events | Implemented |
| Reference benchmark harness | Implemented |
| Generated, versioned compatibility manifest | Implemented |
| Exact package resolution, v2/v3 sources, verified cache, and lock | Initial implementation |
| Direct Roslyn compilation | Planned |
| Incremental and no-op builds | Planned |
| Application runner | Planned |
| Test, pack, and publish | Planned |

SDK discovery supports all documented roll-forward policies, prerelease
filtering, JSON comments, custom errors, .NET 10 search `paths`, and `$host$`
without launching `dotnet`.

Use `--compat dotnet|msbuild|nuget|vstest` to select a pinned reference exit
policy before command discovery. A 45-byte read-only matrix covers success,
usage, unsupported, operation, build, restore, test failure, no-tests, and
cancellation results across the five invocation profiles. Inapplicable tool
outcomes use an explicit sentinel rather than a plausible exit. Native failures
retain exit code 2; current reference-profile failures return 1. Selection is
one indexed byte read with no allocation, filesystem access, or process launch.
The selector is removed from typed command operands during the initial linear
argument scan. The scan-only state is a compile-time-checked five-byte record
shared with global policy, and malformed or duplicate selectors reject before
SDK, project, filesystem, process, or network work. Full executable-name
inference, drop-in grammar, and output-layout work remains in progress.

Selected-profile failures carry one stable `compatibility_profile` context
field in both human and JSON diagnostics. Native failures and invalid or
repeated selectors omit it, so automation can distinguish a deliberate
reference grammar from native parsing without scraping prose. This annotation
is created only on the error path and adds no state or allocation to successful
dispatch.

Reaped child processes retain their exact 32-bit exit code instead of passing
through those failure mappings. Launch and wait failures are separate typed
states, while Unix signals remain distinct until the owning run/test workflow
selects an explicit policy. Application launch itself is still planned, so the
current command surface never claims that a TBI child executed.

Command syntax version `1` and JSON event schema version `19` advance
independently. `dv --json --version` reports the executable and both protocol
versions in one validated event batch, while human `dv --version` remains
unchanged. Version aliases normalize to the same typed command and cannot
select a different wire schema.

All 20 native command spellings currently accepted by `dv` normalize to 15
native semantic kinds. Profile-aware routing expands that to 24 exact command
kinds without enlarging the six-byte request. In particular, `sync` and
`restore` share one `Restore` request, while NuGet `restore` is a distinct
pre-I/O route and MSBuild/VSTest words cannot enter the native restore path.
The original OS spelling remains only in the cold argument owner for
diagnostics and events.

Seven overlapping words (`restore`, `pack`, `push`, `list`, `add`, `remove`,
and `update`) use a 35-byte read-only precedence matrix indexed by the selected
profile. Routing performs one indexed byte read after the existing exact-token
match, allocates nothing, and cannot probe a project or fall through into a
different tool grammar.

The same scan preserves the platform-tokenized argument batch exactly. It does
not attempt to reconstruct quote characters that the OS has already resolved;
instead it retains token boundaries, empty and non-Unicode arguments, option
case, and everything after `--`. Dotnet option names are exact, NuGet command
routing is case-insensitive, and Windows `/` prefixes are recognized only by
the reference profiles that accept them. Implemented Phase 1 options accept
separate, `=`, and `:` values. Singleton repeats fail before project I/O while
repeatable package sources retain their input order.

`dv compat manifest` emits compatibility manifest version `1` as a static JSON
artifact. It records the selected .NET 10 SDK, MSBuild, NuGet, and VSTest
versions; 115 command paths; 769 option records; 74 argument records; observed
environment/exit/output contracts; and every parity row with explicit support
state. The query does not discover an SDK or parse the manifest at runtime.
The [manifest contract](docs/compatibility-manifest.md) documents regeneration,
bounds, and the intentionally explicit missing-work states.

Human output defaults can be set with `DV_COLOR=auto|always|never` and
`DV_VERBOSITY=quiet|minimal|normal|detailed|diagnostic`; a non-empty
`NO_COLOR` supplies the standard lower-priority no-color default. Explicit
command-line output options win. Invalid environment values fail before
discovery without echoing their contents. Human diagnostics and JSON argument
events redact sensitive option values, secret property assignments, URL
userinfo, and query/fragment data before they reach a writer.

`run` and `test` also type child-process environment overlays with Microsoft
precedence: ambient values, `[env:NAME=VALUE]` directives, launch-profile
values, then `-e|--environment NAME=VALUE`. Equal-source entries are applied
left to right, so the last value wins. The plan borrows up to four edits
without allocating and never reports secret values. Launch-profile loading and
the child launch itself remain part of the pending run/test workflow, so this
foundation never claims that a TBI child ran.

`dv sdk compatible-rids RID` loads the selected SDK's portable RID graph as
data and returns NuGet-compatible breadth-first fallbacks. The compiled graph
stores 16-byte sorted nodes, contiguous 32-bit edges, and precomputed
compatibility ranges; it never guesses compatibility by splitting RID text.

`dv project runtime-packs` combines that graph with the selected SDK's bundled
pack manifest, the restored runtime manifest, and the installed host pack. It
selects manifest-defined identities and patch versions, separates managed and
native runtime assets, and returns the exact platform apphost template without
hard-coded SDK or package versions. Validated asset/apphost inventories persist
as compact fingerprinted binary data and rebuild when the selected SDK or pack
generation changes. An unavailable TFM, RID, runtime pack, host
pack, targeting pack, or shared framework produces typed identity, version,
target, RID, and acquisition fields instead of leaving the remedy in prose.

`dv project frameworks` resolves the implicit Core and explicit framework
references from the selected SDK manifest, applies project and item-level
runtime/targeting-pack version precedence, and selects installed shared
frameworks with the runtime's documented roll-forward policies. The retained
plan is one text allocation plus one contiguous 72-byte record batch; no .NET
generation, pack identity, or NuGet source is hard-coded.

Project evaluation supports one `Microsoft.NET.Sdk` C# project targeting one
modern unified .NET TFM, `Exe` and `Library` outputs, default source discovery,
Debug/Release configuration, project-reference paths, and exact, interval, or
floating package references. Literal `RuntimeIdentifier` and
`RuntimeIdentifiers` values become
one compact target-dimension batch rather than copies of the project. Target
family and version are parsed once and shared by pack, compiler,
dependency-group, and package-asset selection. Unsupported MSBuild behavior
fails explicitly.

Direct package references normalize `IncludeAssets`, `ExcludeAssets`, and
`PrivateAssets` into eight-bit family masks during evaluation. The effective
mask flows through dependency discovery, while `NoWarn`, `Aliases`, and
generated `Pkg*` package-root properties stay in a separate 32-byte cold policy
batch. Compiler aliases are sparse index records rather than metadata copied
onto every framework reference.

The baseline tracks [.NET 10](https://dotnet.microsoft.com/en-us/download/dotnet/10.0),
the latest stable LTS release as of 2026-08-01. Preview TFMs are not selected
as the default target.

`dv restore` (also available as `dv sync`) merges the supported
`NuGet.Config` subset, speaks HTTPS NuGet v2 or v3 according to each source,
converges typed NuGet version ranges with lowest-applicable, direct-wins, and
cousin rules, and applies NuGet's matching-first floating preference before
streaming package payloads through SHA-512. It validates v2
source hashes and ZIP boundaries, retracts stale dependency edges when a
selection changes, publishes NuGet-compatible cache entries atomically, and
writes a deterministic `dv.lock.json`. A matching warm lock performs zero
network requests. Selected compile, runtime, analyzer, resource, content,
build, build-multitargeting, build-transitive, and native paths occupy
consecutive ranges in one immutable span batch.

`dv project package-sources` resolves the effective source configuration and
discovers registration, flat-container, search, vulnerability, and publish
resources from NuGet v3 service indexes. Resource-type and `clientVersion`
precedence matches NuGet.Client, while selected endpoint text is compacted
into one allocation with five fixed ranges. Independent source indexes are
requested concurrently through the bounded Tokio scheduler.

Basic/PAT source credentials follow NuGet.Client's config and exact per-source
environment precedence. Cleartext buffers are zeroed, Windows-encrypted
passwords use NuGet-compatible user DPAPI, and one sensitive header is reused
only for the configured HTTPS origin. Locks, diagnostics, events, and human
output never contain usernames, passwords, tokens, or authorization headers;
the same containment applies to provider responses. Source inventory reports
only redacted authentication policy.

Private feeds can use self-contained NuGet cross-platform V2 credential
providers selected through `NUGET_NETCORE_PLUGIN_PATHS` or
`NUGET_PLUGIN_PATHS`. `dv` performs the symmetric JSON-lines handshake,
process monitoring, initialization, operation-claim negotiation, and
noninteractive credential acquisition by default. A rejected provider
credential is refreshed once with `IsRetry=true`; concurrent challenges share
the source's selected provider and cached header. `--interactive` enables
provider login messages, while Ctrl+C and NuGet's handshake/request timeouts
send protocol cancellation, stop, and reap the provider. DLL-only plugins fail
explicitly because production `dv` never invokes `dotnet` as a fallback.

Work-bearing commands install Ctrl+C/SIGINT handling after typed argument
classification and before SDK, project, filesystem, child-process, or network
work. The first signal cancels package and provider waits and starts one
absolute two-second child-shutdown deadline; the second signal escalates to
immediate termination. Help, version, malformed global-option, and
unknown-command paths do not allocate cancellation state or start a handler
thread.

NuGet `clientCertificates` records bind a source to a bounded relative or
absolute PFX file, or to a certificate selected by thumbprint from a Windows
`CurrentUser`/`LocalMachine` store. File secrets and PFX buffers are zeroed,
private keys are validated once, and the resulting native TLS client is reused
only for that HTTPS origin. Certificate clients reject redirects so identity
material cannot cross an origin boundary. Non-Windows stores and unsupported
store selectors fail explicitly; public sources pay no certificate setup cost.

Storage policy follows NuGet precedence for the writable global cache,
read-only fallback folders, HTTP metadata cache, and scratch directory.
Restore verifies signed-package ZIP integrity, CMS signatures, author and
repository identities, RFC 3161 timestamps, certificate chains, and typed
`trustedSigners` policy before atomic publication. It also retains project
audit policy and constructs authenticated proxy and bypass policy only at the
HTTP boundary.
Proxy URLs are stripped of credentials before retention; output reports only
redacted policy flags. Remote work uses bounded per-source permits, secure-only
redirects, NuGet-compatible retry controls, request and body-stall timeouts,
and a zero-network offline path. Advisory lookup, conditional HTTP-cache
semantics, and [online certificate revocation](issues/signature-revocation.md)
remain separately tracked parity work.

`dv build --plan` selects the newest installed reference pack matching the
project target, parses its manifest, selects Roslyn plus built-in and package
analyzers, and emits one immutable compiler input plan. It does not compile
yet.

## Quick Start

Prerequisites:

- Rust `1.94.0`
- an installed .NET SDK

```powershell
# Show the selected SDK
cargo run -p dv-cli --release -- sdk current

# List installed SDKs and mark the selected one
cargo run -p dv-cli --release -- sdk list

# Emit the versioned JSON event stream
cargo run -p dv-cli --release -- sdk current --json

# Inspect portable RID compatibility from the selected SDK
cargo run -p dv-cli --release -- sdk compatible-rids linux-musl-x64

# Inspect the project in the current directory
cargo run -p dv-cli --release -- project inspect

# Inspect an explicit project as structured events
cargo run -p dv-cli --release -- project inspect path\to\App.csproj --json

# Select a project explicitly with the shared named selector
cargo run -p dv-cli --release -- project inspect --project path\to\App.csproj --json

# Inspect effective package sources and NuGet v3 capabilities
cargo run -p dv-cli --release -- project package-sources path\to\App.csproj

# Select runtime/host packs, native assets, and the apphost template
cargo run -p dv-cli --release -- project runtime-packs path\to\App.csproj

# Resolve framework references, targeting packs, and shared runtimes
cargo run -p dv-cli --release -- project frameworks path\to\App.csproj

# Plan Roslyn inputs without compiling
cargo run -p dv-cli --release -- build --plan path\to\App.csproj

# Resolve exact packages and write dv.lock.json
cargo run -p dv-cli --release -- restore path\to\App.csproj

# Prove a locked package graph from cache with no network
cargo run -p dv-cli --release -- restore path\to\App.csproj --offline

# `sync` is an exact alias
cargo run -p dv-cli --release -- sync path\to\App.csproj

# Inspect the selected-tool compatibility surface and support ledger
cargo run -p dv-cli --release -- compat manifest

# Run all implemented and reference benchmarks
cargo bench-all
```

Build the executable directly:

```powershell
cargo build -p dv-cli --release
target\release\dv.exe sdk current
```

## Like-For-Like Benchmarks

README results are limited to commands that produce the same meaningful
result. Unimplemented `dv` workflows appear as `TBI` in the full benchmark
output and are not promoted into this table.

The `CLI-012` forwarding parser has a separate structural baseline because
`dv run` execution remains TBI. It verifies exact `dotnet run --` behavior but
does not claim like-for-like run performance. See the
[forwarding baseline](docs/performance-baselines/2026-08-01-cli-forwarding-windows.md).
The current SDK row includes early handler installation; its deadline and
signal gates are recorded in the
[cancellation baseline](docs/performance-baselines/2026-08-01-cli-cancellation-windows.md).
Command syntax and JSON event schema versioning have a separate structural
baseline because Microsoft has no equivalent dual-version query. See the
[protocol-version baseline](docs/performance-baselines/2026-08-01-cli-protocol-version-windows.md).
The generated compatibility manifest has its own structural baseline because
Microsoft publishes no equivalent query. See the
[compatibility-manifest baseline](docs/performance-baselines/2026-08-01-compatibility-manifest-windows.md).
Accepted command spelling normalization has a like-for-like pre-I/O baseline
in the [command-normalization evidence](docs/performance-baselines/2026-08-01-cli-command-normalization-windows.md).
Explicit invocation-mode classification has a like-for-like pre-I/O baseline
in the [invocation-mode evidence](docs/performance-baselines/2026-08-01-invocation-mode-windows.md).
Structured explicit-profile diagnostics have updated like-for-like evidence in
the [compatibility-diagnostics baseline](docs/performance-baselines/2026-08-02-cli-compat-diagnostics-windows.md).
Profile-aware token case, prefixes, separators, repetition, and `--` behavior
have like-for-like evidence in the
[lexical-preservation baseline](docs/performance-baselines/2026-08-02-cli-lexical-preservation-windows.md).
Ambiguous command precedence has a like-for-like pre-I/O baseline in the
[route-precedence evidence](docs/performance-baselines/2026-08-01-cli-route-precedence-windows.md).

Initial machine:

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after 3 warm-ups for SDK selection, RID expansion,
  project evaluation, named project selection, unknown-option rejection,
  conditional references, runtime evaluation, warm and cold runtime-pack inventory
  planning, NuGet configuration hierarchy, keyed configuration merge, source
  policy sections, request budgets, source telemetry, storage policy, CLI
  overrides, local sources, floating version selection, PackageReference
  metadata, legacy package pruning, central package management, package conflict resolution, package
  diagnostics, project-reference package batches, nuspec framework metadata,
  service-index capability discovery, source
  credentials, credential providers, the
  framework-reference plan,
  unavailable-pack diagnostic, 203-package asset plan, and one-package cold
  case; command normalization, cancellation-ready SDK selection, compiler
  planning, and cold/warm signed-package validation use 5 warm-ups; invocation
  mode, exit policy, lexical preservation, and route precedence use 50 retained
  samples after 10 warm-ups; 10
  retained samples after 2 warm-ups for the large cold graph; warm locked
  restore uses 10 retained samples after 3 warm-ups; the massive graph uses
  5 retained samples after 1 warm-up
- warm OS caches; fixture and prerequisite setup outside timed intervals

<!-- LIKE_FOR_LIKE_BENCHMARKS_START -->
| Operation | Reference command | `dv` command | Reference median | `dv` median | Median ratio | Reference p95 | `dv` p95 |
|---|---|---|---:|---:|---:|---:|---:|
| Select current SDK with cancellation installed before work | `dotnet --version` | `dv sdk current` | 63.347 ms | 4.501 ms | 14.1x | 66.926 ms | 5.029 ms |
| Select current SDK with typed global output policy | `dotnet --version` | `dv sdk --quiet --no-color current` | 74.362 ms | 6.986 ms | 10.6x | 78.493 ms | 7.957 ms |
| Select current SDK through the `dotnet` compatibility profile | `dotnet --version` | `dv --compat dotnet sdk current` | 65.901 ms | 5.225 ms | 12.6x | 67.752 ms | 6.202 ms |
| Reject an unknown build option before unrelated work | `dotnet build --definitely-unknown` | `dv build --definitely-unknown` | 125.249 ms | 4.406 ms | 28.4x | 130.131 ms | 5.615 ms |
| Normalize `sync` to restore and reject an invalid option before work | `dotnet restore --definitely-unknown` | `dv sync --definitely-unknown` | 121.211 ms | 5.462 ms | 22.2x | 128.378 ms | 6.337 ms |
| Select the `dotnet` mode, report its profile, and reject before discovery | `dotnet build --definitely-unknown` | `dv --compat dotnet build --definitely-unknown` | 133.281 ms | 5.125 ms | 26.0x | 147.033 ms | 6.256 ms |
| Preserve missing-project restore failure status | `dotnet restore DefinitelyMissing.csproj` | `dv --compat dotnet restore DefinitelyMissing.csproj` | 122.756 ms | 5.158 ms | 23.8x | 134.338 ms | 6.073 ms |
| Preserve a combined configuration token before sentinel rejection | `dotnet build -c:Release --definitely-unknown` | `dv --compat dotnet build -c:Release --definitely-unknown` | 141.461 ms | 4.912 ms | 28.8x | 176.976 ms | 6.003 ms |
| Route ambiguous `pack` and reject before discovery | `dotnet pack --definitely-unknown` | `dv --compat dotnet pack --definitely-unknown` | 280.174 ms | 5.242 ms | 53.4x | 306.891 ms | 5.841 ms |
| Apply environment defaults and reject an unknown option without exposing environment data | `dotnet build --definitely-unknown` | `dv build --definitely-unknown` | 134.218 ms | 5.503 ms | 24.4x | 150.374 ms | 6.314 ms |
| Expand a portable RID | `dotnet bin/Release/RidGraphOracle.dll linux-musl-x64` | `dv sdk compatible-rids linux-musl-x64` | 36.217 ms | 6.049 ms | 6.0x | 39.263 ms | 6.859 ms |
| Evaluate small project | `dotnet msbuild SmallConsole.csproj` property/item query | `dv project inspect SmallConsole.csproj --json` | 282.186 ms | 3.846 ms | 73.4x | 287.600 ms | 4.074 ms |
| Evaluate a named project selection | `dotnet msbuild SmallConsole.csproj` property/item query | `dv project inspect --project SmallConsole.csproj --json` | 328.778 ms | 6.204 ms | 53.0x | 502.383 ms | 8.214 ms |
| Evaluate TFM/RID/configuration conditional references | `dotnet msbuild ConditionalReferences.csproj --nologo -p:Configuration=Release` property/item query | `dv project inspect ConditionalReferences.csproj --configuration Release --json` | 288.983 ms | 4.765 ms | 60.6x | 321.422 ms | 6.209 ms |
| Evaluate runtime target dimensions | `dotnet msbuild RuntimeProject.csproj` runtime-property query | `dv project inspect RuntimeProject.csproj --json` | 321.215 ms | 5.687 ms | 56.5x | 330.112 ms | 6.897 ms |
| Plan runtime and host packs from a warm inventory | `dotnet msbuild RuntimePackProject.csproj` runtime-pack/apphost item query | `dv project runtime-packs RuntimePackProject.csproj --packages .packages --json` | 360.550 ms | 6.403 ms | 56.3x | 370.695 ms | 8.218 ms |
| Build a cold runtime-pack inventory | `dotnet msbuild RuntimePackProject.csproj` runtime-pack/apphost item query | `dv project runtime-packs RuntimePackProject.csproj --packages .packages --json` | 368.322 ms | 11.118 ms | 33.1x | 380.190 ms | 12.636 ms |
| Diagnose an unavailable runtime pack | `dotnet restore UnavailablePackProject.csproj --source offline-source --packages .packages --no-cache --disable-build-servers -p:NuGetAudit=false --nologo --verbosity minimal` | `dv project runtime-packs UnavailablePackProject.csproj --packages .packages --json` | 532.652 ms | 6.378 ms | 83.5x | 596.360 ms | 6.931 ms |
| Plan framework references and shared runtimes | `dotnet msbuild FrameworkReferenceProject.csproj -t:ResolveTargetingPackAssets` framework item query | `dv project frameworks FrameworkReferenceProject.csproj --json` | 352.715 ms | 5.585 ms | 63.2x | 390.432 ms | 6.530 ms |
| Plan compiler inputs | `dotnet msbuild SmallConsole.csproj -t:ResolveReferences` property/item query | `dv build --plan SmallConsole.csproj --json` | 368.952 ms | 4.979 ms | 74.1x | 374.293 ms | 6.027 ms |
| Validate a six-file NuGet configuration hierarchy | `dotnet restore ConfigHierarchy.csproj --locked-mode --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore ConfigHierarchy.csproj --offline --json` | 532.948 ms | 5.651 ms | 94.3x | 545.049 ms | 6.296 ms |
| Merge keyed NuGet configuration | `dotnet restore ConfigMerge.csproj --locked-mode --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore ConfigMerge.csproj --offline --json` | 558.126 ms | 9.422 ms | 59.2x | 648.298 ms | 10.606 ms |
| Load NuGet source policy sections | `dotnet restore SourceSections.csproj --locked-mode --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore SourceSections.csproj --offline --json` | 527.659 ms | 5.850 ms | 90.2x | 537.783 ms | 8.229 ms |
| Reject an unmapped package before source discovery | `dotnet restore SourceMapping.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore SourceMapping.csproj --packages .packages --json` | 531.249 ms | 9.566 ms | 55.5x | 1153.411 ms | 11.215 ms |
| Restore six packages through bounded delayed feeds | `dotnet restore RequestBudget.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore RequestBudget.csproj --packages .packages --json` | 3109.409 ms | 247.157 ms | 12.6x | 5249.874 ms | 1178.249 ms |
| Restore six packages and attribute source work | `dotnet restore RequestBudget.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore RequestBudget.csproj --packages .packages --json` | 3067.502 ms | 232.130 ms | 13.2x | 5257.469 ms | 1179.197 ms |
| Resolve NuGet storage and restore policy | `dotnet restore StoragePolicy.csproj --locked-mode --no-http-cache --nologo --verbosity quiet` | `dv restore StoragePolicy.csproj --offline --json` | 523.051 ms | 5.370 ms | 97.4x | 605.407 ms | 6.526 ms |
| Apply NuGet CLI overrides | `dotnet restore CliOverrides.csproj --locked-mode --source https://api.nuget.org/v3/index.json --configfile config/selected.config --packages policy/cli-global --no-http-cache --nologo --verbosity quiet` | `dv restore CliOverrides.csproj --source https://api.nuget.org/v3/index.json --configfile config/selected.config --packages policy/cli-global --offline --json` | 524.597 ms | 5.103 ms | 102.8x | 548.166 ms | 5.986 ms |
| Restore from flat and hierarchical local sources | `dotnet restore LocalSources.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | `dv restore LocalSources.csproj --packages .packages --offline --json` | 670.534 ms | 64.522 ms | 10.4x | 694.282 ms | 97.332 ms |
| Resolve the highest stable floating version | `dotnet restore FloatingVersion.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore FloatingVersion.csproj --packages .packages --offline --json` | 667.568 ms | 60.007 ms | 11.1x | 714.356 ms | 74.028 ms |
| Apply direct PackageReference metadata on a warm locked graph | `dotnet restore MetadataProject.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | `dv restore MetadataProject.csproj --packages .packages --offline --json` | 456.722 ms | 6.611 ms | 69.1x | 460.724 ms | 7.714 ms |
| Apply .NET 9 Core and ASP.NET package pruning on a warm locked graph | `dotnet restore LegacyPruningProject.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | `dv restore LegacyPruningProject.csproj --packages .packages --offline --json` | 492.588 ms | 7.339 ms | 67.1x | 544.651 ms | 8.434 ms |
| Verify and publish a repository-signed package from empty local state | `dotnet restore PackageSignatures.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | `dv restore PackageSignatures.csproj --packages .packages --json` | 664.903 ms | 30.841 ms | 21.6x | 1503.948 ms | 36.082 ms |
| Revalidate a repository-signed package on a warm locked restore | `dotnet restore PackageSignatures.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | `dv restore PackageSignatures.csproj --packages .packages --offline --json` | 467.897 ms | 11.463 ms | 40.8x | 488.129 ms | 13.841 ms |
| Apply central versions, overrides, global references, and transitive pinning on a warm 54-package graph | `dotnet restore CentralPackages.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | `dv restore CentralPackages.csproj --packages .packages --offline --json` | 461.826 ms | 29.864 ms | 15.5x | 490.623 ms | 34.461 ms |
| Resolve nested direct-wins and cousin constraints from a warm package cache | `dotnet restore ConflictResolution.csproj --packages .packages -p:NoWarn=NU1605 --nologo --verbosity quiet` | `dv restore ConflictResolution.csproj --packages .packages --offline --json` | 604.023 ms | 16.971 ms | 35.6x | 689.544 ms | 19.661 ms |
| Diagnose a cold local-package constraint conflict | `dotnet restore ConflictFailure.csproj --packages .packages --nologo --verbosity minimal` | `dv restore ConflictFailure.csproj --packages .packages --offline --json` | 569.423 ms | 13.797 ms | 41.3x | 581.503 ms | 17.209 ms |
| Restore a two-project shared package graph from a cold local cache | `dotnet restore PackageBatch.csproj --packages .packages --nologo --verbosity quiet` | `dv restore PackageBatch.csproj --packages .packages --offline --json` | 700.911 ms | 51.502 ms | 13.6x | 880.634 ms | 64.606 ms |
| Select package framework metadata from a cold local source | `dotnet restore FrameworkMetadata.csproj --packages .packages --nologo --verbosity quiet` | `dv restore FrameworkMetadata.csproj --packages .packages --offline --json` | 558.832 ms | 15.989 ms | 35.0x | 578.305 ms | 18.009 ms |
| Select concrete RID and content assets from a cold local source | `dotnet restore WindowsFallback.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | `dv restore WindowsFallback.csproj --packages .packages --offline --json` | 600.782 ms | 23.186 ms | 25.9x | 2009.103 ms | 33.821 ms |
| Reuse a locked concrete RID and content plan | `dotnet restore WindowsFallback.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | `dv restore WindowsFallback.csproj --packages .packages --offline --json` | 456.098 ms | 7.589 ms | 60.1x | 486.470 ms | 9.101 ms |
| Discover NuGet v3 service endpoints | `dotnet oracle/bin/Release/ServiceIndexOracle.dll https://api.nuget.org/v3/index.json` | `dv project package-sources ServiceIndex.csproj --json` | 344.113 ms | 277.336 ms | 1.2x | 868.499 ms | 289.483 ms |
| Select and contain NuGet source credentials | `dotnet oracle/bin/Release/CredentialOracle.dll .` | `dv project package-sources CredentialProject.csproj --offline --json` | 73.624 ms | 4.615 ms | 16.0x | 75.971 ms | 5.388 ms |
| Acquire private-feed credentials through a provider | `dotnet oracle/bin/Release/CredentialProviderOracle.dll https://private.example.test/v3/index.json` | `dv project package-sources CredentialProviderProject.csproj --offline --probe-credentials --json` | 115.621 ms | 22.519 ms | 5.1x | 2238.289 ms | 28.833 ms |
| Select PFX and Windows-store client certificates | `dotnet oracle/bin/Release/ClientCertificateOracle.dll query .` | `dv project package-sources ClientCertificateProject.csproj --offline --json` | 89.254 ms | 30.003 ms | 3.0x | 91.690 ms | 31.361 ms |
| Select NuGet HTTP transport policy | `dotnet oracle/bin/Release/HttpPolicyOracle.dll .` | `dv project package-sources HttpPolicyProject.csproj --offline --json` | 78.286 ms | 6.934 ms | 11.3x | 81.730 ms | 8.059 ms |
| Select explicit NuGet source security policy | `dotnet oracle/bin/Release/SourceSecurityOracle.dll .` | `dv project package-sources SecurityProject.csproj --offline --json` | 71.416 ms | 5.742 ms | 12.4x | 73.370 ms | 6.878 ms |
| Resolve dependencies from cold caches | `dotnet restore PackageConsole.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | `dv restore PackageConsole.csproj --packages .packages --json` | 1028.951 ms | 417.981 ms | 2.5x | 1061.502 ms | 469.712 ms |
| Resolve a cold 50-package graph | `dotnet restore LargePackageGraph.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | `dv restore LargePackageGraph.csproj --packages .packages --json` | 1425.299 ms | 632.458 ms | 2.3x | 1658.964 ms | 662.278 ms |
| Resolve a cold 203-package solution graph | `dotnet restore MassivePackageGraph.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore MassivePackageGraph.csproj --packages .packages --json` | 9977.524 ms | 4325.957 ms | 2.3x | 10416.603 ms | 4852.974 ms |
| Plan assets for a warm 203-package graph | `dotnet restore MassivePackageGraph.csproj --locked-mode --packages .packages -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore MassivePackageGraph.csproj --packages .packages --offline --json` | 702.904 ms | 107.385 ms | 6.5x | 1301.984 ms | 132.369 ms |
| Validate warm locked packages | `dotnet restore PackageConsole.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | `dv restore PackageConsole.csproj --packages .packages --offline --json` | 521.346 ms | 7.362 ms | 70.8x | 560.223 ms | 8.206 ms |
<!-- LIKE_FOR_LIKE_BENCHMARKS_END -->

Before measuring, the harness verifies SDK text, checks unknown-option exit and
diagnostic identity without workspace mutation, and compares every requested
project property plus the ordered compile-item identities. The named-selection
case applies that same gate through `--project`; malformed and mixed selectors
are covered separately by CLI tests. The RID graph case
compares the complete ordered expansion against the selected SDK's shipped
`NuGet.Packaging` implementation; its tiny adapter is built outside timed
intervals. The runtime project case also verifies the selected RID, ordered
plural RID property, and unique target dimension batch. The runtime-pack case
compares the selected runtime and host RIDs, manifest-derived identities and
versions, pack roots, all 172 managed and 15 native assets in order, and the
apphost template. The cold inventory case removes only `dv`'s binary inventory
before each iteration; the warm case verifies one immutable cache entry after
every sample. Restored package contents are prepared outside timing. For
package sync it also compares the complete package
identity, exact-version, archive-SHA-512, and selected asset batches. The
configuration-hierarchy case additionally injects machine and user roots,
checks Microsoft's exact six-path priority order, then compares the effective
repository source, protocol, package directory, package identity/version, and
archive SHA-512 with zero timed network work. The keyed-merge case additionally
exercises case-insensitive replacement, clear/remove operations, disabled
sources, and `%NAME%` expansion across four precedence levels, then applies
the same package and cache parity gate. The source-policy case uses the SDK's
official `NuGet.Configuration` assembly to verify enabled and disabled package
sources, audit sources, v2/v3 metadata, and longest-pattern mapping queries
before applying the same package and cache parity gate. The source-mapping
fixture follows that policy into restore: `Unmapped.Package` has no winning
enabled source while the only configured v3 endpoint is deliberately
unreachable. Microsoft must emit `NU1100`, `dv` must emit typed `DV0412`, and
either source-contact error fails preflight. This makes the retained
`531.249 ms` versus `9.566 ms` comparison like-for-like with zero HTTP work.
The request-budget case then restores the same six exact packages from two
loopback V3 feeds. Every response has the same fixed 25 ms delay, both tools
receive a global budget of four and a per-source budget of two, and the harness
requires every sample to contact both sources, stay within both bounds, and
publish all six archives. Public
network work and package seeding are outside timing, so the retained
`3109.409 ms` versus `247.157 ms` result compares equivalent cold restore work
under deterministic contention.
The source-telemetry case repeats that cold transform independently and checks
`dv`'s configuration-ordered request, response-byte, and duration rows against
the loopback servers on every sample. It also requires six cache misses and
forbids source locations in reporter output. The retained comparison is
`3067.502 ms` versus `232.130 ms` (`13.2x`).
The storage-policy case uses that same official assembly plus an MSBuild
property query to verify
global, HTTP-cache, scratch, ordered fallback, signature, audit, and proxy
policy. Both tools then resolve the same locked package from the fallback root
with empty global roots and zero timed network work. The CLI-override case
conflicts implicit-config, explicit-config, environment,
and command-line values; both tools must select only the explicit config, CLI
source, and CLI package folder while resolving the same package and hash with
zero timed network work. The local-source case maps one package to a flat feed
and one to a hierarchical feed, clears the global cache and restore outputs
before every sample, then requires matching identities, versions, hashes, and
zero HTTP requests. The floating-version case builds a two-version local feed
outside timing, then asks both tools for the highest stable `Newtonsoft.Json`
`13.*` version from empty isolated package state. Preflight requires the same
exact identity, selected version, archive hash, target, and asset batches; this
run selected `13.0.4`, with `dv` publishing one 2,484,726-byte archive and
making zero HTTP requests in every sample. The conditional-reference case
evaluates the same `net10.0`/`win-x64` Release project through MSBuild and
`dv`, then compares the selected TFM, RID, configuration, three package rows,
one project row, and one explicit framework row before retaining samples.
False branches deliberately contain incomplete or unsupported references so
the parity gate also proves they leave the batch before metadata validation.
The measured medians are `288.983 ms` for Microsoft and `4.765 ms` for `dv`
(`60.6x`). The
PackageReference metadata case applies all six policy fields to the same warm
locked `Newtonsoft.Json` graph. Preflight compares Microsoft
`project.assets.json` include/suppress-parent fields, warning codes, compile
aliases, runtime exclusion, and the generated `PkgNewtonsoft_Json` root before
retaining samples. The measured medians are `456.722 ms` for Microsoft and
`6.611 ms` for `dv` (`69.1x`), with no timed network or download work. The
package-pruning case opts a `net9.0` project into the SDK behavior and merges
the implicit Core plus direct ASP.NET framework tables. Preflight restores the
same locked `Newtonsoft.Json` graph, while focused SDK-oracle checks require
the same 420 pruning identities and stable patch ceilings. Thirty samples
measure `492.588 ms` for Microsoft and `7.339 ms` for `dv` (`67.1x`), with no
timed network or download work. The
package-signature cases use the same `MessagePack.Annotations` 2.5.192 archive,
the same required repository certificate fingerprint, and the same platform
trust roots. The cold case starts with empty package and lock state, verifies
the 20,900-byte signed archive, and publishes one package with zero HTTP
requests. The warm case revalidates the immutable cached archive under the
required policy with zero downloads. Thirty samples measure `664.903 ms`
versus `30.841 ms` (`21.6x`) cold and `467.897 ms` versus `11.463 ms` (`40.8x`)
warm. The
central-package case resolves the same 54 identities through versionless
direct references, an override, a global SourceLink reference, and a
`Humanizer.Core` transitive pin. Preflight compares every exact version,
archive hash, asset family, and Microsoft's `CentralTransitive` lock role.
The measured medians are `461.826 ms` for Microsoft and `29.864 ms` for `dv`
(`15.5x`), with zero timed network work. The package-diagnostic case starts
both tools with an empty package cache and the same eight-archive local feed.
The timed conflict must fail as Microsoft's `NU1107` and structured `DV0414`;
preflight also proves `NU1605`/`DV0413`, `NU1108`/`DV0415`,
`NU1101`/`DV0416`, `NU1102`/`DV0417`, and `NU1202`/`DV0402`. Successful
direct-wins warnings must survive `dv`'s native warm lock unchanged. Thirty
samples measure `569.423 ms` for Microsoft and `13.797 ms` for `dv`
(`41.3x`) with no network work. The project-batch case evaluates one root,
walks its two-project reference closure once, and resolves two identical
eight-package graphs through one command-local metadata table and package
cache. Preflight requires exact package/version parity for both children,
three ordered `dv` resolution events including the package-free root, eight
total archive publications rather than sixteen, and zero HTTP work. Thirty
cold local-source samples measure `700.911 ms` for Microsoft and `51.502 ms`
for `dv` (`13.6x`). The nuspec-framework case generates two deterministic
archives and starts both tools with empty package and restore state. Modern
preflight compares the selected dependency and shared framework while
excluding deliberately missing `net8.0` rows; legacy preflight compares the
nearest `net48` assembly group and rejects the unscoped fallback. `dv` must
reproduce both selections from warm locks without reopening the manifests.
Thirty cold local-source samples measure `558.832 ms` for Microsoft
and `15.989 ms` for `dv` (`35.0x`) with zero HTTP work. The unavailable-pack case
uses an empty checked-in source and isolated package cache; both commands must
fail and name
`Microsoft.NETCore.App.Runtime.linux-arm`, while `dv` must also emit the exact
version, TFM, RID, pack kind, acquisition action, and human guidance. The
service-index case makes one uncached HTTPS request per process and compares
all registration, flat-container, search, vulnerability, and package-publish
URIs against NuGet.Client's official resource selection. The `dv` timing also
includes project evaluation and `NuGet.Config` discovery. The live-network
samples are comparable within the same run, not across unrelated network
conditions. The credential case uses the official `NuGet.Configuration`
implementation to prove one environment override and one config-only PAT,
compares the same two redacted source rows, and rejects every fixture secret
in stdout and stderr. Both commands perform zero timed network requests. The
credential-provider case launches the same self-contained fixture through
Microsoft's official `NuGet.Protocol` plugin manager and `dv`'s native V2
client. Preflight proves noninteractive flags, timeout termination, exact
redacted authentication results, and zero secret text in either stream. The
client-certificate case loads the same exportable identity once from a relative
PFX and once from `CurrentUser\\My`. The official NuGet.Configuration oracle
and `dv` must publish the same two redacted source rows, select one certificate
per source, expose no fixture password, and perform zero timed network work.
The timing therefore measures config merge, PFX decode, store lookup,
private-key acquisition, and TLS-client construction rather than network
latency. The source-security case uses the same three-source configuration for
both tools and compares ordered source identity, protocol, explicit HTTP
permission, and disabled TLS validation through the SDK's official
NuGet.Configuration assembly. It is offline, so the timing measures policy
discovery, source-specific client construction, and structured reporting with
zero network requests. The massive case additionally compares runtime, resource,
content, analyzer, build, build-multitargeting, native, and RID runtime-target
paths plus runtime-target metadata. The warm asset-plan case retains that
exact parity gate, then measures locked planning over the populated
203-package caches. The
framework-reference case compares both resolved
framework rows, requested runtime versions, profiles, targeting-pack
identities/versions/roots, and the installed Core/ASP.NET versions observed by
an actual Microsoft host launch. Exact commands are printed in benchmark output and
recorded in the curated
[compiler baseline](docs/performance-baselines/2026-07-31-windows.md),
[project selection baseline](docs/performance-baselines/2026-08-01-project-selection-windows.md),
[unknown-option baseline](docs/performance-baselines/2026-08-01-unknown-option-windows.md),
[cancellation baseline](docs/performance-baselines/2026-08-01-cli-cancellation-windows.md),
[child-exit baseline](docs/performance-baselines/2026-08-01-cli-child-exit-windows.md),
[protocol-version baseline](docs/performance-baselines/2026-08-01-cli-protocol-version-windows.md),
[invocation environment baseline](docs/performance-baselines/2026-08-01-cli-environment-windows.md),
[RID graph baseline](docs/performance-baselines/2026-08-01-rid-graph-windows.md),
[runtime evaluation baseline](docs/performance-baselines/2026-08-01-runtime-evaluation-windows.md),
[runtime pack baseline](docs/performance-baselines/2026-08-01-runtime-pack-windows.md),
[SDK pack inventory cache baseline](docs/performance-baselines/2026-08-01-sdk-pack-inventory-cache-windows.md),
[unavailable pack diagnostic baseline](docs/performance-baselines/2026-08-01-pack-diagnostic-windows.md),
[framework reference baseline](docs/performance-baselines/2026-08-01-framework-reference-windows.md),
[NuGet configuration baseline](docs/performance-baselines/2026-08-01-nuget-config-discovery-windows.md),
[NuGet keyed-merge baseline](docs/performance-baselines/2026-08-01-nuget-config-merge-windows.md),
[NuGet source-policy baseline](docs/performance-baselines/2026-08-01-nuget-source-sections-windows.md),
[NuGet source-mapping baseline](docs/performance-baselines/2026-08-01-nuget-source-mapping-windows.md),
[NuGet request-budget baseline](docs/performance-baselines/2026-08-01-nuget-request-budget-windows.md),
[NuGet source-telemetry baseline](docs/performance-baselines/2026-08-01-nuget-source-telemetry-windows.md),
[NuGet storage-policy baseline](docs/performance-baselines/2026-08-01-nuget-storage-policy-windows.md),
[NuGet CLI-override baseline](docs/performance-baselines/2026-08-01-nuget-cli-overrides-windows.md),
[NuGet local-source baseline](docs/performance-baselines/2026-08-01-nuget-local-sources-windows.md),
[NuGet floating-version baseline](docs/performance-baselines/2026-08-01-nuget-floating-version-windows.md),
[conditional-reference baseline](docs/performance-baselines/2026-08-01-package-reference-conditions-windows.md),
[PackageReference metadata baseline](docs/performance-baselines/2026-08-01-package-reference-metadata-windows.md),
[package-pruning baseline](docs/performance-baselines/2026-08-01-package-pruning-windows.md),
[central package management baseline](docs/performance-baselines/2026-08-01-central-package-management-windows.md),
[package conflict-resolution baseline](docs/performance-baselines/2026-08-01-package-conflict-resolution-windows.md),
[package diagnostic baseline](docs/performance-baselines/2026-08-01-package-diagnostics-windows.md),
[package batch-resolution baseline](docs/performance-baselines/2026-08-01-package-batch-resolution-windows.md),
[nuspec framework-metadata baseline](docs/performance-baselines/2026-08-01-nuspec-framework-metadata-windows.md),
[package RID/content baseline](docs/performance-baselines/2026-08-01-package-rid-content-windows.md),
[NuGet service-index baseline](docs/performance-baselines/2026-08-01-nuget-service-index-windows.md),
[NuGet credential baseline](docs/performance-baselines/2026-08-01-nuget-credentials-windows.md),
[NuGet credential-provider baseline](docs/performance-baselines/2026-08-01-nuget-credential-provider-windows.md),
[NuGet client-certificate baseline](docs/performance-baselines/2026-08-01-nuget-client-certificates-windows.md),
[NuGet HTTP-policy baseline](docs/performance-baselines/2026-08-01-nuget-http-policy-windows.md),
[NuGet source-security baseline](docs/performance-baselines/2026-08-01-nuget-source-security-windows.md),
[package baseline](docs/performance-baselines/2026-08-01-package-assets-windows.md), and
[warm package asset-plan baseline](docs/performance-baselines/2026-08-01-package-asset-plan-windows.md).

The cold dependency result starts each timed process with a fresh project copy
and empty isolated package directory. The reference command also bypasses
NuGet's HTTP cache. It is a network-sensitive first-restore measurement, not a
claim that Windows page cache, DNS, TLS, or CDN state was reset.

The large-graph fixture has one direct `Humanizer` `2.14.1` reference and a
real 50-package closure. `dv` reported 50 package downloads, 51 HTTP requests,
and 3,241,550 payload bytes per retained sample. This case emphasizes graph
expansion and scheduling across many small archives rather than bandwidth.
Streaming dependency discovery, a measured sixteen-worker crossover, and
removal of redundant staging I/O reduced the scoped-worker `dv` median from
904.097 ms to 562.799 ms. The current bounded Tokio scheduler with typed graph
convergence plus SDK-owned pruning measures 632.458 ms in the latest
network-sensitive run and has separate congested-network A/B evidence in the
package baseline.

The massive acceptance fixture unions 51 direct package references from
Microsoft's eShop application into one `net10.0` restore workload. The .NET
SDK selected 203 packages and populated 272 package archives totaling
197,860,237 bytes. The current five-sample run measured 9,977.524 ms median
for `dotnet` and 4,325.957 ms for `dv`, a 2.3x median improvement. Both outputs
contain the same 203 selected package identities, versions, hashes, and
portable asset families. `dv` downloaded 203 retained packages and observed
at most 208 requests and 164,964,741 payload bytes; the eager streaming graph
can vary slightly in speculative request work between network samples.

The warm one-shot target for lightweight commands on this machine is `5 ms`
end to end. It is a local engineering budget, not a universal Windows
guarantee.

Reproduce the comparison:

```powershell
cargo bench-all --case sdk_current --samples 30 --warmups 3
cargo bench-all --case sdk_current_globals --samples 30 --warmups 3
cargo bench-all --case sdk_current_compat --samples 30 --warmups 3
cargo bench-all --case cli_command_normalization --samples 30 --warmups 5
cargo bench-all --case cli_mode_classification --samples 50 --warmups 10
cargo bench-all --case cli_exit_policy --samples 50 --warmups 10
cargo bench-all --case cli_lexical_preservation --samples 50 --warmups 10
cargo bench-all --case cli_route_precedence --samples 50 --warmups 10
cargo bench-all --case cli_cancellation --samples 30 --warmups 5
cargo bench-all --case cli_unknown_option --samples 30 --warmups 3
cargo bench-all --case cli_environment --samples 30 --warmups 3
cargo bench-all --case cli_forwarding --samples 30 --warmups 5
cargo bench-all --case cli_child_exit --samples 30 --warmups 5
cargo bench-all --case cli_protocol_version --samples 30 --warmups 5
cargo bench-all --case rid_graph --samples 30 --warmups 3
cargo bench-all --case project_evaluate --samples 30 --warmups 3
cargo bench-all --case project_select_named --samples 30 --warmups 3
cargo bench-all --case package_reference_conditions --samples 30 --warmups 3
cargo bench-all --case runtime_evaluate --samples 30 --warmups 3
cargo bench-all --case runtime_pack_plan --samples 30 --warmups 3
cargo bench-all --case runtime_pack_inventory_cold --samples 30 --warmups 3
cargo bench-all --case pack_diagnostic --samples 30 --warmups 3
cargo bench-all --case framework_reference_plan --samples 30 --warmups 3
cargo bench-all --case compiler_plan --samples 30 --warmups 5
cargo bench-all --case nuget_config_hierarchy --samples 30 --warmups 3
cargo bench-all --case nuget_config_merge --samples 30 --warmups 3
cargo bench-all --case nuget_source_sections --samples 30 --warmups 3
cargo bench-all --case nuget_source_mapping --samples 30 --warmups 3
cargo bench-all --case nuget_request_budget --samples 30 --warmups 3
cargo bench-all --case nuget_source_telemetry --samples 30 --warmups 3
cargo bench-all --case nuget_storage_policy --samples 30 --warmups 3
cargo bench-all --case nuget_cli_overrides --samples 30 --warmups 3
cargo bench-all --case nuget_local_sources --samples 30 --warmups 3
cargo bench-all --case nuget_floating_version --samples 30 --warmups 3
cargo bench-all --case package_reference_metadata --samples 30 --warmups 3
cargo bench-all --case central_package_management --samples 30 --warmups 3
cargo bench-all --case package_conflict_resolution --samples 30 --warmups 3
cargo bench-all --case package_diagnostics --samples 30 --warmups 3
cargo bench-all --case package_batch_resolution --samples 30 --warmups 3
cargo bench-all --case nuspec_framework_metadata --samples 30 --warmups 3
cargo bench-all --case package_rid_content_cold --samples 30 --warmups 3
cargo bench-all --case package_rid_content_warm --samples 30 --warmups 3
cargo bench-all --case nuget_service_index --samples 30 --warmups 3
cargo bench-all --case nuget_credentials --samples 30 --warmups 3
cargo bench-all --case nuget_credential_provider --samples 30 --warmups 3
cargo bench-all --case nuget_client_certificates --samples 30 --warmups 3
cargo bench-all --case package_sync_cold --samples 30 --warmups 3
cargo bench-all --case package_graph_cold --samples 10 --warmups 2
cargo bench-all --case package_graph_massive --samples 5 --warmups 1
cargo bench-all --case package_asset_plan --samples 30 --warmups 3
cargo bench-all --case package_sync_warm --samples 10 --warmups 3
```

Run the full suite:

```powershell
cargo bench-all
```

The full report includes exact commands, raw-sample JSON, min, median, p95, and
max. Performance results are comparable only on the same machine, fixture,
tool versions, power state, and cache conditions.

## Architecture

```text
project files + SDKs + package sources
                  |
                  v
      native discovery and parsing
                  |
                  v
       compact indexed build plan
                  |
        +---------+---------+
        |                   |
        v                   v
 package/cache work    Roslyn compilation
        |                   |
        +---------+---------+
                  |
                  v
      artifacts + structured events
```

The real platform is the hardware and filesystem, not the abstraction stack.
Subsystems start with observed input distributions, explicit ownership and
lifetime, stated memory/access costs, and a measurable definition of done.

See:

- [Project plan](PLAN.md)
- [Feature parity implementation order](docs/implementation-order.md)
- [Feature parity implementation map](docs/feature-parity-map.md)
- [Package signature verification contract](docs/package-signature-contract.md)
- [Data-oriented agent rules](AGENTS.md)
- [SDK discovery contract](docs/sdk-discovery.md)
- [Exit behavior contract](docs/exit-behavior.md)
- [Child process exit contract](docs/child-process-exit.md)
- [Command and event protocol versioning](docs/protocol-versioning.md)
- [Project evaluation contract](docs/project-evaluation.md)
- [Runtime pack planning contract](docs/runtime-pack-planning.md)
- [SDK pack inventory cache contract](docs/sdk-pack-inventory-cache.md)
- [NuGet configuration discovery contract](docs/nuget-config-discovery.md)
- [NuGet keyed configuration merge contract](docs/nuget-config-merge.md)
- [NuGet source sections and mapping contract](docs/nuget-source-sections.md)
- [NuGet storage and restore policy contract](docs/nuget-storage-policy.md)
- [NuGet CLI override contract](docs/nuget-cli-overrides.md)
- [NuGet local source contract](docs/nuget-local-sources.md)
- [Central package management contract](docs/central-package-management.md)
- [NuGet service-index capability contract](docs/nuget-service-index.md)
- [NuGet source credential contract](docs/nuget-credentials.md)
- [NuGet credential-provider contract](docs/nuget-credential-providers.md)
- [NuGet client-certificate contract](docs/nuget-client-certificates.md)
- [NuGet HTTP transport policy contract](docs/nuget-http-policy.md)
- [NuGet source security contract](docs/nuget-source-security.md)
- [Unavailable pack diagnostic contract](docs/pack-diagnostics.md)
- [Framework reference planning contract](docs/framework-reference-planning.md)
- [Compiler input planning contract](docs/compiler-input-planning.md)
- [Package resolution and cache contract](docs/package-resolution.md)
- [Performance method](docs/performance-method.md)
- [Events and diagnostics](docs/events-and-diagnostics.md)
- [Compatibility matrix](docs/compatibility-matrix.md)
- [Direct Roslyn strategy](docs/roslyn-invocation.md)

## Workspace

```text
crates/dv-cli       dv executable and command surface
crates/dv-core      typed diagnostics, SDK selection, and project evaluation
tools/dv-bench      process-level benchmark harness
benchmarks/fixtures immutable representative .NET inputs
docs                contracts, evidence, and architecture decisions
issues              unresolved design questions requiring real data
```

## Development

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
cargo bench-all --quick
```

The CI matrix runs formatting, linting, tests, and release builds on Windows,
Linux, and macOS. See [CONTRIBUTING.md](CONTRIBUTING.md) before changing a
subsystem or making a performance claim.
