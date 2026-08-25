# GFM Remaining Work

Date: 2026-08-24

This is the living unfinished-work ledger for GFM. When a capability is implemented, verified, and accepted as production-grade, remove it from this file.

## Native App Shell

1. Create the GPUI application crate and production window lifecycle.
2. Build native macOS menu bar integration with Finder-matched menus, enabled states, shortcuts, and Services behavior.
3. Build Finder-matched toolbar composition, including navigation controls, title/path presentation, view controls, action controls, share, tags, more menu, and search field.
4. Implement Finder-matched sidebar sections, row heights, icon sizing, indentation, separators, disclosure behavior, drag targets, tags, iCloud entries, mounted volumes, network locations, and eject controls.
5. Implement Finder-matched titlebar, traffic-light spacing, focus appearance, vibrancy/material behavior, active/inactive states, and full-screen behavior.
6. Implement multi-window support, tab support, restoration, window placement persistence, and macOS scene activation behavior.
7. Implement Finder-matched context menus for files, folders, volumes, sidebar items, empty space, selected sets, search results, and Trash.
8. Implement Finder-matched alert sheets, rename fields, popovers, disclosure triangles, progress sheets, conflict dialogs, and permission prompts.

## Pixel Parity

9. Capture reference Finder screenshots for every target macOS build and appearance.
10. Build a Finder fixture generator that creates deterministic directory states for icon, list, column, gallery, sidebar, toolbar, search, selection, rename, drag, empty, huge, iCloud, external-volume, network-volume, and Trash scenarios.
11. Build a GFM screenshot harness that renders the same fixture matrix with deterministic fonts, scale factors, window sizes, focus state, and appearance.
12. Build pixel diffing with explicit masks only for unavoidable OS-owned dynamic pixels.
13. Define hard failure thresholds for layout, text, icon, selection, focus, hover, toolbar, thumbnail, and preview drift.
14. Add CI gates that fail on any unapproved Finder parity drift.
15. Add a human review artifact bundle for every parity baseline update.
16. Add per-macOS-build parity profiles for dimensions, materials, colors, typography, symbols, animations, and interaction timing.

## Views

17. Implement icon view with Finder-matched grid spacing, snap behavior, sorting, grouping, selection rectangles, file labels, thumbnails, badges, and Desktop behavior.
18. Implement list view with Finder-matched columns, disclosure rows, resizing, sorting, grouping, inline rename, keyboard navigation, alternating row behavior where applicable, and huge-directory virtualization.
19. Implement column view with Finder-matched column sizing, preview column, keyboard flow, scroll behavior, branch loading, and selection persistence.
20. Implement gallery view with Finder-matched preview area, filmstrip behavior, metadata panel, quick actions, keyboard flow, and thumbnail loading.
21. Implement search results view with Finder-matched scopes, grouping, metadata columns, ranking display behavior, and progressive result refinement.
22. Implement Trash view behavior, including restore location metadata, permanent delete flows, empty Trash, and permission failures.
23. Implement package traversal behavior for app bundles and document packages.
24. Implement virtualized rendering that keeps interaction latency stable in directories with hundreds of thousands of entries.

## macOS Integration

25. Build typed AppKit/Foundation/CoreServices bridges behind narrow Rust APIs.
26. Implement native file icons via LaunchServices and Finder-compatible badge composition.
27. Implement Quick Look previews and preview controller integration.
28. Implement thumbnail generation through QuickLookThumbnailing with cache policy and invalidation.
29. Implement Spotlight metadata ingestion and reconciliation without depending on Spotlight for primary correctness.
30. Implement Finder tags, labels, comments, kind strings, localized display names, bundle names, aliases, symlinks, packages, hidden files, and extension hiding behavior.
31. Implement iCloud Drive and FileProvider state reads, badges, eviction/download commands, conflict states, and offline behavior.
32. Implement DiskArbitration volume discovery, eject, mount/unmount changes, local/network/removable volume classification, and capacity display.
33. Implement Security-scoped access, TCC-aware permission prompts, Full Disk Access diagnostics, and least-privilege failure paths.

## Filesystem Indexing

34. Implement per-volume persistent index state with volume identity, mount identity, scan epoch, and schema versioning.
35. Implement durable FSEvents cursors with restart continuation.
36. Implement dropped-event detection and subtree repair scheduling.
37. Implement rename correlation that preserves identity and avoids delete/create churn where possible.
38. Implement incremental metadata updates for chmod, chown, xattrs, tags, Finder comments, timestamps, and size changes.
39. Implement backpressure so file event bursts do not stall UI or starve user-visible operations.
40. Implement crash-safe commit points for scan progress, segment publication, tombstones, and compaction.
41. Implement large-directory scan scheduling with fairness between visible directories and background crawl.
42. Implement network-volume and external-volume indexing policy with opt-in, throttling, and disconnected-state handling.

## Search Engine

43. Implement streaming search results with immediate hot-index results and progressive deeper results.
44. Implement metadata ranking that cleanly composes exact, prefix, substring, fuzzy, path, recency, frequency, kind, user-pinned, tag, and content signals.
45. Implement typo-tolerant fuzzy retrieval that avoids full-record scans at machine scale.
46. Implement phrase and proximity search for content.
47. Implement snippet extraction with highlighted matches and bounded IO.
48. Implement per-volume search shards with parallel fanout and deterministic merge ordering.
49. Implement user-intent ranking for Applications, Recents, Downloads, Desktop, project folders, screenshots, and recently touched files.
50. Implement search cancellation and supersession so stale queries stop consuming IO and CPU immediately.

