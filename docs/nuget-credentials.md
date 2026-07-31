# NuGet Source Credential Contract

`NUGET-008` supplies static Basic credentials, including personal access
tokens used as passwords, to native NuGet v2 and v3 requests. Production code
does not launch NuGet or a credential provider.

## Input And Output Shapes

Input is the merged `packageSourceCredentials` configuration batch and the
effective package-source batch. Each credential group contains an XML-decoded
source name, `Username`, either Windows-encrypted `Password` or
`ClearTextPassword`, and optional `ValidAuthenticationTypes`. The exact
`NuGetPackageSourceCredentials_{source name}` environment value has NuGet's
`Username=...;Password=...;ValidAuthenticationTypes=...` grammar and wins when
valid. A malformed environment value falls back to configuration.

Output is a source-indexed batch of optional command-lifetime credentials.
The no-credential common case owns no entries. Each configured entry retains
one precomputed, sensitive HTTP `Authorization` header and one HTTPS origin.
`PackageSourceRecord` remains 28 bytes with 4-byte alignment, so adding the
reported authentication kind consumes existing padding and does not enlarge
the source-inventory working set. Five fields fit two complete records in an
assumed 64-byte cache line, with 8 bytes unused. `ASSUMPTION: the benchmark
machine has 64-byte cache lines - affects the records-per-line estimate, not
correctness.` The two-source fixture therefore retains 56 source-record bytes.

## Transform

```text
merged credential groups + effective source rows
  -> match source names exactly in one linear pass
  -> select valid environment value or merged configuration value
  -> decrypt a Windows Password with current-user DPAPI and NuGet entropy
  -> reject empty values and authentication sets that exclude Basic
  -> construct one sensitive Basic header per authenticated source
  -> attach it only to requests for the configured HTTPS origin
  -> report only none/basic in the source inventory
```

Configuration bytes, usernames, plaintext passwords, `username:password`
buffers, and encoded intermediate buffers use zeroizing owners. The final
header is marked sensitive and dies with the command configuration. Secrets
are never copied into lock files, events, diagnostics, command arguments, or
benchmark output.

Credential setup is cold configuration work over the usually tiny source
batch. Network requests clone the already parsed header instead of formatting
or encoding credentials per request. Service-index fetches, v2 metadata,
version pages, and package downloads all use the same authenticated request
boundary.

## Boundaries

- Only Basic is implemented; challenge-driven mechanisms and credential
  providers remain `NUGET-009`.
- Static credentials apply only to HTTPS remote sources. Local sources never
  retain credentials.
- Credentials are not sent to a different host or port advertised by a
  service index. Cross-origin private-feed flows require a separate explicit
  trust contract.
- Windows-encrypted `Password` values are decrypted only on Windows and fail
  explicitly elsewhere. `ClearTextPassword` remains portable.
- Embedded URL user information is rejected before a source can be reported.
- Empty, incomplete, duplicate, or nested config credential groups fail as
  configuration errors; malformed environment values fall back to config.
- Offline inspection resolves credential policy but performs zero requests.

## Verification

Core tests cover config hierarchy replacement, XML source-name decoding,
environment precedence and fallback, password semicolons, sensitive header
construction, same-origin containment, secret-free diagnostics, and Windows
DPAPI compatibility. CLI and benchmark preflight reject any username, token,
or decoy secret in stdout and stderr.

The process benchmark compares two source rows and their selected credential
origins against the selected SDK's `NuGet.Configuration` implementation. It
keeps oracle compilation and all network work outside the timed boundary.

```powershell
cargo bench-all --case nuget_credentials --samples 30 --warmups 3
```
