# Obtain Representative Large And Test-Heavy Fixture Data

## Missing Data

The workspace contains no sanitized large solution, test-heavy repository, or
private multi-source package configuration. Counts and distributions must not be
invented.

## Required Sample

For each repository shape, collect:

- projects, target frameworks, source files, generated files, references, and
  package counts;
- graph depth, fan-out, shared-dependency frequency, and cycle failures;
- file-size and item-count histograms;
- clean, warm, incremental, and no-op filesystem/process/network traces;
- test case, adapter, result, and output-size distributions;
- package source latency, concurrency, authentication, and cache-hit data with
  secrets removed.

## Close When

Sanitized fixture data or a deterministic generator based on observed
distributions is checked in, with provenance and representativeness limits.
