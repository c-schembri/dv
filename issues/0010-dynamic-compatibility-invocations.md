# Dynamic compatibility invocations

`dv compat check` intentionally classifies only literal command positions.
Multiline YAML scalars, PowerShell expressions, shell functions, variable or
alias-based executable selection, and composed quoting can change the command
that would execute. The static scanner reports such observable shapes as
`uncheckable` rather than interpreting or executing them.

Before expanding this surface:

- define bounded parsers for each owned automation language;
- retain source ranges and deterministic ordering across multiline constructs;
- prove that expansion cannot execute user code or access the network;
- add cross-platform fixtures for quoting, variables, aliases, and continuations;
- benchmark representative large repositories before adding parallel work.

This issue remains open until those language-specific contracts exist. A
general shell, YAML, or PowerShell interpreter is not part of `DROP-021`.
