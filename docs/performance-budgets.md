# Performance Budgets

GFM is a native macOS file manager, so latency regressions are product
regressions. A technically correct operation that feels delayed is still a
failure. Performance budgets are therefore part of the architecture, not an
optional benchmark report.

This document records the current budget contract implemented by
`crates/telemetry`.

## Ownership

`crates/telemetry` owns:

- bounded latency histograms;
- p50, p95, and p99 summaries;
- frame timing and UI-thread stall detection;
- resource counters for IO, CPU, memory, allocations, queues, compaction, cache
  behavior, slow paths, index lag, and operation failures;
- hard budget evaluation;
- privacy-reviewed local diagnostics export.

`crates/diagnostics` owns operator-facing commands that surface telemetry and
regression gate output. `crates/testkit` owns repeatable fixtures and
macrobenchmarks. Product UI code should record observations but must not invent
local budget semantics.

## Latency Metrics

The telemetry crate currently tracks these latency metrics:

| Metric | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| navigation | 4 ms | 8 ms | 16 ms |
| selection | 2 ms | 4 ms | 8 ms |
| rename | 8 ms | 16 ms | 33 ms |
| search keystroke | 8 ms | 16 ms | 33 ms |
| result streaming | 12 ms | 25 ms | 50 ms |
| thumbnail display | 16 ms | 50 ms | 100 ms |
| preview open | 25 ms | 75 ms | 150 ms |
| copy start | 25 ms | 75 ms | 150 ms |
| cancel | 8 ms | 16 ms | 33 ms |
| window render | 8 ms | 16 ms | 25 ms |

These numbers are intentionally aggressive. They are not promises that every
cold path has finished. They are the interactive budgets for the part of the
system the user can feel.

## Scenario Budgets

Scenario metrics currently use max-duration budgets:

| Scenario | Max |
| --- | ---: |
| cold start | 800 ms |
| warm start | 180 ms |
| first result | 25 ms |
| full result | 250 ms |
| directory open | 50 ms |
| visible thumbnail completion | 500 ms |

Scenario observations belong in deterministic benchmark and regression gates,
not ad hoc manual timing notes.

## Histogram Contract

Latency histograms use fixed buckets from 100 microseconds through 32 seconds.
Every metric reports:

- count;
- minimum;
- maximum;
- mean;
- p50;
- p95;
- p99;
- overflow count.

The budget evaluator ignores metrics with no observations. A missing
observation is not a pass; it means the scenario did not exercise that metric.
Production-readiness claims need both passing budgets and coverage of the
relevant metrics.

## Frame Stalls

The default UI-thread stall threshold is 50 ms. Any visible workflow that
produces a frame stall above that threshold needs investigation, even if
aggregate p50/p95 latency still looks acceptable.

Frame timing should be recorded during:

- directory navigation;
- search typing;
- large-directory scroll;
- drag selection;
- rename;
- thumbnail publication;
- preview open;
- progress updates for long operations;
- window resize;
- appearance/theme changes.

No filesystem, network, indexing, thumbnail generation, Quick Look work, or
permission prompt preparation may block the render/update path.

## Search Budgets

Search latency is split deliberately:

- `search_keystroke` covers accepting a new visible query and starting the
  latest query path.
- `result_streaming` covers publishing result batches.
- `first_result` covers the scenario-level time to show the first useful result.
- `full_result` covers the scenario-level time to complete the bounded result
  set.

A healthy search path returns name/path/metadata hot results before deep content
or fuzzy work completes. Content and archive search are allowed to refine; they
are not allowed to block the first useful result for ordinary queries.

Search implementations must avoid:

- full-record scans when bounded candidates exist;
- unbounded prefix, substring, fuzzy, or content expansion;
- repeated mmap archive opens per keystroke;
- repeated lowercasing/stringifying inside ranking comparators;
- stale superseded query work continuing behind the active query;
- query text in diagnostics without privacy approval.

## Directory And Thumbnail Budgets

`directory_open` and `navigation` measure different parts of the experience.
Navigation should feel instant even when thumbnails, metadata, and deep
directory details continue loading.

Visible thumbnail completion has a wider 500 ms scenario budget because
thumbnail generation can involve Quick Look, FileProvider/iCloud state, decoding,
and disk cache publication. That does not permit thumbnail work to starve
navigation or search. Thumbnail workers must remain lower priority than
foreground interaction unless the selected item preview depends on them.

## Operation Budgets

`copy_start` measures how quickly a foreground operation is admitted, journaled,
planned enough to show progress, and moved off the UI path. It does not require
the full copy to finish.

`cancel` measures control responsiveness. Cancellation must propagate through
planning, recursive traversal, byte-copy fallback, retry backoff, extraction
subprocess supervision, and job scheduler scopes quickly enough to meet the
interactive budget.

Long-running operations need throughput telemetry and progress snapshots, but
they should not be judged only by wall-clock duration. Volume class, APFS clone
availability, network behavior, sparse files, metadata preservation, and
verification policy all affect throughput.

## Resource Budgets

Performance gates should evaluate more than wall-clock latency:

- peak resident memory;
- index-size density;
- mmap footprint;
- sidecar cache hit/miss rates;
- content cache hit/miss rates;
- truncation counts for prefix, substring, fuzzy, metadata, and content lookups;
- IO bytes and operation counts;
- CPU user/system percentage;
- allocation volume;
- queue depth and starvation;
- compaction work retained for later passes;
- recovery or repair work admitted under pressure.

Latency improvements that explode memory, index density, or IO are not
production wins.

## Diagnostics Privacy

Diagnostics export is local-only and privacy-reviewed. The current exporter
rejects path, query text, or user identifier inclusion unless the privacy policy
explicitly allows it. Aggregate performance data should be useful without
capturing personal file names or queries.

Budget artifacts should prefer:

- metric names;
- histogram summaries;
- counts;
- anonymized scenario labels;
- cache and truncation counters;
- index sizes;
- resource totals;
- pass/fail violations.

They should avoid:

- raw paths;
- raw query strings;
- usernames;
- document contents;
- file preview content.

## Regression Gate Rules

Budget gates fail when:

1. A required p50, p95, or p99 exceeds its metric budget.
2. A scenario observation exceeds its max budget.
3. A supposedly hot path has no observations.
4. A search path silently falls back to full-record scans for ordinary anchored
   queries.
5. A sidecar lookup truncates without telemetry.
6. Index-size density drifts above the profile threshold.
7. Peak memory drifts above the profile threshold.
8. UI frame stalls exceed 50 ms in an interactive workflow.
9. A diagnostic artifact includes private fields without approval.
10. A test claims production readiness without deterministic trace replay.

## Engineering Rules

New latency-sensitive features must answer these before merge:

1. Which metric records the visible user latency?
2. Which scenario gate exercises the end-to-end path?
3. What is the cold path?
4. What is the warm path?
5. What work is cancelled when the visible request is superseded?
6. What memory or cache budget prevents unbounded growth?
7. What telemetry proves the intended fast path was used?
8. What deterministic trace can replay the behavior?

GFM's standard is not "fast in a sample folder." It is fast under real macOS
trees, huge directories, multiple volumes, iCloud/FileProvider state, and
background maintenance pressure.
