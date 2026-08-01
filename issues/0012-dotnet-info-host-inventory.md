# Complete dotnet info host inventory

## Question

Which hostfxr, hostpolicy, RID, workload, commit, MSBuild, and installation-
provenance inputs are required to reproduce the script-consumed sections of
.NET 10 `dotnet --info` without loading Microsoft managed tooling?

## Why this remains open

`--list-sdks` and `--list-runtimes` now have exact current and selected-
architecture rows, but `--info` combines host-native state, SDK files, workload
manifests, OS metadata, all-architecture registration, and `global.json`
selection. Guessing or omitting those fields would make `DNCLI-001` look
complete while scripts still observe different data.

## Required evidence

1. Capture .NET 10 output on Windows, Linux, and macOS for native and alternate
   installed architectures, with and without workloads and `global.json`.
2. Identify each output field's authoritative local source and malformed-input
   behavior.
3. Define compact typed batches for host installations, workloads, and
   provenance with explicit bounds and stable ordering.
4. Compare normalized text and JSON events against the captured matrix, then
   benchmark cold and warm queries against Microsoft.
