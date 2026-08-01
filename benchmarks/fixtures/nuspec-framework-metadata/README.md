# nuspec framework metadata fixture

`Framework.Metadata` has separate dependency and framework-reference groups
for `net8.0` and `net10.0`, followed by legacy framework assemblies. The
generated local feed intentionally omits the `net8.0` dependency so crossing
container or group boundaries fails preflight instead of producing a plausible
graph.

For `net10.0`, both tools must select `Framework.Child` and
`Microsoft.AspNetCore.App` while excluding the `net8.0` rows and all legacy
.NET Framework assemblies. Every timed sample starts with an empty package
cache and no restore outputs or locks.

The untimed `net48` oracle must select the exact/nearest `System.Data` and
`System.Xml` group. It must not merge the unscoped `System.Runtime` fallback or
the `net472`-only `System.Net.Http` row into that compatible group.

The untimed `net48` parity gate selects the exact framework-assembly group,
`System.Data` and `System.Xml`, while excluding modern framework references and
the unscoped fallback assembly. It also proves that `dv`'s warm lock reproduces
the same sparse metadata without reopening either nuspec.
