# GFM Remaining Work

Date: 2026-08-24

This is the living unfinished-work ledger for GFM. When a capability is implemented, verified, and accepted as production-grade, remove it from this file.

## Native App Shell

- Create the GPUI application crate and production window lifecycle.
- Build native macOS menu bar integration with Finder-matched menus, enabled states, shortcuts, and Services behavior.
- Build Finder-matched toolbar composition, including navigation controls, title/path presentation, view controls, action controls, share, tags, more menu, and search field.
- Implement Finder-matched sidebar sections, row heights, icon sizing, indentation, separators, disclosure behavior, drag targets, tags, iCloud entries, mounted volumes, network locations, and eject controls.
- Implement Finder-matched titlebar, traffic-light spacing, focus appearance, vibrancy/material behavior, active/inactive states, and full-screen behavior.
- Implement multi-window support, tab support, restoration, window placement persistence, and macOS scene activation behavior.
- Implement Finder-matched context menus for files, folders, volumes, sidebar items, empty space, selected sets, search results, and Trash.
- Implement Finder-matched alert sheets, rename fields, popovers, disclosure triangles, progress sheets, conflict dialogs, and permission prompts.

## Pixel Parity

- Capture reference Finder screenshots for every target macOS build and appearance.
- Build a Finder fixture generator that creates deterministic directory states for icon, list, column, gallery, sidebar, toolbar, search, selection, rename, drag, empty, huge, iCloud, external-volume, network-volume, and Trash scenarios.
- Build a GFM screenshot harness that renders the same fixture matrix with deterministic fonts, scale factors, window sizes, focus state, and appearance.
- Build pixel diffing with explicit masks only for unavoidable OS-owned dynamic pixels.
- Define hard failure thresholds for layout, text, icon, selection, focus, hover, toolbar, thumbnail, and preview drift.
- Add CI gates that fail on any unapproved Finder parity drift.
- Add a human review artifact bundle for every parity baseline update.
- Add per-macOS-build parity profiles for dimensions, materials, colors, typography, symbols, animations, and interaction timing.

## Views

- Implement icon view with Finder-matched grid spacing, snap behavior, sorting, grouping, selection rectangles, file labels, thumbnails, badges, and Desktop behavior.
- Implement list view with Finder-matched columns, disclosure rows, resizing, sorting, grouping, inline rename, keyboard navigation, alternating row behavior where applicable, and huge-directory virtualization.
- Implement column view with Finder-matched column sizing, preview column, keyboard flow, scroll behavior, branch loading, and selection persistence.
- Implement gallery view with Finder-matched preview area, filmstrip behavior, metadata panel, quick actions, keyboard flow, and thumbnail loading.
- Implement search results view with Finder-matched scopes, grouping, metadata columns, ranking display behavior, and progressive result refinement.
- Implement Trash view behavior, including restore location metadata, permanent delete flows, empty Trash, and permission failures.
- Implement package traversal behavior for app bundles and document packages.
- Implement virtualized rendering that keeps interaction latency stable in directories with hundreds of thousands of entries.

## macOS Integration

- Build typed AppKit/Foundation/CoreServices bridges behind narrow Rust APIs.
- Implement native file icons via LaunchServices and Finder-compatible badge composition.
- Implement Quick Look previews and preview controller integration.
- Implement thumbnail generation through QuickLookThumbnailing with cache policy and invalidation.
- Implement Spotlight metadata ingestion and reconciliation without depending on Spotlight for primary correctness.
- Implement Finder tags, labels, comments, kind strings, localized display names, bundle names, aliases, symlinks, packages, hidden files, and extension hiding behavior.
- Implement iCloud Drive and FileProvider state reads, badges, eviction/download commands, conflict states, and offline behavior.
- Implement DiskArbitration volume discovery, eject, mount/unmount changes, local/network/removable volume classification, and capacity display.
- Implement Security-scoped access, TCC-aware permission prompts, Full Disk Access diagnostics, and least-privilege failure paths.

## Filesystem Indexing

- Implement per-volume persistent index state with volume identity, mount identity, scan epoch, and schema versioning.
- Implement durable FSEvents cursors with restart continuation.
- Implement dropped-event detection and subtree repair scheduling.
- Implement rename correlation that preserves identity and avoids delete/create churn where possible.
- Implement incremental metadata updates for chmod, chown, xattrs, tags, Finder comments, timestamps, and size changes.
- Implement backpressure so file event bursts do not stall UI or starve user-visible operations.
- Implement crash-safe commit points for scan progress, segment publication, tombstones, and compaction.
- Implement large-directory scan scheduling with fairness between visible directories and background crawl.
- Implement network-volume and external-volume indexing policy with opt-in, throttling, and disconnected-state handling.

## Search Engine

- Implement remaining query parser support for tag filters, scope prefixes, and content-backed phrase semantics.
- Implement streaming search results with immediate hot-index results and progressive deeper results.
- Implement metadata ranking that cleanly composes exact, prefix, substring, fuzzy, path, recency, frequency, kind, user-pinned, tag, and content signals.
- Implement typo-tolerant fuzzy retrieval that avoids full-record scans at machine scale.
- Implement phrase and proximity search for content.
- Implement snippet extraction with highlighted matches and bounded IO.
- Implement per-volume search shards with parallel fanout and deterministic merge ordering.
- Implement user-intent ranking for Applications, Recents, Downloads, Desktop, project folders, screenshots, and recently touched files.
- Implement search cancellation and supersession so stale queries stop consuming IO and CPU immediately.

