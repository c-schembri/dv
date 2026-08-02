# dv

[![CI](https://github.com/c-schembri/dv/actions/workflows/ci.yml/badge.svg)](https://github.com/c-schembri/dv/actions/workflows/ci.yml)

`dv` is an experimental, native .NET development toolchain written in Rust.
It aims to replace the orchestration around the .NET SDK, MSBuild, NuGet, and
test tooling with one fast executable. Roslyn remains the compiler and .NET
remains the runtime.

The project is under active development. SDK discovery, project evaluation,
compiler planning, and package restore are usable today. Compilation, run,
test, pack, and publish workflows are not complete yet.

## Why dv

- Native startup with no managed process for routine discovery and planning.
- Data-oriented, batch-first internals with compact layouts and bounded work.
- Concurrent package I/O with deterministic results.
- Explicit failures instead of silently falling back to Microsoft tools.
- Human output and a versioned JSON event stream from the same typed data.
- Correctness gates and like-for-like benchmarks for every performance claim.

## Current Capabilities

- Discover and select installed .NET SDKs using `global.json` rules.
- List installed SDKs and shared runtimes, including architecture selection.
- Inspect SDK-style C# projects and select explicit or implicit project files.
- Discover the nearest Git repository root without project evaluation.
- Discover ancestor build inputs with their native precedence rules.
- Discover ancestor SDK, NuGet, build-import, and central-package inputs in one walk.
- Evaluate conditional project, package, and framework references.
- Plan framework, runtime-pack, apphost, and Roslyn compiler inputs.
- Resolve and cache NuGet dependencies from v2, v3, and local sources.
- Apply NuGet configuration, credentials, source mapping, signing, lock, and
  package asset policies.
- Inspect the captured compatibility surface and emit structured diagnostics.

The detailed implementation ledger lives in the
[feature parity map](docs/feature-parity-map.md).

## Quick Start

Prerequisites:

- Rust `1.94.0`
- An installed .NET SDK

Build `dv`:

```powershell
cargo build -p dv-cli --release
```

Try the implemented workflows:

```powershell
# Show the selected .NET SDK (`dotnet --version` compatible)
target\release\dv.exe --version

# Show dv's own version
target\release\dv.exe self-version

# List installed SDKs and runtimes
target\release\dv.exe sdk list
target\release\dv.exe sdk runtimes

# Inspect a project
target\release\dv.exe project inspect path\to\App.csproj

# Find the nearest Git repository root
target\release\dv.exe project root path\to\working-directory

# List controlling global.json, NuGet.Config, and Directory.* inputs
target\release\dv.exe project inputs path\to\working-directory

# Create a compiler input plan without compiling
target\release\dv.exe build --plan path\to\App.csproj

# Restore packages; sync is an exact alias
target\release\dv.exe restore path\to\App.csproj
target\release\dv.exe sync path\to\App.csproj

# Emit versioned JSON events
target\release\dv.exe project inspect path\to\App.csproj --json

# Use an implemented dotnet-compatible spelling
target\release\dv.exe --compat dotnet --list-runtimes
```

Run `dv --help` or `dv <command> --help` for the current command surface.

## Benchmarks

These are representative like-for-like results from Windows 11 on a Ryzen 9
9900X with .NET SDK `10.0.100`. Setup happens outside the timed interval and
each case verifies equivalent meaningful output before retaining samples.
Results from different machines are not directly comparable.

<!-- LIKE_FOR_LIKE_BENCHMARKS_START -->
| Operation | Reference command | `dv` command | Reference median | `dv` median | Speedup |
|---|---|---|---:|---:|---:|
| Select current SDK | `dotnet --version` | `dv --version` | 65.047 ms | 5.559 ms | 11.7x |
| List installed runtimes | `dotnet --list-runtimes` | `dv --compat dotnet --list-runtimes` | 4.618 ms | 4.551 ms | 1.01x |
| Print build help | `dotnet build -?` | `dv --compat dotnet build -?` | 135.885 ms | 5.518 ms | 24.6x |
| Evaluate a project | `dotnet msbuild SmallConsole.csproj` query | `dv project inspect SmallConsole.csproj --json` | 282.186 ms | 3.846 ms | 73.4x |
| Select and evaluate the only project | `dotnet msbuild` query | `dv project inspect --json` | 303.399 ms | 6.051 ms | 50.1x |
| Find the nearest repository root | `dotnet msbuild` ancestor query | `dv project root nested/src` | 137.639 ms | 5.007 ms | 27.5x |
| Discover ancestor build inputs | `dotnet msbuild` five-input query | `dv project inputs nested/src --json` | 139.917 ms | 4.845 ms | 28.9x |
| Plan compiler inputs | `dotnet msbuild SmallConsole.csproj -t:ResolveReferences` query | `dv build --plan SmallConsole.csproj --json` | 368.952 ms | 4.979 ms | 74.1x |
| Cold one-package restore | `dotnet restore PackageConsole.csproj` | `dv restore PackageConsole.csproj` | 1028.951 ms | 417.981 ms | 2.5x |
| Cold 50-package restore | `dotnet restore LargePackageGraph.csproj` | `dv restore LargePackageGraph.csproj` | 1425.299 ms | 632.458 ms | 2.3x |
| Cold 203-package restore | `dotnet restore MassivePackageGraph.csproj` | `dv restore MassivePackageGraph.csproj` | 9977.524 ms | 4325.957 ms | 2.3x |
| Warm locked restore | `dotnet restore PackageConsole.csproj --locked-mode` | `dv restore PackageConsole.csproj --offline` | 521.346 ms | 7.362 ms | 70.8x |
<!-- LIKE_FOR_LIKE_BENCHMARKS_END -->

Run every benchmark with one command:

```powershell
cargo bench-all
```

For a fast verification pass or one named case:

```powershell
cargo bench-all --quick
cargo bench-all --case package_graph_massive --samples 5 --warmups 1
```

The console report includes the actual commands, status, min, median, p95,
max, and workload evidence. Full methodology and retained case evidence are in
[the performance docs](docs/performance-method.md) and
[performance baselines](docs/performance-baselines/).

## Project Direction

The implementation order is deliberately narrow: finish a correct vertical
workflow, benchmark it, then move to the next dependency. Near-term work closes
workspace and project input behavior before direct Roslyn compilation.

- [Project plan](PLAN.md)
- [Implementation order](docs/implementation-order.md)
- [Feature parity map](docs/feature-parity-map.md)
- [Compatibility matrix](docs/compatibility-matrix.md)
- [Protocol versioning](docs/protocol-versioning.md)
- [Data-oriented engineering rules](AGENTS.md)

## Development

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo build --workspace --release
cargo bench-all --quick
```

CI runs formatting, linting, tests, release builds, and benchmark smoke checks
on Windows, Linux, and macOS. See [CONTRIBUTING.md](CONTRIBUTING.md) before
changing a subsystem or publishing a performance claim.
