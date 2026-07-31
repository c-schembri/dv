# Validate Cache-Line And Parallel Crossover Assumptions

## Missing Data

No hot execution records or worker queues exist yet. Aligning speculative
structures now would waste memory and may reduce cache capacity.

## Resolve By

When the first graph/hash/parse batches exist:

1. Record hot record sizes, alignment, field access, and working-set bytes.
2. Query or document cache-line sizes for benchmark CPUs.
3. Measure sequential versus bounded parallel batches across item counts.
4. Measure allocation count, false sharing, contention, and merge cost.
5. Select the sequential/parallel crossover and isolate per-worker mutable
   state using one platform layout constant.

## Close When

Compile-time layout assertions and benchmark evidence protect the selected
records, alignment, worker count, and crossover threshold.