## Content Extraction

51. Implement PDF text extraction with sandboxing, page limits, incremental updates, and corrupt-file isolation.
52. Implement Office document extraction for DOCX, XLSX, PPTX, and legacy formats where practical.
53. Implement rich text, HTML, Markdown, source code, plist, JSON, CSV, log, email, and archive metadata extraction policies.
54. Implement OCR strategy for image-only PDFs and screenshots without blocking primary indexing.
55. Implement binary type detection beyond extension heuristics.
56. Implement extraction budgets by file type, size, volume class, thermal state, battery state, and user activity.
57. Implement extraction caching keyed by file identity, content signature, extractor version, and metadata epoch.
58. Implement failure quarantine for repeatedly crashing or timing-out extractors.

## Storage Engine

59. Implement mmap-backed immutable archive readers for records, dictionaries, metadata postings, and content postings.
60. Implement dictionary compression for terms, paths, extensions, tags, kinds, metadata keys, and repeated path prefixes.
61. Implement block-level compression policy with fast random access and bounded decompression windows.
62. Implement large-index merge policy across hot buffers, immutable segments, compacted tiers, and tombstone cleanup.
63. Implement record column stores for high-cardinality fields and cache-friendly scan/rank passes.
64. Implement prefix/fuzzy lookup structures suitable for machine-wide scale.
65. Implement checksums, schema migration, crash recovery, corruption detection, and rebuild plans.
66. Implement index size telemetry and compaction scheduling heuristics.
67. Implement benchmark fixtures for millions of files and realistic developer, media, documents, and iCloud trees.

## File Operations

68. Implement APFS clone fast paths using platform-native clone semantics.
69. Implement copyfile/Finder-compatible metadata preservation, xattrs, ACLs, resource forks, quarantine attributes, package behavior, and symlink policies.
70. Implement operation pause, resume, cancellation, retry, and crash recovery replay.
71. Implement progress accounting for recursive operations before and during execution.
72. Implement conflict UI/state machine for replace, keep both, merge folders, skip, apply to all, and per-item decisions.
73. Implement Trash restore metadata and restore operation.
74. Implement privileged-operation flow for protected paths.
75. Implement network-volume fallbacks and slow-volume throttling.
76. Implement post-operation verification policy for high-risk moves/copies.

## Jobs And Runtime

77. Implement durable job payload catalog for all operation, indexing, extraction, thumbnail, preview, and repair jobs.
78. Implement job dependency graph and fairness between foreground, visible, background, maintenance, and repair queues.
79. Implement persistent progress snapshots and user-visible progress restoration after restart.
80. Implement thermal, battery, IO pressure, and user-activity adaptive scheduling.
81. Implement per-volume concurrency limits and operation isolation.
82. Implement structured cancellation propagation across nested jobs and subprocess extractors.
83. Implement retry backoff with classified transient, permission, missing-file, corrupt-file, and offline-volume failures.

## Preview And Thumbnails

84. Build preview cache with memory and disk tiers.
85. Implement icon, thumbnail, and Quick Look request coalescing.
86. Implement visible-window prioritization and cancellation for offscreen preview work.
87. Implement Finder-compatible generic icons, custom icons, app icons, folder icons, package icons, aliases, symlinks, tags, iCloud badges, and volume badges.
88. Implement preview security policy for untrusted files.
89. Implement thumbnail invalidation on content, metadata, tag, and iCloud state changes.

## Configuration

90. Define target macOS version matrix and supported hardware profiles.
91. Implement config crate for parity profiles, user settings, feature flags, and diagnostics toggles.
92. Implement persistent settings storage with schema versioning and migration.
93. Implement hidden/internal performance controls without exposing non-Finder UI by default.
94. Implement operator diagnostics commands for index rebuild, trace export, parity baseline selection, and storage inspection.

## Telemetry And Performance

95. Implement latency histograms for navigation, selection, rename, search keystrokes, result streaming, thumbnail display, preview open, copy start, cancel, and window render.
96. Implement frame timing and UI-thread stall detection.
97. Implement IO, CPU, memory, allocation, queue-depth, and compaction telemetry.
98. Implement local-only diagnostics export with privacy review.
99. Define hard budgets for p50, p95, p99, cold start, warm start, first result, full result, directory open, and visible thumbnail completion.
100. Build repeatable macrobenchmarks against small, medium, huge, developer, media, iCloud, external, and network-volume trees.
101. Add regression gates that fail on latency, memory, index size, or frame-time drift.

## Packaging

102. Build signed `.app` bundle with icons, entitlements, Info.plist, launch services registration, and document associations.
103. Implement hardened runtime settings.
104. Implement notarization pipeline.
105. Implement first-run permission onboarding that remains Finder-parity by default.
106. Implement update, rollback, crash-report, and diagnostics policy.
107. Implement release artifact validation on clean macOS machines.

## Documentation

108. Expand `PLAN.md` when architectural decisions change materially.
109. Keep `README.md` written as the completed product contract.
110. Keep this file limited to unfinished work only.
111. Add internal architecture docs for storage format, search ranking, operation recovery, macOS bridges, parity harness, and performance budgets.
