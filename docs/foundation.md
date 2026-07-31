# Phase 0 Foundation

## Scope

Phase 0 established contracts and evidence collection. Phase 1 now includes
native SDK discovery and strict evaluation of the initial SDK-style C# project
subset. Unsupported build commands and project behaviors return stable
diagnostics instead of delegating to Microsoft orchestration.

## Real Platform

The first observed development machine is:

- Windows 11 `10.0.22631`, x86-64
- AMD Ryzen 9 9900X, 12 cores and 24 hardware threads
- Rust toolchain `1.94.0`; crate MSRV remains `1.85.0`
- .NET SDKs `9.0.308` and `10.0.100`

These observations describe one machine, not a portable performance claim.
CI covers Windows, macOS, and Linux correctness. Performance comparisons are
valid only within one recorded machine and machine state.

## Foundation Data Flow

```text
OS arguments
  -> minimal command classification
  -> typed command/work/diagnostic events
  -> human reporter or versioned JSON-lines reporter

fixture directory
  -> isolated mutable copy
  -> reference command batch
  -> raw nanosecond samples
  -> deterministic summary statistics + JSON evidence
```

Production code and the benchmark harness are separate workspace packages.
Only `dv-bench` may launch the reference `dotnet` executable.

## Allocation And Layout

The execution model will use dense indices, arenas, reused buffers, and compact
internal records. Those records do not exist yet and must not be guessed before
we observe project, package, and graph distributions.

The Phase 0 wire types use owned `String` and `Vec` values deliberately:

- command-line and diagnostic text are variable-sized external data;
- diagnostics are a rare/error path;
- events cross the reporter boundary and may outlive producer scratch storage;
- one event describes a batch, not one event per file or graph node.

This is not permission to use the wire representation as the hot execution
layout. Phase 1 must transform compact indexed execution records into reporter
events only at the output edge, in batches, reusing capacity.

The first such record is `ProjectSpec`. It stores retained project text in one
immutable buffer and represents source/project paths with 8-byte spans.
Package references are 16-byte pairs of spans. Temporary filesystem paths are
necessary while traversing variable external input, then discarded after the
final compact batch is built.

## Simplification Decisions

- No plugin system, service locator, logging framework, or async runtime exists.
- CLI parsing is direct because Phase 0 has only help, version, and explicit
  unsupported-command failures.
- One small fixture drives the executable benchmark suite. Additional fixture
  shapes are specified before building more harness machinery.
- Raw samples are kept; averages alone would hide skew and outliers.
- The harness reports median and p95 but defines no pass/fail threshold until
  representative distributions exist.

## Done Evidence

Phase 0 is complete when the workspace formats, lints, tests, and builds in
release mode; the quick benchmark records raw reference samples; CI expresses
the same checks; and the compatibility, protocol, performance, and Roslyn
boundaries are documented.

Evidence against this design would be reporter allocation showing up in a
representative dominant path, event batches requiring unbounded retention, or
the benchmark setup affecting the interval it claims to measure. Any of these
requires changing the representation or harness rather than explaining away
the result.
