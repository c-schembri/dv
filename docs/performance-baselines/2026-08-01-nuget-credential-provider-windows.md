# NuGet credential-provider baseline - Windows - 2026-08-01

This baseline promotes `NUGET-009` after cross-platform V2 protocol parity,
timeout/cancellation, noninteractive-policy, and secret-containment checks.

## Environment

- Windows x86_64
- 24 logical CPUs
- .NET SDK 10.0.100
- Rust 1.94.0
- release profile with thin LTO
- 3 warm-ups and 30 retained process samples

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft `NuGet.Protocol` oracle | 115.621 ms | 2238.289 ms | 107.793 ms | 2249.576 ms |
| `dv` | 22.519 ms | 28.833 ms | 18.459 ms | 28.950 ms |

`dv` is 5.1x faster by median.

Two retained Microsoft samples took about 2.24 seconds while waiting for the
managed plugin process to close. They remain in the raw sample set and p95;
the median is robust to those observed lifecycle stalls.

## State boundary

Both commands launch the same release-built self-contained fixture provider
and perform the symmetric V2 handshake, process monitoring, initialization,
authentication-claim query, noninteractive credential request, and shutdown.
Oracle compilation and provider compilation are outside the timed boundary.
No HTTP request occurs.

Preflight compares the redacted Basic result, proves exactly one provider was
selected by Microsoft, verifies `IsNonInteractive=true` and
`CanShowDialog=false` in both traces, rejects both fixture secrets from stdout
and stderr, verifies opt-in interactive login output and its protocol
acknowledgement, and forces `dv` through its one-second timeout path while
proving the client sent `Cancel` before reaping the provider.

## Reproduce

```powershell
cargo bench-all --case nuget_credential_provider --samples 30 --warmups 3
```
