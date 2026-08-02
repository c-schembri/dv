# Project Path Identity Windows Baseline

This like-for-like baseline measures `WS-006`. Both commands resolve the same
lexically spelled missing project reference,
`missing/../missing/Absent.csproj`, emit their missing-project failure, and
leave an immutable fixture unchanged. Preflight rejects canonicalization
failures or normalized-away diagnostic spelling before timing begins.

## Environment

- Windows 11, x86-64
- AMD Ryzen 9 9900X, 24 logical processors
- .NET SDK `10.0.100`, MSBuild `18.0.2.52411`
- release binaries; 30 retained samples after 5 warm-ups

## Commands

```text
dotnet msbuild Microsoft.proj --nologo -t:ResolveProjectIdentities
C:\Projects\dv\target\release\dv.exe restore Root.csproj --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 200.255 ms | 221.558 ms | 183.078 ms | 223.272 ms |
| `dv` | 5.423 ms | 6.631 ms | 4.378 ms | 6.886 ms |

`dv` is **36.9x faster** at the median. No sample was removed. The timed
interval includes process startup, path validation, diagnostic production,
output capture, and failed status propagation. It does not include fixture
setup or parity validation.

Reproduce:

```powershell
cargo bench-all --case path_identity --samples 30 --warmups 5 --output target/path-identity-ws006-final.json
```
