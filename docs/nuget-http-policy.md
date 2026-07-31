# NuGet HTTP Transport Policy

`NUGET-011` and `NUGET-014` convert merged NuGet configuration and process
environment into immutable transport policy and request budgets before remote
source work begins.

## Transform Contract

Input is the merged `<config>` key set, process environment, ordered package
sources, and the command's offline flag. The common measured project has one
HTTPS source; the implementation supports the repository's bounded 24-task
download batch. Config `http_proxy` takes precedence over `http_proxy` then
`HTTP_PROXY`; bypass lookup similarly accepts `no_proxy` then `NO_PROXY`.
`maxHttpRequestsPerSource` uses NuGet's positive-value behavior and otherwise
defaults to 64.

Output is a 16-byte, four-byte-aligned `PackageHttpPolicy`, a two-byte,
two-byte-aligned command-wide `PackageRequestBudget`, one secure shared reqwest
client, optional source-specific clients described by `NUGET-012`, one shared
global Tokio semaphore when the selected budget is below the measured 24-task
ceiling, and one distinct semaphore per remote source only when its configured
limit is tighter than the global budget. Positive `NUGET_CONCURRENCY_LIMIT`
values select a global budget up to that ceiling; missing, malformed,
non-positive, and oversized values use or clamp to it. Service-index discovery,
metadata expansion, and package acquisition all pass through the same shared
global permit. A task acquires the narrower source permit first so a saturated
source cannot reserve global capacity, then retains both permits through
response-body consumption. Reads are linear during configuration and then
read-only/random by source index; common request branches are predictable.
Retry and authentication branches are paid only after failures.

`ASSUMPTION: useful local concurrency does not exceed the existing 24-request
global scheduler ceiling - affects environment clamping and whether the common
path needs a global semaphore.`

`ASSUMPTION: benchmark hosts use 64-byte cache lines - affects the expectation
that four policy records fit per line.` Typical one-to-three-source projects
therefore keep the complete policy working set within 48 bytes. The record has
seven integer/flag fields, is immutable after discovery, and cannot false-share.
The cold authentication/source context layout and access cost are recorded
below.

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

Default public sources pay one 16-byte policy copy, one two-byte command budget,
and no semaphore allocation: the already-required 24-task Tokio sets provide
the default global bound. A smaller global selection adds one shared
`Arc<Semaphore>`; each source constrained below it adds one more. Configured
authentication already requires a source context, while a selected budget
creates one for otherwise anonymous remote sources. The cold context is 128
bytes and eight-byte aligned after adding the two optional limiter handles; it
is indexed once per request and is not scanned by graph loops. Unsafe source
policy additionally adds one dedicated client and connection pool. A request
holds up to two permits and one response wrapper. Retries add no work on success
and add their exact request and delay cost only on transient failure.

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

The request-budget benchmark seeds six exact packages, serves the same package
bytes from two local V3 feeds with a fixed 25 ms delay, and rejects any retained
sample that exceeds four combined or two per-source active requests:

```powershell
cargo bench-all --case nuget_request_budget --samples 30 --warmups 3
```

Thirty retained Windows samples measured `3109.409 ms` for Microsoft and
`247.157 ms` for `dv`, a `12.6x` median improvement. The full contract and
distribution are retained in the
[request-budget baseline](performance-baselines/2026-08-01-nuget-request-budget-windows.md).
