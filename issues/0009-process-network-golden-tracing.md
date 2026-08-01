# Observe Process And Network Golden Traces

## Question

How should the compatibility harness observe child-process and network activity
for short-lived Microsoft and `dv` commands on Windows, Linux, and macOS
without changing the command or relying on sampling?

## Current Boundary

`compatibility/traces/phase1-ci.json` records the real CI SDK-selection and
valid offline-restore substitutions. The harness verifies argv, controlled
environment overrides, stdin, stdout, stderr, exit status, and sorted
filesystem deltas. Restore uses an empty local source plus unreachable proxy
overrides, but that setup is not proof that neither tool opened a socket.
Process-tree and network dimensions are explicitly `TBI`; starting the root
command is not proof that it launched no child or opened no socket.

## Resolve By

1. Select event-driven observers for Windows, Linux, and macOS that report
   process creation and socket activity without polling races.
2. Normalize platform events into a versioned bounded record with root/child
   identity, executable role, endpoint role, and deterministic ordering.
3. Prove observer overhead and loss behavior with commands that deliberately
   launch children and contact an isolated loopback endpoint.
4. Fail closed when tracing is unavailable, loses events, or exceeds its
   bounded buffers; never convert missing evidence into a zero count.

## Close When

Every supported CI platform verifies process and network trace batches for the
golden substitution corpus, including positive controls, and both dimensions
can move from `TBI` to measured expectations.
