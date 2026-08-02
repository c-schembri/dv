# Active Filesystem Case Windows Baseline

This `WS-008` baseline measures a cold, network-free project restore. Setup
resets the workspace outside timing and adds a lowercase library only when the
active directory exposes it as a distinct file. On the benchmark NTFS directory
both reference spellings resolve to one physical library, so each tool restores
the same root-plus-library graph.

Validation requires one Microsoft `project.assets.json` reference and two `dv`
`package_resolution_created` events, zero `dv` network requests, and a successful
completion. Package sources are cleared by the fixture.

## Environment

- Windows 11, x86-64
- AMD Ryzen 9 9900X, 24 logical processors
- .NET SDK `10.0.100`
- release binaries; 30 retained samples after 5 warm-ups

## Commands

```text
dotnet restore Root.csproj --nologo --configfile NuGet.Config --packages .packages --disable-parallel
C:\Projects\dv\target\release\dv.exe restore Root.csproj --offline --configfile NuGet.Config --packages .packages --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 498.186 ms | 503.552 ms | 492.935 ms | 505.580 ms |
| `dv` | 5.541 ms | 6.146 ms | 5.199 ms | 6.288 ms |

`dv` is **89.9x faster** at the median. No sample was removed. The timed
interval includes process startup, project/config parsing, graph identity,
restore planning, output writes, and capture. Fixture reset and active-case
setup are outside timing.

The affected ancestor-input case was also remeasured with 30 samples after 5
warm-ups: Microsoft measured `128.756 ms` median and `131.403 ms` p95, while
`dv` measured `4.155 ms` and `4.810 ms`. The `dv` median is 14.2% below the
`WS-005` baseline, so the additional spelling-preservation work shows no
regression in this run.

Reproduce:

```powershell
cargo bench-all --case filesystem_case_identity --samples 30 --warmups 5 --output target/filesystem-case-ws008-final.json
cargo bench-all --case workspace_inputs --samples 30 --warmups 5 --output target/workspace-inputs-ws008-final.json
```
