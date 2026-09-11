# Search And Storage Architecture

This document is the production contract for GFM search/storage work. It exists to keep latency-sensitive backend changes aligned while the native UI is built separately.

## Goals

GFM search must feel instantaneous on macOS-local machines while remaining correct under large indexes, multiple mounted volumes, iCloud/FileProvider state, changing metadata, and interrupted background maintenance.

The backend target is not "Finder-compatible enough." The target is a native file manager search engine that can answer common name/path/metadata queries from hot memory immediately, refine with deeper content/archive results without blocking interaction, and keep every slow path cancellable, bounded, and observable.

## Ownership

`crates/search` owns query parsing, candidate construction, ranking, bounded result merging, in-memory indexes, sidecar posting import, and sharded fanout across volumes.

`crates/store` owns durable mmap-readable archive formats: records, columns, metadata postings, prefix postings, substring postings, fuzzy dictionaries/postings, content archives, content archive sets, content manifests, and checksum-backed readers.

`crates/index` owns live indexing, mmap archive/session orchestration, sidecar query sessions, content query sessions, recovery planning, index diagnostics, and the bridge between durable storage and interactive search.

`crates/app` owns operator and test-harness entrypoints for exercising the production paths. These are not a product CLI surface.

## Hot Query Path

The first search response should avoid disk, full-record scans, and unbounded allocation whenever a bounded candidate source exists.

The preferred flow is:

1. Parse and normalize the query with cancellation checks.
2. Use exact, prefix, token, metadata, tag, kind, and scoped sidecar candidates to bound the result set.
3. Use positive anchors for boolean/filter expressions so `NOT` and expensive filters do not force a full universe unless correctness requires it.
4. Merge results through `BoundedHitMerge`, retaining at most a small multiple of the requested limit.
5. Sort by cached `(score, normalized name, path, id)` keys so repeated trims do not reallocate sort keys for surviving hits.
6. Return hot batches before deep content/fuzzy/archive work when streaming is requested.

Full-record universe scans are allowed only for queries whose semantics require them, such as unsupported filter-only queries or unanchored boolean expressions. Those paths must remain cancellable and visible in tests.

## Sharding

`ShardedSearchIndex` partitions search state by `VolumeId`. Single-volume searches dispatch directly into the owning shard to avoid thread fanout overhead. Multi-volume searches fan out deterministically, then merge through the same bounded global top-k path used by single-shard searches.

Sidecar postings must be partitioned by the owning volume before import. Partitioning should be one-pass over the posting IDs, should not rescan every posting for every shard, and should avoid intermediate sets unless ordering or dedupe requires them.

## Sidecar Sessions

`SidecarIndexQuerySession` keeps mmap record, column, metadata, prefix, substring, fuzzy, dictionary, and content sidecars open across search-as-you-type changes. A session must prefer query-scoped imports over startup hydration.

The session contract is:

1. Parse query and derive the bounded sidecar terms needed for this query.
2. Resolve missing metadata/prefix/substring/fuzzy/content postings through sorted batch mmap directory scans.
3. Import only bounded selected postings into the live query index.
4. Hydrate only matching records and columns through sorted batch file-ID lookup.
5. Reuse record/column/content caches across repeated queries.
6. Recover poisoned internal cache locks by replacing the cache instead of failing foreground search.
7. Report cache, cutoff, miss, truncation, and hydration telemetry with the query result.

Repeated UI keystrokes must cancel stale query work before importing more sidecar data or hydrating more mmap rows.

## Content Sessions

`ContentIndexQuerySession` keeps mmap record archives and content archive sets/manifests open across repeated content queries.

The content-session contract is:

1. Resolve selected content terms through sorted batch mmap directory scans.
2. Decode only bounded compressed ID/position heads per term.
3. Cache positive content postings and complete negative term misses.
4. Skip record hydration entirely for complete negative content-term lookups.
5. Hydrate only matching file IDs through sorted batch record lookup.
6. Refresh record-cache recency on hits so repeated candidates remain hot.
7. Report posting and hydration cache hits/misses.
8. Recover poisoned caches instead of failing foreground content search.

