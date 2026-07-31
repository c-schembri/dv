# dv

[![CI](https://github.com/c-schembri/dv/actions/workflows/ci.yml/badge.svg)](https://github.com/c-schembri/dv/actions/workflows/ci.yml)

`dv` is a native, data-oriented toolchain for C# and .NET development, written
in Rust.

The goal is one fast executable for SDK selection, dependency resolution,
project evaluation, building, running, testing, packaging, and publishing.
Roslyn remains the compiler and Microsoft .NET remains the runtime; `dv` owns
the expensive orchestration around them.

```text
dotnet --version               62.959 ms median
dv sdk current                  3.389 ms median

dotnet msbuild project query  296.365 ms median
dv project inspect              3.299 ms median

dotnet msbuild compiler plan  379.938 ms median
dv build --plan                 5.292 ms median
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
| .NET 10 framework and compiler input planning | Implemented |
| Human and JSON diagnostics/events | Implemented |
| Reference benchmark harness | Implemented |
| Package resolution and cache | Planned |
| Direct Roslyn compilation | Planned |
| Incremental and no-op builds | Planned |
| Application runner | Planned |
| Test, pack, and publish | Planned |

SDK discovery supports all documented roll-forward policies, prerelease
filtering, JSON comments, custom errors, .NET 10 search `paths`, and `$host$`
without launching `dotnet`.

Project evaluation supports one `Microsoft.NET.Sdk` C# project targeting
`net10.0`, `Exe` and `Library` outputs, default source discovery, Debug/Release
configuration, project-reference paths, and exact package-reference capture.
Unsupported MSBuild behavior fails explicitly.

The baseline tracks [.NET 10](https://dotnet.microsoft.com/en-us/download/dotnet/10.0),
the latest stable LTS release as of 2026-07-31. Preview TFMs are not selected
as the default target.

`dv build --plan` selects the newest installed compatible .NET 10 reference
pack, parses its manifest, validates 167 framework references on the current
SDK, selects Roslyn plus built-in analyzers, and emits one immutable compiler
input plan. It does not compile yet.

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

# Inspect the project in the current directory
cargo run -p dv-cli --release -- project inspect

# Inspect an explicit project as structured events
cargo run -p dv-cli --release -- project inspect path\to\App.csproj --json

# Plan Roslyn inputs without compiling
cargo run -p dv-cli --release -- build --plan path\to\App.csproj

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
- 30 retained samples after 3 warm-ups (5 for compiler planning)
- warm OS caches; fixture and prerequisite setup outside timed intervals

<!-- LIKE_FOR_LIKE_BENCHMARKS_START -->
| Operation | Reference command | `dv` command | Reference median | `dv` median | Median ratio | Reference p95 | `dv` p95 |
|---|---|---|---:|---:|---:|---:|---:|
| Select current SDK | `dotnet --version` | `dv sdk current` | 62.959 ms | 3.389 ms | 18.6x | 68.050 ms | 4.033 ms |
| Evaluate small project | `dotnet msbuild SmallConsole.csproj` property/item query | `dv project inspect SmallConsole.csproj --json` | 296.365 ms | 3.299 ms | 89.8x | 333.900 ms | 3.785 ms |
| Plan compiler inputs | `dotnet msbuild SmallConsole.csproj -t:ResolveReferences` property/item query | `dv build --plan SmallConsole.csproj --json` | 379.938 ms | 5.292 ms | 71.8x | 408.786 ms | 6.184 ms |
<!-- LIKE_FOR_LIKE_BENCHMARKS_END -->

Before measuring, the harness verifies SDK text and compares every requested
project property plus the ordered compile-item identities. The exact MSBuild
query is printed in benchmark output and recorded in the
[curated baseline](docs/performance-baselines/2026-07-31-windows.md).

The warm one-shot target for lightweight commands on this machine is `5 ms`
end to end. It is a local engineering budget, not a universal Windows
guarantee.

Reproduce the comparison:

```powershell
cargo bench-all --case sdk_current --samples 30 --warmups 3
cargo bench-all --case project_evaluate --samples 30 --warmups 3
cargo bench-all --case compiler_plan --samples 30 --warmups 5
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
