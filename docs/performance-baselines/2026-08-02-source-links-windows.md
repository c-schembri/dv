# Source Link Safety Windows Baseline

This like-for-like baseline measures `WS-007` on a real directory junction.
Both tools scan one immutable SDK project and return the same logical source
batch: `Alias/Shared.cs`, `Program.cs`, and `Shared/Shared.cs`. `Alias` targets
the in-root `Shared` directory.

Preflight also requires typed `DV0207` failures for source ancestor and
sibling-directory cycles, a source workspace escape, a project-reference
physical cycle, and a project-reference escape hidden beneath `obj`. Every
preflight snapshots its tree before execution and rejects mutation.

## Environment

- Windows 11, x86-64
- AMD Ryzen 9 9900X, 24 logical processors
- .NET SDK `10.0.100`, MSBuild `18.0.2.52411`
- release binaries; 30 retained samples after 5 warm-ups

## Commands

```text
dotnet msbuild Root.csproj --nologo -getItem:Compile
C:\Projects\dv\target\release\dv.exe project inspect Root.csproj --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 301.738 ms | 305.440 ms | 294.915 ms | 310.089 ms |
| `dv` | 4.473 ms | 5.252 ms | 4.072 ms | 5.498 ms |

`dv` is **67.5x faster** at the median. No sample was removed. The timed
interval includes process startup, project parsing, recursive source
enumeration, junction target/root identity checks, sorted source
materialization, output serialization, and capture. Link construction and all
parity/safety controls run outside timing.

Reproduce:

```powershell
cargo bench-all --case source_link_traversal --samples 30 --warmups 5 --output target/source-links-ws007-final.json
```
