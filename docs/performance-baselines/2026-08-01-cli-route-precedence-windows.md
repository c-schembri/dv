# CLI Route Precedence Baseline - Windows - 2026-08-01

This baseline promotes `DROP-010`. It measures ambiguous `pack` routing at a
like-for-like pre-I/O rejection boundary and retains SDK selection as the
common startup control.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- release binaries and maximum Cargo compiler concurrency
- 50 retained samples after ten warm-ups; warm OS caches

## Command Boundary

```text
dotnet pack --definitely-unknown
C:\Projects\dv\target\release\dv.exe --compat dotnet pack --definitely-unknown
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 280.174 ms | 306.891 ms | 263.165 ms | 315.002 ms |
| `dv` | 5.242 ms | 5.841 ms | 4.232 ms | 6.099 ms |

Both commands reject the same invalid pack option before mutating the fixture.
The Microsoft oracle requires exit 1, `MSB1001`, and `Unknown switch`; `dv`
requires compatibility exit 1, `DV0002`, and the original `pack` spelling.
`dv` is `53.4x` faster at the median. Every sample remains in the raw result.

Unit coverage exhausts the native/dotnet, NuGet, MSBuild-input, and
VSTest-input outcomes for `restore`, `pack`, `push`, `list`, `add`, `remove`,
and `update`. Integration coverage places malformed project inputs behind
NuGet/MSBuild/VSTest `restore` routes and proves those routes return TBI before
project, SDK, filesystem, process, or network work.

## SDK Control

| Tool | Command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| Microsoft | `dotnet --version` | 62.840 ms | 64.847 ms | 61.039 ms | 67.818 ms |
| `dv` | `dv sdk current` | 4.908 ms | 5.550 ms | 4.532 ms | 5.642 ms |

The unchanged selected-SDK result remains `12.8x` faster at the median.

## Cost

Routing maps the borrowed first token to one of seven rows, then indexes one
35-byte read-only matrix with the already-typed mode. It adds no table scan,
hash lookup, dynamic allocation, copied token, second argument scan,
filesystem operation, network request, or process launch. The semantic
command remains one byte and `InvocationRequest` remains 6 bytes at alignment
2.

Reproduce:

```powershell
cargo bench-all --case cli_route_precedence --samples 50 --warmups 10 --output benchmarks/results/2026-08-01-cli-route-precedence-windows.json
cargo bench-all --case sdk_current --samples 50 --warmups 10 --output benchmarks/results/2026-08-01-cli-route-precedence-sdk-control-windows.json
```
