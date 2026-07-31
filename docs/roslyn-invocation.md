# Direct Roslyn Invocation Strategy

## Boundary

Roslyn remains the C# compiler. `dv` owns the data transformation that discovers
and evaluates projects, resolves references, creates compiler input batches,
invokes the selected compiler, validates outputs, and records incremental
state.

Production execution must not invoke `dotnet`, MSBuild, NuGet, or VSTest. During
Phase 0 and compatibility tests, Microsoft orchestration is a reference oracle
only.

## Minimal Compiler Input Batch

For one `net9.0` console project, the compiler batch consists of:

- selected SDK and Roslyn version;
- ordered source paths and generated source paths;
- ordered reference-assembly paths from
  `packs/Microsoft.NETCore.App.Ref/<version>/ref/net9.0`;
- analyzer and source-generator paths required by the SDK contract;
- defines, language version, nullable mode, warning policy, optimization and
  debug settings;
- output assembly/PDB paths;
- deterministic path mapping and generated editor-config inputs;
- resource and additional-file inputs when present.

The batch owns normalized path-table indices rather than repeated path strings.
Sources and references are contiguous index ranges. The owner is the build plan;
the batch remains immutable from cache-key calculation through compiler exit.
Missing files, unsupported properties, cycles, and out-of-range indices reject
the batch before process creation.

## Compiler Hosting

The installed SDK contains `Roslyn/bincore/csc.dll`, but directly executing that
managed entry point still requires a Microsoft runtime host. Phase 1 will use
the selected runtime's native hosting interface (`hostfxr`) or a small
build-pinned managed compiler host launched through the native hosting
interface. Calling `dotnet exec` is acceptable for a compatibility experiment,
not production.

The initial choice must be measured for:

- cold and warm host startup;
- ability to reuse a compiler process without stale state;
- cancellation and crash isolation;
- compiler version fidelity to the selected SDK;
- structured capture of diagnostics without console scraping.

`ASSUMPTION: one long-lived compiler host amortizes managed runtime startup for
repeated builds - affects the hosting design and must be verified against cold
and daemonized compiler batches.`

Roslyn server reuse is not accepted merely because Microsoft uses it. A
persistent host adds state, invalidation, protocol, recovery, and memory cost.
The simple isolated host is Plan B until measurements show that persistence is
required to meet the startup target.

## Runtime Launch Data

An executable build also needs:

- selected Microsoft runtime version and RID;
- application assembly path;
- generated `.runtimeconfig.json`;
- generated `.deps.json` or an equivalent validated dependency description;
- probing roots and native asset selection;
- argument and environment byte data;
- working directory, cancellation, exit-code, stdout, and stderr policy.

Runtime launch is a typed process request. Console output is application data,
not orchestration state.

## Verification

Phase 1 must capture the reference compiler invocation for
`benchmarks/fixtures/small-console`, normalize it into the input batch above,
invoke the same Roslyn compiler through native hosting, and compare artifacts
and observable execution. The exact trace is machine/SDK-specific evidence and
must be regenerated when the selected SDK changes.

## Initial Observed Trace

On 2026-07-31, SDK `10.0.100` building the `net9.0` fixture with shared
compilation disabled invoked:

```text
C:\Program Files\dotnet\sdk\10.0.100\Roslyn\bincore\csc.exe
```

Observed compiler version: `5.0.0-2.25523.111`. The 21,651-character command
contained:

- 164 reference-assembly arguments from
  `Microsoft.NETCore.App.Ref\9.0.11`;
- 8 analyzer/source-generator arguments;
- 3 analyzer-config arguments;
- `Program.cs` plus 3 generated C# inputs;
- portable PDB, deterministic compilation, C# 13, nullable enabled, and the
  SDK-defined target-framework constants.

The compiler emitted a 4,608-byte managed assembly and 11,340-byte portable PDB
into intermediate output. The SDK then produced/copied a 156,160-byte apphost,
428-byte dependency manifest, and 268-byte runtime configuration.

These counts are observed data for this SDK and fixture, not constants. Phase 1
must discover them from selected SDK contents and evaluated inputs rather than
hard-coding the values.
