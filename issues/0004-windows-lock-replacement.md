# Atomic Lock Replacement On Windows

## Question

How should `dv` replace an existing `dv.lock.json` atomically on Windows while
the workspace forbids unsafe code and avoids a platform dependency for one
filesystem operation?

## Current Boundary

Publishing a new package cache entry is atomic and create-once. Creating the
first lock file is a same-directory rename. Replacing an existing lock on
Windows currently removes the old file before renaming the flushed temporary
file, leaving a short interval with no lock file.

The next reader therefore sees either the old or new complete lock in the
common unchanged case, but a concurrent reader can observe no lock during a
changed replacement. It cannot observe a partially written lock.

## Evidence Needed

- Whether supported Windows filesystems and Rust versions gain an atomic
  replace API with the required durability semantics.
- Measured dependency, binary-size, startup, and audit cost of a safe wrapper
  around `ReplaceFileW`.
- Actual concurrent writer requirements for solution-wide package resolution.

## Decision Trigger

Resolve this before concurrent commands are allowed to update the same project
lock. Candidate designs are a safe platform API dependency or immutable
versioned lock files plus an atomically replaceable pointer.
