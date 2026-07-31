# NuGet credential-provider contract

`dv` implements the cross-platform NuGet V2 authentication-plugin subset for
private HTTPS feeds. Production code never invokes `dotnet`, NuGet, or MSBuild;
providers must therefore be self-contained `nuget-plugin-*` executables.

## Data contract

Input is an ordered batch of absolute provider paths from
`NUGET_NETCORE_PLUGIN_PATHS`, falling back to `NUGET_PLUGIN_PATHS`, plus one
challenged HTTPS source URI. Typical commands have zero to two sources and zero
to two providers.

Output is either no applicable provider or one sensitive Basic authorization
header owned by the source for the command lifetime. Username and password
strings move into zeroizing owners and never enter events, diagnostics, locks,
traces, or benchmark output.

`ASSUMPTION: provider protocol messages fit within 1 MiB and interactive log
messages fit within 64 KiB - affects fixed read limits and rejection policy.`

`ASSUMPTION: the benchmark machine has 64-byte cache lines - affects the
working-set description, not correctness.`

## Transform

```text
HTTP 401 or explicit offline probe
  -> discover and deduplicate configured executable paths once per acquisition
  -> launch one bounded provider subprocess with piped stdin/stdout
  -> symmetric Handshake (protocol 2.0.0, minimum 1.0.0)
  -> MonitorNuGetProcessExit -> Initialize -> GetOperationClaims
  -> SetLogLevel -> GetAuthenticationCredentials
  -> validate Basic policy -> zero source secrets -> cache sensitive header
  -> Close and reap provider -> retry the challenged request
  -> on another 401, reacquire once with IsRetry=true and retry once more
```

Provider candidates are evaluated in configured order. Protocol input is
linear JSON-lines I/O with predictable message-type dispatch in the normal
response path. Progress resets the inactivity timeout. Interactive `Log`
requests are handled out of line and receive a typed response.

The common public-feed path has no provider allocation: without provider
environment configuration and without static credentials, the source-aligned
credential batch is empty. On 64-bit targets the three-field cold
`SourceCredential` record is 72 bytes at 8-byte alignment; its provider field
is one optional pointer. The larger source URI, options, atomic acquired flag,
and mutex-protected header/provider-index/generation state live in a separate
allocation only for provider-enabled sources. The mutex serializes the rare
authentication challenge and credential refresh; it is never acquired by
public feeds or graph/asset loops. A 72-byte
authenticated-source record spans two assumed 64-byte cache lines, but it is
read only at the HTTP authorization boundary, not in graph or asset loops.
Compile-time layout assertions protect the intentional x64 record size.

## Boundaries

- Commands are noninteractive unless `--interactive` is explicit.
- `NUGET_PLUGIN_HANDSHAKE_TIMEOUT_IN_SECONDS` and
  `NUGET_PLUGIN_REQUEST_TIMEOUT_IN_SECONDS` accept 1 through 86,400 seconds;
  defaults are 30 seconds.
- Timeout, library cancellation, or CLI Ctrl+C sends the protocol `Cancel`,
  waits 250 ms, then terminates and reaps the process. `Close` has a two-second
  bound.
- Messages over 1 MiB, logs over 64 KiB, malformed JSON, unexpected methods,
  invalid response codes, premature EOF, empty credentials, and unsupported
  authentication types fail explicitly.
- Provider headers are sent only to the exact configured HTTPS host and port.
- One repeated 401 asks the selected provider again with `IsRetry=true`; a
  generation check coalesces concurrent challenges and the third 401 fails.
- DLL-only providers fail with `DV0410`; `dv` does not host them through
  `dotnet`. Cancellation is `DV0411`.
- Provider paths and protocol state are command-local. No global process pool,
  unbounded queue, or credential persistence exists. An HTTP operation makes
  at most three attempts: unauthenticated/static, acquired provider credential,
  and one provider refresh explicitly marked `IsRetry=true`.

## Verification

Core tests cover cancellation before process launch, DLL rejection, timeout
shape, secret ownership, and payload-free malformed-message diagnostics. The
benchmark preflight launches the same Rust fixture through the selected SDK's
official `NuGet.Protocol` plugin manager and through `dv`, compares redacted
Basic results, verifies noninteractive and interactive flags, acknowledges
opt-in login output, forces a bounded timeout, proves `Cancel` was sent, and
rejects fixture secrets in stdout and stderr.

```powershell
cargo bench-all --case nuget_credential_provider --samples 30 --warmups 3
```
