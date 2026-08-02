# Project Path Identity

`WS-006` separates the path a user supplied from the identity used for one
command. This keeps diagnostics faithful without paying for physical
canonicalization or accidentally treating a missing path as a discovered
project.

## Data Contract

Input is one borrowed platform path from explicit selection or a
`ProjectReference`. The common successful input is an existing regular
`.csproj`; dot segments are allowed. Missing paths are valid diagnostic input.

The transform is:

1. Validate the extension and query metadata using the original spelling.
2. Reject missing and non-file inputs before identity allocation.
3. Make an existing path absolute and remove `.` and `..` lexically.
4. Search the sorted command-local identity vector.
5. Evaluate a new project and insert its identity only after success.

Output is a temporary `ResolvedProjectPath`: one borrowed `&Path` spelling and
one owned `PathBuf` identity. It is 48 bytes on 64-bit Windows and 40 bytes on
other 64-bit targets, aligned to `usize`, so one temporary record fits inside
the assumed 64-byte benchmark-host cache line. The record lives only through
one evaluation. Successful `ProjectSpec` data owns the normalized identity;
errors retain the original spelling.

The closure access pattern is linear over each project's reference batch with
binary search in a sorted identity vector. Existing/missing and
new/already-seen branches are expected to be predictable for ordinary graphs.
The only allocation added by the transform is the identity buffer required by
an existing variable-length external path. A shared offset path table is
deliberately deferred to `WS-010`, where reuse can be measured across commands.

## Boundaries

- Unsupported extensions, missing paths, directories, malformed XML, and
  invalid properties fail with typed diagnostics using input spelling.
- A missing reference never enters the identity vector and reports `DV0200`,
  not a canonicalization I/O failure.
- Lexical normalization itself does not resolve symlinks or junctions.
  `WS-007` now resolves physical identity only when source traversal or project
  closure requires a cycle/escape proof; active-filesystem case behavior
  belongs to `WS-008`.
- Physical canonicalization remains in Unix SDK discovery because a PATH host
  such as `/usr/bin/dotnet` must resolve to the installation containing `sdk/`.

## Verification

Focused tests cover successful normalization, exact error spelling, missing
reference classification, and lexical deduplication of a diamond graph. The
immutable benchmark makes both tools resolve
`missing/../missing/Absent.csproj`, requires the same spelling in their typed
missing-project failures, and proves neither command mutates its fixture.

Thirty warm Windows samples measured Microsoft at `200.255 ms` median and
`221.558 ms` p95 versus `5.423 ms` and `6.631 ms` for `dv`. The median
improvement is `36.9x`; no retained sample was removed. Cold filesystem-cache
behavior is unverified.

```powershell
cargo bench-all --case path_identity --samples 30 --warmups 5
```

Raw samples: `target/path-identity-ws006-final.json`.
