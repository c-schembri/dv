# Package conflict resolution fixture

Benchmark setup creates a deterministic local feed with two independent
graphs. `Direct.Top` requests `Direct.Leaf` 1.0.0 beside a deeper request for
2.0.0, exercising direct-dependency-wins inside a package subgraph. The cousin
graph requests `Cousin.Leaf` from unrelated branches at different depths and
must converge on 2.0.0. Its two versions have different children; the selected
graph must retain `Cousin.Current` and retract `Cousin.Stale`. A diamond graph
also reaches the project-direct `Diamond.Relational` through
`Diamond.Provider`; its independent root path keeps its `Diamond.Leaf` 2.0.0
constraint active as a cousin.

Both tools resolve the same fifteen local archives into eleven selected
identities from a warm package cache with restore outputs and locks removed
before every sample. Microsoft restore receives `NoWarn=NU1605` because the
SDK promotes that valid direct-wins downgrade warning to an error by default;
preflight separately requires the unsuppressed warning and selection before
timing.

Five additional projects make package diagnostics deterministic. The timed
`ConflictFailure.csproj` case starts with an empty package cache and resolves
two incompatible exact leaf constraints from an eight-archive local feed. It
must fail as Microsoft `NU1107` and structured `dv` `DV0414`. Preflight also
checks cycle (`NU1108`/`DV0415`), missing package (`NU1101`/`DV0416`), missing
version (`NU1102`/`DV0417`), and incompatible framework
(`NU1202`/`DV0402`) failures. The successful direct-wins project emits
`DV0413` from both cold resolution and the native warm lock.

`PackageBatch.csproj` references two child projects with identical cousin and
diamond graphs. Each child selects the same eight identities from the shared
fifteen-archive feed. The cold benchmark removes package caches, project
outputs, and locks before every sample. Microsoft restores the root project;
`dv` expands the literal project-reference closure and emits the package-free
root followed by both child resolutions. Preflight requires exact child graph
parity, 16 resolved rows, only eight archive publications, and zero HTTP work.
