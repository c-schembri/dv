# Package RID And Content Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86-64, 24 logical CPUs.
- SDK: latest installed stable .NET `10.0.100`; all projects target `net10.0`.
- Samples: 30 retained samples after 3 warm-ups for each state.
- Input: one deterministic local archive and four projects covering portable,
  exact `win-x64`, exact `linux-x64`, and `win-arm64` fallback selection.
- Parity: Microsoft `project.assets.json` and `dv` must agree on runtime,
  resource, native, and content paths plus runtime-target RID/type and content
  build-action/copy/flatten metadata for all four projects.
- Cold state: isolated packages, outputs, and locks are removed before every
  sample. The local feed eliminates network variance, but OS filesystem caches
  remain warm.
- Warm state: package caches and matching tool-native locks are retained. Both
  tools still launch a fresh process for every sample.

## Commands

```text
dotnet restore WindowsFallback.csproj --packages .packages --no-http-cache --nologo --verbosity quiet
dv restore WindowsFallback.csproj --packages .packages --offline --json
```

Warm Microsoft samples additionally use `--locked-mode`; warm `dv` samples
validate and consume `dv.lock.json`.

## Results

| State | Tool | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| Cold | Microsoft | 600.782 ms | 2009.103 ms | 563.109 ms | 2072.628 ms |
| Cold | `dv` | 23.186 ms | 33.821 ms | 21.194 ms | 85.959 ms |
| Warm | Microsoft | 456.098 ms | 486.470 ms | 444.827 ms | 539.736 ms |
| Warm | `dv` | 7.589 ms | 9.101 ms | 6.426 ms | 10.118 ms |

`dv` is 25.9x faster by cold median and 60.1x faster by warm median. Cold
timing includes process startup, project/config/SDK discovery, local archive
validation and publication, RID/content selection, lock writing, and structured
output. Warm timing includes startup, semantic RID-graph fingerprint proof,
lock validation, plan materialization, and output.

Reproduce:

```powershell
cargo bench-all --case package_rid_content_cold --samples 30 --warmups 3
cargo bench-all --case package_rid_content_warm --samples 30 --warmups 3
```
