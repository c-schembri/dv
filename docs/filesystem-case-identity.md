# Active Filesystem Case Identity

`WS-008` makes filesystem equality an observed property of each lookup and
existing path. It does not infer case behavior from the Rust target, host OS,
or target framework.

## Data Contract

Ancestor discovery receives one start path, a five-bit family request, and up
to 65,535 parent directories. NuGet configuration has exactly three recognized
spellings in fixed priority: `nuget.config`, `NuGet.config`, and `NuGet.Config`.
The output remains the existing contiguous `AncestorInput` batch, with one
spelling byte and no new retained case-policy state.

Enumerated NuGet fragment names first use their two exact supported extension
spellings. A non-standard case variant is accepted only when the corresponding
canonical spelling resolves to the same entry. SDK root candidates use the same
rule on their rare case-only collision branch before deduplication.

Project closure receives each evaluated project's literal reference batch.
Only existing references enter the physical identity transform:

```text
absolute lexical path
  -> canonical physical path
  -> binary search physical path/project-index rows
  -> distinct project | no-link alias | link alias
```

Distinct identities are evaluated and appended in deterministic root-first
order. A no-link alias is inserted into the lexical index and produces no
second project. A link alias fails as `DV0207`. Multi-root restore applies a
physical merge only when the request contains more than one root. These are
batch transforms; the one start directory used by ancestor discovery is the
documented true singleton.

## Access And Cost

Ancestor probes are linear over directories and the fixed three-name table.
The first successful name triggers one directory enumeration to retain actual
recognized spelling. Case-insensitive directories normally hit on the first
probe; absent configs pay three metadata misses. No dynamic allocation or
branch is added to non-NuGet singleton discovery.

Project-reference access remains linear over each reference batch with binary
search in contiguous lexical and physical offset indices. `WS-010` stores path
bytes in one command-local arena; lexical spans are 8 bytes and physical spans
plus project index are 12 bytes, or eight/five rows per assumed 64-byte cache
line. Only a physical collision scans divergent path components for link
metadata. Zero-reference workflows allocate no identity table. Multi-root
canonicalization is paid only for explicit root batches.
Exact SDK roots and exact NuGet fragment spellings retain their prior
allocation-free comparison; only a case-only ambiguity pays canonicalization.

`ASSUMPTION: case aliases and link aliases are rare relative to ordinary
distinct project references - affects the linear divergent-component scan,
not correctness.`

No worker state, lock, alignment padding, or asynchronous work is introduced.
These batches are too small and filesystem-dependent for parallel scheduling
to recover its overhead.

## Boundaries

- Case-distinct files on a sensitive directory remain distinct.
- Differently spelled paths to one regular file on an insensitive directory
  collapse deterministically.
- A symlink or junction on either divergent suffix remains an explicit cycle,
  not a case alias.
- Missing paths fail before physical identity insertion.
- Metadata or canonicalization races fail with typed path context.
- SDK roots on sensitive storage remain separate even when their text differs
  only by case; aliases on insensitive storage collapse.
- File identity beyond canonical path, including hard-link identity, belongs to
  `WS-011`.
- Default output-tree name policy remains in `WS-009`; protocol-defined project
  extensions and MSBuild/NuGet identifiers remain case-insensitive by contract.

## Verification

Cross-platform tests create case variants in the active temporary filesystem.
They expect one project on insensitive storage and two on sensitive storage,
while the existing real-link controls must still fail. A Windows `dv.exe`
probe against WSL ext4 retained both case-distinct projects, proving behavior
comes from the active storage rather than the Windows compile target.

The cold restore benchmark resets outputs and adapts the fixture outside timing:
one physical library on insensitive storage, or two case-distinct libraries on
sensitive storage. Microsoft assets and `dv` events must retain that graph;
cleared package sources and `dv` telemetry prove the input is network-free.
Thirty Windows samples measured Microsoft at `498.186 ms` median and
`503.552 ms` p95 versus `5.541 ms` and `6.146 ms` for `dv`, an `89.9x` median
improvement. No sample was removed. The affected ancestor-input benchmark also
measured `dv` at `4.155 ms` median and `4.810 ms` p95, 14.2% below its
`WS-005` median; the additional spelling preservation did not regress that
workflow.

```powershell
cargo bench-all --case filesystem_case_identity --samples 30 --warmups 5 --output target/filesystem-case-ws008-final.json
cargo bench-all --case workspace_inputs --samples 30 --warmups 5 --output target/workspace-inputs-ws008-final.json
```
