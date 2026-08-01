# CLI protocol-version baseline - Windows - 2026-08-01

This baseline promotes `CLI-017`. It measures the explicit `dv` protocol query
and a like-for-like SDK-selection control. Microsoft tooling exposes no command
that reports `dv`'s command-syntax and event-schema versions, so no fabricated
ratio is shown for the feature-specific case.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 logical CPUs
- .NET SDK `10.0.100`
- release binaries and maximum Cargo compiler concurrency
- 30 retained samples after 5 warm-ups

## Protocol Query

Command:

```text
C:\Projects\dv\target\release\dv.exe --json --version
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft equivalent | TBI | - | - | - |
| `dv` | 4.479 ms | 5.593 ms | 4.104 ms | 5.724 ms |

These original retained samples were validated as one ordered three-event
schema-19 batch
reporting command syntax version `1`, event schema version `19`, a non-empty
tool version, and a successful terminal event. Preflight repeats the contract
through `version`, `--version`, `-V`, and explicit `dotnet` compatibility
spellings. Raw samples are retained in
`benchmarks/results/2026-08-01-cli-protocol-version-windows.json`.

The unchanged human command `dv --version` measured `4.389 ms` median,
`4.954 ms` p95, `4.053 ms` minimum, and `5.330 ms` maximum. Raw samples are in
`benchmarks/results/2026-08-01-cli-protocol-version-human-control-windows.json`.

## Like-For-Like Control

Commands:

```text
dotnet --version
C:\Projects\dv\target\release\dv.exe sdk current
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft CLI | 63.022 ms | 64.736 ms | 62.331 ms | 65.861 ms |
| `dv` | 4.828 ms | 5.252 ms | 4.459 ms | 5.364 ms |

Both commands select and print the same installed SDK. `dv` is `13.1x` faster
at the median after schema-19 reporting was added. Raw samples are retained in
`benchmarks/results/2026-08-01-cli-protocol-version-sdk-control-windows.json`.

## Cost

The syntax version remains two bytes and does not enlarge the 16-byte request.
Human output performs no added allocation or I/O. JSON output adds one integer
field per `command_started` event; only the explicit version query adds the
three string/integer fields of `tool_version`.

Reproduce:

```powershell
cargo bench-all --case cli_protocol_version --samples 30 --warmups 5
cargo bench-all --case cli_version --samples 30 --warmups 5
cargo bench-all --case sdk_current --samples 30 --warmups 5
```
