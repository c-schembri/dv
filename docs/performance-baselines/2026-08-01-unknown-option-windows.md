# Unknown-option rejection baseline - Windows - 2026-08-01

This baseline promotes `CLI-011` after the active command surface was changed
to reject unknown options before unrelated filesystem, SDK, project, or network
work. The accepted SDK path produces one borrowed typed request without dynamic
allocation.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after 3 warm-ups
- warm OS file caches; release builds and parity checks outside timed intervals
- default Cargo compiler concurrency

## Commands

```text
dotnet build --definitely-unknown
C:\Projects\dv\target\release\dv.exe build --definitely-unknown
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| `dotnet` | 146.054 ms | 152.690 ms | 141.150 ms | 154.149 ms |
| `dv` | 4.827 ms | 6.424 ms | 3.914 ms | 6.467 ms |

`dv` was 30.3x faster at the median.

## Parity gate

The harness requires both commands to fail on the same option. The Microsoft
result must return exit code 1 with `MSB1001` and `Unknown switch`; the native
`dv` result must return exit code 2 with `DV0002` at the build boundary. It also
snapshots the fixture before each preflight and rejects any workspace mutation.

Focused CLI tests place malformed `global.json` and project XML in the working
directory, then exercise unknown global, help/version, SDK, project, build,
restore, and sync options. Every case must return `DV0002` without SDK/project
diagnostics or generated `obj`/package state. The compatibility profile is
separately required to preserve reference exit code 1.

Reproduce:

```powershell
cargo bench-all --case cli_unknown_option --samples 30 --warmups 3
```
