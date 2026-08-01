# CLI Option Effect Baseline - Windows - 2026-08-02

This like-for-like baseline exercises the `DROP-022` closed option boundary.
Both tools receive the same `test --definitely-unknown` request, reject the
same sentinel before project work, return exit 1, and leave an identical
fixture tree.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- release binaries and maximum Cargo compiler concurrency
- 50 retained samples after ten warm-ups; warm OS caches

## Commands

```text
dotnet test --definitely-unknown
C:\Projects\dv\target\release\dv.exe --compat dotnet test --definitely-unknown
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 140.183 ms | 159.044 ms | 133.144 ms | 178.738 ms |
| `dv` | 6.035 ms | 7.126 ms | 4.280 ms | 7.819 ms |

`dv` is **23.2x faster** at the median. The Microsoft oracle must report
`MSB1001`, `Unknown switch`, and the exact sentinel. `dv` must report `DV0002`
and the same sentinel under the dotnet exit profile, without falling through
to its `DV0003` unimplemented-operation boundary. Preflight snapshots the
fixture before and after both commands and rejects any mutation.

An earlier run was discarded before retention because unrelated system load
produced a visibly bimodal 5/105 ms `dv` distribution. The retained rerun is
unimodal and the raw sample batch is stored at the command below.

## Cost

The `dv` path performs one linear scan over the borrowed semantic token batch.
The common zero-environment case allocates no option storage and performs no
filesystem read, SDK discovery, child-process launch, or network request.
The 136-byte child option batch holds a borrowed project selection, one-byte
configuration, and four inline environment edits; a fifth externally supplied
edit promotes once to contiguous dynamic storage. The measured Windows release
executable is 7,441,408 bytes.

Reproduce:

```powershell
cargo bench-all --case cli_option_effects --samples 50 --warmups 10 --output benchmarks/results/2026-08-02-cli-option-effects-windows.json
```
