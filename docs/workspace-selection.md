# Workspace Selection

`WS-003` is the directory-candidate decision that follows `WS-001` discovery.
It selects exactly one immediate project or solution without assuming that the
selected kind can already be evaluated.

## Contract

`select_workspace` discovers and then consumes one `WorkspaceInventory`. Zero
candidates return a typed `NotFound` error. One candidate of any recognized C#, F#, Visual Basic,
`.sln`, or `.slnx` kind becomes a `WorkspaceSelection`. Multiple candidates
return `Ambiguous` with the first 16 stable sorted candidate rows and a
remaining-count field. Selection reads no candidate contents.

The selected record is 40 bytes on 64-bit Windows and 32 bytes on other current
64-bit targets. It owns one absolute `PathBuf` plus a one-byte kind. The
inventory's root buffer is moved into the result and extended with the selected
arena slice, avoiding a second copy of the root. The candidate vector and path
arena are dropped immediately after the decision.

Ambiguity details occupy ordered machine-readable diagnostic context rather
than one unstructured message. Context allocation occurs only on the cold
ambiguous path and is bounded to 17 rows. Converting immutable error messages
to `Box<str>` keeps `ProjectError` at one 64-byte cache line on 64-bit Windows
and 56 bytes on other current 64-bit targets despite the optional cold context.

Evaluation consumes the typed result separately. A selected C# project enters
the existing evaluator; other kinds reach their explicit not-yet-implemented
evaluation boundary only after selection has succeeded. Solution parsing and
F#/Visual Basic evaluation therefore do not leak into workspace selection.

## Verification

Focused tests select all five candidate kinds without parsing their deliberately
invalid contents, cover empty directories, and validate stable 16-row ambiguity
plus its remainder count. CLI tests validate ordered human and JSON candidate
context. The benchmark preflight also checks zero-write empty and ambiguous
selection failures before timing the success case.

The retained like-for-like success benchmark implicitly selects and evaluates
the only project in the same immutable `small-console` directory. Thirty
Windows samples after three warm-ups measured Microsoft at `303.399 ms` median
and `320.158 ms` p95 versus `6.051 ms` and `7.540 ms` for `dv`, a `50.1x`
median improvement.
