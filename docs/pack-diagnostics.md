# Unavailable Pack Diagnostics

`PACKS-009` turns unavailable TFM, RID, runtime, host, targeting-pack, and
shared-framework decisions into typed requirements. Production code does not
launch or scrape `dotnet`, MSBuild, NuGet, or the host to construct them.

## Data Contract

Input:

- an already evaluated project TFM and optional requested RID;
- manifest-selected pack identity and optional exact version;
- the pack role: runtime, host, targeting, or shared framework;
- the boundary which disproved availability: SDK manifest compatibility,
  installed pack roots, global package cache, or installed shared runtimes.

One failed selection produces one `PackRequirement`. This is a documented true
singleton: planning stops at the first dependency that prevents a valid plan,
so no downstream work can produce a truthful second requirement. A future
whole-solution diagnostic collector must batch these records at its boundary;
it must not weaken fail-fast planning.

Output fields, in reporter order:

1. `pack_kind`;
2. `pack_identity`;
3. optional `pack_version`;
4. `target_framework`;
5. optional `runtime_identifier`;
6. `acquisition`.

The owner is the existing planning error. The requirement lives exactly as
long as that error, and the CLI borrows it while materializing human or JSON
output. Identity, version, TFM, and RID are non-empty values already validated
by their manifest or project boundary. Absent dimensions use a sentinel span,
not an allocated empty string.

## Transform And Boundaries

```text
evaluated dimensions + manifest selection + availability proof
  -> classify the unavailable dependency
  -> copy its variable text once
  -> retain four offsets plus kind and acquisition
  -> append stable diagnostic context at the reporting edge
```

Boundary behavior is explicit:

- a requested RID absent from the SDK graph/manifest combination reports the
  expanded runtime or host identity and `choose_runtime_identifier`;
- an exact runtime or host pack absent from installed and global-package roots
  reports `restore_package`;
- a compiler targeting pack supplied only by an SDK reports `install_sdk`;
- an SDK framework targeting pack available through either supported root
  reports `install_sdk_or_restore_package`;
- a missing or roll-forward-incompatible shared framework reports
  `install_runtime`;
- malformed manifests, versions, and paths retain
  their existing dedicated diagnostic rather than inventing a requirement.

Unsupported inputs fail. No path silently chooses another TFM, guesses RID
compatibility by splitting text, downloads a package, or changes roll-forward
policy.

## Layout And Cost

On 64-bit targets `PackRequirement` is 56 bytes with pointer alignment,
protected by compile-time assertions. It owns one `Box<str>` and four 8-byte
end offsets; the two small enums occupy the remaining header space. Variable
external text requires storage beyond the error header, so one right-sized
allocation on the rare failure path is justified. Per-field
`String`, `Vec`, maps, trait objects, reference counting, and extra indirection
were removed by the simplification pass.

Successful planning constructs no requirement, performs no extra filesystem or
network operation, and allocates no diagnostic text. Failure construction
copies each retained byte once. Reporting is a fixed straight-line sequence
with predictable optional-version and optional-RID branches. The record is
cold, immutable, and never independently mutated by workers, so cache-line
padding or stronger alignment would only increase retained bytes.

The dominant cost remains process startup plus the existing SDK manifest, RID
graph, and pack-root reads needed to prove availability. Diagnostic typing adds
no process, filesystem, or network operation.

## Verification

Unit tests cover unsupported RID, missing runtime/host/targeting packs, missing
shared frameworks, exact fields, layout, and acquisition classification. The
CLI integration test proves stable JSON context and guidance.

The process oracle uses a self-contained `net10.0` project requesting the
SDK-recognized `linux-arm` RID, an empty checked-in package source, and a fresh
isolated package cache. Before timing, Microsoft restore must fail with
`NU1101` and `Microsoft.NETCore.App.Runtime.linux-arm`; `dv` must fail with
`DV0124` and the exact identity, `10.0.0`, `net10.0`, `linux-arm`,
`runtime_pack`, `restore_package`, and acquisition guidance.

The 30-sample Windows result is `532.652 ms` for Microsoft restore versus
`6.378 ms` for `dv`, an `83.5x` median improvement. Existing successful
runtime-pack and framework-plan benchmarks cover the no-error path.

```powershell
cargo bench-all --case pack_diagnostic --samples 30 --warmups 3
```
