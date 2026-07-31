# NuGet floating-version fixture

This project isolates floating version selection. Benchmark setup acquires real
`Newtonsoft.Json` `13.0.3` and `13.0.4` archives outside timing and builds one
local folder feed. Both tools enumerate that same feed, select the highest
stable `13.*` release, acquire it into an empty package root, and materialize
their typed restore state with zero HTTP work.

The project property is overridden with exact versions only while seeding. The
timed floating expression remains `13.*`; benchmark verification compares the
exact version, archive hash, target, and assets selected by the installed
stable Microsoft SDK and `dv` before timing either tool.
