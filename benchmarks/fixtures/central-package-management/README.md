# Central package management fixture

This `net10.0` project exercises the RES-006 contract through a real restore:
nearest-ancestor `Directory.Packages.props` discovery, versionless direct
references, `VersionOverride`, `GlobalPackageReference`, and central transitive
pinning of `Humanizer.Core`.

The benchmark measures an equivalent warm locked restore after both tools have
materialized their own package caches and lock state.
