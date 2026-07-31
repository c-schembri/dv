# NuGet Source Mapping Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86_64, 24 logical CPUs.
- Samples: 30 retained samples after 3 warm-ups.
- Fixture: one `net10.0` project requesting exact `Unmapped.Package` `1.0.0`,
  an empty isolated package cache, and one unreachable v3 source mapped only
  by `Mapped.*`.
- Reference: `dotnet restore` must fail with `NU1100` and state that
  `PackageSourceMapping` excluded the source.
- `dv`: restore must fail with `DV0412` and typed `package_id` value
  `unmapped.package`.
- Preflight: either tool contacting the unreachable source fails validation;
  both retained runs therefore perform zero HTTP requests and transfer zero
  response bytes.
- Timed state: a fresh fixture copy for every warm-up and retained sample.

## Commands

```text
dotnet restore SourceMapping.csproj --packages .packages --no-http-cache -p:NuGetAudit=false --nologo --verbosity quiet
dv restore SourceMapping.csproj --packages .packages --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 639.131 ms | 1684.299 ms | 515.672 ms | 1692.450 ms |
| `dv` | 8.445 ms | 9.459 ms | 7.628 ms | 9.649 ms |

`dv` is 75.7x faster by median. This measures process launch, project and
configuration discovery, empty-cache proof, longest-pattern source selection,
and structured expected failure. It is not a network-transfer benchmark.
