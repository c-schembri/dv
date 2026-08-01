# Repository Root Discovery Windows Baseline

This like-for-like baseline measures `WS-004` independently of project
selection. Both commands start from the same three-level nested directory and
return the nearest ancestor containing the same `.git` marker. The preflight
uses MSBuild's own ancestor-file query as the reference oracle, validates
`dv`'s typed kind and probe count, and proves zero fixture mutation.

## Environment

- Windows 11, x86_64
- AMD Ryzen 9 9900X, 24 logical processors
- .NET SDK `10.0.100`
- 30 retained samples after 3 warm-ups

## Commands

```text
dotnet msbuild nested/src/RepositoryRoot.proj --nologo -getProperty:RepositoryRoot
dv project root nested/src
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 137.639 ms | 161.395 ms | 127.391 ms | 162.118 ms |
| `dv` | 5.007 ms | 5.696 ms | 4.685 ms | 5.790 ms |

`dv` is `27.5x` faster at the median. The timed `dv` path performs process
startup, argument parsing, start-path metadata, three `.git` metadata probes,
and one path write. The Microsoft path also starts MSBuild and evaluates the
small oracle project. No network, project discovery, or output mutation occurs.

Raw machine-local samples are retained at
`target/repository-root-final.json`.
