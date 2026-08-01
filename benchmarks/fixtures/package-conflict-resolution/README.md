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

Both tools resolve the same fifteen local archives into eleven selected identities
from a warm package cache with restore outputs and locks removed before every
sample. Microsoft restore receives `NoWarn=NU1605` because the SDK promotes
that valid direct-wins downgrade warning to an error by default; selection is
still verified before timing.
