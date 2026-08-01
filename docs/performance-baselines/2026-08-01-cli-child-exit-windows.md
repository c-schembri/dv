# Child exit contract baseline - Windows - 2026-08-01

This baseline promotes `CLI-015`. It contains one structural child-boundary
measurement and one like-for-like outer-exit measurement. `dv run` remains TBI,
so the structural result is not an application-run speed claim.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 logical CPUs
- .NET SDK `10.0.100`
- release binaries; oracle compilation outside timed intervals
- default maximum Cargo compiler concurrency

## Child Boundary

Commands:

```text
dotnet bin/Release/net10.0/ArgumentForwarding.dll exit 23
C:\Projects\dv\target\release\dv.exe --json run -- exit 23
```

Thirty retained samples after five warm-ups measured:

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft managed-child oracle | 26.474 ms | 28.187 ms | 25.108 ms | 28.481 ms |
| `dv` typed child boundary | 4.883 ms | 5.768 ms | 4.287 ms | 5.923 ms |

The `dv` number measures argument capture, the declared preserve policy, and
the explicit TBI diagnostic. It does not launch the managed child, so the
apparent `5.4x` difference is structural evidence only and is deliberately
excluded from the README like-for-like table. Raw samples are retained in
`benchmarks/results/2026-08-01-cli-child-exit-windows.json`.

Preflight builds the real .NET 10 fixture and requires Microsoft to return
`23` with empty stdout/stderr. The `dv` boundary must return its documented TBI
code and report `forwarded_argument_count=2` plus
`child_exit_policy=preserve`; it cannot plausibly claim the child ran.

## Like-For-Like Exit Boundary

Commands:

```text
dotnet build --definitely-unknown
C:\Projects\dv\target\release\dv.exe build --definitely-unknown
```

Thirty retained samples after three warm-ups measured:

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft CLI | 125.249 ms | 130.131 ms | 121.475 ms | 147.346 ms |
| `dv` | 4.406 ms | 5.615 ms | 3.997 ms | 5.971 ms |

Both commands reject the same unknown build option before project/SDK work and
return their documented native failure policy. `dv` is `28.4x` faster at the
median. The gate requires Microsoft's `MSB1001`, `dv`'s `DV0002`, the exact
option spelling, and no workspace mutation. This exercises the new four-byte
outer process-exit path on a genuinely like-for-like operation. Raw samples
are retained in
`benchmarks/results/2026-08-01-cli-exit-boundary-windows.json`.

## Successful Exit Control

The like-for-like `dotnet --version` / `dv sdk current` control measured
`64.242 ms` versus `5.085 ms` median and `66.537 ms` versus `5.732 ms` p95,
leaving `dv` `12.6x` faster at the median after the outer exit change. The
non-work `dv --version` control measured `4.624 ms` median and `5.132 ms` p95.
Raw samples are retained in
`benchmarks/results/2026-08-01-cli-child-exit-sdk-control-windows.json` and
`benchmarks/results/2026-08-01-cli-child-exit-version-control-windows.json`.

## Cost

`ChildExitPolicy` is one byte. `ChildTermination` is eight bytes with four-byte
alignment; its numeric classification is allocation-free. The final CLI exit
record is one four-byte `i32`. No-child commands allocate none of the cold
launch/wait error data and perform no additional I/O, process launch, or
network work.

Reproduce:

```powershell
cargo bench-all --case cli_child_exit --samples 30 --warmups 5
cargo bench-all --case cli_unknown_option --samples 30 --warmups 3
cargo bench-all --case sdk_current --samples 30 --warmups 5
cargo bench-all --case cli_version --samples 30 --warmups 5
```
