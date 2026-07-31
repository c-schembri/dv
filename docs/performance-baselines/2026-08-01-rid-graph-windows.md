# Portable RID graph baseline - Windows - 2026-08-01

This baseline promotes `PACKS-005` after `dv` matched the selected SDK's
official `NuGet.Packaging` breadth-first expansion for `linux-musl-x64`.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- selected graph: 85 nodes, 133 direct edges, 494 compatibility indices
- 30 retained samples after 3 warm-ups
- warm OS file caches; oracle and Rust builds outside timed intervals
- Cargo compiler concurrency restricted to one job

## Commands

```text
dotnet bin/Release/RidGraphOracle.dll linux-musl-x64
C:\Projects\dv\target\release\dv.exe sdk compatible-rids linux-musl-x64
```

The reference executable is a minimal adapter over the selected SDK's shipped
`NuGet.Packaging.dll`. Its build and graph copy occur before timing.

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 36.217 ms | 39.263 ms | 33.959 ms | 39.416 ms |
| `dv` | 6.049 ms | 6.859 ms | 5.261 ms | 7.073 ms |

`dv` was 6.0x faster at the median.

## Parity gate

Before timing, the harness verifies identical selected SDK versions, builds
the oracle in an isolated workspace, and compares every output RID in order.
For this query both tools emit:

```text
linux-musl-x64
linux-musl
linux-x64
linux
unix-x64
unix
any
base
```

Reproduce:

```powershell
cargo bench-all --case rid_graph --samples 30 --warmups 3
```
