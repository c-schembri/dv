# NuGet Proxy Credential Transport

## Question

How should `dv` decrypt and supply NuGet's separate `http_proxy.user` and
`http_proxy.password` configuration values without adding plaintext persistence
or a broad platform dependency?

## Current Boundary

`http_proxy` addresses and `no_proxy` bypass values are honored. Environment
proxy URLs may carry credentials according to the HTTP client contract. On
Windows, a config that supplies the separate user or encrypted password keys
fails explicitly; the ciphertext is never treated as a Basic Auth password or
emitted. Like NuGet, `dv` ignores those Windows-only keys on other platforms.

## Evidence Needed

- a Windows fixture written by `dotnet nuget config set http_proxy.password`;
- authenticated proxy tests for config, environment, bypass, cancellation, and
  redacted diagnostics;
- startup, binary-size, and build-time comparison of a narrow DPAPI adapter
  against any safe Rust dependency;
- a non-Windows compatibility decision, because NuGet only reads the separate
  encrypted credential pair on Windows.

This is part of `NUGET-011`, not the feed credential-provider protocol.
