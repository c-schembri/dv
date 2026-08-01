# Architecture-Selected Dotnet Inventory Baseline - Windows - 2026-08-02

This like-for-like baseline measures `SDK-006` through the .NET 10
architecture-selected runtime inventory. Both commands resolve the registered
x86 installation, enumerate the same 12 shared frameworks, and emit
byte-identical rows after CRLF normalization.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 12 x86 runtimes across three framework families
- release binaries and maximum Cargo compiler concurrency
- 200 retained samples after 20 warm-ups; warm OS caches

## Commands

```text
dotnet --list-runtimes --arch x86
C:\Projects\dv\target\release\dv.exe --compat dotnet --list-runtimes --arch x86
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 8.539 ms | 10.646 ms | 5.149 ms | 15.847 ms |
| `dv` | 5.760 ms | 7.334 ms | 4.409 ms | 9.152 ms |

`dv` is **1.48x faster** at the median and lower at p95. This includes registry
selection, filesystem enumeration, semantic ordering, and one buffered output
write in both tools.

Preflight also compares current and x86 SDK/runtime inventories exactly. The
x86 SDK inventory is empty in both tools, and missing alternate installations
are accepted only when both commands return successful empty output. The
fixture is snapshotted before and after the read-only queries.

Reproduce:

```powershell
cargo bench-all --case dotnet_runtime_inventory_arch --samples 200 --warmups 20 --output benchmarks/results/2026-08-02-dotnet-driver-inventory-arch-windows.json
```
