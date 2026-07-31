# Events And Diagnostics

Human output and JSON output are views over the same typed event batch. Core
logic never emits prose for another subsystem to scrape.

## Event Batch Contract

Input layout:

- a contiguous slice of `Event`;
- schema version `6`;
- sequence numbers exactly `0..count`;
- monotonic microseconds from one command-local clock.

Output layout:

- JSON Lines, one complete object per event;
- tagged event payload flattened into the object;
- stable snake-case enum values.

Ownership and lifetime:

- producers own events until the reporter call completes;
- reporters borrow the whole batch;
- JSON output owns no references to internal execution data.

Range and failure behavior:

- unsupported schema, sequence gaps, and elapsed-time regression reject the
  whole batch before output;
- empty batches are valid and write nothing;
- writer failures return immediately;
- durations saturate at `u64::MAX` microseconds at the clock conversion edge.

The common path is one validation scan followed by one serialization scan.
Events describe command- or batch-level transitions, not individual hot-loop
items.

## Event Types

- `command_started`
- `work_started`
- `work_finished`
- `cache_decision`
- `sdk_selected`
- `sdk_inventory`
- `runtime_compatibility`
- `runtime_pack_plan_created`
- `framework_reference_plan_created`
- `project_evaluated`
- `compiler_plan_created`
- `package_resolution_created`
- `diagnostic`
- `command_finished`

New variants require a real consumer and a version-compatibility decision.
Schema 6 adds framework-reference metadata to `project_evaluated` and the
`framework_reference_plan_created` event. Schema 5 added
`runtime_pack_plan_created`; schema 4 added
`runtime_compatibility`; schema 3 added selected, plural, and materialized
runtime-dimension fields to `project_evaluated`.

## Diagnostic Contract

Every diagnostic contains:

- `DV` plus four digits as its stable code;
- severity;
- a short message;
- ordered name/value context;
- an ordered causal chain without wrapper duplication;
- an optional next action.

Unavailable-pack diagnostics append stable ordered context fields from the
same typed requirement retained by the planner: `pack_kind`, `pack_identity`,
optional `pack_version`, `target_framework`, optional `runtime_identifier`,
and `acquisition`. Valid acquisition values are `install_sdk`,
`install_sdk_or_restore_package`, `restore_package`, `install_runtime`, and
`choose_runtime_identifier`.
Reporters do not recover these fields by parsing the human message.

Malformed diagnostic identifiers are rejected at construction. Empty diagnostic
messages are programmer errors. External malformed data becomes a normal
diagnostic at the boundary where it is parsed.

Initial codes:

| Code | Meaning |
|---|---|
| `DV0001` | Unknown command |
| `DV0002` | Invalid command-line text |
| `DV0003` | Known but unsupported Phase 0 command |
| `DV0100` | No .NET installation root |
| `DV0101` | SDK discovery filesystem failure |
| `DV0102` | Invalid `global.json` |
| `DV0103` | Invalid SDK version |
| `DV0104` | No compatible installed SDK |
| `DV0105` | SDK path cannot be represented losslessly in JSON |
| `DV0110` | Selected SDK portable RID graph missing |
| `DV0111` | Portable RID graph filesystem failure |
| `DV0112` | Invalid portable RID graph JSON or data |
| `DV0113` | Portable RID graph compact range overflow |
| `DV0120` | Runtime-pack filesystem failure |
| `DV0121` | Invalid SDK/runtime-pack manifest |
| `DV0122` | Runtime-pack planning requires a selected RID |
| `DV0123` | No compatible manifest-declared pack RID |
| `DV0124` | Required runtime or host pack missing |
| `DV0125` | Selected runtime/host asset missing |
| `DV0126` | Runtime-pack NuGet configuration failure |
| `DV0127` | Runtime-pack path is not Unicode |
| `DV0128` | Runtime-pack compact range overflow |
| `DV0130` | Framework-reference filesystem failure |
| `DV0131` | Invalid SDK framework manifest |
| `DV0132` | Unknown framework reference for selected TFM |
| `DV0133` | Invalid runtime or targeting-pack version |
| `DV0134` | Required targeting pack missing |
| `DV0135` | No shared framework satisfies roll-forward |
| `DV0136` | Framework-plan NuGet configuration failure |
| `DV0137` | Framework-plan path is not Unicode |
| `DV0138` | Framework-plan compact range overflow |
| `DV0200` | No project found |
| `DV0201` | Ambiguous implicit project selection |
| `DV0202` | Project filesystem failure |
| `DV0203` | Malformed project XML |
| `DV0204` | Unsupported project behavior |
| `DV0205` | Invalid project property |
| `DV0206` | Project path cannot be represented in the compact UTF-8 table |
| `DV0300` | Compatible target reference pack not found |
| `DV0301` | Invalid framework-pack manifest |
| `DV0302` | Required compiler or pack asset missing |
| `DV0303` | Selected SDK or target is unsupported by captured compiler policy |
| `DV0304` | Compiler-plan filesystem failure |
| `DV0305` | Compiler-plan path is not Unicode |
| `DV0306` | Compiler-plan compact range overflow |
| `DV0307` | Package resolution failed during compiler planning |
| `DV0400` | Unsupported package configuration or source |
| `DV0401` | Invalid or conflicting package graph |
| `DV0402` | Package has no compatible supported assets |
| `DV0403` | Offline package cache miss |
| `DV0404` | Package source network failure |
| `DV0405` | Package identity, size, or integrity failure |
| `DV0406` | Unsafe or malformed package archive |
| `DV0407` | Package cache or lock filesystem failure |
| `DV0408` | Package path is not Unicode |
| `DV0409` | Package compact range overflow |

Codes are never reused for a different meaning.

## Layout Note

The owned wire representation is cold output-edge data. Execution subsystems
must not store one `Event` or `Diagnostic` per project node, source file, or
package in hot state. They should keep compact indexed records and materialize
ordered event batches only when reporting.