## Content Extraction

- Implement PDF text extraction with sandboxing, page limits, incremental updates, and corrupt-file isolation.
- Implement Office document extraction for DOCX, XLSX, PPTX, and legacy formats where practical.
- Implement rich text, HTML, Markdown, source code, plist, JSON, CSV, log, email, and archive metadata extraction policies.
- Implement OCR strategy for image-only PDFs and screenshots without blocking primary indexing.
- Implement binary type detection beyond extension heuristics.
- Implement extraction budgets by file type, size, volume class, thermal state, battery state, and user activity.
- Implement extraction caching keyed by file identity, content signature, extractor version, and metadata epoch.
- Implement failure quarantine for repeatedly crashing or timing-out extractors.

## Storage Engine

- Implement mmap-backed immutable archive readers for records, dictionaries, metadata postings, and content postings.
- Implement dictionary compression for terms, paths, extensions, tags, kinds, metadata keys, and repeated path prefixes.
- Implement block-level compression policy with fast random access and bounded decompression windows.
- Implement large-index merge policy across hot buffers, immutable segments, compacted tiers, and tombstone cleanup.
- Implement record column stores for high-cardinality fields and cache-friendly scan/rank passes.
- Implement prefix/fuzzy lookup structures suitable for machine-wide scale.
- Implement checksums, schema migration, crash recovery, corruption detection, and rebuild plans.
- Implement index size telemetry and compaction scheduling heuristics.
- Implement benchmark fixtures for millions of files and realistic developer, media, documents, and iCloud trees.

## File Operations

- Implement APFS clone fast paths using platform-native clone semantics.
- Implement copyfile/Finder-compatible metadata preservation, xattrs, ACLs, resource forks, quarantine attributes, package behavior, and symlink policies.
- Implement operation pause, resume, cancellation, retry, and crash recovery replay.
- Implement progress accounting for recursive operations before and during execution.
- Implement conflict UI/state machine for replace, keep both, merge folders, skip, apply to all, and per-item decisions.
- Implement Trash restore metadata and restore operation.
- Implement privileged-operation flow for protected paths.
- Implement network-volume fallbacks and slow-volume throttling.
- Implement post-operation verification policy for high-risk moves/copies.

## Jobs And Runtime

- Implement durable job payload catalog for all operation, indexing, extraction, thumbnail, preview, and repair jobs.
- Implement job dependency graph and fairness between foreground, visible, background, maintenance, and repair queues.
- Implement persistent progress snapshots and user-visible progress restoration after restart.
- Implement thermal, battery, IO pressure, and user-activity adaptive scheduling.
- Implement per-volume concurrency limits and operation isolation.
- Implement structured cancellation propagation across nested jobs and subprocess extractors.
- Implement retry backoff with classified transient, permission, missing-file, corrupt-file, and offline-volume failures.

## Preview And Thumbnails

- Build preview cache with memory and disk tiers.
- Implement icon, thumbnail, and Quick Look request coalescing.
- Implement visible-window prioritization and cancellation for offscreen preview work.
- Implement Finder-compatible generic icons, custom icons, app icons, folder icons, package icons, aliases, symlinks, tags, iCloud badges, and volume badges.
- Implement preview security policy for untrusted files.
- Implement thumbnail invalidation on content, metadata, tag, and iCloud state changes.

## Configuration

- Define target macOS version matrix and supported hardware profiles.
- Implement config crate for parity profiles, user settings, feature flags, and diagnostics toggles.
- Implement persistent settings storage with schema versioning and migration.
- Implement hidden/internal performance controls without exposing non-Finder UI by default.
- Implement operator diagnostics commands for index rebuild, trace export, parity baseline selection, and storage inspection.

## Telemetry And Performance

- Implement latency histograms for navigation, selection, rename, search keystrokes, result streaming, thumbnail display, preview open, copy start, cancel, and window render.
- Implement frame timing and UI-thread stall detection.
- Implement IO, CPU, memory, allocation, queue-depth, and compaction telemetry.
- Implement local-only diagnostics export with privacy review.
- Define hard budgets for p50, p95, p99, cold start, warm start, first result, full result, directory open, and visible thumbnail completion.
- Build repeatable macrobenchmarks against small, medium, huge, developer, media, iCloud, external, and network-volume trees.
- Add regression gates that fail on latency, memory, index size, or frame-time drift.

## Packaging

- Build signed `.app` bundle with icons, entitlements, Info.plist, launch services registration, and document associations.
- Implement hardened runtime settings.
- Implement notarization pipeline.
- Implement first-run permission onboarding that remains Finder-parity by default.
- Implement update, rollback, crash-report, and diagnostics policy.
- Implement release artifact validation on clean macOS machines.

## Documentation

- Expand `PLAN.md` when architectural decisions change materially.
- Keep `README.md` written as the completed product contract.
- Keep this file limited to unfinished work only.
- Add internal architecture docs for storage format, search ranking, operation recovery, macOS bridges, parity harness, and performance budgets.
