# NuGet Source Security Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86_64, 24 logical CPUs.
- Samples: 30 retained samples after 3 warm-ups.
- Fixture: one opted-in HTTP v3 source, one HTTPS source with TLS validation
  disabled, and one secure HTTPS v3 source.
- Oracle: the selected .NET SDK's `NuGet.Configuration` assembly.
- Timed state: offline; oracle build is outside the timed region.
- Preflight: both tools select the same ordered names, locations, protocol
  versions, `allowInsecureConnections`, and
  `disableTLSCertificateValidation` values. `dv` additionally reports the
  aggregate risk flags and proves zero HTTP requests.

## Commands

```text
dotnet oracle/bin/Release/SourceSecurityOracle.dll .
dv project package-sources SecurityProject.csproj --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 71.416 ms | 73.370 ms | 69.420 ms | 74.194 ms |
| `dv` | 5.742 ms | 6.878 ms | 4.836 ms | 6.913 ms |

`dv` is 12.4x faster by median. This isolates project/config discovery,
source-security policy selection, dedicated-client construction, and
structured output; it does not measure a network transfer or TLS handshake.
