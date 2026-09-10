# Search Ranking Architecture

GFM search ranking exists to make machine-wide search feel immediate and
predictable without handing relevance to an opaque platform index. The ranking
engine must produce Finder-familiar results for ordinary file-manager intent,
while remaining deterministic, bounded, cancellable, and explainable.

This document is the ranking contract for `crates/search`. It complements
`search-storage.md`, which covers index formats, sidecars, query sessions, and
candidate hydration.

## Ownership

`crates/search` owns:

- query parsing and normalization;
- candidate construction from hot in-memory indexes and mmap sidecars;
- exact, prefix, substring, fuzzy, metadata, kind, tag, content, phrase, and
  proximity scoring;
- deterministic bounded top-k merging;
- hot/deep streaming batches;
- search supersession and cancellation.

`crates/index` owns persisted search session setup and mmap-backed archive
lookup. `crates/store` owns the durable archive formats. `crates/app` exposes
operator and deterministic test harness routes only; it is not the product
search surface.

## Ranking Inputs

Search scoring combines these signal families:

1. Exact name matches.
2. Name prefix matches.
3. Name token matches.
4. Name substring matches.
5. Extension matches.
6. Finder-visible tag matches.
7. Path component matches.
8. Content term matches.
9. Fuzzy name matches.
10. Exact content phrase matches.
11. Content proximity matches.
12. User-pinned result boosts.
13. Kind matches.
14. Bounded name, path, and content term-frequency boosts.
15. Recency boosts from file modification time.

The current score constants live in `crates/search/src/ranking.rs`. They are
not arbitrary decoration; they encode product intent. Name/path intent must beat
deep content by default because common file-manager search is usually "find the
thing I can name," not "search every byte before showing the obvious file."

Current base weights:

| Signal | Score |
| --- | ---: |
| exact name | 1000 |
| prefix name | 700 |
| user pinned | 650 |
| name token | 500 |
| exact phrase | 450 |
| proximity | 375 |
| extension | 350 |
| tag | 325 |
| substring name | 300 |
| path component | 250 |
| content | 150 |
| fuzzy name | 100 |
| kind match | 90 |

Frequency boosts are capped at eight observed occurrences:

| Frequency Signal | Points Per Occurrence |
| --- | ---: |
| name frequency | 12 |
| content frequency | 8 |
| path frequency | 6 |

Recency is bounded from 100 down to 0 by whole days since modification, capped
at 100 days. Recency can separate otherwise similar candidates, but it should
not overpower exact identity signals.

## Deterministic Ordering

Ranking must remain stable across:

- repeated runs;
- single-volume and multi-volume searches;
- hot in-memory and sidecar-backed searches;
- cache hits and cache misses;
- streaming hot/deep batches;
- different thread interleavings.

Final ordering is:

1. Higher score first.
2. Lowercased display name.
3. Full path string.
4. Stable `FileId`.

Bounded merge code should cache these sort keys while retaining top-k hits.
Comparators must not repeatedly lowercase or stringify paths inside hot loops.

## Match Reason

`RankAccumulator` retains both total score and the strongest match reason. The
strongest reason is the single highest-weight signal that explains why a result
is present.

This keeps UI and diagnostics honest:

- an exact-name hit should not be described as a weak content hit just because
  content also matched;
- a fuzzy hit should remain explainable as fuzzy when fuzzy is the strongest
  source;
- additive boosts should improve ordering without overwriting the visible reason
  unless they are themselves a primary reason.

## Hot Pass Versus Deep Pass

The hot pass serves immediate search-as-you-type feedback. It should prefer:

- exact name;
- prefix name;
- name tokens;
- extension;
- kind;
- tags and Finder-visible metadata already in hot state;
- path terms;
- intent boosts that can be derived without full-record scans.

The deep pass may add:

- content terms;
- phrase and proximity matches;
- fuzzy expansion;
- sidecar postings not already imported;
- hydrated metadata or column data needed for full semantics.

Streaming search emits hot results first, then deep refinements. A deep hit that
duplicates a hot hit should appear only when the score improves. Duplicate
unchanged hits waste UI work and make search feel unstable.

## Candidate Discipline

Ranking must score bounded candidates whenever a bounded candidate source
exists. It must not casually expand to the full record universe.

Allowed anchors include:

- exact name terms;
- name prefixes;
- name substring grams above the short-query cutoff;
- fuzzy delete keys under the fuzzy budget;
- extension postings;
- tag/comment metadata postings;
- kind postings;
- positive content postings;
- positive boolean expression anchors.

Unanchored negative queries and filter-only expressions may require a full
universe for correctness. Those cases must remain visible in tests and
telemetry, and they must stay cancellable.

## Fuzzy Ranking

Fuzzy search is a fallback relevance signal, not the primary path for ordinary
typing. Delete-key expansion and verified fuzzy candidates must stay budgeted so
typo tolerance cannot become a machine-wide latency cliff.

Fuzzy candidates should:

- enter only through bounded delete-key and term expansion;
- dedupe against exact/prefix candidates before scoring when practical;
- score below substring and content relevance by default;
- retain deterministic tie-break ordering;
- expose truncation telemetry when budgets cut off expansion.

Short queries must not expand into every fuzzy key. Exact and prefix paths are
the bounded route for very short user input.

## Content Ranking

Content search must not compromise filename latency. Normal content-term
queries should score directly from content postings without constructing large
temporary ID sets.

Phrase and proximity queries must anchor on the rarest posting lists. The query
engine should verify positional matches from the smallest useful candidate set,
then hydrate records only for matching IDs.

Content ranking must report useful matches without eagerly decoding or
highlighting every matching file. Snippets are bounded and produced only after
candidate selection.

## Metadata And Finder Intent

Metadata relevance includes tags, comments, kinds, extensions, and product
intent boosts such as Applications, Recents, Downloads, Desktop, screenshots,
and project folders.

Finder-style intent scoring should be preclassified once per query. Ordinary
name/path/content searches must not pay all-record Finder-intent scoring costs
on every keystroke.

User-pinned boosts are high enough to make known preferred files surface, but
they still participate in deterministic ordering and should not hide exact-name
correctness regressions.

## Cancellation

Ranking loops must check cancellation before and during:

- query parsing;
- candidate expansion;
- sidecar lookup;
- content posting decode;
- score seeding;
- additive score updates;
- hydration before final hit construction;
- hot/deep stream transitions.

Cancelled stale keystrokes should stop quickly enough that they do not compete
with the active visible query for CPU, mmap page faults, or allocation budget.

## Telemetry

Ranking-related telemetry should expose:

- candidate counts by source;
- truncation by source and budget;
- cache hits/misses for sidecar, content, and hydrated record paths;
- whether a full-record universe was required;
- hot batch result count and latency;
- deep batch result count and latency;
- final top-k size before and after dedupe;
- scoring loop cancellation points;
- repeated-cache hit behavior across search-as-you-type sessions.

Telemetry must not include raw query text unless a privacy-reviewed diagnostic
mode explicitly permits it, and current diagnostics reject query text by
default.

## Change Rules

Ranking changes are production-sensitive. Before merging, prove:

1. The intended ordering changed for the right reason.
2. The stable tie-break order still holds.
3. Single-shard and sharded searches agree where the same records participate.
4. Hot/deep streaming dedupe remains stable.
5. New candidate sources are budgeted.
6. New boosts do not force full-record scoring for ordinary queries.
7. Cancellation still works before expensive loops.
8. Cache hits and misses do not affect ordering.
9. Tests cover both strong matches and tie cases.

The target is not clever search. The target is boringly fast, boringly stable,
and obviously better than Finder under large real-world trees.
