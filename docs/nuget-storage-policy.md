# NuGet Storage And Restore Policy

`NUGET-004` extends the ordered NuGet configuration fold with storage,
signature, proxy, and project audit policy. The implementation follows the
selected SDK's `NuGet.Configuration` behavior rather than embedding one SDK
or registry generation.

## Precedence

The global-packages directory is selected in this order:

1. the explicit `--packages` value;
2. `NUGET_PACKAGES`;
3. the merged `globalPackagesFolder` configuration value;
4. the platform NuGet default.

`NUGET_FALLBACK_PACKAGES` replaces configured fallback folders when present.
Otherwise, named `fallbackPackageFolders` entries merge case-insensitively
through add, replace, remove, and clear operations. Higher-precedence files
are searched first while preserving entry order within each file. Package
lookup checks the writable global cache before every read-only fallback root.

`NUGET_HTTP_CACHE_PATH` selects the HTTP metadata-cache root and
`NUGET_SCRATCH` selects temporary storage. The HTTP root is retained in the
typed resolution for `RES-017/018`; conditional reuse, revalidation, negative
entries, and corruption quarantine remain those dedicated resolver features.
Downloaded archives use the configured scratch root, then move through a
same-volume staging directory before atomic publication into the global
cache. A hard link avoids a second payload copy when the volumes permit it.

## Typed Policy

`signatureValidationMode` is normalized to `accept` or `require`; NuGet's
documented behavior of treating an unknown value as `accept` is preserved.
The selected mode is retained for the signature verifier tracked by
`RES-015`. This feature does not claim signature verification before that row
is complete.

Proxy configuration recognizes `http_proxy`, `http_proxy.user`,
`http_proxy.password`, and `no_proxy`. An explicit config proxy takes
precedence over the lowercase environment values used by NuGet. Credentials
embedded in an environment proxy URL remain an HTTP-client concern. On
Windows, separate config user/password keys fail explicitly until `NUGET-011`
supplies encrypted-credential support; ciphertext is never mistaken for a
plaintext Basic Auth password. NuGet ignores those keys on other platforms,
and so does `dv`. Immutable results and reporter events expose only whether a
proxy policy was configured.

Project evaluation parses `NuGetAudit`, `NuGetAuditMode`, and
`NuGetAuditLevel` into booleans and compact enums. The .NET 10 default is
enabled, mode `all`, and level `low`; older modern targets default to mode
`direct`. Because advisory retrieval and policy enforcement remain `RES-024`,
an enabled audit fails explicitly rather than silently succeeding without an
audit; projects may set `NuGetAudit=false` to opt out.

## Data Layout

The hot package records remain unchanged. Actual per-package cache roots and
ordered fallback roots live in separate compact path-span batches so compiler
and asset scans do not pay for storage-policy fields. Variable path text is
owned once by `PackageResolution`. Proxy addresses and credentials never
enter that text allocation.

Network tasks borrow one immutable, 48-byte, pointer-aligned `PackageStorage`
view containing the global root, fallback slice, and scratch root. Fallback
enumeration is batched once per package identity, versions are sorted and
deduplicated once, and no fallback content is copied into the writable cache
merely to produce a plan. `ASSUMPTION: the benchmark CPU uses 64-byte cache
lines - affects the expectation that one complete storage view fits in one
cache line.` The view is immutable, so workers do not introduce false sharing.

## Verification

The benchmark builds a small adapter against the selected SDK's official
`NuGet.Common.dll` and `NuGet.Configuration.dll`. Before timing, it compares:

- global-packages, fallback, HTTP-cache, and scratch paths;
- signature-validation, proxy, and bypass policy;
- `NuGetAudit`, mode, and level as evaluated by MSBuild;
- package identity, version, archive SHA-512, and selected compile asset;
- the effective Microsoft `project.assets.json` package-folder list;
- zero timed downloads and HTTP requests in `dv`.

Both timed commands validate the same locked `Newtonsoft.Json` package from
the same fallback-only state. The global cache is absent, HTTP caching is
disabled for Microsoft, and `dv` is offline.

```powershell
cargo bench-all --case nuget_storage_policy --samples 30 --warmups 3
```

The curated result is recorded in
[`performance-baselines/2026-08-01-nuget-storage-policy-windows.md`](performance-baselines/2026-08-01-nuget-storage-policy-windows.md).
