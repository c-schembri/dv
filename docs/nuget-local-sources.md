# NuGet Local Source Contract

`NUGET-006` completes the initial source transport boundary with NuGet flat
folders, hierarchical local feeds, and the existing HTTPS v2/v3 clients. The
production resolver remains native Rust and never invokes Microsoft tooling.

## Source Shapes

Local sources may be supplied as configuration-relative paths, absolute paths,
`file://` URIs, or CLI paths resolved against the working directory. Source
order and package-source mapping indices remain those of the merged
configuration.

Two official folder layouts are recognized:

- flat v2 folders containing `.nupkg` files at the root or one directory deep;
- hierarchical v3 folders containing
  `{lower-id}/{normalized-version}/{lower-id}.{version}.nupkg` with the matching
  root nuspec and SHA-512 sidecar.

The resolver detects each layout once. Flat archive paths are sorted and
retained in one immutable `Arc<[PathBuf]>`; concurrent graph workers filter
that batch without repeating directory enumeration. Hierarchical lookups use
the normalized identity/version path directly. Variable archive and manifest
text is external data and therefore owns bounded buffers only while being
validated.

HTTPS v2 sources support exact OData metadata plus bounded, cycle-checked
`FindPackagesById` continuation pages for version ranges. HTTPS v3 sources
continue to discover their advertised `PackageBaseAddress` and bounded version
indices from the service index. Every remote URL remains HTTPS-only.

## Transform

```text
ordered source rows
  -> partition local paths from HTTPS v2/v3 endpoints
  -> detect each local layout once
  -> enumerate compatible versions when the range is not exact
  -> verify archive nuspec identity/version
  -> hard-link into cache-volume staging, or copy across volumes
  -> stream SHA-512 once and verify hierarchical source metadata
  -> validate ZIP limits and paths
  -> extract and atomically publish the NuGet-compatible cache entry
  -> materialize the existing contiguous package/asset batches
```

Local acquisition is valid under `--offline`: it performs no HTTP request.
Independent packages retain the bounded Tokio task scheduler and blocking
archive workers, while final graph and output ordering remain deterministic.

## Boundaries

- Missing or inaccessible folders fail explicitly; they are never interpreted
  as remote endpoints.
- Insecure HTTP remains rejected until `NUGET-012` defines explicit opt-in.
- Unknown URI schemes fail instead of becoming filesystem guesses.
- Flat package identity and normalized version come from the archive nuspec,
  not only its filename.
- Hierarchical feeds require their nuspec and SHA-512 sidecars, and a hash
  mismatch fails before publication.
- Archive size, expanded size, entry count, traversal, duplicate-path, and
  symbolic-link limits are identical to remote acquisition.

## Verification

The `nuget-local-sources` fixture maps `Newtonsoft.Json` to a flat feed and
`Humanizer.Core` to a hierarchical feed. Before every retained sample the
global package cache and restore outputs are removed. Preflight compares the
Microsoft assets source paths and both tools' package identities, versions,
and archive SHA-512 values. `dv` must publish both packages while reporting
zero HTTP requests.

```powershell
cargo bench-all --case nuget_local_sources --samples 30 --warmups 3
```
