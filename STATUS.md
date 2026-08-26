# GFM Remaining Work

Date: 2026-08-26

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

15. Finish icon view Finder spacing, snap behavior, grouping controls, thumbnail/icon providers, drag images, inline rename, Desktop placement, keyboard navigation, and pixel baselines.
16. Finish list view Finder column metrics, resizing behavior, grouping controls, inline rename, keyboard navigation, huge-directory rendering, and pixel baselines.
17. Finish column view Finder column metrics, preview behavior, keyboard timing, scroll physics, and pixel baselines.
18. Finish gallery view Finder preview sizing, filmstrip behavior, thumbnail loading, metadata layout, quick-action availability, keyboard timing, and pixel baselines.
19. Finish search results view Finder scope controls, grouping presentation, metadata columns, ranking disclosure behavior, progressive refinement timing, and pixel baselines.
20. Finish Trash view Finder restore metadata sources, destructive confirmation flows, permission prompts, empty-trash behavior, and pixel baselines.
21. Finish LaunchServices UTType/package metadata binding, Finder package exceptions, user override UI, indexing/search/preview/package-icon behavior, and captured Finder pixel baselines.
22. Finish GPUI huge-directory virtualization binding, lazy row/cell materialization, incremental sort/filter sources, thumbnail/icon backpressure, measured hundred-thousand-entry latency budgets, and captured Finder pixel baselines.

## macOS Integration

23. Finish direct AppKit, Foundation, CoreServices, LaunchServices, Quick Look, Security, DiskArbitration, FileProvider, Spotlight, and FSEvents Rust APIs with ownership isolation, thread-affinity enforcement, error mapping, and host-version gates.
24. Finish direct LaunchServices/AppKit raster icon extraction, Finder custom icons, extension-hidden names, package/app/document icons, alias/iCloud/tag badge compositing, cache invalidation, and captured pixel baselines.
25. Finish direct QLPreviewController and QLPreviewItem integration, sandboxed generator execution, native preview lifecycle events, cache publication, cancellation, error surfacing, and captured Finder preview pixel baselines.
26. Finish direct QLThumbnailGenerator requests, decoded raster publication, memory/disk cache writes, content-signature invalidation, visible-window cancellation, error surfacing, and captured Finder thumbnail pixel baselines.
27. Finish Finder-visible metadata direct macOS binding for FinderInfo alias resolution, sidebar/tag UI propagation, and captured Finder pixel baselines.
28. Finish direct FileProvider.framework/NSFileProviderManager state reads, native download/evict operations, provider progress callbacks, materialized placeholder detection, conflict-resolution UI plumbing, sidebar/icon badge propagation, live invalidation, and captured Finder pixel baselines.
29. Finish DiskArbitration volume integration for long-lived session callbacks, native eject/unmount/mount operations, APFS/container metadata, network-volume reachability, sidebar propagation, live index policy invalidation, and captured Finder pixel baselines.
30. Finish Security-scoped access for TCC prompt orchestration, Full Disk Access diagnostics, index/preview worker enforcement, GPUI permission sheets, captured Finder prompt baselines, and operation permission regression under a full GPUI app build.

## Content Extraction

31. Complete PDF extraction with sandboxed workers, compressed/encrypted PDF coverage, incremental updates, extractor-version invalidation, and corrupt-file quarantine.
32. Complete Office legacy binary format strategy, protected/encrypted document handling, sandboxed workers, extractor-version invalidation, and corrupt-package quarantine.
33. Complete extraction policy for richer archive formats and corrupt-input quarantine.
34. Implement OCR strategy for image-only PDFs and screenshots without blocking primary indexing.
35. Complete hardened extraction-worker isolation with production read-deny Seatbelt feasibility, XPC/App Sandbox entitlement minimization, crash telemetry, and sandbox violation diagnostics.

## Storage Engine

36. Run and retain production macOS telemetry for the million-file materialized filesystem fixture across developer, media, documents, iCloud, external-volume, and network-volume trees.

## File Operations

37. Finish Finder-compatible copy preservation edge behavior for remaining clonefile/APFS edge cases, locked files, quarantine propagation, Finder-specific package exceptions, cross-volume metadata degradation UX, and captured Finder baselines.
38. Finish native GPUI operation pause/resume progress-surface binding, recovery UX, Finder-compatible Pause/Resume/Stop progress sheets, and captured Finder baselines.
39. Finish conflict UI/state machine binding, Finder-parity sheet presentation, per-item review table, keyboard/focus behavior, and captured Finder baselines.
40. Finish direct platform Trash item identity, Put Back parity, collision sheets, native destructive delete sheets, native Empty Trash confirmation/policy binding, and captured Finder baselines.
41. Finish privileged-operation flow for protected paths with native GPUI permission sheets, security-scoped bookmark acquisition UX, privileged helper or authorization strategy where required, Finder-parity Full Disk Access guidance, and captured protected-path baselines.
42. Finish native volume-specific network-volume fallback and slow-volume throttling with direct DiskArbitration/FileProvider volume classification and captured slow-volume baselines.

## Jobs And Runtime

43. Complete automatic durable job payload catalog integration across remaining specialized extraction, thumbnail, preview, and repair producers.
44. Complete automatic dependency-aware fair scheduling integration across foreground, visible, background, maintenance, and repair queues.
45. Complete automatic persistent job progress restoration and GPUI progress-surface integration across remaining specialized producers.
46. Bind thermal, battery, IO pressure, and user-activity adaptive scheduling into remaining foreground, preview, and thumbnail producers.
47. Complete automatic structured cancellation propagation across remaining indexing, extraction, thumbnail, preview, and repair producers.
48. Complete retry/backoff integration across remaining operation, indexing, extraction, thumbnail, preview, and repair producers.

## Preview And Thumbnails

49. Implement Finder-compatible generic icons, custom icons, app icons, folder icons, package icons, aliases, symlinks, tags, iCloud badges, and volume badges.

## Packaging

50. Wire the first-run permission onboarding contract into the GPUI shell with Finder-parity presentation.

## Documentation

51. Expand `PLAN.md` when architectural decisions change materially.
52. Keep `README.md` written as the completed product contract.
53. Keep this file limited to unfinished work only.
54. Add internal architecture docs for storage format, search ranking, operation recovery, macOS bridges, parity harness, and performance budgets.
