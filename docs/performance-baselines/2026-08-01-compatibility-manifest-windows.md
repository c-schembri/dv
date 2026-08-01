# Compatibility Manifest Baseline - Windows - 2026-08-01

## Environment

- Windows 11 `10.0.22631`, x86-64.
- AMD Ryzen 9 9900X, 12 cores, 24 hardware threads.
- .NET SDK `10.0.100`, MSBuild `18.0.2.52411`, NuGet `7.0.0.0`, and VSTest
  `18.0.1`.
- Release binaries, warm OS cache, 30 retained samples after five warm-ups.

## Structural Query

Command: `dv compat manifest`.

Preflight requires the normal and global-JSON spellings to return byte-identical
valid JSON with schema/version `1`, command syntax `1`, event schema `19`, the
selected SDK, at least 100 command records, and all 468 parity rows. The current
artifact contains 115 commands, 769 options, 74 arguments, and 270,082 bytes.

| Tool | Median | p95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | TBI | TBI | TBI | TBI |
| `dv` | 5.306 ms | 6.045 ms | 4.884 ms | 6.190 ms |

Microsoft has no command that emits `dv`'s selected-tool compatibility and
support ledger, so no like-for-like speed ratio is claimed.

## Like-For-Like Control

Preflight requires both commands to select SDK `10.0.100`.

| Tool | Command | Median | p95 | Min | Max |
|---|---|---:|---:|---:|---:|
| Microsoft | `dotnet --version` | 63.347 ms | 66.926 ms | 62.007 ms | 67.980 ms |
| `dv` | `dv sdk current` | 4.501 ms | 5.029 ms | 4.204 ms | 5.059 ms |

`dv` is `14.1x` faster by median for the comparable selected-SDK result.

## Startup And Size Cost

Embedding the minified artifact increased the Windows release executable from
7,061,504 to 7,333,376 bytes, a 271,872-byte cost. Two interleaved 50-sample
orders compared `dv --version` before and after the feature:

| Order | Before median | After median |
|---|---:|---:|
| before, then after | 4.385 ms | 4.490 ms |
| after, then before | 4.536 ms | 4.429 ms |

The sign reverses with order and p95 remains overlapping, so no startup change
is claimed. The query itself pays one 270,082-byte stdout write. Other commands
perform no manifest read or parse.

## Reproduce

```text
cargo bench-all --case cli_compat_manifest --samples 30 --warmups 5 --output benchmarks/results/2026-08-01-compatibility-manifest-windows.json
cargo bench-all --case sdk_current --samples 30 --warmups 5 --output benchmarks/results/2026-08-01-compatibility-manifest-sdk-control-windows.json
```

Raw samples are machine-local and gitignored; this reviewed document retains
the result.
