# CLI Command Normalization Baseline - Windows - 2026-08-01

This baseline promotes `DROP-002`. It measures one accepted alias at the
pre-I/O rejection boundary and retains SDK selection as the common startup
control.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- release binaries and maximum Cargo compiler concurrency
- 30 retained samples after five warm-ups; warm OS caches

## Command Boundary

```text
dotnet restore --definitely-unknown
C:\Projects\dv\target\release\dv.exe sync --definitely-unknown
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 121.211 ms | 128.378 ms | 118.184 ms | 130.433 ms |
| `dv` | 5.462 ms | 6.337 ms | 4.253 ms | 6.596 ms |

Both commands reject the same invalid restore option before mutating the
fixture. The Microsoft oracle requires exit 1, `MSB1001`, and `Unknown switch`;
`dv` requires native exit 2, `DV0002`, and the original `sync` spelling in its
diagnostic. `dv` is `22.2x` faster at the median.

Unit coverage exhausts all 19 accepted spellings and 14 semantic kinds.
`restore`, `sync`, and explicit `dotnet` provenance produce equal six-byte
semantic requests and identical borrowed operand batches. Raw spelling and
compatibility mode remain separate cold data. Unknown and non-Unicode command
text still fail before discovery.

## SDK Control

| Tool | Command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| Microsoft | `dotnet --version` | 65.717 ms | 74.226 ms | 63.236 ms | 77.629 ms |
| `dv` | `dv sdk current` | 4.930 ms | 5.758 ms | 4.461 ms | 5.959 ms |

The unchanged selected-SDK result remains `13.3x` faster at the median. No
startup improvement is claimed from shrinking the semantic record; process
startup noise dominates this transform at one request per process.

## Cost

The common scan remains linear and adds no allocation, filesystem operation,
network request, process launch, copied token, or hash lookup. The hot request
shrinks from 16 bytes at machine-word alignment to 6 bytes at alignment 2. One
machine-word raw index and one-byte compatibility mode remain in the cold
process-lifetime owner, where reporting and failure policy need them.

Reproduce:

```powershell
cargo bench-all --case cli_command_normalization --samples 30 --warmups 5 --output benchmarks/results/2026-08-01-cli-command-normalization-windows.json
cargo bench-all --case sdk_current --samples 30 --warmups 5 --output benchmarks/results/2026-08-01-cli-command-normalization-sdk-control-windows.json
```
