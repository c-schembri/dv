# NuGet Client Certificate Contract

`dv` consumes the merged top-level `clientCertificates` section after package
sources, disabled-source policy, proxy policy, and static credentials have
been resolved. The section accepts NuGet's flat certificate records:

- `fileCert` requires `packageSource` and `path`, with at most one of
  `password` or `clearTextPassword`;
- `storeCert` requires `packageSource` and `findValue`, and defaults to
  `CurrentUser`, `My`, and `Thumbprint`;
- `clear` removes lower-priority certificate records;
- one effective certificate record is allowed per source, case-insensitively.

Configuration-relative PFX paths are resolved against the file which declares
the record. Values receive the same single-pass `%NAME%` expansion as other
NuGet settings. Certificate files are read once with an 8 MiB limit. Cleartext
password buffers and PFX bytes are zeroed after the native TLS identity has
copied them; encrypted passwords use NuGet-compatible user DPAPI on Windows.

## Platform Stores

Windows store records support `CurrentUser` and `LocalMachine` plus NuGet's
eight store names. `Thumbprint` selection accepts a 40-digit SHA-1 fingerprint
with optional whitespace, requires an accessible private key, and exports only
the selected certificate through an in-memory PKCS#12 store. Other NuGet
`findBy` selectors fail explicitly rather than selecting a plausible
certificate. Non-Windows platforms fail store records explicitly and continue
to support `fileCert`.

## Transform And Layout

Input is a usually empty linear batch of certificate records and a source
batch in merged configuration order. Output is a source-index-aligned optional
native TLS client owned for the command lifetime. Certificate parsing, store
enumeration, and client construction happen once; the request path performs
one predictable optional-client selection. Certificate clients are used only
for the configured HTTPS origin and disable automatic redirects so a client
identity cannot cross an origin boundary.

On 64-bit targets `SourceCredential` is 80 bytes at 8-byte alignment. The cold
certificate client adds 8 bytes to the existing authentication record. A
public source without credentials, providers, or certificates retains no
source-authentication batch, native TLS client, certificate read, or store
scan. This intentional layout is protected by a compile-time assertion.

Malformed attributes, conflicting passwords, duplicate source bindings,
oversized or missing files, invalid PFX data, unsupported stores/selectors,
missing certificates, inaccessible private keys, and local-store use on an
unsupported platform are configuration errors. Diagnostics and structured
events report only `none`, `basic`, `client_certificate`, or
`basic_and_client_certificate`; they never contain passwords, PFX bytes,
thumbprints, or key material.

## Evidence

The checked-in fixture creates one exportable client certificate, binds the
same identity through a relative PFX and the Windows `CurrentUser\\My` store,
and uses the selected SDK's official `NuGet.Configuration` implementation as
the oracle. Preflight requires both tools to select one certificate for each
source and rejects fixture secrets in stdout and stderr. The timed command is
offline so it isolates config merge, PFX decode, store lookup, private-key
acquisition, native TLS-client construction, and reporting.

On the Windows x64 baseline, 30 retained samples after three warm-ups measured
Microsoft at 89.254 ms median and `dv` at 30.003 ms median, a 3.0x speedup.
