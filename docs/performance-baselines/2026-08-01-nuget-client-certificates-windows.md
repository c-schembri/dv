# NuGet Client Certificate Baseline - Windows x64 - 2026-08-01

## Contract

- Host: Windows x86_64, 24 logical CPUs.
- Samples: 30 retained samples after 3 warm-ups.
- Fixture: two HTTPS v3 sources, one relative PFX binding and one
  `CurrentUser\\My` thumbprint binding to the same exportable certificate.
- Oracle: the selected .NET SDK's `NuGet.Configuration` assemblies.
- Timed state: network disabled; certificate creation, store installation,
  oracle build, and cleanup are outside the timed region.
- Preflight: both tools select one certificate per source, publish the same
  redacted source rows, perform zero HTTP requests, and expose no fixture
  password.

## Commands

```text
dotnet oracle/bin/Release/ClientCertificateOracle.dll query .
dv project package-sources ClientCertificateProject.csproj --offline --json
```

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 89.254 ms | 91.690 ms | 86.228 ms | 91.977 ms |
| `dv` | 30.003 ms | 31.361 ms | 28.287 ms | 31.899 ms |

`dv` is 3.0x faster by median. This result measures local certificate
selection and TLS-client construction, not network or TLS-handshake latency.
