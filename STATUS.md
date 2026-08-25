# GFM Remaining Work

Date: 2026-08-24

This is the living unfinished-work ledger for GFM. When a capability is implemented, verified, and accepted as production-grade, remove it from this file.

## Native App Shell

1. Finish byte-for-byte Finder toolbar parity by calibrating the native GPUI toolbar's exact symbols, spacing, vibrancy, hover/focus states, search-field behavior, enabled-state transitions, and menu/action wiring against captured Finder baselines.
2. Finish byte-for-byte Finder sidebar parity by calibrating the native GPUI sidebar's exact icons, row metrics, indentation, vibrancy, separators, disclosure behavior, drag targets, tag rendering, iCloud state transitions, mounted-volume behavior, network locations, eject controls, selection/focus states, and baseline-captured spacing.
3. Finish byte-for-byte Finder titlebar parity by calibrating the native GPUI titlebar's exact traffic-light spacing, focus appearance, vibrancy/material behavior, active/inactive transitions, tab/full-screen behavior, and baseline-captured title/path chrome against target macOS builds.
4. Implement multi-window support, tab support beyond the initial tab group contract, restoration, window placement persistence, and macOS scene activation behavior.
5. Implement Finder-matched context menus for files, folders, volumes, sidebar items, empty space, selected sets, search results, and Trash.
6. Implement Finder-matched alert sheets, rename fields, popovers, disclosure triangles, progress sheets, conflict dialogs, and permission prompts.

## Pixel Parity

7. Capture reference Finder screenshots for every target macOS build and appearance.
8. Build a Finder fixture generator that creates deterministic directory states for icon, list, column, gallery, sidebar, toolbar, search, selection, rename, drag, empty, huge, iCloud, external-volume, network-volume, and Trash scenarios.
9. Build a GFM screenshot harness that renders the same fixture matrix with deterministic fonts, scale factors, window sizes, focus state, and appearance.
10. Build pixel diffing with explicit masks only for unavoidable OS-owned dynamic pixels.
11. Define hard failure thresholds for layout, text, icon, selection, focus, hover, toolbar, thumbnail, and preview drift.
12. Add CI gates that fail on any unapproved Finder parity drift.
13. Add a human review artifact bundle for every parity baseline update.
14. Add per-macOS-build parity profiles for dimensions, materials, colors, typography, symbols, animations, and interaction timing.

## Views

15. Implement icon view with Finder-matched grid spacing, snap behavior, sorting, grouping, selection rectangles, file labels, thumbnails, badges, and Desktop behavior.
16. Implement list view with Finder-matched columns, disclosure rows, resizing, sorting, grouping, inline rename, keyboard navigation, alternating row behavior where applicable, and huge-directory virtualization.
17. Implement column view with Finder-matched column sizing, preview column, keyboard flow, scroll behavior, branch loading, and selection persistence.
18. Implement gallery view with Finder-matched preview area, filmstrip behavior, metadata panel, quick actions, keyboard flow, and thumbnail loading.
19. Implement search results view with Finder-matched scopes, grouping, metadata columns, ranking display behavior, and progressive result refinement.
20. Implement Trash view behavior, including restore location metadata, permanent delete flows, empty Trash, and permission failures.
21. Implement package traversal behavior for app bundles and document packages.
22. Implement virtualized rendering that keeps interaction latency stable in directories with hundreds of thousands of entries.

## macOS Integration

23. Build typed AppKit/Foundation/CoreServices bridges behind narrow Rust APIs.
24. Implement native file icons via LaunchServices and Finder-compatible badge composition.
25. Implement Quick Look previews and preview controller integration.
26. Implement thumbnail generation through QuickLookThumbnailing with cache policy and invalidation.
27. Implement Spotlight metadata ingestion and reconciliation without depending on Spotlight for primary correctness.
28. Implement Finder tags, labels, comments, kind strings, localized display names, bundle names, aliases, symlinks, packages, hidden files, and extension hiding behavior.
29. Implement iCloud Drive and FileProvider state reads, badges, eviction/download commands, conflict states, and offline behavior.
30. Implement DiskArbitration volume discovery, eject, mount/unmount changes, local/network/removable volume classification, and capacity display.
31. Implement Security-scoped access, TCC-aware permission prompts, Full Disk Access diagnostics, and least-privilege failure paths.

## Filesystem Indexing

