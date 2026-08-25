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
25. Finish Quick Look previews by binding the implemented session/controller/security/invalidation/scheduling contract to direct QLPreviewController and QLPreviewItem integration, sandboxed generator execution, native preview lifecycle events, cache publication, cancellation, error surfacing, and captured Finder preview pixel baselines.
26. Finish thumbnail generation by binding the implemented QuickLookThumbnailing-targeted generator/cache/security/invalidation/scheduling contract to direct QLThumbnailGenerator requests, decoded raster publication, memory/disk cache writes, content-signature invalidation, visible-window cancellation, error surfacing, and captured Finder thumbnail pixel baselines.
27. Finish Spotlight metadata ingestion by replacing the implemented mdls-backed reconciliation reader/fixture contract with direct background-safe Metadata.framework or CoreServices APIs, batched attribute reads, index-health detection, stale-result quarantine, per-volume throttling, and persisted secondary metadata publication without depending on Spotlight for primary correctness.
28. Finish Finder-visible metadata by binding the implemented filesystem/xattr-backed tags, label colors, comments, kind-string, localized-name, package, symlink, alias, hidden-file, and extension-hiding report to direct LaunchServices/CoreServices localized kind resolution, FinderInfo alias resolution, bundle display-name rules, sidebar/tag UI propagation, search-index publication, live xattr invalidation, and captured Finder pixel baselines.
29. Finish iCloud Drive and FileProvider integration by binding the implemented filesystem/xattr/path-hint state, badge, command-policy, conflict, and offline contract to direct FileProvider.framework/NSFileProviderManager state reads, native download/evict operations, provider progress callbacks, materialized placeholder detection, conflict-resolution UI plumbing, sidebar/icon badge propagation, live invalidation, and captured Finder pixel baselines.
30. Finish DiskArbitration volume integration by binding the implemented filesystem/capacity/fixture-marker discovery, local/network/removable classification, mounted-state, capacity, and eject/mount command-policy contract to direct DiskArbitration session callbacks, DADisk descriptions, native eject/unmount/mount operations, APFS/container metadata, network-volume reachability, sidebar propagation, live index policy invalidation, and captured Finder pixel baselines.
31. Finish Security-scoped access by binding the implemented per-path access/probe/bookmark/Full-Disk/degraded-mode contract to direct Security.framework scoped bookmark creation and resolution, TCC prompt orchestration, persistent bookmark storage, stale-bookmark repair, Full Disk Access diagnostics, operation/index/preview worker enforcement, GPUI permission sheets, and captured Finder prompt baselines.

## Content Extraction

32. Complete PDF extraction with sandboxed workers, compressed/encrypted PDF coverage, incremental updates, extractor-version invalidation, and corrupt-file quarantine beyond the bounded in-process text-stream extractor.
33. Complete Office extraction beyond bounded OOXML with legacy binary format strategy, protected/encrypted document handling, sandboxed workers, extractor-version invalidation, and corrupt-package quarantine.
34. Complete extraction policy depth beyond current UTF-8 text, HTML, RTF, email, ZIP metadata, JSON, CSV, and plist paths, including MIME multipart emails, richer archive formats, per-format invalidation, and corrupt-input quarantine.
35. Implement OCR strategy for image-only PDFs and screenshots without blocking primary indexing.
36. Complete hardened extraction-worker isolation with production read-deny Seatbelt feasibility, XPC/App Sandbox entitlement minimization, crash telemetry, and sandbox violation diagnostics beyond the implemented mutation-deny Seatbelt wrapper, timeout-supervised adaptive subprocess worker, and persistent worker-failure quarantine integration.

## Storage Engine

37. Run and retain production macOS telemetry for the million-file materialized filesystem fixture across developer, media, documents, iCloud, external-volume, and network-volume trees.

## File Operations

38. Finish Finder-compatible copy preservation for ownership, ACLs, birthtime, package behavior, and verified edge cases beyond the implemented symlink preservation, permissions, access/modified timestamps, and copyable xattrs.
39. Implement operation pause and resume beyond the implemented journaled cancellation checkpoints, interrupted-operation recovery replay, and explicit capped retry for classified transient failed operations.
40. Implement conflict UI/state machine for replace, keep both, merge folders, skip, apply to all, and per-item decisions.
41. Implement Trash restore metadata and restore operation.
42. Implement privileged-operation flow for protected paths.
43. Implement network-volume fallbacks and slow-volume throttling.

## Jobs And Runtime

44. Implement durable job payload catalog for all operation, indexing, extraction, thumbnail, preview, and repair jobs.
45. Implement job dependency graph and fairness between foreground, visible, background, maintenance, and repair queues.
46. Implement persistent progress snapshots and user-visible progress restoration after restart.
47. Bind thermal, battery, IO pressure, and user-activity adaptive scheduling into remaining foreground, preview, and thumbnail producers beyond the implemented jobs-layer policy, background content indexing hook, content maintenance hook, sidecar repair hook, persistent-index repair hook, and diagnostics-rebuild hook.
48. Complete structured cancellation propagation across nested jobs and remaining producers beyond the implemented search/index cancellation contracts, operation cancellation checkpoints, preview offscreen cancellation, and adaptive extraction subprocess cancellation supervision.
49. Implement retry backoff with classified transient, permission, missing-file, corrupt-file, and offline-volume failures.

## Preview And Thumbnails

50. Implement Finder-compatible generic icons, custom icons, app icons, folder icons, package icons, aliases, symlinks, tags, iCloud badges, and volume badges.

## Packaging

51. Wire the first-run permission onboarding contract into the GPUI shell with Finder-parity presentation.

## Documentation

52. Expand `PLAN.md` when architectural decisions change materially.
53. Keep `README.md` written as the completed product contract.
54. Keep this file limited to unfinished work only.
55. Add internal architecture docs for storage format, search ranking, operation recovery, macOS bridges, parity harness, and performance budgets.
