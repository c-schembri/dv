# Small Console Fixture

This is the smallest compatibility and performance input: one SDK-style
`net10.0` executable project, one source file, no package references, and no
network requirement.

The benchmark harness copies this directory before mutating it. Generated
`bin/` and `obj/` state must never be committed here.
