# Dotnet Driver Inventory Baseline - Windows - 2026-08-02

This like-for-like baseline measures the first executable `DNCLI-001` inventory
slice. Both commands enumerate the same installed current-architecture shared
frameworks and emit byte-identical rows after CRLF normalization.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- two complete SDKs and 15 runtimes across three framework families
- release binaries and maximum Cargo compiler concurrency
- 50 retained samples after ten warm-ups; warm OS caches

## Commands

```text
dotnet --list-runtimes
C:\Projects\dv\target/release\dv.exe --compat dotnet --list-runtimes
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 4.942 ms | 5.647 ms | 4.648 ms | 5.759 ms |
| `dv` | 4.901 ms | 5.525 ms | 4.318 ms | 5.788 ms |

`dv` is **1.01x faster** at the median and also lower at p95. This native
Microsoft query is already near the Windows process-start floor, so the small
margin is reported without exaggeration.

Preflight also compares `--list-sdks`, requires exact complete output for both
queries, and snapshots the immutable fixture before and after execution. The
timed runtime query performs the larger filesystem inventory: 15 semantic
version directories are sorted and packed into 16-byte records, then formatted
through one buffered stdout write. No project, `global.json`, managed process,
or network work is included.

Reproduce:

```powershell
cargo bench-all --case dotnet_runtime_inventory --samples 50 --warmups 10 --output benchmarks/results/2026-08-02-dotnet-driver-inventory-windows.json
```
