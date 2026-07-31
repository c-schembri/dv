# Establish Numeric Performance Thresholds

## Missing Data

No representative `dv` implementation or repeated reference distribution
exists yet, so numeric pass/fail thresholds would be fabricated.

`ASSUMPTION: developers perceive repeated no-op commands as the highest-value
early latency target - affects benchmark prioritization.`

## Resolve By

1. Record at least 30 release samples for each initial case on one controlled
   Windows machine and one CI Linux machine.
2. Capture process count, CPU time, peak resident memory, allocation count, and
   filesystem operations in addition to wall time.
3. Sample at least one real small repository and one real multi-project
   repository.
4. Set explicit startup, no-op, clean-build throughput, and memory budgets from
   those distributions and the product goal.

## Close When

`docs/performance-method.md` contains numeric budgets, machine conditions, and a
regression policy that identifies noise separately from failure.
