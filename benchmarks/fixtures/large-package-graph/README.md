# Large Package Graph

This immutable `net10.0` fixture anchors cold dependency-graph throughput.
`Humanizer` `2.14.1` supplies a real, versioned 50-package graph
NuGet dependency graph rather than a generated collection of unrelated direct
references. Its many small locale packages make graph expansion, scheduling,
and cache publication visible without turning the case into a raw bandwidth
test.
