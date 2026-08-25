# GFM Remaining Work

Date: 2026-08-24

This is the living unfinished-work ledger for GFM. When a capability is implemented, verified, and accepted as production-grade, remove it from this file.

## Native App Shell

1. Build native macOS menu bar integration with Finder-matched menus, enabled states, shortcuts, and Services behavior.
2. Build Finder-matched toolbar composition, including navigation controls, title/path presentation, view controls, action controls, share, tags, more menu, and search field.
3. Implement Finder-matched sidebar sections, row heights, icon sizing, indentation, separators, disclosure behavior, drag targets, tags, iCloud entries, mounted volumes, network locations, and eject controls.
4. Implement Finder-matched titlebar, traffic-light spacing, focus appearance, vibrancy/material behavior, active/inactive states, and full-screen behavior beyond the initial transparent GPUI titlebar lifecycle.
5. Implement multi-window support, tab support beyond the initial tab group contract, restoration, window placement persistence, and macOS scene activation behavior.
6. Implement Finder-matched context menus for files, folders, volumes, sidebar items, empty space, selected sets, search results, and Trash.
7. Implement Finder-matched alert sheets, rename fields, popovers, disclosure triangles, progress sheets, conflict dialogs, and permission prompts.

## Pixel Parity

8. Capture reference Finder screenshots for every target macOS build and appearance.
9. Build a Finder fixture generator that creates deterministic directory states for icon, list, column, gallery, sidebar, toolbar, search, selection, rename, drag, empty, huge, iCloud, external-volume, network-volume, and Trash scenarios.
10. Build a GFM screenshot harness that renders the same fixture matrix with deterministic fonts, scale factors, window sizes, focus state, and appearance.
11. Build pixel diffing with explicit masks only for unavoidable OS-owned dynamic pixels.
12. Define hard failure thresholds for layout, text, icon, selection, focus, hover, toolbar, thumbnail, and preview drift.
13. Add CI gates that fail on any unapproved Finder parity drift.
14. Add a human review artifact bundle for every parity baseline update.
15. Add per-macOS-build parity profiles for dimensions, materials, colors, typography, symbols, animations, and interaction timing.

## Views

16. Implement icon view with Finder-matched grid spacing, snap behavior, sorting, grouping, selection rectangles, file labels, thumbnails, badges, and Desktop behavior.
17. Implement list view with Finder-matched columns, disclosure rows, resizing, sorting, grouping, inline rename, keyboard navigation, alternating row behavior where applicable, and huge-directory virtualization.
18. Implement column view with Finder-matched column sizing, preview column, keyboard flow, scroll behavior, branch loading, and selection persistence.
19. Implement gallery view with Finder-matched preview area, filmstrip behavior, metadata panel, quick actions, keyboard flow, and thumbnail loading.
20. Implement search results view with Finder-matched scopes, grouping, metadata columns, ranking display behavior, and progressive result refinement.
21. Implement Trash view behavior, including restore location metadata, permanent delete flows, empty Trash, and permission failures.
22. Implement package traversal behavior for app bundles and document packages.
23. Implement virtualized rendering that keeps interaction latency stable in directories with hundreds of thousands of entries.

## macOS Integration

24. Build typed AppKit/Foundation/CoreServices bridges behind narrow Rust APIs.
25. Implement native file icons via LaunchServices and Finder-compatible badge composition.
26. Implement Quick Look previews and preview controller integration.
27. Implement thumbnail generation through QuickLookThumbnailing with cache policy and invalidation.
28. Implement Spotlight metadata ingestion and reconciliation without depending on Spotlight for primary correctness.
29. Implement Finder tags, labels, comments, kind strings, localized display names, bundle names, aliases, symlinks, packages, hidden files, and extension hiding behavior.
30. Implement iCloud Drive and FileProvider state reads, badges, eviction/download commands, conflict states, and offline behavior.
31. Implement DiskArbitration volume discovery, eject, mount/unmount changes, local/network/removable volume classification, and capacity display.
32. Implement Security-scoped access, TCC-aware permission prompts, Full Disk Access diagnostics, and least-privilege failure paths.

## Filesystem Indexing

