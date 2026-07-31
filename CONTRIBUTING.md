# Contributing

## Prerequisites

- Rust 1.94.0, installed automatically by `rustup` from
  `rust-toolchain.toml`
- .NET SDK 10.x for the current stable reference benchmark fixture

## Before changing a subsystem

Describe the concrete input and output data, the common case, known ranges,
ownership and lifetime, the dominant cost, and the evidence that will establish
success. Record unobservable facts as `ASSUMPTION` rather than inventing data.
Performance changes require before/after measurements from the same fixture and
machine state.

Do not add a fallback to `dotnet`, MSBuild, NuGet, or VSTest in production code.
The benchmark harness is the only place allowed to invoke the reference
Microsoft toolchain.

## Required checks

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

Run the quick benchmark smoke test when changing the harness:

```powershell
cargo bench-all --quick
```

Generated benchmark results under `benchmarks/results/` are deliberately
ignored. Curated, reviewed baselines belong in `docs/performance-baselines/`.
