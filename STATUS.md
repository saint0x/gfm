# GFM Remaining Work

Date: 2026-08-24

This is the living unfinished-work ledger for GFM. When a capability is implemented, verified, and accepted as production-grade, remove it from this file.

## Native App Shell

1. Finish byte-for-byte Finder toolbar parity by calibrating the native GPUI toolbar's exact symbols, spacing, vibrancy, hover/focus states, search-field behavior, enabled-state transitions, and menu/action wiring against captured Finder baselines.
2. Finish byte-for-byte Finder sidebar parity by calibrating the native GPUI sidebar's exact icons, row metrics, indentation, vibrancy, separators, disclosure behavior, drag targets, tag rendering, iCloud state transitions, mounted-volume behavior, network locations, eject controls, selection/focus states, and baseline-captured spacing.
3. Finish byte-for-byte Finder titlebar parity by calibrating the native GPUI titlebar's exact traffic-light spacing, focus appearance, vibrancy/material behavior, active/inactive transitions, tab/full-screen behavior, and baseline-captured title/path chrome against target macOS builds.
4. Finish Finder-matched multi-window, tab, restoration, placement, and scene behavior by calibrating native tab grouping, launch restoration, cascade limits, per-display placement, window close/reopen state, macOS scene activation, and crash-safe placement persistence against Finder baselines.
5. Finish Finder-matched context menus for files, folders, volumes, sidebar items, empty space, selected sets, search results, and Trash by calibrating exact item order, native presentation, Services/Open With population, enabled-state ownership, destructive action sheets, and operation hooks against Finder baselines.
6. Finish Finder-matched alert sheets, rename fields, popovers, disclosure triangles, progress sheets, conflict dialogs, and permission prompts by calibrating exact native presentation, animation, focus order, button spacing, keyboard behavior, accessibility roles, and operation/permission bindings against Finder baselines.

## Pixel Parity

7. Capture reference Finder screenshots for every target macOS build and appearance.
8. Finish the Finder fixture generator by extending the deterministic parity fixture matrix with captured Finder view settings, xattrs, tags, package metadata, iCloud/FileProvider state, real external/network volume descriptors, Trash restore metadata, and per-build baseline manifests.
9. Build a GFM screenshot harness that renders the same fixture matrix with deterministic fonts, scale factors, window sizes, focus state, and appearance.
10. Finish pixel diffing by connecting the strict RGBA diff core to screenshot capture outputs, PNG ingestion, explicit per-build mask governance, visual diff artifact generation, and CI failure reporting for unavoidable OS-owned dynamic pixels only.
11. Finish hard failure thresholds by binding the strict per-surface pixel threshold contract to captured layout, text, icon, selection, focus, hover, toolbar, thumbnail, and preview regions with per-build CI reports and review artifacts.
12. Finish CI parity enforcement by wiring the manifest-driven parity gate into screenshot-capture CI jobs, baseline artifact publication, per-build mask approvals, and required failure reporting for every unapproved Finder drift.
13. Finish human review artifact bundles by attaching the generated review bundle to every baseline-update CI path, including Finder/GFM screenshots, visual diff images, per-mask justification files, reviewer sign-off metadata, and retained build provenance.
14. Finish per-macOS-build parity profiles by binding the generated profile contract to captured Finder token calibration for every supported build, appearance, scale factor, color profile, accent setting, accessibility variant, animation timing, and interaction timing baseline.

## Views

15. Finish icon view by binding the implemented grid/selection/badge/virtualization contract to captured Finder spacing, snap behavior, grouping controls, thumbnail/icon providers, drag images, inline rename, Desktop placement, keyboard navigation, and pixel baselines.
16. Finish list view by binding the implemented column/disclosure/sorting/selection/alternating-row/virtualization contract to captured Finder column metrics, resizing behavior, grouping controls, inline rename, keyboard navigation, huge-directory rendering, and pixel baselines.
17. Finish column view by binding the implemented column sizing, branch-selection, preview-column, keyboard-flow, scroll-position, branch-loading, selection-persistence, and virtualization contract to captured Finder column metrics, preview behavior, keyboard timing, scroll physics, and pixel baselines.
18. Finish gallery view by binding the implemented preview-area, filmstrip, metadata-panel, quick-action, keyboard-flow, selection, and virtualization contract to captured Finder preview sizing, filmstrip behavior, thumbnail loading, metadata layout, quick-action availability, keyboard timing, and pixel baselines.
19. Finish search results view by binding the implemented scope, grouping, metadata-column, ranking-display, progressive-stage, selection, snippet, and virtualization contract to captured Finder search scope controls, grouping presentation, metadata columns, ranking disclosure behavior, progressive refinement timing, and pixel baselines.
20. Finish Trash view by binding the implemented restore-location metadata, permanent-delete, empty-trash, selection, permission-failure, command-state, and virtualization contract to captured Finder Trash restore metadata sources, destructive confirmation flows, permission prompts, empty-trash behavior, and pixel baselines.
21. Finish package traversal by binding the implemented package classification and opaque-vs-traverse scan policy to LaunchServices UTType/package metadata, Finder package exceptions, user override UI, indexing/search/preview/package-icon behavior, and captured Finder pixel baselines.
22. Finish huge-directory virtualization by binding the implemented shared visible-window contract to GPUI scroll containers, lazy row/cell materialization, incremental sort/filter sources, thumbnail/icon backpressure, measured hundred-thousand-entry latency budgets, and captured Finder pixel baselines.

## macOS Integration

23. Finish native macOS bridges by replacing the implemented bridge registry's required surfaces with narrow Rust APIs for direct AppKit, Foundation, CoreServices, LaunchServices, Quick Look, Security, DiskArbitration, FileProvider, Spotlight, and FSEvents bindings, with ownership isolation, thread-affinity enforcement, error mapping, and host-version gates.
24. Finish native file icons by binding the implemented LaunchServices-targeted icon descriptor, type-hint, cache-key, and badge contract to direct LaunchServices/AppKit raster extraction, Finder custom icons, extension-hidden names, package/app/document icons, alias/iCloud/tag badge compositing, cache invalidation, and captured pixel baselines.
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
