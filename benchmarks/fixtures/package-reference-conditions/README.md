# Conditional Reference Fixture

This `net10.0`/`win-x64` project exercises the bounded reference-condition
evaluator introduced by `RES-005`. The Release evaluation selects three package
references, one project reference, and one framework reference through TFM,
RID, and configuration comparisons.

False branches deliberately omit versions or use unsupported identities. This
proves excluded items leave the evaluation batch before reference metadata is
validated. The benchmark preflight compares the selected property and reference
batches with Microsoft's MSBuild evaluation before collecting timings.
