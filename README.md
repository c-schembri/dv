# dv

[![CI](https://github.com/c-schembri/dv/actions/workflows/ci.yml/badge.svg)](https://github.com/c-schembri/dv/actions/workflows/ci.yml)

`dv` is a native, data-oriented toolchain for C# and .NET development, written
in Rust.

The goal is one fast executable for SDK selection, dependency resolution,
project evaluation, building, running, testing, packaging, and publishing.
Roslyn remains the compiler and Microsoft .NET remains the runtime; `dv` owns
the expensive orchestration around them.

```text
dotnet --version               60.637 ms median
dv sdk current                  3.798 ms median

dotnet NuGet RID expansion     36.217 ms median
dv sdk compatible-rids          6.049 ms median

dotnet msbuild project query  282.186 ms median
dv project inspect              3.846 ms median

dotnet msbuild runtime query  321.215 ms median
dv runtime project inspect      5.687 ms median

dotnet msbuild compiler plan  368.952 ms median
dv build --plan                 4.979 ms median

dotnet restore (cold deps)     966.711 ms median
dv restore (cold deps)         411.275 ms median

dotnet restore (50 packages)  1364.792 ms median
dv restore (50 packages)       598.220 ms median

dotnet restore (warm locked)   552.265 ms median
dv restore (warm locked)         7.019 ms median
```

The benchmark preflight verifies the same selected SDK and the same evaluated
project properties and source items before retaining samples.

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
| Native CLI and self-version | Implemented |
| Installed SDK discovery | Implemented |
| `global.json` SDK selection | Implemented |
| Initial SDK-style project evaluation | Implemented |
| Target-aware framework and compiler input planning | Implemented |
| Human and JSON diagnostics/events | Implemented |
| Reference benchmark harness | Implemented |
| Exact package resolution, v2/v3 sources, verified cache, and lock | Initial implementation |
| Direct Roslyn compilation | Planned |
| Incremental and no-op builds | Planned |
| Application runner | Planned |
| Test, pack, and publish | Planned |

SDK discovery supports all documented roll-forward policies, prerelease
filtering, JSON comments, custom errors, .NET 10 search `paths`, and `$host$`
without launching `dotnet`.

`dv sdk compatible-rids RID` loads the selected SDK's portable RID graph as
data and returns NuGet-compatible breadth-first fallbacks. The compiled graph
stores 16-byte sorted nodes, contiguous 32-bit edges, and precomputed
compatibility ranges; it never guesses compatibility by splitting RID text.

Project evaluation supports one `Microsoft.NET.Sdk` C# project targeting one
modern unified .NET TFM, `Exe` and `Library` outputs, default source discovery,
Debug/Release configuration, project-reference paths, and exact package
references. Literal `RuntimeIdentifier` and `RuntimeIdentifiers` values become
one compact target-dimension batch rather than copies of the project. Target
family and version are parsed once and shared by pack, compiler,
dependency-group, and package-asset selection. Unsupported MSBuild behavior
fails explicitly.

The baseline tracks [.NET 10](https://dotnet.microsoft.com/en-us/download/dotnet/10.0),
the latest stable LTS release as of 2026-07-31. Preview TFMs are not selected
as the default target.

`dv restore` (also available as `dv sync`) merges the supported
`NuGet.Config` subset, speaks HTTPS NuGet v2 or v3 according to each source,
converges typed NuGet version ranges with lowest-applicable, direct-wins, and
cousin rules, then streams package payloads through SHA-512. It validates v2
source hashes and ZIP boundaries, retracts stale dependency edges when a
selection changes, publishes NuGet-compatible cache entries atomically, and
writes a deterministic `dv.lock.json`. A matching warm lock performs zero
network requests.

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

# Plan Roslyn inputs without compiling
cargo run -p dv-cli --release -- build --plan path\to\App.csproj

# Resolve exact packages and write dv.lock.json
cargo run -p dv-cli --release -- restore path\to\App.csproj

# Prove a locked package graph from cache with no network
cargo run -p dv-cli --release -- restore path\to\App.csproj --offline

# `sync` is an exact alias
cargo run -p dv-cli --release -- sync path\to\App.csproj

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

Initial machine:

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after 3 warm-ups for SDK selection, RID expansion,
  project evaluation, and the one-package cold case; compiler planning uses 5 warm-ups; 10
  retained samples after 2 warm-ups for the large cold graph; 10 retained
  samples after 3 warm-ups for warm locked restore; the massive graph uses 5
  retained samples after 1 warm-up
- warm OS caches; fixture and prerequisite setup outside timed intervals

<!-- LIKE_FOR_LIKE_BENCHMARKS_START -->
| Operation | Reference command | `dv` command | Reference median | `dv` median | Median ratio | Reference p95 | `dv` p95 |
|---|---|---|---:|---:|---:|---:|---:|
| Select current SDK | `dotnet --version` | `dv sdk current` | 60.637 ms | 3.798 ms | 16.0x | 61.595 ms | 4.322 ms |
| Expand a portable RID | `dotnet bin/Release/RidGraphOracle.dll linux-musl-x64` | `dv sdk compatible-rids linux-musl-x64` | 36.217 ms | 6.049 ms | 6.0x | 39.263 ms | 6.859 ms |
| Evaluate small project | `dotnet msbuild SmallConsole.csproj` property/item query | `dv project inspect SmallConsole.csproj --json` | 282.186 ms | 3.846 ms | 73.4x | 287.600 ms | 4.074 ms |
| Evaluate runtime target dimensions | `dotnet msbuild RuntimeProject.csproj` runtime-property query | `dv project inspect RuntimeProject.csproj --json` | 321.215 ms | 5.687 ms | 56.5x | 330.112 ms | 6.897 ms |
| Plan compiler inputs | `dotnet msbuild SmallConsole.csproj -t:ResolveReferences` property/item query | `dv build --plan SmallConsole.csproj --json` | 368.952 ms | 4.979 ms | 74.1x | 374.293 ms | 6.027 ms |
| Resolve dependencies from cold caches | `dotnet restore PackageConsole.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | `dv restore PackageConsole.csproj --packages .packages --json` | 1028.951 ms | 417.981 ms | 2.5x | 1061.502 ms | 469.712 ms |
| Resolve a cold 50-package graph | `dotnet restore LargePackageGraph.csproj --packages .packages --no-http-cache --nologo --verbosity quiet` | `dv restore LargePackageGraph.csproj --packages .packages --json` | 1425.299 ms | 632.458 ms | 2.3x | 1658.964 ms | 662.278 ms |
| Resolve a cold 203-package solution graph | `dotnet restore MassivePackageGraph.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet` | `dv restore MassivePackageGraph.csproj --packages .packages --json` | 9977.524 ms | 4325.957 ms | 2.3x | 10416.603 ms | 4852.974 ms |
| Validate warm locked packages | `dotnet restore PackageConsole.csproj --locked-mode --packages .packages --nologo --verbosity quiet` | `dv restore PackageConsole.csproj --packages .packages --offline --json` | 521.346 ms | 7.362 ms | 70.8x | 560.223 ms | 8.206 ms |
<!-- LIKE_FOR_LIKE_BENCHMARKS_END -->

Before measuring, the harness verifies SDK text and compares every requested
project property plus the ordered compile-item identities. The RID graph case
compares the complete ordered expansion against the selected SDK's shipped
`NuGet.Packaging` implementation; its tiny adapter is built outside timed
intervals. The runtime project case also verifies the selected RID, ordered
plural RID property, and unique target dimension batch. For package sync it
also compares the complete package identity, exact-version, archive-SHA-512,
and selected asset batches. The massive case additionally compares runtime,
resource, content, analyzer, build, build-multitargeting, native, and RID
runtime-target paths plus runtime-target metadata. Exact commands are printed in benchmark
output and recorded in the curated
[compiler baseline](docs/performance-baselines/2026-07-31-windows.md),
[RID graph baseline](docs/performance-baselines/2026-08-01-rid-graph-windows.md),
[runtime evaluation baseline](docs/performance-baselines/2026-08-01-runtime-evaluation-windows.md), and
[package baseline](docs/performance-baselines/2026-08-01-package-assets-windows.md).

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
cargo bench-all --case rid_graph --samples 30 --warmups 3
cargo bench-all --case project_evaluate --samples 30 --warmups 3
cargo bench-all --case runtime_evaluate --samples 30 --warmups 3
cargo bench-all --case compiler_plan --samples 30 --warmups 5
cargo bench-all --case package_sync_cold --samples 30 --warmups 3
cargo bench-all --case package_graph_cold --samples 10 --warmups 2
cargo bench-all --case package_graph_massive --samples 5 --warmups 1
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
- [Feature parity implementation map](docs/feature-parity-map.md)
- [Data-oriented agent rules](AGENTS.md)
- [SDK discovery contract](docs/sdk-discovery.md)
- [Project evaluation contract](docs/project-evaluation.md)
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
