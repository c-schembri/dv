# Ancestor Build Input Discovery Baseline

Date: 2026-08-02
Host: Windows 11, AMD Ryzen 9 9900X, 24 logical CPUs
Reference: .NET SDK 10.0.100 / MSBuild 18.0.2.52411

## Workload

The immutable three-level fixture contains one `global.json`, two
`NuGet.Config` files, one `Directory.Build.props`, one nearer
`Directory.Build.targets`, and one nearer `Directory.Packages.props`.
Preflight requires identical absolute paths and precedence order from both
tools and an unchanged fixture tree.

```text
dotnet msbuild nested/src/WorkspaceInputs.proj --nologo -getProperty:GlobalJson,NuGetConfigs,DirectoryBuildProps,DirectoryBuildTargets,DirectoryPackagesProps
dv project inputs nested/src --json
```

Setup and correctness checks occur outside timing. Each retained sample starts
a new process and performs the full ancestor discovery and output transform.

## Results

| Tool | Min | Median | P95 | Max |
|---|---:|---:|---:|---:|
| Microsoft | 136.288 ms | 139.917 ms | 145.349 ms | 145.587 ms |
| `dv` | 4.372 ms | 4.845 ms | 6.203 ms | 6.309 ms |

Median improvement: **28.9x**. The retained run used 30 samples and five
warm-ups; no sample was removed.

Raw report: `target/workspace-inputs-final-2.json`.
