# Recursive Workspace Discovery Boundaries

## Question

How should repository-wide discovery traverse nested directories, symlinks,
junctions, case-colliding names, and configured output/cache trees without
escaping the selected workspace or duplicating file identities?

## Current Boundary

`WS-001` enumerates only immediate files in an explicit directory. It does not
follow directory entries, so cycles and workspace escape cannot occur in that
transform. This matches the current single-project implicit-selection stage but
does not satisfy repository-wide discovery, watch, or solution-scale inputs.

## Required Evidence

1. Capture implicit selection and repository behavior on Windows, Linux, and
   macOS for nested candidates, directory symlinks/junctions, inaccessible
   trees, case collisions, and non-Unicode components.
2. Define platform file-identity records separately from preserved path
   spelling and normalized protocol paths.
3. Define ordered exclusion inputs for `bin`, `obj`, VCS metadata, tool caches,
   and configured output/artifacts paths.
4. Measure sequential and bounded-parallel enumeration on representative large
   repositories before adding workers or persistent path caches.
5. Add positive cycle/escape controls and fail closed when identity cannot be
   established.

Close this issue only when `WS-004` through `WS-012` have cross-platform
fixtures and retained cold/warm evidence.
