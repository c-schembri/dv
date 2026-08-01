# Command And Event Protocol Versioning

`CLI-017` gives command grammar and JSON compatibility separate version
identities. The current command syntax is `3`; the current event schema is
`21`. A command alias can therefore be added or retired under the syntax
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

`dv --json --version` emits exactly `command_started`, `tool_version`, and
`command_finished`. The normal `dv --version` text remains unchanged.

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

Cross-platform CLI tests execute native `version`, `--version`, and `-V`.
Every native alias must produce the same three-event schema-21 shape, canonical
`version` command, syntax version `3`, and successful terminal event. Under the
explicit dotnet profile, `--version` instead selects the same SDK as Microsoft
`dotnet --version`; this compatibility correction is why syntax versions are
not interchangeable. The benchmark preflight validates both
contracts before retaining samples.

Microsoft tooling has no equivalent query for `dv`'s two protocol versions, so
the dedicated benchmark reports Microsoft as `TBI`; it is not a like-for-like
speed claim. A separate `dotnet --version` / `dv sdk current` control proves the
shared startup path remains faster. Results are recorded in the
[Windows baseline](performance-baselines/2026-08-01-cli-protocol-version-windows.md).

Reproduce:

```powershell
cargo bench-all --case cli_protocol_version --samples 30 --warmups 5
cargo bench-all --case sdk_current_compat --samples 30 --warmups 5
```
