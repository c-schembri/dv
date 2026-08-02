# NuGet Configuration Discovery

`NUGET-001` discovers the same configuration scopes used by current NuGet
client tooling without launching `dotnet`, MSBuild, or NuGet. The merge stage
consumes paths from lowest to highest precedence so later values replace
earlier values directly.

## Data Contract

Input:

- one absolute project directory;
- zero or one explicit configuration file;
- platform machine and user roots derived once from process environment;
- non-recursive machine and additional-user configuration directories.

Output:

- one precedence-ordered `Vec<PathBuf>` owned for the configuration merge;
- at most one ancestor config per directory;
- only the explicit file when `--configfile` is present.

The representative hierarchy contains six files: two machine fragments, two
additional-user fragments, the main user file, and the repository file. Paths
are externally sized OS data, so the vector and each retained `PathBuf` own
dynamic storage until the configuration merge completes. The output vector is
the batch-first API; the project directory is a genuine singleton because it
defines one precedence chain. There are no per-setting objects in discovery,
and paths are not converted to UTF-8.

## Precedence

Implicit discovery emits paths in this low-to-high order:

1. machine-wide `*.config` fragments;
2. additional-user `config/*.config` fragments;
3. the main user `NuGet.Config`;
4. one config per ancestor from filesystem root to project directory.

Fragment names are ordered deterministically in reverse filesystem order for
the low-to-high merge, matching NuGet's most-significant-first fragment
priority. Every ancestor probes `nuget.config`, then `NuGet.config`, then
`NuGet.Config`; the active directory decides whether those are aliases or
distinct files, and a successful lookup retains the actual recognized entry
spelling. `NuGet.Config` inside the additional-user fragment directory is
excluded because it is the main-file name, not an add-on. Non-standard casing
of that name or a fragment extension is accepted only when it resolves to the
same physical entry as the canonical spelling on the active filesystem.

Windows uses `%ProgramFiles(x86)%\NuGet\Config` and
`%APPDATA%\NuGet`. Linux uses `/etc/opt/NuGet/Config`; macOS uses
`/Library/Application Support/NuGet/Config`. On macOS and Linux,
`NUGET_COMMON_APPLICATION_DATA` replaces the machine base. The .NET CLI user
root is `~/.nuget/NuGet` on Unix.

An explicit file is resolved against the caller's working directory by the
CLI, must exist as a file, and suppresses every implicit scope. Missing or
inaccessible implicit fragment directories are skipped like NuGet; a missing
explicit file or unreadable selected file fails before network work. Symlinked
config files are accepted. Discovery does not canonicalize paths or recursively
scan fragment directories.

## Cost And Access

Ancestor traversal, directory enumeration, sorting, and merge are linear.
Branches are strongly predictable: most ancestors have no config and most
directory entries are rejected by extension. Work is sequential because the
observed six-file batch is far below a useful threading crossover.

There is no purpose-built hot record to align: traversal consumes contiguous
`PathBuf` entries from the standard-library vector, while filesystem metadata
and XML reads dominate. Adding a padded record or cache-line assumption would
increase retained bytes without improving this access pattern.

The simplification pass retained no discovery cache: config files may change
between commands, while the measured full process is already near the local
5 ms startup budget. It removed a global OS case flag, write-based probing,
recursive scans, and an abstraction for unobserved configuration locations.
Canonicalization occurs only for an enumerated non-standard case variant.

## Verification

Unit tests inject isolated roots and prove machine, additional-user, user,
drive/repository, casing, fragment ordering, explicit isolation, and missing
explicit-file failure. A CLI test proves a relative `--configfile` suppresses
an invalid repository config and resolves its relative package directory.

The process oracle gives both tools the same six-file hierarchy, warm package
cache, and native lock. Every retained sample launches a cold process and runs
the uncached configuration transform; package state stays warm so network and
extraction do not hide discovery cost. There is no distinct in-process warm
state to benchmark. Preflight checks Microsoft's exact six-path priority order,
then compares the effective cache root, source and protocol, `Newtonsoft.Json`
`13.0.3` identity, version, and SHA-512. Thirty
retained Windows samples measured `532.948 ms` for `dotnet restore` and
`5.651 ms` for `dv restore`, a `94.3x` median improvement.

```powershell
cargo bench-all --case nuget_config_hierarchy --samples 30 --warmups 3
```

Authoritative references:

- [Microsoft configuration hierarchy](https://learn.microsoft.com/nuget/consume-packages/configuring-nuget-behavior)
- [NuGet `Settings` implementation](https://source.dot.net/NuGet.Configuration/Settings/Settings.cs.html)
