# Typed Transform Equivalence Windows Baseline

`DROP-019` compares the pinned .NET 10 restore rejection with the explicit
dotnet compatibility spelling consumed by `dv`. Both commands parse the same
operand batch, reject the same sentinel before project discovery, and leave
the fixture tree unchanged.

## Method

- Windows x86_64, 24 logical CPUs
- .NET SDK `10.0.100`
- 10 warm-ups and 50 retained samples per command
- warm OS caches and release binaries
- immutable `small-console` fixture
- preflight snapshots the fixture before and after both commands
- timed output validates the reference failure and the typed canonical `dv`
  restore boundary, including explicit compatibility provenance

```text
dotnet restore --definitely-unknown
target\release\dv.exe --compat dotnet restore --definitely-unknown
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 141.390 ms | 163.790 ms | 133.809 ms | 187.782 ms |
| `dv` | 5.606 ms | 6.352 ms | 4.620 ms | 6.480 ms |

`dv` is `25.2x` faster at the median. Its measured path captures argv once,
selects the compatibility profile, creates a one-word borrowed transform view,
and rejects the unknown option. It performs no SDK, project, filesystem,
network, or managed-process work.

Raw samples are retained in
`benchmarks/results/2026-08-02-cli-transform-equivalence-windows.json`.

## Reproduce

```powershell
cargo bench-all --case cli_transform_equivalence --samples 50 --warmups 10
```
