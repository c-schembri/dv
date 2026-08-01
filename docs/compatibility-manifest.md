# Compatibility Manifest

`DROP-001` makes the selected Microsoft command surface a retained, versioned
data artifact rather than an assumption buried in parser code. Manifest schema
and content version `1` currently target:

- .NET SDK `10.0.100`;
- MSBuild `18.0.2.52411`;
- NuGet `7.0.0.0`;
- VSTest `18.0.1` on Windows x64.

The checked-in support source is
`compatibility/phase1-support.json`. The generated artifact is
`compatibility/manifest.json`; the versioned content, not its path, identifies
the selected reference set.

`ASSUMPTION: selected SDK help plus the reviewed environment, exit, and output
inventories describe the release-owned surface - affects separately installed
SDK extensions and optional workloads.`

## Data Contract

The generator receives one selected `dotnet` executable, its required exact SDK
version, the support source, and `docs/feature-parity-map.md`. It produces one
sorted JSON batch containing:

- selected reference-tool versions and capture platform;
- command paths, the exact help probe argv, help exit, usage shapes, positional
  arguments with explicit zero-based positions, option syntax, aliases, value
  shapes, defaults, descriptions, and child paths;
- canonical `dv` paths and implemented/partial/missing state independently for
  options, arguments, defaults, environment, exits, and outputs;
- declared environment inputs, observed per-tool invalid-input exits, and
  script-consumed output formats;
- the complete parity ledger, including every known unsupported row.

The current Windows capture contains 115 command records, 769 option records,
74 argument records, 17 environment records, four failure-exit records, ten
output-format records, and 468 parity rows. MSBuild option records expand the
documented `-`, `/`, and `--` prefixes plus advertised short forms before the
batch is sorted and deduplicated.

Capture owns its dynamic strings and vectors only until the JSON file is
published. It walks a bounded queue of at most 512 command paths to depth four.
Each reference process has a 20-second deadline; stdout and stderr are drained
concurrently and retained only up to 1 MiB each. A timeout, excess output,
depth/count overflow, unexpected SDK, duplicate path, dangling child, unknown
parity reference, or malformed source fails without replacing an existing
manifest.

Some SDK help injects the process working directory as an argument default.
Capture normalizes only that observed path to `$CWD`; documented example paths
remain unchanged. Invalid UTF-8 fails instead of being replaced lossily.

## Production Query

The production transform is deliberately smaller:

```text
typed `compat manifest` request
  -> borrow one compile-time byte slice
  -> one `write_all` to stdout
```

`dv compat manifest` performs no SDK discovery, JSON parsing, manifest-model
allocation, filesystem read, process launch, or network request. It linearly
writes the embedded artifact once. Other commands never access those pages.
The embedded data increases the Windows release executable by 271,872 bytes;
interleaved before/after startup samples did not show a directional regression.
The retained query writes 270,222 output bytes after the repository-root
support and protocol-version update.

The manifest is an inventory and claim boundary, not evidence that missing or
partial commands execute. A command cannot become drop-in compatible until its
dimension states and referenced parity rows are implemented and independently
verified.

The capture path is intentionally cold and dynamically allocates contiguous
buffers because reference help is externally sized. Those buffers die with the
generator. The production artifact is an immutable byte-aligned byte array, so
there is no mutable hot record, worker state, false-sharing boundary, or cache
alignment requirement. `--json` is idempotent for this command because the
singleton artifact is already the versioned structured JSON output.

## Regeneration

The generator refuses to overwrite an existing artifact. Review the support
source and parity map, capture to a new file, and inspect its deterministic
diff before replacing the reviewed artifact:

```powershell
$sdk = (dotnet --version).Trim()
cargo run -p dv-compat-manifest --release -- capture `
  --dotnet (Get-Command dotnet).Source `
  --expected-sdk $sdk `
  --support compatibility\phase1-support.json `
  --parity-map docs\feature-parity-map.md `
  --output target\compatibility-manifest.next.json

cargo run -p dv-compat-manifest --release -- check `
  target\compatibility-manifest.next.json
```

Generation is release/development tooling and may invoke Microsoft reference
tools. Production `dv` never uses them as a fallback.

## Performance Evidence

The structural query and its like-for-like SDK control are recorded in
[the Windows baseline](performance-baselines/2026-08-01-compatibility-manifest-windows.md).
The unresolved release capture matrix is tracked in
[issue 0008](../issues/0008-compatibility-capture-matrix.md).
