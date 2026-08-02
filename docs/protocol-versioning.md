# Command And Event Protocol Versioning

`CLI-017` gives command grammar and JSON compatibility separate version
identities. The current command syntax is `6`; the current event schema is
`23`. A command alias can therefore be added or retired under the syntax
contract without pretending that the JSON object layout changed.

## Data Contract

`CommandSyntaxVersion` is a two-byte transparent value stored inside the
existing 6-byte `InvocationRequest`. It is assigned during the single linear
argv scan and copied into `command_started` only when JSON output is requested.
Human commands pay no formatting, allocation, filesystem, process, or network
cost for it.

Every JSON event retains the independent top-level `schema_version`. Schema 19
added `command_syntax_version` to `command_started` and one `tool_version`
payload containing:

- the `dv` executable version;
- the current command syntax version;
- the current event schema version.

Schema 20 adds the typed `compatibility_checked` payload for a bounded static
scan. The syntax remains version 2 because no existing command spelling or
precedence changed incompatibly.

Schema 21 adds the ordered `runtime_inventory` payload. Command syntax 3 adds
the executable `dotnet --list-sdks` and `dotnet --list-runtimes` compatibility
queries; the two versions advance independently for these separate contract
changes.

Schema 22 adds the typed `repository_root_discovered` payload. Command syntax
4 adds `dv project root [PATH]`; the syntax and schema advance together because
the slice introduces both a command surface and a JSON payload.

Schema 23 adds the typed `workspace_inputs_discovered` payload. Command syntax
5 adds `dv project inputs [PATH]`; the command reports the same shared ancestor
batch consumed internally by SDK, NuGet, and central-package discovery.

Command syntax 6 changes native `version`, `--version`, and `-V` to select and
report the active .NET SDK, matching `dotnet --version`. The new
`dv self-version` command owns dv's executable identity. This is a syntax-only
change: `sdk_selected` and `tool_version` already exist in schema 23.

`dv --json --version` emits `command_started`, `sdk_selected`, and
`command_finished`. `dv --json self-version` emits `command_started`,
`tool_version`, and `command_finished`.

## Change Rules

- Increment the command syntax version when accepted command/option spelling,
  placement, precedence, or alias behavior changes incompatibly.
- Increment the event schema only when JSON field, type, tag, ordering, or
  semantic compatibility changes.
- A syntax-only change must not increment the event schema.
- An event-only change must not increment the command syntax version.
- Raw reporter-safe argv remains event data, so aliases may appear in `args`;
  the typed event command and schema do not derive from that spelling.
- Syntax versions are build-owned and never inferred from argv. Event readers
  reject unsupported schema versions; neither value is clamped, guessed, or
  silently downgraded.

The versions are genuine command-wide singletons. No per-event version object,
lookup table, heap allocation, dynamic dispatch, or compatibility negotiation
is added to the common path.

## Verification

Cross-platform CLI tests execute native `version`, `--version`, and `-V`
against an isolated SDK root. Every alias must produce the same schema-23
`sdk_selected` event, canonical `sdk current` command, syntax version `6`, and
successful terminal event. Separate tests prove `self-version` succeeds without
an SDK installation and reports the executable plus both protocol versions.
The benchmark preflight validates both contracts before retaining samples.

Microsoft tooling has no equivalent query for `dv`'s two protocol versions, so
the dedicated `self-version` benchmark reports Microsoft as `TBI`; it is not a
like-for-like speed claim. The `cli_version` case now compares the identical
`dotnet --version` and `dv --version` selected-SDK result. Thirty warm Windows
samples measured `65.047 ms` versus `5.559 ms`, an `11.7x` median improvement.
The separate JSON self-version query measured `5.037 ms`; no Microsoft speed
comparison is claimed. See the
[syntax-6 baseline](performance-baselines/2026-08-02-cli-version-routing-windows.md).

Reproduce:

```powershell
cargo bench-all --case cli_protocol_version --samples 30 --warmups 5
cargo bench-all --case cli_version --samples 30 --warmups 5
```
