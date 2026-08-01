# CLI Lexical Preservation Baseline - Windows - 2026-08-02

This baseline promotes `DROP-013`. It measures a combined configuration token
at a like-for-like pre-I/O rejection boundary and retains SDK selection as the
common startup control.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- release binaries and maximum Cargo compiler concurrency
- 50 retained samples after ten warm-ups; warm OS caches

## Command Boundary

```text
dotnet build -c:Release --definitely-unknown
C:\Projects\dv\target\release\dv.exe --compat dotnet build -c:Release --definitely-unknown
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 141.461 ms | 176.976 ms | 127.015 ms | 206.037 ms |
| `dv` | 4.912 ms | 6.003 ms | 4.234 ms | 7.008 ms |

Both commands accept the exact `-c:Release` combined token and reject the same
following sentinel without mutating the fixture. The Microsoft oracle requires
exit 1 and `MSB1001` for `--definitely-unknown`; `dv` requires compatibility
exit 1, `DV0002`, the same sentinel, one `compatibility_profile: dotnet` row,
and no discovery diagnostic. `dv` is `28.8x` faster at the median. Every raw
sample is retained in the result file.

Focused tests cover exact command and option case, Windows slash-prefix policy,
separate/equals/colon configuration forms, mixed singleton repetitions,
ordered repeatable sources, non-Unicode and empty tokens, and both leading and
command-level `--` boundaries.

## SDK Control

| Tool | Command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| Microsoft | `dotnet --version` | 71.282 ms | 111.638 ms | 65.280 ms | 258.786 ms |
| `dv` | `dv sdk current` | 6.386 ms | 7.402 ms | 5.597 ms | 8.158 ms |

The unchanged selected-SDK result remains `11.2x` faster at the median.

## Cost

The hot transform remains one linear scan over the contiguous process-owned OS
tokens. Direct command arguments are one borrowed slice. Interleaved dv globals
use up to sixteen inline 16-bit indices and spill only for larger externally
variable batches. Combined values borrow a suffix after one exact ASCII
separator check. Successful parsing adds no token copy, filesystem operation,
network request, or process launch; error diagnostics retain their existing
error-only allocations.

Reproduce:

```powershell
cargo bench-all --case cli_lexical_preservation --samples 50 --warmups 10 --output benchmarks/results/2026-08-02-cli-lexical-preservation-windows.json
cargo bench-all --case sdk_current --samples 50 --warmups 10 --output benchmarks/results/2026-08-02-cli-lexical-preservation-sdk-control-windows.json
```
