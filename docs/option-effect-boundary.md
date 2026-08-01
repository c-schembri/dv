# Option Effect Boundary

`DROP-022` makes accepted Phase 1 options part of typed command data instead of
allowing a parser or reporter to silently discard them. Unsupported options
fail at the active command boundary before project, SDK, filesystem, child
process, or network work.

## Transform Contract

The input is the existing ordered `CommandArguments` view plus the ordered
environment-directive view. Build, restore, run, and test consume every token
in one strict linear pass:

```text
borrowed semantic arguments + environment directives
  -> one ordered option scan
  -> project selection + configuration + environment edit batch
  -> typed unsupported child boundary, or immediate usage failure
```

The accepted run/test dimensions are one positional or named project, one
Debug/Release configuration, repeatable environment edits, and the already
separated child tail after `--`. Project spelling remains a borrowed OS path.
Configuration is a one-byte enum. Environment edits retain source, byte
offsets, and secret classification without copying values. An unknown option,
second project, repeated singleton, malformed assignment, missing value, or
non-Unicode text value fails rather than becoming a no-op.

The 64-bit `ChildCommandOptions` batch is 136 bytes: a 24-byte project
selection, adjacent compact configuration state, and the 104-byte four-edit
inline environment batch. Up to four environment edits
allocate nothing; the fifth promotes once to contiguous dynamic storage
because the count is externally variable. Diagnostics allocate only after the
command is known to terminate.

Access is linear and branch behavior follows the option token. Common project,
configuration, and zero-to-four environment batches remain stack backed. The
simplification pass removed both the build marker pre-scan and the child
environment rescan: each parser now validates every argument and constructs
its owned typed effect in the same pass. A registry, hash table, trait
hierarchy, filesystem probe, or speculative parser for future Microsoft
options would add cost without strengthening the current contract.

## Evidence

Unit coverage proves accepted project, configuration, directive, separate
environment, and combined environment forms change the typed batch. Boundary
tests prove the values reach structured diagnostic context without reading a
deliberately malformed project, while unsupported child options fail as
`DV0002` rather than falling through to the unimplemented-operation result.

The benchmark replaces only the executable route around `test
--definitely-unknown`. .NET 10 and `dv --compat dotnet` both return exit 1,
retain the exact sentinel, reject it as an unknown switch/option, and leave the
fixture byte-for-byte unchanged. Results are retained in the
[Windows option-effect baseline](performance-baselines/2026-08-02-cli-option-effects-windows.md).
