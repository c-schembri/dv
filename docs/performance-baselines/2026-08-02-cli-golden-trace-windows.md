# Golden CI Trace Windows Baseline

`DROP-020` replays the valid offline-restore command from the repository's
schema-version-1 GitHub Actions fixture. The reference and candidate receive
identical arguments after the executable token, environment, empty stdin,
project tree, empty local package source, and isolated package directory.

The preflight also checks the fixture's selected-SDK command. It validates both
tools' exact normalized stdout, stderr, exit code, and sorted filesystem delta
against the checked-in golden observations. Process and network dimensions are
explicitly `TBI` and are not interpreted as zero.

## Method

- Windows x86_64, 24 logical CPUs
- .NET SDK `10.0.100`
- 10 warm-ups and 50 retained samples per command
- warm OS caches, no network dependency
- fresh `small-console` copy and empty local source before every sample
- fixture reset, trace parsing, full preflight, and filesystem snapshots
  outside the timed interval

```text
dotnet restore SmallConsole.csproj --packages .packages --source offline-source --verbosity quiet
target\release\dv.exe restore SmallConsole.csproj --packages .packages --source offline-source --verbosity quiet
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 626.828 ms | 2658.372 ms | 533.239 ms | 5416.511 ms |
| `dv` | 7.507 ms | 9.153 ms | 6.254 ms | 9.929 ms |

`dv` is `83.5x` faster at the median. This is a like-for-like zero-package
resolution workload, not full output-artifact parity: the golden deliberately
records that Microsoft creates its `obj` restore artifacts while `dv` reports
its native resolution result without writing those Microsoft-owned files.
Downstream `dv` workflows consume the native result; `project.assets.json`
compatibility remains owned by `RES-021`.

Raw samples are retained in
`benchmarks/results/2026-08-02-cli-golden-trace-windows.json`.

## Reproduce

```powershell
cargo bench-all --case cli_golden_trace --samples 50 --warmups 10
```
