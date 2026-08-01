# Static Compatibility Check Baseline

Measured on Windows x86_64 with 24 logical CPUs using the release binary,
ten warm-ups and 50 retained samples:

```text
cargo bench-all --case cli_compat_check --warmups 10 --samples 50 --output benchmarks/results/2026-08-02-cli-compat-check-windows.json
```

The fixture contains one SDK-style `net10.0` project and a GitHub Actions
script with literal `dotnet --version` and `dotnet restore` rows. Preflight
requires event schema 20, compatibility manifest version 1, exactly two
invocations, at least one unresolved row, unsupported exit 2, empty stderr,
and no project artifact creation.

| Tool | Command | Median | P95 | Min | Max |
|---|---|---:|---:|---:|---:|
| Microsoft | No equivalent static compatibility command | TBI | - | - | - |
| `dv` | `dv --json compat check ci.yml SmallConsole.csproj` | 5.791 ms | 7.314 ms | 4.528 ms | 8.249 ms |

This is structural performance evidence, not a like-for-like speed ratio:
Microsoft's CLI has no equivalent command. The observed `dv` median remains
within 0.8 ms of the approximately 5 ms Windows process-start target. Network
requests and discovered process launches are zero by contract; the scan reads
the two input files and the compatibility manifest is embedded in the
executable.

The measured release executable is 7,434,240 bytes. The timed transform reads
473 fixture bytes and writes one three-event report. Its tokenizer and manifest
lookup allocate nothing; variable path, command, parity-row, and reporter data
own bounded dynamic storage because their sizes come from external input.
