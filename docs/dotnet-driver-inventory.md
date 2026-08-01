# Dotnet Driver Inventory

This executable slice of `DNCLI-001` implements .NET 10 `--list-sdks` and
`--list-runtimes`, including architecture-selected roots, without starting a
managed process. The native `sdk runtimes` alias uses the same runtime
transform; the existing selection-aware `sdk list` remains distinct.

## Contract

Without a selector, inputs are the active host roots discovered from `PATH`,
`DOTNET_ROOT_<ARCH>`, `DOTNET_ROOT`, and platform defaults. A case-insensitive
`--arch <arch>` is parsed into one of the ten architecture names exposed by the
.NET 10 host. The current architecture reuses the active host root. A different
architecture first consults `HKLM\SOFTWARE\dotnet\Setup\InstalledVersions\<arch>`
using the 32-bit registry view on Windows or
`/etc/dotnet/install_location_<arch>` on Unix. If no registration exists, only
Microsoft-supported default pairs are considered: Windows x64/x86 and
Arm64/x64 layouts, plus macOS Arm64-to-x64. A missing alternate installation
is a successful empty inventory.

SDK enumeration accepts only semantic version directories containing
`dotnet.dll`; runtime enumeration accepts semantic version directories below
`shared/<family>`. Neither query reads `global.json`, evaluates a project,
starts a process, or accesses the network.

Human compatibility output is byte-for-byte equal after CRLF normalization:

```text
<version> [<root>/sdk]
<family> <version> [<root>/shared/<family>]
```

JSON reports one ordered `sdk_inventory` or `runtime_inventory` payload under
event schema 21. Unsupported operands and malformed, combined, missing-value,
or repeated architecture selectors fail before host discovery. This avoids
copying Microsoft spellings such as `--arch=x86` that are silently ignored and
therefore do not select x86.

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

Tests cover all ten architecture names, case-insensitive typed parsing,
incomplete SDK directories, deterministic runtime ordering, current-host root
selection, malformed-argument rejection before unrelated SDK input, exact
Microsoft row shapes, and the schema-21 runtime event batch. Benchmark
preflight compares current and x86 SDK/runtime output with Microsoft and
verifies the fixture is unchanged.

The reference machine exposes two complete x64 SDKs, one incomplete SDK
directory that both tools omit, 15 x64 shared runtimes, and 12 x86 shared
runtimes. The x86 timed case retained 200 samples after 20 warm-ups: Microsoft
measured `8.539 ms` median and `10.646 ms` p95, while `dv` measured `5.760 ms`
and `7.334 ms`, making `dv` `1.48x` faster at the median. Hostfxr/hostpolicy
inventory, RID/provenance reporting, and the remainder of `--info` stay open
under `DNCLI-001` and `SDK-007`; their unresolved evidence contract is recorded
in `issues/0012-dotnet-info-host-inventory.md`.