33. Implement per-volume persistent index state with volume identity, mount identity, scan epoch, and schema versioning.
34. Implement durable FSEvents cursors with restart continuation.
35. Implement dropped-event detection and subtree repair scheduling.
36. Implement rename correlation that preserves identity and avoids delete/create churn where possible.
37. Implement incremental metadata updates for chmod, chown, xattrs, tags, Finder comments, timestamps, and size changes.
38. Implement backpressure so file event bursts do not stall UI or starve user-visible operations.
39. Implement crash-safe commit points for scan progress, segment publication, tombstones, and compaction.
40. Implement large-directory scan scheduling with fairness between visible directories and background crawl.
41. Implement network-volume and external-volume indexing policy with opt-in, throttling, and disconnected-state handling.

## Content Extraction

42. Complete PDF extraction with sandboxed workers, compressed/encrypted PDF coverage, incremental updates, extractor-version invalidation, and corrupt-file quarantine beyond the bounded in-process text-stream extractor.
43. Complete Office extraction beyond bounded OOXML with legacy binary format strategy, protected/encrypted document handling, sandboxed workers, extractor-version invalidation, and corrupt-package quarantine.
44. Complete extraction policy depth beyond current UTF-8 text, HTML, RTF, email, ZIP metadata, JSON, CSV, and plist paths, including MIME multipart emails, richer archive formats, per-format invalidation, and corrupt-input quarantine.
45. Implement OCR strategy for image-only PDFs and screenshots without blocking primary indexing.
46. Implement extraction budgets by file type, size, volume class, thermal state, battery state, and user activity.
47. Implement extraction caching keyed by file identity, content signature, extractor version, and metadata epoch.
48. Implement failure quarantine for repeatedly crashing or timing-out extractors.

## Storage Engine

49. Implement mmap-backed immutable archive readers for records, dictionaries, metadata postings, and content postings.
50. Implement dictionary compression for terms, paths, extensions, tags, kinds, metadata keys, and repeated path prefixes.
51. Implement block-level compression policy with fast random access and bounded decompression windows.
52. Implement large-index merge policy across hot buffers, immutable segments, compacted tiers, and tombstone cleanup.
53. Implement record column stores for high-cardinality fields and cache-friendly scan/rank passes.
54. Implement prefix/fuzzy lookup structures suitable for machine-wide scale.
55. Implement checksums, schema migration, crash recovery, corruption detection, and rebuild plans.
56. Implement index size telemetry and compaction scheduling heuristics.
57. Implement benchmark fixtures for millions of files and realistic developer, media, documents, and iCloud trees.

## File Operations

58. Implement APFS clone fast paths using platform-native clone semantics.
59. Implement copyfile/Finder-compatible metadata preservation, xattrs, ACLs, resource forks, quarantine attributes, package behavior, and symlink policies.
60. Implement operation pause, resume, cancellation, retry, and crash recovery replay.
61. Implement progress accounting for recursive operations before and during execution.
62. Implement conflict UI/state machine for replace, keep both, merge folders, skip, apply to all, and per-item decisions.
63. Implement Trash restore metadata and restore operation.
64. Implement privileged-operation flow for protected paths.
65. Implement network-volume fallbacks and slow-volume throttling.
66. Implement post-operation verification policy for high-risk moves/copies.

## Jobs And Runtime

67. Implement durable job payload catalog for all operation, indexing, extraction, thumbnail, preview, and repair jobs.
68. Implement job dependency graph and fairness between foreground, visible, background, maintenance, and repair queues.
69. Implement persistent progress snapshots and user-visible progress restoration after restart.
70. Implement thermal, battery, IO pressure, and user-activity adaptive scheduling.
71. Implement per-volume concurrency limits and operation isolation.
72. Implement structured cancellation propagation across nested jobs and subprocess extractors.
73. Implement retry backoff with classified transient, permission, missing-file, corrupt-file, and offline-volume failures.

## Preview And Thumbnails

74. Implement Finder-compatible generic icons, custom icons, app icons, folder icons, package icons, aliases, symlinks, tags, iCloud badges, and volume badges.

## Packaging

75. Wire the first-run permission onboarding contract into the GPUI shell with Finder-parity presentation.

## Documentation

76. Expand `PLAN.md` when architectural decisions change materially.
77. Keep `README.md` written as the completed product contract.
78. Keep this file limited to unfinished work only.
79. Add internal architecture docs for storage format, search ranking, operation recovery, macOS bridges, parity harness, and performance budgets.
