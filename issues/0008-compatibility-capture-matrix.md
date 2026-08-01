# Define The Compatibility Capture Matrix

## Question

Which OS, architecture, SDK feature band, optional workload, and separately
installed SDK-extension combinations must ship as reviewed compatibility
manifests for a release?

## Current Boundary

`compatibility/manifest.json` captures one explicitly selected Windows x64
.NET 10 SDK and its bundled MSBuild, NuGet, and VSTest surfaces. The artifact
records that provenance. The generator can capture another selected tool set,
but the release currently embeds one artifact and does not claim that optional
extensions absent from that installation were inventoried.

## Resolve By

1. Collect deterministic captures for supported Windows, Linux, and macOS
   release targets and compare command and alias distributions.
2. Identify architecture-specific and workload-added commands from actual
   installations rather than assumptions.
3. Decide whether releases embed a union with availability predicates or one
   manifest per target/reference set.
4. Add a release gate that rejects unexplained manifest drift and selects the
   correct reviewed artifact without runtime Microsoft-tool discovery.

## Close When

The supported capture matrix, artifact-selection rule, drift policy, and
optional-extension boundary are explicit and enforced in release CI.
