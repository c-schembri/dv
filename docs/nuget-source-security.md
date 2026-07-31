# NuGet Source Security

`NUGET-012` keeps insecure transport exceptions explicit and source-local.
One source's opt-in never weakens the default client or another source.

## Transform Contract

Input is the merged ordered `packageSources` batch. Each source may carry
NuGet's `allowInsecureConnections` and `disableTLSCertificateValidation`
attributes. Attribute values receive the same single-pass environment
expansion as other config values; trimmed case-insensitive `true` opts in and
every other value preserves the secure default.

Output is a 32-byte `PackageSource` containing owned location text, a typed
protocol byte, and two security bits. The compact inventory record remains 28
bytes and exposes those bits without retaining any certificate or credential.
The six-field inventory row is four-byte aligned; a three-source fixture owns
84 contiguous row bytes plus its single text table. The configuration owns the
source batch for the command lifetime, while inventory consumers borrow indexed
views and the reporter materializes owned event text only at the output edge.
The common all-secure path creates no source-specific client. An opted-in
remote source creates one source-scoped client and reuses it for its service
index, v2 continuations, metadata, and package content.

Discovery reads the source batch linearly once; later request selection is a
direct source-index lookup with a predictable secure-default branch. No policy
text is copied after compaction. The layout is not explicitly aligned because
sources are immutable after discovery and never written by independent workers.

Configuration access is a linear read with predictable false/default branches.
Request work performs one random source-index lookup into immutable state; no
policy parsing or string allocation enters package graph loops. The rare source
context is 120 bytes and eight-byte aligned. It is allocated only when source
authentication, a rate limit, a client certificate, or security opt-in already
requires request-local state.

`ASSUMPTION: command-line sources have no independent security switch - an
HTTP override must exactly match an opted-in configured source.`

`ASSUMPTION: benchmark hosts use 64-byte cache lines - affects the expectation
that two 28-byte inventory rows or two 32-byte configuration sources fit per
line.` The batches are immutable, so worker writes cannot false-share them.

## Behavior

- HTTP sources fail configuration unless their own
  `allowInsecureConnections=true` attribute is present.
- HTTP v3 resource endpoints, v2 continuation links, and v2 package-content
  URLs are rejected unless the originating source carries the same opt-in.
- `disableTLSCertificateValidation=true` disables certificate-chain and
  hostname validation only in that source's dedicated client.
- Redirects remain bounded at ten. HTTP redirect targets are followed only for
  an opted-in source; all other schemes are rejected.
- Basic/provider credentials and client-certificate identity remain bound to
  the configured scheme, host, and port. A source transport exception does not
  broaden credential containment.
- A CLI HTTP source is accepted only when it exactly selects an opted-in
  configured source. A new ad hoc HTTP source fails with the required config
  action.

Missing hosts, malformed URLs, embedded credentials, unsupported schemes, and
unapproved HTTP in a derived v2/v3 URL fail at the boundary that reads them;
none are dropped, clamped, or interpreted as local paths.

Human source output names `insecure-http` and `tls-validation` for the aggregate
and each source. Event schema 12 exposes
`allow_insecure_connections` and `disable_tls_certificate_validation` per
source plus redacted aggregate flags. This deliberately makes the security
consequence machine-readable without exposing sensitive material.

## Cost And Verification

Tests cover missing opt-in rejection, case-insensitive/expanded attributes,
CLI containment, source-client allocation, HTTP redirects, v3 resources, v2
continuations, and v2 content URLs. The benchmark compares the selected SDK's
shipped `NuGet.Configuration` source records against `dv` for an opted-in HTTP
source, a TLS-validation-disabled HTTPS source, and a secure HTTPS source. The
timed operation is offline policy selection, not a claim about network or TLS
handshake speed:

```powershell
cargo bench-all --case nuget_source_security --samples 30 --warmups 3
```

The curated result is recorded in
[`performance-baselines/2026-08-01-nuget-source-security-windows.md`](performance-baselines/2026-08-01-nuget-source-security-windows.md).
