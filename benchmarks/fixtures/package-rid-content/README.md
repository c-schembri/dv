# Package RID and content metadata fixture

The benchmark generates one deterministic local package outside the timed
interval. Portable, exact Windows/Linux, and Windows fallback projects let
Microsoft `project.assets.json` and `dv` prove the same runtime, resource,
native, and `contentFiles` choices.

The archive contains portable runtime/resource assets, exact and fallback RID
groups, a root native decoy, and content in `any`, C#, and VB language folders.
Ordered nuspec rules exercise default metadata, later overrides, `.pp` paths,
copy-to-output, flattening, and build-action selection. Every retained sample
must pass complete path and metadata parity first.

Cold samples remove isolated package caches, outputs, and locks. Warm samples
retain published packages and matching locks. Both states launch fresh tool
processes, and neither state performs network work.
