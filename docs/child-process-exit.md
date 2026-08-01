# Child Process Exit Contract

`CLI-015` keeps the operating system's child result typed until the outermost
CLI boundary. A nonzero child exit is a completed child operation, not a `dv`
usage or orchestration failure, so compatibility profiles do not remap it.

## Oracle Snapshot

The Windows oracle was captured on 2026-08-01 with .NET SDK `10.0.100`. The
checked-in `ArgumentForwarding.csproj` application returns the integer supplied
after its `exit` sentinel.

| Invocation | Observed process exit |
|---|---:|
| `dotnet run --project ArgumentForwarding.csproj --no-build --no-restore -- exit 23` | 23 |
| `dotnet bin/Release/net10.0/ArgumentForwarding.dll exit 23` | 23 |

The probes establish that the `dotnet run` contract preserves the
application's numeric exit rather than collapsing a nonzero value to `1`.
Launch failure is established separately with a nonexistent native executable;
it never produces a `ChildTermination`.

## Data Contract

`ChildTermination` is an eight-byte, four-byte-aligned value produced only
after a launched child has been reaped. It retains one exact `i32` exit code,
one Unix signal, or the rare platform state in which neither is available.
The common numeric path is one predictable `Option` branch and performs no
allocation, formatting, copying, filesystem access, or process launch.

`ChildProcessFailure` is cold data and separately names the failed OS stage as
`Launch` or `Wait`, retaining the original `io::Error`. A nonzero
`ChildTermination::Exited` can therefore never be mistaken for failure to
launch the program.

`ChildExitPolicy` is a one-byte command contract. `run` declares `Preserve`;
test execution declares `MapToCommandFailure` because test-host exits are
inputs to the test command's aggregated result rather than application exits.

The CLI's final `ExitCode` is a four-byte `i32` passed to
`std::process::exit`. Numeric child exits bypass native and compatibility
failure mappings without narrowing to Rust's portable `u8` `ExitCode` API.
Signal-to-parent-exit policy stays explicit because exiting with `128 + signal`
is not equivalent to the parent being terminated by that signal.

`ASSUMPTION: supported application commands own one foreground child at a
time - affects keeping this terminal record singular; compiler and test-host
scheduling will retain batches at their own subsystem boundaries.`

## Ownership And Boundaries

The process owner constructs argv, environment, working directory, streams,
and cancellation policy without copying them into this terminal record. The OS
owns the live process; `dv` owns the classified result from reap through final
reporting. Application launch and process-group behavior remain deliberately
ordered under `RUN-006` and `RUN-009`; those features consume this contract
rather than redefining exit semantics.

Malformed or out-of-range behavior is explicit:

- every platform `i32` exit value is retained exactly;
- Unix signals outside `u8` become `Unknown` rather than wrapping;
- `Signalled` and `Unknown` cannot be converted into a numeric CLI exit without
  an explicit command/profile policy;
- launch and wait errors retain their stage and never synthesize a child code.

## Verification

Cross-platform tests launch real shell children returning `0`, `37`, and `211`
and require exact classification. A nonexistent executable must produce a
`Launch` failure. Unix additionally sends `SIGTERM` to a child and requires a
signal record rather than a fabricated numeric exit. CLI tests cover full
32-bit child-code transfer and reject implicit conversion of signals or
unknown status.

The dedicated structural benchmark requires Microsoft's real managed child to
return `23`; `dv` must instead expose its typed preserve policy and explicit
TBI diagnostic. Its timings are not compared as application execution. The
like-for-like failure guard uses the existing unknown-option contract because
`CLI-015` changes the outer process-exit boundary for every command. A separate
SDK-selection control covers successful termination. Full results are recorded
in the [Windows baseline](performance-baselines/2026-08-01-cli-child-exit-windows.md).

Reproduce:

```powershell
cargo bench-all --case cli_unknown_option --samples 30 --warmups 3
cargo bench-all --case cli_child_exit --samples 30 --warmups 5
cargo bench-all --case sdk_current --samples 30 --warmups 5
```
