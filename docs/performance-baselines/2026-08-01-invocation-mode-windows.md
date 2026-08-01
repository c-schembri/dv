# Invocation Mode Classification Baseline - Windows - 2026-08-01

This baseline promotes `DROP-003`. It measures explicit `dotnet`-profile
classification at a like-for-like pre-I/O rejection boundary and retains SDK
selection as the common startup control.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- release binaries and maximum Cargo compiler concurrency
- 50 retained samples after ten warm-ups; warm OS caches

## Command Boundary

```text
dotnet build --definitely-unknown
C:\Projects\dv\target\release\dv.exe --compat dotnet build --definitely-unknown
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 152.984 ms | 193.102 ms | 134.513 ms | 198.339 ms |
| `dv` | 5.641 ms | 6.630 ms | 4.361 ms | 6.879 ms |

Both commands reject the same invalid build option before mutating the
fixture. The Microsoft oracle requires exit 1 and its unknown-switch failure;
`dv` requires compatibility exit 1, `DV0002`, and no discovery diagnostic.
`dv` is `27.1x` faster at the median. Every retained sample remains in the raw
result without outlier removal.

Unit coverage exercises all four compatibility profiles in separated and
combined forms before and after the command. Missing, unsupported,
non-Unicode, and repeated selectors reject, while selector-looking text after
a run/test `--` delimiter remains opaque child data.

## SDK Control

| Tool | Command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| Microsoft | `dotnet --version` | 67.918 ms | 170.184 ms | 64.160 ms | 2139.604 ms |
| `dv` | `dv sdk current` | 5.637 ms | 6.374 ms | 5.065 ms | 6.459 ms |

The unchanged selected-SDK result remains `12.0x` faster at the median. One
Microsoft sample reached 2.140 seconds; it remains in the retained batch.

## Cost

Classification is one linear scan over the already-owned OS argument batch.
The transient state is 5 bytes at byte alignment: three global-policy bytes,
one mode byte, and one explicitness bitset. It replaces independent booleans
without adding persistent request state, allocation, copying, hashing,
filesystem access, network requests, or process launches. The semantic request
remains 6 bytes at alignment 2.

Reproduce:

```powershell
cargo bench-all --case cli_mode_classification --samples 50 --warmups 10 --output benchmarks/results/2026-08-01-invocation-mode-windows.json
cargo bench-all --case sdk_current --samples 50 --warmups 10 --output benchmarks/results/2026-08-01-invocation-mode-sdk-control-windows.json
```
