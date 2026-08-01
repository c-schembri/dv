# Cancellation Exit And Process-Group Parity

## Question

Which exit codes, escalation behavior, and child process-group boundaries must
each `dotnet`, MSBuild, NuGet, and VSTest compatibility profile reproduce on
Windows, Linux, and macOS?

## Current Boundary

`CLI-014` owns early signal installation, one absolute two-second child grace,
cooperative NuGet provider cancellation, forced kill/reap, stable `DV0005`, and
the JSON `cancelled` outcome. Native cancellation currently uses the existing
operation-failure process code. Application, compiler, and test child creation
remain TBI, so `dv` does not yet claim process-tree or reference exit parity.

## Evidence Needed

- Exact first- and repeated-signal exit codes for every selected reference tool
  and compatibility profile.
- Windows console-group behavior for directly launched managed children.
- Unix process-group behavior when children create descendants.
- Whether compiler, application, test host, and data collector children require
  different cooperative messages or grace periods.
- Timed evidence that process-group ownership and escalation do not regress
  startup or normal child throughput.

## Decision Trigger

Resolve this with `CLI-015`, `RUN-006`, and `RUN-009` before claiming child-exit
or cancellation parity. Keep the existing typed outcome and deadline; change
only the profile exit mapping and platform child owner proven by the oracle.
