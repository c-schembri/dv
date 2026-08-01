# Child argument forwarding baseline - Windows - 2026-08-01

This baseline promotes `CLI-012`. It measures the process-entry forwarding
transform, not application execution: `dv run` remains explicitly TBI until
the ordered build/run workflow is implemented.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after 5 warm-ups
- warm OS file caches; release builds and oracle compilation outside timing
- default Cargo compiler concurrency

## Commands

```text
dotnet bin/Release/net10.0/ArgumentForwarding.dll alpha "" --color "two words"
C:\Projects\dv\target\release\dv.exe --json run -- alpha "" --color "two words"
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft host | 44.698 ms | 51.902 ms | 40.795 ms | 55.777 ms |
| `dv` forwarding boundary | 5.606 ms | 6.184 ms | 4.716 ms | 6.763 ms |

The native forwarding transform was 8.0x faster at the median. This ratio is
not a like-for-like `run` claim because `dv` does not launch the application in
this slice.

The affected no-delimiter control measured `dotnet --version` at `77.833 ms`
and `dv sdk current` at `6.102 ms` median (`12.8x`). Its `dv` p95 was
`6.892 ms`. This matches the earlier published `6.102 ms` median and therefore
shows no practical common-path regression; no speedup is attributed to the new
delimiter branch.

## Correctness gate

Before timing, the harness builds `ArgumentForwarding.csproj`, invokes the real
.NET 10 `dotnet run --` boundary, and requires the managed application to
observe this exact ordered batch:

```text
alpha
<empty>
--color
two words
```

The `dv` JSON stream must retain the identical tail after its delimiter and
the typed run diagnostic must report four forwarded arguments. Unit tests also
cover a platform-native non-Unicode token, an empty tail, interspersed globals
before the delimiter, opaque global spellings after it, and rejection by
non-child commands before SDK/project/filesystem/network discovery.

## Cost

The raw process argument batch remains the only owner. One machine-word optional
nonzero delimiter index and one 16-byte borrowed slice describe the tail. Successful parsing
performs no forwarding-specific allocation, copy, Unicode conversion,
filesystem access, process launch, or network request. JSON reporting allocates
only at the existing output edge.

Reproduce:

```powershell
cargo bench-all --case cli_forwarding --samples 30 --warmups 5
cargo bench-all --case sdk_current --samples 30 --warmups 3
```
