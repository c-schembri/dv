# RID Graph Oracle Fixture

This fixture is a minimal timed adapter over the selected SDK's shipped
`NuGet.Packaging` runtime-graph parser and breadth-first expansion API. The
harness builds it outside timed intervals, copies the same SDK-owned portable
graph beside the output, and compares its RID sequence with
`dv sdk compatible-rids` before retaining samples.
