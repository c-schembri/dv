# Nuspec Framework-Metadata Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: .NET `10.0.100`; timed samples target `net10.0` and parity additionally
  covers `net48`.
- Samples: 30 retained samples after 3 warm-ups.
- Input: two deterministic local archives. `Framework.Metadata` contains three
  dependency groups, two modern framework-reference groups, and four legacy
  framework-assembly rows; `Framework.Child` is the selected dependency.
- State: `.packages`, `obj`, `packages.lock.json`, and `dv.lock.json` are
  removed before every sample. No network source exists.
- Output: modern restore must select `Framework.Child` and
  `Microsoft.AspNetCore.App` without leaking the `net8.0` or legacy rows.
  Untimed legacy parity must select `System.Data` and `System.Xml`, exclude the
  modern and fallback rows. `dv` must reproduce both target selections exactly
  from warm locks without reopening either manifest.

## Commands

```text
dotnet restore FrameworkMetadata.csproj --packages .packages --nologo --verbosity quiet
dv restore FrameworkMetadata.csproj --packages .packages --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 558.832 ms | 578.305 ms | 551.141 ms | 626.120 ms |
| `dv` | 15.989 ms | 18.009 ms | 15.256 ms | 24.093 ms |

`dv` is 35.0x faster by median. The timed interval includes process startup,
project/config parsing, local-feed discovery, dependency and framework-group
selection, two archive validations/publications, lock writing, and structured
output. Each `dv` sample resolves and downloads two packages totaling 2,250
archive bytes and performs zero HTTP requests.

Reproduce:

```powershell
cargo bench-all --case nuspec_framework_metadata --samples 30 --warmups 3
```
