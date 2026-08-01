# Dotnet Driver Inventory

This is the first executable slice of `DNCLI-001`. It implements the current-
architecture .NET 10 `--list-sdks` and `--list-runtimes` queries without
starting a managed process. The native `sdk runtimes` alias uses the same
runtime transform; the existing selection-aware `sdk list` remains distinct.

## Contract

Inputs are the active host roots discovered from `PATH`, `DOTNET_ROOT_<ARCH>`,
`DOTNET_ROOT`, and platform defaults. SDK enumeration accepts only semantic
version directories containing `dotnet.dll`; runtime enumeration accepts
semantic version directories below `shared/<family>`. Neither query reads
`global.json`, evaluates a project, starts a process, or accesses the network.

Human compatibility output is byte-for-byte equal after CRLF normalization:

```text
<version> [<root>/sdk]
<family> <version> [<root>/shared/<family>]
```

JSON reports one ordered `sdk_inventory` or `runtime_inventory` payload under
event schema 21. Unsupported operands fail before host discovery. Architecture
selectors are deliberately rejected until architecture-specific roots can be
matched rather than guessed.

## Data Layout

The runtime transform enumerates cold filesystem names into temporary work
rows, sorts once by root, family, and semantic version, then packs text into one
contiguous arena. Each immutable `RuntimeInstallation` is 16 bytes at alignment
4, so four records occupy one 64-byte cache line. It stores two `u32` text
offsets, two `u16` lengths, and one `u16` root index. The 64-bit
`RuntimeInventory` owner is 72 bytes.

Dynamic allocation is confined to externally sized filesystem paths and names.
The final record and text buffers allocate once from measured capacities. SDK
and runtime batches are each bounded to 4,096 installations; roots, component
lengths, and text offsets are checked before narrowing. Full paths are created
only at the output boundary.

The query is bounded and read-only, so it does not install the command-lifetime
cancellation handler. Host-root discovery preserves `PATH` precedence. Windows
uses the matching directory directly, while Unix canonicalizes the common
launcher-symlink case only after finding the executable.

`ASSUMPTION: the Windows benchmark machine has 64-byte cache lines - affects
the stated records-per-line count, not the 16-byte record contract.`

## Verification

Tests cover incomplete SDK directories, deterministic runtime ordering, exact
Microsoft row shapes, malformed-argument rejection before unrelated SDK input,
and the schema-21 runtime event batch. Benchmark preflight compares both complete
SDK and runtime outputs with Microsoft and verifies the fixture is unchanged;
the timed runtime case exercises the larger inventory.

The reference machine exposes two complete SDKs, one incomplete SDK directory
that both tools omit, and 15 shared runtimes across three framework families.
Architecture selection, hostfxr/hostpolicy inventory, RID/provenance reporting,
and the remainder of `--info` stay open under `DNCLI-001`, `SDK-006`, and
`SDK-007`; their unresolved evidence contract is recorded in
`issues/0012-dotnet-info-host-inventory.md`.
