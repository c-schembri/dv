# Explicit Compatibility Diagnostics Baseline - Windows - 2026-08-02

This baseline promotes `DROP-011`. It measures explicit `dotnet`-profile
selection plus stable diagnostic context at a like-for-like pre-I/O rejection
boundary and retains SDK selection as the common startup control.

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
| Microsoft | 133.281 ms | 147.033 ms | 122.485 ms | 152.080 ms |
| `dv` | 5.125 ms | 6.256 ms | 4.336 ms | 6.638 ms |

Both commands reject the same invalid build option before mutating the
fixture. The Microsoft oracle requires exit 1 and its unknown-switch failure;
`dv` requires compatibility exit 1, `DV0002`, no discovery diagnostic, and
exactly one `compatibility_profile: dotnet` context row. `dv` is `26.0x`
faster at the median. Every retained sample remains in the raw result.

Human tests cover all four explicit profiles. JSON coverage proves the context
is one structured name/value row, while native failures omit it. Invalid and
repeated selectors return native usage exit 2 and cannot claim a profile.

## SDK Control

| Tool | Command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| Microsoft | `dotnet --version` | 67.806 ms | 72.111 ms | 64.291 ms | 72.479 ms |
| `dv` | `dv sdk current` | 5.763 ms | 7.395 ms | 4.794 ms | 8.197 ms |

The unchanged selected-SDK result remains `11.8x` faster at the median.

## Cost

The selected mode remains one byte in the existing four-byte invocation
options and adds nothing to the six-byte semantic request. Only the common
failure boundary allocates the two owned strings required by the diagnostic
wire format and appends one context record; an empty or full context vector
also grows its error-only storage. Successful commands perform no new branch,
allocation, copy, filesystem operation, network request, or process launch.

Reproduce:

```powershell
cargo bench-all --case cli_mode_classification --samples 50 --warmups 10 --output benchmarks/results/2026-08-02-cli-compat-diagnostics-windows.json
cargo bench-all --case sdk_current --samples 50 --warmups 10 --output benchmarks/results/2026-08-02-cli-compat-diagnostics-sdk-control-windows.json
```
