# Command Cancellation Contract

`CLI-014` defines one cancellation lifetime for every work-bearing `dv`
invocation. The handler is installed after lossless argument capture and typed
command classification, but before SDK discovery, current-directory access,
project evaluation, filesystem mutation, process launch, or network work.
Help, version, malformed global-option, and unknown-command requests skip
installation entirely.

## Signal Policy

The first Ctrl+C/SIGINT request:

- records its time against a process-local monotonic epoch;
- changes the command from running to cancelling;
- wakes Tokio package and credential-provider waits;
- gives children at most two seconds to stop cooperatively.

The deadline is absolute. A child that observes cancellation late receives
only the time remaining from the original signal. A second request changes the
state to forced and makes the remaining grace zero. Further requests are
idempotent. `CLI-015` now owns typed child termination and numeric exit-code
propagation. Child creation, platform process-group ownership, and signal-exit
policy remain in the ordered `RUN-006` and `RUN-009` slices; the run/test
boundary already receives both cancellation and exit policies.

`ASSUMPTION: a two-second grace is long enough for well-behaved compiler,
application, test, and provider children to flush and exit - affects forced
termination latency and will be validated when those child workflows land.`

## Data Layout

`CancellationToken` is one pointer and clones by incrementing one reference
count. Its single shared state allocation is explicitly 64-byte aligned and
keeps its atomic microsecond timestamp and one-byte phase first under `repr(C)`.
The following monotonic epoch and platform-sized Tokio notification keep the
complete record at 64 bytes on Windows/Linux and 128 bytes on macOS. Compile
assertions cap it at two assumed cache lines; the command working set is one
pointer plus at most 128 allocation bytes.
No per-wait allocation, deadline task, timer thread, hash lookup, or dynamic
dispatch is introduced.

One allocation is necessary because the process signal callback and spawned
Tokio tasks require shared `'static` ownership. Commands that cannot perform
work do not create it. The legacy `PackageCancellation` public name remains a
type alias so existing callers keep source compatibility.

`ASSUMPTION: the supported x86-64 and arm64 benchmark hosts use 64-byte cache
lines - affects the isolated state layout; platform validation remains tracked
in issues/0003-cache-line-platform-data.md.`

## Transform Contract

The input is a genuine process singleton: a command-lifetime batch of zero or
more signal requests. Zero is the common value, one requests cancellation, and
two or more saturate at forced. The output is one compact phase plus an
optional monotonic deadline, owned from handler installation through process
exit. There is no malformed signal value; phase and timestamp arithmetic
saturate instead of wrapping.

The common access is a predictable false atomic check at coarse project,
graph, scheduler, and I/O boundaries. Signal writes are rare and random only
with respect to those reads. Async waiters borrow one cloned pointer; tasks do
not receive per-item objects. Results retain deterministic project and package
order regardless of which wait observes cancellation first.

## Observation Boundaries

Package resolution checks the token before each project and graph boundary.
Pending semaphore acquisition, HTTP send, retry delay, response streaming,
service-index discovery, and task joins race their work against the same Tokio
notification. Credential-provider cancellation sends the NuGet protocol
`Cancel` message, waits only for the absolute remaining grace, and kills and
reaps the provider on timeout or a second signal.

Cancellation emits `DV0005` and the JSON `cancelled` outcome. Failure to own
the process signal handler emits `DV0004` before work starts. `CLI-015`
preserves normal numeric child exits where the command owns that contract;
cancellation and signal exits retain the existing operation-failure value
until their workflow-specific reference policies are proven.

## Evidence

Unit tests prove monotonic first/second-signal transitions, stable absolute
deadlines, zero remaining grace after escalation, the fixed cache layout,
pre-cancelled zero-work batches, and classification of every work-bearing
command. Cross-platform CLI tests verify the two-second policy reaches
run/test. Unix integration tests deliver real SIGINT while an HTTP request is
stalled and require prompt `DV0005` cancellation without lock publication.

The benchmark preflight also launches an uncooperative credential-provider
fixture. After its one-second protocol timeout, `dv` sends `Cancel`, enforces
the same two-second grace, then kills and reaps the child. This proves the
deadline is consumed at a real child-process boundary rather than existing
only as metadata.

The like-for-like Windows case compares `dotnet --version` with `dv sdk
current`; both select and print the active SDK, while `dv` additionally installs
the new handler before discovery. Thirty retained samples after five warm-ups
measured Microsoft at `68.400 ms` median and `72.780 ms` p95, versus `5.302 ms`
and `6.346 ms` for `dv`, a `12.9x` median advantage. The non-work `dv
--version` control measured `4.330 ms` median and `5.332 ms` p95. Because SDK
discovery is also included, the `0.972 ms` difference is only an upper bound on
the end-to-end handler cost. Raw samples
are retained in
`benchmarks/results/2026-08-01-cli-cancellation-windows.json` and
`benchmarks/results/2026-08-01-cli-cancellation-version-control-windows.json`.

Reproduce:

```powershell
cargo bench-all --case cli_cancellation --samples 30 --warmups 5
cargo bench-all --case cli_version --samples 30 --warmups 5
```