32. Implement per-volume persistent index state with volume identity, mount identity, scan epoch, and schema versioning.
33. Implement durable FSEvents cursors with restart continuation.
34. Implement dropped-event detection and subtree repair scheduling.
35. Implement rename correlation that preserves identity and avoids delete/create churn where possible.
36. Implement incremental metadata updates for chmod, chown, xattrs, tags, Finder comments, timestamps, and size changes.
37. Implement backpressure so file event bursts do not stall UI or starve user-visible operations.
38. Implement crash-safe commit points for scan progress, segment publication, tombstones, and compaction.
39. Implement large-directory scan scheduling with fairness between visible directories and background crawl.
40. Implement network-volume and external-volume indexing policy with opt-in, throttling, and disconnected-state handling.

## Content Extraction

41. Complete PDF extraction with sandboxed workers, compressed/encrypted PDF coverage, incremental updates, extractor-version invalidation, and corrupt-file quarantine beyond the bounded in-process text-stream extractor.
42. Complete Office extraction beyond bounded OOXML with legacy binary format strategy, protected/encrypted document handling, sandboxed workers, extractor-version invalidation, and corrupt-package quarantine.
43. Complete extraction policy depth beyond current UTF-8 text, HTML, RTF, email, ZIP metadata, JSON, CSV, and plist paths, including MIME multipart emails, richer archive formats, per-format invalidation, and corrupt-input quarantine.
44. Implement OCR strategy for image-only PDFs and screenshots without blocking primary indexing.
45. Implement extraction budgets by file type, size, volume class, thermal state, battery state, and user activity.
46. Implement extraction caching keyed by file identity, content signature, extractor version, and metadata epoch.
47. Implement failure quarantine for repeatedly crashing or timing-out extractors.

## Storage Engine

48. Implement mmap-backed immutable archive readers for records, dictionaries, metadata postings, and content postings.
49. Implement dictionary compression for terms, paths, extensions, tags, kinds, metadata keys, and repeated path prefixes.
50. Implement block-level compression policy with fast random access and bounded decompression windows.
51. Implement large-index merge policy across hot buffers, immutable segments, compacted tiers, and tombstone cleanup.
52. Implement record column stores for high-cardinality fields and cache-friendly scan/rank passes.
53. Implement prefix/fuzzy lookup structures suitable for machine-wide scale.
54. Implement checksums, schema migration, crash recovery, corruption detection, and rebuild plans.
55. Implement index size telemetry and compaction scheduling heuristics.
56. Implement benchmark fixtures for millions of files and realistic developer, media, documents, and iCloud trees.

## File Operations

57. Implement APFS clone fast paths using platform-native clone semantics.
58. Implement copyfile/Finder-compatible metadata preservation, xattrs, ACLs, resource forks, quarantine attributes, package behavior, and symlink policies.
59. Implement operation pause, resume, cancellation, retry, and crash recovery replay.
60. Implement progress accounting for recursive operations before and during execution.
61. Implement conflict UI/state machine for replace, keep both, merge folders, skip, apply to all, and per-item decisions.
62. Implement Trash restore metadata and restore operation.
63. Implement privileged-operation flow for protected paths.
64. Implement network-volume fallbacks and slow-volume throttling.
65. Implement post-operation verification policy for high-risk moves/copies.

## Jobs And Runtime

66. Implement durable job payload catalog for all operation, indexing, extraction, thumbnail, preview, and repair jobs.
67. Implement job dependency graph and fairness between foreground, visible, background, maintenance, and repair queues.
68. Implement persistent progress snapshots and user-visible progress restoration after restart.
69. Implement thermal, battery, IO pressure, and user-activity adaptive scheduling.
70. Implement per-volume concurrency limits and operation isolation.
71. Implement structured cancellation propagation across nested jobs and subprocess extractors.
72. Implement retry backoff with classified transient, permission, missing-file, corrupt-file, and offline-volume failures.

## Preview And Thumbnails

73. Implement Finder-compatible generic icons, custom icons, app icons, folder icons, package icons, aliases, symlinks, tags, iCloud badges, and volume badges.

## Packaging

74. Wire the first-run permission onboarding contract into the GPUI shell with Finder-parity presentation.

## Documentation

75. Expand `PLAN.md` when architectural decisions change materially.
76. Keep `README.md` written as the completed product contract.
77. Keep this file limited to unfinished work only.
78. Add internal architecture docs for storage format, search ranking, operation recovery, macOS bridges, parity harness, and performance budgets.
