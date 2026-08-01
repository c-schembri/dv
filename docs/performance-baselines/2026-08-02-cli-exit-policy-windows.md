# CLI Exit Policy Baseline - Windows - 2026-08-02

This baseline advances the Phase 1 portion of `DROP-016`. It measures a
like-for-like missing-project restore failure after both tools have accepted
the command and attempted project-path discovery.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- release binaries and maximum Cargo compiler concurrency
- 50 retained samples after ten warm-ups; warm OS caches

## Command Boundary

```text
dotnet restore DefinitelyMissing.csproj
C:\Projects\dv\target\release\dv.exe --compat dotnet restore DefinitelyMissing.csproj
```

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft | 122.756 ms | 134.338 ms | 118.724 ms | 148.064 ms |
| `dv` | 5.158 ms | 6.073 ms | 4.349 ms | 6.137 ms |

Both commands return exit `1`, name `DefinitelyMissing.csproj`, and leave the
fixture byte-for-byte unchanged. The Microsoft oracle requires `MSB1009`;
`dv` requires `DV0200` and one `compatibility_profile: dotnet` context row.
`dv` is `23.8x` faster at the median. Every raw sample is retained in the
result file.

## Data And Cost

The terminal transform consumes a one-byte invocation profile and a one-byte
result class. It indexes a 45-byte immutable table and returns one byte after a
single inapplicable-sentinel comparison. The lookup allocates nothing and
performs no I/O, synchronization, formatting, or branching by profile. Process
launch and missing-project discovery dominate the measured boundary;
diagnostic construction remains cold error-path work.

The current matrix covers success, usage, unsupported, operation, build,
restore, test-failure, no-tests, and cancellation outcomes for native, dotnet,
MSBuild, NuGet, and VSTest profiles. Tool/outcome combinations that do not
exist use a sentinel and cannot be mistaken for exit 0. Exact child exits remain
a separate typed `i32` path. Test execution and signal behavior stay open until
their workflows can exercise the Microsoft contract like for like.

Reproduce:

```powershell
cargo bench-all --case cli_exit_policy --samples 50 --warmups 10 --output benchmarks/results/2026-08-02-cli-exit-policy-windows.json
```
