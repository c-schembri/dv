# CLI Version Routing Baseline

Date: 2026-08-02
Host: Windows 11, AMD Ryzen 9 9900X, 24 logical CPUs
Reference: .NET SDK 10.0.100

## Contract

Command syntax 6 makes the default version query SDK-compatible:

```text
dotnet --version
dv --version
```

Preflight requires exact selected-SDK text equality. `dv self-version` is the
separate executable identity query. Its JSON form reports the dv package,
command-syntax, and event-schema versions; Microsoft has no equivalent query.

## Results

| Tool | Command | Min | Median | P95 | Max |
|---|---|---:|---:|---:|---:|
| Microsoft | `dotnet --version` | 63.599 ms | 65.047 ms | 67.846 ms | 69.384 ms |
| `dv` | `dv --version` | 4.565 ms | 5.559 ms | 6.091 ms | 6.576 ms |

The like-for-like selected-SDK query is **11.7x** faster at the median.

| Tool | Command | Min | Median | P95 | Max |
|---|---|---:|---:|---:|---:|
| Microsoft | protocol identity | TBI | TBI | TBI | TBI |
| `dv` | `dv --json self-version` | 4.292 ms | 5.037 ms | 6.650 ms | 7.117 ms |

The second table is not a speed claim. It retains the cost of dv's structured
self-version boundary without inventing a Microsoft comparison.

Both runs used 30 samples and five warm-ups with no removed samples. Raw
reports: `target/cli-version-syntax6-final.json` and
`target/cli-self-version-syntax6.json`.