Content phrase and proximity search must anchor on the rarest posting lists. It must not materialize full per-term ID sets for ordinary phrase/proximity checks.

## Storage Formats

Mmap archives are the query-time authority. Sequential or legacy formats are migration inputs, not hot-path readers.

Every mmap format must provide:

1. A magic/version header.
2. A compact directory for sorted lookup or sorted batch lookup.
3. Bounds-checked decode of compressed payloads.
4. A checksum footer verified before use.
5. Cancellable checked open/lookup variants for foreground paths.
6. Deterministic corruption and unsupported-version errors.
7. Tests for checksum mismatch, missing/unreadable inputs, bounded reads, and cancellation before open and during decode.

Record archives are durable primary state. Columns, metadata, prefix, substring, fuzzy, dictionary, and content sidecars are derived or independently recoverable search accelerators. Recovery should quarantine bad bytes before rebuilding or migrating.

## Cache Rules

Caches in hot search paths must have explicit ownership and bounded capacity. They must define:

1. Key shape.
2. Positive value semantics.
3. Negative value semantics, when used.
4. Eviction order.
5. Invalidation source.
6. Poison recovery behavior, when protected by a mutex.
7. Telemetry exposed to callers.

Avoid O(n) recency refreshes in query-time caches. Prefer generation-stamped order queues or another bounded constant-time refresh strategy with stale-entry compaction.

Do not introduce process-global mutable search caches unless they have a clear invalidation source and a memory budget. Per-session caches are preferred because they line up with UI search lifetimes.

## Ranking

Ranking combines exact name, prefix, substring, fuzzy, path component, metadata, kind, tag, content, recency, term frequency, and user-pinned signals.

Ranking changes must preserve deterministic ordering:

1. Higher score first.
2. Lowercased name.
3. Full path string.
4. Stable `FileId`.

Any new ranking signal needs tests that prove both the intended ordering and the tie-break behavior.

## Cancellation

Foreground search work must check cancellation before and during:

1. Query parsing.
2. Candidate construction.
3. Sidecar lookup/import.
4. Mmap record hydration.
5. Content posting decode.
6. Scoring loops.
7. Stream batch transitions.

Cancellation is a correctness feature, not only a performance feature. Stale keystroke work must stop before it competes with the next visible query.

## Verification

Search/storage changes should usually run:

```sh
cargo fmt --all -- --check
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p gfm-search -- --nocapture
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test -p gfm-index content -- --nocapture
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo clippy -p gfm-index -p gfm-search --all-targets -- -D warnings
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo clippy -p gfm --all-targets -- -D warnings
fozzy doctor --deep --scenario tests/scenarios/gfm-cli-host.fozzy.json --runs 5 --seed 424242 --json
fozzy test --det --strict-verify tests/scenarios/gfm-cli-host.fozzy.json --json
fozzy run tests/scenarios/gfm-cli-host.fozzy.json --det --record /tmp/gfm-cli-host.trace.fozzy --proc-backend host --fs-backend host --http-backend host --json
fozzy trace verify /tmp/gfm-cli-host.trace.fozzy --strict --json
fozzy replay /tmp/gfm-cli-host.trace.fozzy --json
fozzy ci /tmp/gfm-cli-host.trace.fozzy --json
fz doctor project . --strict
```

Use narrower test filters first while developing, but do not claim production readiness from a narrow filter when a broader gate is feasible.

## Performance Review Checklist

Before merging search/storage work, check:

1. Does this add any per-keystroke full-record scan where a bounded candidate source exists?
2. Does this allocate a full temporary set or vector in a loop that can stream into the target structure?
3. Does this decode full postings where bounded heads are enough?
4. Does this reopen mmap archives per query instead of reusing sessions?
5. Does this lose cache telemetry or make invalidation ambiguous?
6. Does this preserve deterministic ordering across single-shard and multi-shard results?
7. Does this preserve cancellation before filesystem, mmap, and decode work?
8. Does this recover poisoned hot-path locks where the surrounding subsystem promises recovery?
9. Does this keep the product surface native-app-first rather than adding user-facing CLI assumptions?
