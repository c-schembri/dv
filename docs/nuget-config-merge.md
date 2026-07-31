# NuGet Keyed Configuration Merge

`NUGET-002` merges the keyed NuGet settings currently consumed by package
resolution without launching Microsoft tooling. Configuration files arrive as
the low-to-high precedence batch produced by `NUGET-001`; each operation is
applied once in stream order.

## Data Contract

Input:

- one ordered batch of configuration paths, owned through the merge;
- UTF-8 XML bytes read once per file;
- `packageSources`, `disabledPackageSources`, and `config` keyed operations;
- process environment values referenced as `%NAME%`.

Output:

- one contiguous `Vec<(String, PackageSource)>` in deterministic source order;
- one contiguous `Vec<String>` of disabled source names;
- zero or one owned global-packages path.

Source names, URLs, and paths are externally sized, so their owned strings are
necessary dynamic allocations. The vectors retain capacity for the whole
merge and live only until package resolution owns the selected configuration.
The representative four-file input is 1,632 bytes and contains 18 keyed
operations.

## Transform

Each file is read, XML-tokenized, and merged before the next file:

1. `<clear />` empties the active section.
2. `<add key=... value=... />` replaces a case-insensitive existing key in
   place or appends one new value.
3. `<remove key=... />` removes every case-insensitive match.
4. `disabledPackageSources` tracks key presence, matching current NuGet client
   behavior; valid stored values are still restricted to Boolean text.
5. `globalPackagesFolder` resolves relative to the file that supplied it.
6. `%NAME%` values expand in one pass. Missing variables and markers produced
   by a replacement remain literal.

Malformed XML, missing keys or values, invalid disabled-source Boolean text,
invalid protocol versions, unreadable files, and non-Unicode expanded values
fail with a configuration diagnostic before network work. Unknown sections
and unsupported keys are ignored rather than guessed.

## Cost And Access

XML access is linear and source/disabled updates are linear scans over tiny,
contiguous batches. Branches select one of three known sections and one of
three operations. Unknown elements stay on the cold path. The disabled-name
batch deliberately uses a vector rather than a tree: NuGet matching is
case-insensitive, so a `BTreeSet` still required linear replacement scans while
adding node allocation and pointer chasing.

Environment expansion returns the original string without allocating when no
known marker resolves. On the first match it allocates one capacity-sized
output and appends subsequent spans. The observed batch is far below a useful
threading crossover; parallel XML parsing would add scheduling, buffering, and
ordered-merge costs without removing the filesystem latency.

The simplification pass removed a generic setting-object graph, recursive
expansion, a map per section, per-operation allocation, and concurrent parsing.
There is no purpose-built fixed-size hot record or cache-line alignment: the
dominant data is variable external text and the complete batch fits within a
few cache lines once parsed.

## Verification

Unit tests prove low-to-high clear/add/remove behavior, case-insensitive source
replacement, disabled-source removal, config clearing, relative path origin,
single-pass expansion, unknown markers, and chained replacement preservation.

The paired process oracle gives both tools one machine, additional-user, main
user, and repository config, an isolated warm package cache, and native lock.
Preflight compares the effective environment-expanded v3 source, disabled and
removed sources, package path, `Newtonsoft.Json` `13.0.3` identity/version, and
archive SHA-512. Every retained sample starts a cold process and performs an
uncached merge; package state stays warm to keep network work out of timing.

Thirty retained Windows samples measured `558.126 ms` for `dotnet restore`
and `9.422 ms` for `dv restore`, a `59.2x` median improvement.

```powershell
cargo bench-all --case nuget_config_merge --samples 30 --warmups 3
```

Authoritative references:

- [NuGet configuration file reference](https://learn.microsoft.com/nuget/reference/nuget-config-file)
- [NuGet `Settings` merge implementation](https://source.dot.net/NuGet.Configuration/Settings/Settings.cs.html)
- [NuGet package-source provider](https://github.com/NuGet/NuGet.Client/blob/dev/src/NuGet.Core/NuGet.Configuration/PackageSource/PackageSourceProvider.cs)
