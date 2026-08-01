# Compatibility Help Windows Baseline

`DROP-017` compares the pinned .NET 10 build-help spelling with the explicit
`dv` dotnet profile. Both commands exit successfully and print their build
syntax. The `dv` output additionally names the canonical `dv build --plan`
entry point.

## Method

- Windows x86_64, 24 logical CPUs
- .NET SDK `10.0.100`
- 10 warm-ups and 50 retained samples per command
- warm OS caches
- immutable `small-console` fixture
- preflight snapshots the fixture before and after both commands and rejects
  any filesystem mutation
- timed output is validated for the reference and canonical command spellings

```text
dotnet build -?
target\release\dv.exe --compat dotnet build -?
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 135.885 ms | 152.847 ms | 125.156 ms | 155.358 ms |
| `dv` | 5.518 ms | 6.732 ms | 4.684 ms | 7.251 ms |

`dv` is `24.6x` faster at the median. The measured `dv` path performs one
lossless argument capture, profile-aware static dispatch, and stdout output;
it does not discover an SDK or project and does not touch the filesystem or
network.

Raw samples are retained in
`benchmarks/results/2026-08-02-cli-compat-help-windows.json`.

The selected-SDK compatibility control was also recaptured after correcting
the exact spelling to `dv --compat dotnet --version`: `63.402 ms` versus
`5.088 ms` median (`12.5x`). Its raw samples are retained in
`benchmarks/results/2026-08-02-sdk-current-compat-v2-windows.json`.

## Reproduce

```powershell
cargo bench-all --case cli_compat_help --samples 50 --warmups 10
cargo bench-all --case sdk_current_compat --samples 50 --warmups 10
```
