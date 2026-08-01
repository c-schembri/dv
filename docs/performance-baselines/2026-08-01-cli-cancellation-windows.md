# Command cancellation baseline - Windows - 2026-08-01

This baseline promotes `CLI-014`: early Ctrl+C/SIGINT installation and one
bounded cancellation deadline shared with child processes.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after 5 warm-ups
- warm OS file caches; release builds and verification outside timing
- default maximum Cargo compiler concurrency

## Timed Contract

The like-for-like commands select and print the active SDK:

```text
dotnet --version
dv sdk current
```

`dv` installs its command-lifetime handler before SDK discovery. The timed path
therefore includes native process launch, one shared cancellation-state
allocation, handler registration, SDK discovery and selection, and output.

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft .NET 10 | 68.262 ms | 71.868 ms | 65.596 ms | 72.110 ms |
| `dv` | 6.309 ms | 7.356 ms | 5.308 ms | 7.863 ms |

`dv` was 10.8x faster at the median. The complete stable batch is reported; no
retained sample was removed.

The non-work `dv --version` control, which deliberately skips cancellation
installation and SDK discovery, measured 5.364 ms median, 5.764 ms p95,
4.636 ms minimum, and 7.104 ms maximum. Its 0.945 ms median difference from
`dv sdk current` is only an upper bound on handler cost because the latter also
performs SDK discovery and selection.

## Correctness Gate

Before samples are accepted, the harness requires:

- identical selected-SDK text from Microsoft and `dv`;
- `cancellation_grace_ms=2000` at the typed run/test child boundary;
- an uncooperative credential-provider child to receive NuGet `Cancel`, ignore
  it, then be killed and reaped at the two-second deadline;
- no credential text in either output stream.

Core tests prove stable absolute-deadline calculation, first/second signal
transitions, cancellation before work, and prompt interruption of an in-flight
HTTP request. Unix integration additionally sends real SIGINT to a blocked
`dv restore` process and requires `DV0005`, a `cancelled` event outcome, and no
lock publication. All supported CI platforms execute ordinary work commands,
which fails immediately if handler installation is unavailable.

## Cost And Layout

`CancellationToken` is one pointer. Its one necessary shared allocation is
64-byte aligned and keeps the atomic first-signal timestamp and one-byte phase
first. The monotonic epoch and platform-sized Tokio notification follow; the
full state is 64 bytes on this Windows host and compile-time capped at two
assumed cache lines on supported platforms. Clones share that allocation;
waits do not allocate a timer task or use dynamic dispatch. Help, self-version,
global-option failure, and unknown-command paths do not allocate this state or
start the handler thread.

Raw samples are retained as
`benchmarks/results/2026-08-01-cli-cancellation-windows.json` and
`benchmarks/results/2026-08-01-cli-cancellation-version-control-windows.json`.

Reproduce:

```powershell
cargo bench-all --case cli_cancellation --samples 30 --warmups 5
cargo bench-all --case cli_version --samples 30 --warmups 5
```
