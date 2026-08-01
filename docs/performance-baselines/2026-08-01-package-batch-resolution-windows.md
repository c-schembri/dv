# Package Batch-Resolution Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: .NET `10.0.100`; all projects target `net10.0`.
- Samples: 30 retained samples after 3 warm-ups.
- Input: one root project with two literal project references. Both children
  select the same eight-package cousin/diamond graph from a deterministic
  fifteen-archive local feed.
- State: `.packages`, all `obj` directories, `packages.lock.json`, and
  `dv.lock.json` are removed before every sample. No network source exists.
- Output: Microsoft and `dv` must select the same eight identity/version rows
  for each child. `dv` must emit the package-free root then both children,
  report 16 resolved rows, publish eight archives total, and make zero HTTP
  requests.

## Commands

```text
dotnet restore PackageBatch.csproj --packages .packages --nologo --verbosity quiet
dv restore PackageBatch.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 700.911 ms | 880.634 ms | 670.221 ms | 963.705 ms |
| `dv` | 51.502 ms | 64.606 ms | 36.796 ms | 68.278 ms |

`dv` is 13.6x faster by median. The timed interval includes process startup,
project-reference closure evaluation, project/config parsing, local-feed
discovery, cold archive validation/publication, two independent graph
convergences, command-local parsed-metadata reuse, lock writes, and structured
output.

Reproduce:

```powershell
cargo bench-all --case package_batch_resolution --samples 30 --warmups 3
```
