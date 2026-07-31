# PackageReference Metadata Fixture

This `net10.0` fixture exercises every direct `PackageReference` policy field
implemented by `RES-004`. Newtonsoft.Json supplies one compile/runtime assembly;
runtime is deliberately excluded, the compile assembly is assigned `JsonAlias`,
and `PkgNewtonsoft_Json` must point at the selected package root.

Benchmarks prepare matching warm package and lock state before timing restore.
Preflight separately verifies compiler aliases and path properties. The local
NuGet configuration keeps the generated package-root property inside the
isolated `.packages` fixture cache for deterministic verification.
