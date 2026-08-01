# Invocation environment baseline - Windows - 2026-08-01

This baseline promotes `CLI-013`: typed invocation environment precedence and
secret-safe human/JSON argument reporting.

## Environment

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- .NET SDK `10.0.100`
- 30 retained samples after 3 warm-ups
- warm OS file caches; release builds and fixture setup outside timing
- default maximum Cargo compiler concurrency

## Timed Contract

Both tools receive the same process contract:

```text
NO_COLOR=environment-benchmark-secret
DV_COLOR=never
DV_VERBOSITY=normal
dotnet|dv build --definitely-unknown
```

The `DV_*` names are intentionally present in Microsoft's environment too; it
ignores them while `dv` parses them as typed defaults. Replacing only the
executable token leaves arguments, environment, working directory, and input
filesystem identical.

## Results

| Tool | Median | P95 | Min | Max |
|---|---:|---:|---:|---:|
| Microsoft .NET 10 | 134.218 ms | 150.374 ms | 130.490 ms | 153.938 ms |
| `dv` | 5.503 ms | 6.314 ms | 4.542 ms | 6.896 ms |

`dv` was 24.4x faster at the median. The complete stable batch is reported; no
retained sample was removed.

## Correctness Gate

Before timing, the harness requires Microsoft's `MSB1001`, `dv`'s `DV0002`,
the exact unknown option spelling, no ANSI escape sequence, no environment
sentinel in either stream, and identical before/after workspace snapshots.

A separate untimed .NET 10 child oracle proves ambient -> `[env:...]` ->
`--environment` precedence by selecting the command-line value. It receives a
secret sentinel but reports only presence. The matching `dv run` boundary must
retain three borrowed edits, one sensitive edit, and no secret text in
schema-18 JSON. Actual child launch remains tracked by
`RUN-006`/`RUN-007`.

Cross-platform unit and process tests additionally prove:

- command line beats `DV_COLOR`/`DV_VERBOSITY`;
- `DV_COLOR` beats a non-empty `NO_COLOR` default;
- invalid lower-precedence values are ignored when explicitly replaced;
- other invalid and non-Unicode values fail without retaining supplied text;
- child overlays order ambient, directive, launch-profile, and command-line
  sources, with stable last-wins behavior;
- the first four borrowed child edits require no dynamic allocation and a
  fifth edit takes the explicit spill path;
- child directives on commands other than `run` and `test` fail before
  discovery instead of becoming no-ops;
- human and JSON output redact separated/combined secrets, MSBuild secret
  properties, URL userinfo, and query/fragment data.

NuGet environment/config/provider credentials retain their existing zeroizing
owners and are tested separately; the invocation layer never copies them into
its environment policy.

## Cost And Layout

Three exact `var_os` lookups produce one five-byte, byte-aligned transient
policy. Unset values allocate nothing. Valid values become enums immediately;
invalid raw values are discarded before any diagnostic can own them. The
16-byte hot invocation request is unchanged.

The child overlay plan stores up to four borrowed 24-byte edits inline in a
104-byte, `usize`-aligned record and is compile-time bounded to two assumed
64-byte cache lines. Two edits fit per line; the empty variant writes only its
discriminant, and larger external batches spill to one contiguous vector. It
does not copy ambient environment data, sort, or hash. Launch-profile ingestion
and child launch are deliberately TBI in the ordered run/test workflow; the
benchmark preflight uses an official `dotnet run` oracle to prove the selected
precedence and checks `dv`'s typed pre-launch plan without claiming equivalent
execution timing.

Redaction is cold output work. Human rejection text uses a borrowed string when
unchanged. JSON already owns its event argument strings; classification adds a
64-byte stack buffer and allocates an additional string only when replacement
is necessary.

The no-environment control measured `dotnet --version` at `68.493 ms` and
`dv sdk current` at `5.596 ms` median (`12.2x`); `dv` p95 was `5.980 ms`.
That remains below the earlier published `6.102 ms` median, so no practical
common-path regression is observed.

The existing no-environment child boundary also remained below its CLI-012
baseline: Microsoft direct-host capture measured `42.927 ms`, while `dv`
measured `5.848 ms` median with `7.149 ms` p95.

Raw samples are retained as
`benchmarks/results/cli-environment-cli013-final.json`,
`benchmarks/results/sdk-current-cli013-control.json`, and
`benchmarks/results/cli-forwarding-cli013-control.json`.

Reproduce:

```powershell
cargo bench-all --case cli_environment --samples 30 --warmups 3
cargo bench-all --case sdk_current --samples 30 --warmups 3
cargo bench-all --case cli_forwarding --samples 30 --warmups 3
```
