# NuGet HTTP Transport Policy

`NUGET-011` converts merged NuGet configuration and process environment into
one immutable transport policy before remote source work begins.

## Transform Contract

Input is the merged `<config>` key set, process environment, ordered package
sources, and the command's offline flag. The common measured project has one
HTTPS source; the implementation supports the repository's bounded 24-task
download batch. Config `http_proxy` takes precedence over `http_proxy` then
`HTTP_PROXY`; bypass lookup similarly accepts `no_proxy` then `NO_PROXY`.
`maxHttpRequestsPerSource` uses NuGet's positive-value behavior and otherwise
defaults to 64.

Output is a 16-byte, four-byte-aligned `PackageHttpPolicy`, one secure shared
reqwest client, optional source-specific clients described by `NUGET-012`, and,
only below the global 24-request ceiling, one shared Tokio
semaphore per remote source. Source tasks retain the semaphore permit through
response-body consumption. Reads are linear during configuration and then
read-only/random by source index; common request branches are predictable.
Retry and authentication branches are paid only after failures.

`ASSUMPTION: useful local concurrency does not exceed the existing 24-request
global scheduler ceiling - affects when a per-source semaphore is allocated.`

`ASSUMPTION: benchmark hosts use 64-byte cache lines - affects the expectation
that four policy records fit per line.` Typical one-to-three-source projects
therefore keep the complete policy working set within 48 bytes. The record has
seven integer/flag fields, is immutable after discovery, and cannot false-share.
The cold authentication/source context is 120 bytes and eight-byte aligned; it
is dereferenced once per request, not scanned by package graph loops.

## Behavior

- Proxy URL userinfo is percent-decoded into zeroized Basic fields and removed
  from the retained URL. Windows config passwords use NuGet-compatible DPAPI.
- TLS peer and hostname validation remains enabled by default. `NUGET-012`
  permits HTTP or disabled validation only on an explicitly configured source.
- General clients follow at most ten redirects and reject non-HTTPS targets
  unless the matching source permits HTTP. Client-certificate clients continue
  to reject all redirects.
- Default enhanced retry uses six total attempts, a 1,000 ms base delay, HTTP
  408/429/5xx classification, `Retry-After`, and a 3,600 second server-delay
  cap. NuGet's five enhanced retry environment variables override those values.
- Untrusted retry counts and delays are bounded to 32 attempts, 60 seconds of
  base delay, and 24 hours of `Retry-After`; invalid values use NuGet defaults.
- Requests time out after 100 seconds. A response body that produces no chunk
  for 60 seconds fails independently.
- `--offline` performs no service discovery, metadata request, package request,
  DNS lookup, or TLS connection. Explicit credential-provider probing remains
  local process work requested by the caller.

Malformed proxy URLs fail before client construction. Unsupported schemes,
missing hosts, invalid UTF-8 credentials, closed rate limiters, timeout, and
retry exhaustion produce typed configuration or network errors. No proxy URL,
username, password, bypass entry, or authorization header reaches reporter
text. Event schema 12 exposes only redacted policy values and per-source
security consequences.

## Cost And Verification

Default public sources pay one 16-byte config copy and no semaphore allocation.
Configured authentication already requires one source context; constrained
sources add one `Arc<Semaphore>`, while unsafe source policy adds one dedicated
client and connection pool. A request holds one permit and one response
wrapper. Retries add no work on success and add their exact request and delay
cost only on transient failure.

Unit tests use local TCP servers to prove 503 retry, HTTPS redirect enforcement,
shared per-source capacity, and body-stall timeout. Pure tests cover environment
selection, bounds, proxy credential stripping, and redacted flags. The process
benchmark builds an adapter against the selected SDK's shipped
`NuGet.Configuration` and `NuGet.Protocol` assemblies, compares eleven policy
fields, then measures both tools offline:

```powershell
cargo bench-all --case nuget_http_policy --samples 30 --warmups 3
```

The curated result is recorded in
[`performance-baselines/2026-08-01-nuget-http-policy-windows.md`](performance-baselines/2026-08-01-nuget-http-policy-windows.md).
