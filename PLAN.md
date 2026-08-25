# Native macOS File Manager Plan

Status: initial product and engineering plan  
Date: 2026-08-24  
Stack: Rust, GPUI, macOS native APIs, APFS-aware storage, full-machine search

## Instruction Boundary

The attached Finder screenshot is reference material only. It contains no operative instructions. The user request is the authority:

- Build a macOS native file manager.
- Use Rust and GPUI.
- Produce a native binary.
- Make it elite: low-latency, instantaneous-feeling, high-integrity engineering.
- Search must span the machine and be lightning fast.
- Search index/storage must be compact and engineered seriously.
- The UI must be byte-for-byte a Finder UI match with zero deviation.
- Create this full, detailed `PLAN.md`.

## Non-Negotiable Product Contract

This project is not "Finder-inspired." It is a Finder-compatible native file manager with a strict visual parity requirement and a performance profile materially better than Finder.

### Hard Requirements

1. Finder UI parity is mandatory.
   - Default window chrome, toolbar layout, sidebar, icon grid, list view, column view, gallery view, typography, spacing, hover states, selection states, focus rings, scroll behavior, animation timing, icon sizing, truncation, metadata layout, context menus, sheets, alerts, drag images, and empty states must match Finder on the target macOS build.
   - Any power-user feature must either preserve exact Finder default UI or be hidden behind an explicit mode/setting.
   - The first-run experience must look like Finder, not like a new product trying to explain itself.

2. Performance requirements are mandatory.
   - Directory navigation must feel instantaneous.
   - Search must begin returning relevant results immediately and refine progressively.
   - The UI thread must never block on disk, network, indexing, thumbnail generation, metadata extraction, or permission prompts.
   - Heavy work must be interruptible, backpressured, cached, measured, and observable.

3. Search must cover the machine.
   - Filename search.
   - Path search.
   - Metadata search.
   - Content search for common document/text/code formats.
   - Recent changes must appear quickly after file mutations.
   - Search must degrade gracefully around permissions, protected locations, offline network volumes, iCloud placeholders, corrupt files, and excluded directories.

4. Native macOS behavior is mandatory.
   - Permissions, aliases, symlinks, packages, bundles, tags, labels, comments, hidden files, iCloud status, external volumes, network shares, Quick Look, Trash, copy/move semantics, and Finder-compatible keyboard/mouse gestures must be treated as product-critical behavior.

5. Engineering quality is mandatory.
   - No prototype-only architecture.
   - No fake data paths.
   - No blocking filesystem calls in UI render/update paths.
   - No global mutable state as a coordination substitute.
   - No unmeasured performance claims.
   - Performance-critical systems must be implemented in this codebase from first principles: index formats, query execution, ranking, scheduling, UI virtualization, cache policy, operation orchestration, and hot filesystem paths cannot be outsourced to generic libraries when doing so would cap latency, memory efficiency, control, or observability.
   - External dependencies are acceptable for platform access, standards compliance, cryptography, image/document decoding, and other narrow integration surfaces, but they must sit behind GFM-owned contracts and remain replaceable.

## Research Summary

Research was performed with Aegis CLI where available. The Aegis runtime repeatedly dropped after bounded search calls, so official and primary sources were also checked through browser search to complete the plan.

### Finder And Spotlight Failure Modes

Finder itself is not one bug. It is a bundle of old assumptions:

- It is optimized for broad consumer familiarity, not power-user throughput.
- It tends to expose slow operations as global UI stalls rather than contained background jobs.
- Search depends heavily on Spotlight behavior, whose indexing state can be opaque to users.
- Search semantics can be surprising: "search everywhere," "search this folder," "search name," and "search content" are not always obvious or fast.
- Copy/move operations are often perceived as poorly queued and hard to reason about during concurrent operations.
- Network file operations have a long history of user complaints around reliability and throughput.
- The UI provides several useful views, but each view carries legacy layout constraints and limited power-user controls.

This does not mean Finder is technically incompetent. It means Finder prioritizes conservative platform integration and continuity over radical latency, explicit job control, and inspectable indexing.

### Useful Evidence

- Apple File System Events let applications receive notifications when a directory hierarchy changes. This is the right primitive for keeping an index fresh after the initial crawl. Source: [Apple File System Events](https://developer.apple.com/documentation/coreservices/file_system_events).
- Apple's FSEvents programming guide frames FSEvents as the way to detect modifications without manually rescanning trees. Source: [File System Events Programming Guide](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/Introduction/Introduction.html).
- `NSMetadataQuery` wraps Spotlight metadata queries, but Apple warns that copying the full results proxy can cause performance and memory issues; individual access through count/index APIs is preferred. Source: [NSMetadataQuery](https://developer.apple.com/documentation/foundation/nsmetadataquery).
- APFS supports features directly relevant to an elite file manager: cloning, snapshots, space sharing, fast directory sizing, atomic safe-save, sparse files, copy-on-write design, and I/O coalescing. Source: [About Apple File System](https://developer.apple.com/documentation/foundation/about-apple-file-system) and [APFS Features](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/APFS_Guide/Features/Features.html).
- GPUI is a Rust UI framework from the Zed team. It is hybrid immediate/retained mode and GPU accelerated. Source: [GPUI](https://gpui.rs/) and [Zed GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md).
- Zed's rationale for GPUI emphasizes 120 FPS responsiveness and game-like hardware-accelerated rendering. Source: [Zed blog: Leveraging Rust and the GPU to render user interfaces at 120 FPS](https://zed.dev/blog/videogame).
- Competing macOS file managers show where users feel Finder is weak: dual panes, transfer queues, remote connections, archive browsing, keyboard navigation, previews, sync, customization. Sources: [ForkLift](https://binarynights.com/), [Marta](https://marta.sh/), [Marta docs](https://marta.sh/docs/), [Commander One](https://commander-one.com/), [Path Finder](https://cocoatech.io/).

## Product Thesis

The right abstraction is not "a prettier Finder." It is:

> A Finder-perfect native shell backed by a modern, low-latency file graph, explicit operation scheduler, and compact machine-wide search engine.

Finder parity buys trust. The internal architecture wins speed.

The project should therefore be split into two distinct truths:

- Surface truth: visually and behaviorally indistinguishable from Finder in default mode.
- Engine truth: a new filesystem engine with deterministic scheduling, APFS-aware primitives, indexed search, measured latency budgets, and modern concurrent Rust internals.

## UI Parity Doctrine

### Target Definition

"Byte-for-byte UI match" means the rendered output of our app must match Finder reference captures on the same:

- macOS version and build.
- display scale factor.
- display color profile.
- accent color.
- appearance mode.
- reduce motion / transparency settings.
- language and locale.
- Finder view settings.
- folder contents.
- icon size, grid spacing, text size, sort mode, grouping mode, and sidebar visibility.

Cross-version byte-for-byte parity is not physically stable because Apple can change Finder assets, font rasterization, SF Symbols, vibrancy, spacing, and animation behavior between macOS releases. The plan therefore creates per-macOS-build baselines and requires zero-pixel deviation against the matching baseline.

### Reference Matrix

Capture Finder references for:

- Light mode and dark mode.
- 1x and 2x scale.
- Compact, medium, and large windows.
- Empty folder.
- Small folder under 20 items.
- Medium folder around 500 items.
- Huge folder around 50,000 items.
- Icon view.
- List view.
- Column view.
- Gallery view.
- Sidebar hidden and visible.
- Toolbar compact and expanded.
- Search active and inactive.
- Selection single and multi.
- Dragging, rename edit mode, context menu, file operation progress, error sheets.
- Desktop, home, Documents, Downloads, Applications, iCloud Drive, external volume, SMB/NFS mount, Trash.

### Pixel Acceptance

- Golden screenshots are captured from Finder with automated setup.
- Our app is captured under the same conditions.
- Default acceptance is exact pixel equality for the target build.
- Any nondeterministic native compositor areas must be isolated and either made deterministic or masked only with explicit written justification.
- Masking is not allowed for layout, text, icons, file thumbnails, selection, focus, hover, or toolbar controls.

### Implementation Strategy

1. Reconstruct Finder layout as declarative GPUI components.
2. Use system fonts, system colors, vibrancy/materials, SF Symbols where public APIs provide them, and native icon/thumbnail services.
3. Build a Finder visual token registry:
   - font family, weight, size, line height.
   - row heights.
   - sidebar widths.
   - icon dimensions.
   - grid spacing.
   - toolbar heights.
   - separator colors.
   - selection colors.
   - corner radii.
   - animation durations.
4. Create a capture harness that launches Finder and our app against the same synthetic filesystem fixtures.
5. Fail CI on pixel drift.

### Legal And Distribution Risk

Strict Finder cloning may raise copyright, trade dress, App Store review, and platform policy issues if distributed publicly. Engineering can enforce parity, but release strategy needs legal review. The safer public posture is "system-native compatibility" with public macOS APIs and system-provided assets. The strict byte-for-byte requirement should initially be treated as an internal/personal build target.

## System Architecture

The application should be a multi-crate Rust workspace with a thin native macOS bridge.

```text
gfm/
  Cargo.toml
  crates/
    app/          GPUI app composition, window lifecycle, command routing
    ui/           Finder-parity components, layout tokens, view renderers
    mac/          Objective-C/Swift/CoreServices/AppKit/QuickLook bridges
    fs/           enumeration, stat, permissions, aliases, packages, volumes
    ops/          copy, move, rename, delete, trash, clone, conflict handling
    index/        crawler, FSEvents ingestion, metadata/content pipeline
    search/       query parser, ranking, index readers, streaming results
    store/        mmap segment store, dictionaries, postings, metadata records
    preview/      thumbnails, Quick Look, text previews, icon cache
    jobs/         scheduler, cancellation, progress, prioritization
    config/       settings, feature flags, Finder parity profiles
    telemetry/    metrics, traces, logs, latency histograms
    testkit/      fixtures, synthetic trees, screenshot/pixel utilities
  tests/
    scenarios/
    fixtures/
  scripts/
    capture-finder/
    perf/
    release/
  docs/
    research/
    parity/
    architecture/
```

### Crate Responsibilities

`app`

- Owns GPUI startup, app state composition, command registration, menus, windows, and platform lifecycle.
- Does not perform direct filesystem work.
- Does not know index storage internals.

`ui`

- Owns Finder parity.
- Components: sidebar, toolbar, path/title area, icon grid, list table, column browser, gallery, inspector, popovers, sheets, context menus, operation progress windows.
- Receives view models only.
- Must be screenshot-tested aggressively.

`mac`

- Owns bindings to AppKit, Foundation, CoreServices, QuickLook, Spotlight, FSEvents, Security, DiskArbitration, FileProvider where needed.
- Keeps unsafe and Objective-C interop isolated.
- Public API is narrow, typed Rust.
- Defines and evaluates the target support matrix: primary support for Apple Silicon on macOS 15 or newer, compatibility support down to macOS 14, Intel x86_64 compatibility where hardware budgets are met, an 8 GiB memory floor, and a 4 logical CPU floor.
- Probes the current host through macOS system tools and returns an explicit primary, compatible, or unsupported tier with reasons suitable for release validation and operator diagnostics.
- Probes first-run permission readiness for Finder-relevant protected roots, preserves Finder parity by default by deferring prompts until needed, and emits degraded-mode onboarding decisions for machine-wide search.

`fs`

- Owns filesystem model and enumeration.
- Handles APFS, non-APFS, external volumes, hidden files, package detection, aliases, symlinks, bookmarks, permissions, TCC-denied paths.
- Uses bulk attribute APIs where possible.

`ops`

- Owns mutating operations and user-visible jobs.
- Supports APFS clone fast paths, atomic renames, trash semantics, conflict dialogs, resume/retry, checksums where necessary, and network-volume fallbacks.

`index`

- Owns initial crawl and change ingestion.
- Consumes FSEvents.
- Maintains durable state per volume.
- Schedules metadata and content extraction.

`search`

- Owns query semantics and ranking.
- Streams partial results to UI.
- Separates exact filename/path matching, fuzzy matching, metadata filtering, content matching, and recency scoring.

`store`

- Owns compressed durable index segments.
- Exposes append/merge/read APIs.
- Uses memory-mapped immutable segments plus compact mutable buffers.

`preview`

- Owns icon and thumbnail generation.
- Integrates Quick Look where possible.
- Enforces budgets so thumbnails never block navigation.

`jobs`

- Owns priority queues, cancellation tokens, backpressure, progress accounting, and worker pools.
- UI-visible jobs and invisible background jobs share one scheduler.

`config`

- Owns the versioned TOML configuration contract for Finder parity profiles, user settings, feature flags, diagnostics toggles, and operator-facing config commands.
- Persists settings atomically through a schema-versioned store and migrates older config documents before validation.
- Keeps power and diagnostic controls explicit so the default Finder-parity UI remains uncontaminated.
- Owns hidden internal performance controls for background index threads, extractor threads, thumbnail workers, I/O budget, search-keystroke budget, visible-directory budget, prefetch policy, and mmap read-ahead.
- Derives an effective runtime performance policy that ignores internal controls unless `features.internal_power_mode` and `performance.enabled` are both explicitly set.

`telemetry`

- Owns local-only metrics.
- Records bounded p50/p95/p99 latency histograms for navigation, selection, rename, search keystrokes, result streaming, thumbnail display, preview open, copy start, cancel, and window render.
- Defines hard p50/p95/p99 latency budgets plus cold start, warm start, first result, full result, directory open, and visible thumbnail completion budgets for regression gates.
- Records frame timing histograms and UI-thread stalls against explicit stall thresholds.
- Records IO byte/op counters, CPU sample means and peaks, memory and allocation peaks, queue depth summaries, compaction summaries, cache hit rates, slow paths, index lag, and operation failures.
- Exports local-only aggregate diagnostics atomically and rejects path, query-text, or user-identifier inclusion before writing any artifact.

`diagnostics`

- Owns operator-facing recovery and inspection commands.
- Rebuilds record and content indexes through the production indexing and extraction paths.
- Exports local-only telemetry traces through the privacy-reviewed diagnostics exporter.
- Selects Finder parity baselines by updating the versioned config store atomically.
- Inspects persisted record and content stores by using the same readers as search/runtime code, reporting bytes, record counts, kind counts, hidden/tagged counts, and content term counts.

`testkit`

- Owns deterministic filesystem fixture generation and repeatable macrobenchmarks for small, medium, huge, developer, media, iCloud, external-volume, and network-volume shaped trees.
- Runs macrobenchmarks through the real index build, hot search, streaming search, and content search paths, then evaluates observations against telemetry budgets.
- Provides regression gates that fail on latency budget violations, peak memory drift, index-size density drift, and frame-time/stall drift.

`packaging`

- Owns deterministic macOS `.app` construction for release and host validation.
- Generates `Info.plist` with Finder-compatible document associations for folders and files, bundle metadata, category, icon reference, executable metadata, and minimum macOS version.
- Copies the native binary and icon resources into the canonical app bundle layout.
- Generates signing entitlements as release inputs and signs bundles with ad-hoc or Developer ID identities.
- Enables hardened runtime during signing and verifies the signature after bundle creation.
- Archives signed bundles through `xcrun ditto`, submits notarization through `xcrun notarytool`, waits for Apple acceptance, staples the ticket through `xcrun stapler`, and validates the stapled app.
- Supports keychain-profile, Apple ID, and App Store Connect API key credential modes without storing secrets in project files.
- Exposes Launch Services registration as an explicit operator command so release/install flows can register GFM without hiding host mutation inside validation.
- Owns the release policy contract for update channels, HTTPS update feeds, notarized update staging, bounded rollback retention, local-only crash reports, and privacy-reviewed diagnostics.
- Validates release artifacts on clean macOS hosts by checking bundle layout, Finder document associations, minimum macOS version, executable permissions, code signature, stapled notarization ticket, and Gatekeeper assessment without mutating machine state.

## Data Model

### Core Identity

Every file record needs stable identity beyond path:

- volume UUID.
- file ID / inode where available.
- parent ID.
- canonical path.
- normalized display name.
- raw byte name if representable.
- type: file, directory, package, symlink, alias, app bundle, volume, network item, iCloud placeholder.
- size fields: logical, physical, allocated, directory aggregate if available.
- timestamps: created, modified, changed, accessed, indexed.
- permissions and ownership.
- extended attributes summary.
- tags and Finder comments.
- content hash only when explicitly needed.

Path is a label, not identity. Rename and move should update path edges without invalidating all search references.

### File Graph

Represent the filesystem as a per-volume graph:

- Nodes are file identities.
- Edges are parent-child relationships.
- Paths are materialized from graph edges.
- Deleted nodes are tombstoned until compaction.
- FSEvents update graph edges incrementally.

This avoids treating every rename as delete-plus-create when the platform exposes stable identifiers.

## Search Architecture

### Search Goals

- First keystroke under 30 ms for cached filename/path results.
- Top visible results under 50 ms for warm index.
- Progressive full result stream under 150 ms for common queries.
- Search streams are emitted as stable hot/deep batches: hot name/path/metadata/intent hits first, then deeper content and fuzzy results without duplicate unchanged records.
- Newly created/renamed files visible in search within 250 ms after event ingestion on local APFS volumes.
- Cold start can begin with last durable index immediately while background validation catches up.

### Index Layers

1. Hot name index
   - In-memory or memory-mapped finite-state transducer for filenames and path components.
   - Lowercased, Unicode-normalized, tokenized, and extension-aware.
   - Prefix, substring via n-gram side index, fuzzy via delete-key candidate indexes plus bounded edit-distance verification.

2. Path component index
   - Component-level postings.
   - Path depth and directory affinity ranking.
   - Supports queries like `src index rust`, `~/Downloads pdf`, and `Desktop screenshot`.
   - Executed per volume with deterministic global merge ordering so local, external, network, and iCloud volumes can fan out independently without unstable result ordering.

3. Metadata index
   - Kind, extension, UTI, size buckets, timestamps, tags, comments, author, app-origin metadata where available.
   - Spotlight can be queried as a secondary source, but our app should not depend on Spotlight for core filename/path search.

4. Content index
   - Text/code/Markdown/JSON/XML/CSV/PDF text extraction.
   - Pluggable extractors.
   - Large binary files excluded by default.
   - Chunked content with per-file byte budgets.
   - PDF extraction is policy-bounded separately from plain text, with byte, page, and object caps plus corrupt-file isolation before wider sandboxed extractor coverage lands.
   - DOCX/XLSX/PPTX extraction reads bounded OOXML ZIP packages, selected XML content parts, and capped decoded text output without blocking unrelated content indexing.
   - HTML, RTF, email, and ZIP archive metadata take format-specific extraction paths so markup, transport headers, control words, and archive entry names become searchable without indexing binary payloads.
   - JSON, CSV, XML plist, and binary plist extraction exposes searchable structural keys, cells, primitive values, and plist dictionaries under explicit text-output budgets.
   - Positional postings support exact quoted phrases and explicit `near:N:alpha,beta` proximity windows after durable reload.

5. Recency and usage index
   - Opened, previewed, moved, copied, renamed, searched, selected.
   - Stored locally and privately.
   - Boosts expected results without hiding exact matches.
   - Includes explicit intent boosts for Applications, Recents, Downloads, Desktop, screenshots, and project folders so common Finder-style searches land on the object the user meant.

### Compression Strategy

- Immutable segments written append-only.
- Memory-mapped dictionaries.
- Front-coded sorted term dictionaries.
- Delta-encoded document IDs.
- Roaring bitmaps for dense postings.
- Varint or SIMD-BP128 style integer compression for sparse postings.
- Zstd for cold stored metadata blobs.
- Separate hot/cold tiers:
  - hot: names, paths, top metadata, recent documents.
  - warm: full metadata.
  - cold: content postings, archived tombstones.
- Background segment merging with strict I/O budgets.

### Query Pipeline

1. Parse query into terms, filters, operators, and implied intent.
2. Issue hot name/path query immediately.
3. Stream top results to UI.
4. Merge metadata/content results as available.
5. Re-rank using:
   - exact filename match.
   - prefix match.
   - substring and token-frequency signals.
   - path proximity.
   - extension, tag, metadata filter, and content matches.
   - fuzzy candidate verification quality.
   - recency.
   - file kind.
   - user-pinned boosts.
   - current folder affinity.
   - user usage signals.
   - explicit Finder-style intent boosts for applications, recent items, common folders, screenshots, and project folders.
6. Cancel stale queries on every keystroke.
7. Preserve result stability enough that the UI does not jump violently while typing.

### Spotlight Strategy

Use Spotlight as an integration layer, not the foundation.

- Query Spotlight for metadata/content coverage we do not yet index.
- Use `NSMetadataQuery` carefully, avoiding whole-result snapshots.
- Surface when Spotlight is rebuilding or unavailable only if it materially affects user expectations.
- Never allow Spotlight slowness to block app search UI.

## Filesystem Engine

### Enumeration

The initial crawl should use the fastest safe per-platform path:

- Prefer bulk attribute enumeration where available.
- Avoid per-file metadata calls in the hot loop.
- Batch allocations.
- Avoid building full path strings for every node unless necessary.
- Store interned path components.
- Use bounded workers per volume to avoid destroying interactive I/O.
- Detect and skip cycles, symlink traps, forbidden paths, and package internals according to policy.

### Change Ingestion

- Use FSEvents per mounted volume.
- Maintain event cursor per volume.
- Coalesce event bursts.
- Detect dropped events and schedule bounded subtree repair.
- Prioritize visible folders and recent search-affecting changes.
- Treat network volumes separately because event semantics and latency may differ.

### Operation Scheduler

All operations go through a scheduler:

- copy.
- move.
- rename.
- duplicate.
- trash.
- delete.
- restore.
- tag.
- chmod/chown where supported.
- archive/unarchive.
- remote transfer later.

Scheduler properties:

- Per-volume queues.
- Operation dependencies.
- Priority inheritance from visible UI.
- Cancellation.
- Pause/resume where semantically safe.
- Conflict policy engine.
- Dry-run phase for permission/space/conflict estimation.
- Durable operation journal for crash recovery.

### APFS Fast Paths

- Use clone operations for same-volume duplicate/copy where safe.
- Use atomic rename for same-directory rename and same-volume move where possible.
- Use APFS fast directory sizing when exposed by system APIs.
- Preserve metadata, xattrs, tags, permissions, and timestamps according to Finder-compatible semantics.
- Fall back cleanly on non-APFS volumes.

## Native macOS Integration

### Required APIs And Capabilities

- AppKit/Foundation for system appearance, windows, menus, pasteboard, drag/drop, alerts, sheets, services, accessibility.
- CoreServices FSEvents for change tracking.
- Spotlight/Metadata APIs for optional metadata integration.
- QuickLookThumbnailing / Quick Look for previews.
- DiskArbitration or equivalent for volume mount/unmount awareness.
- Security-scoped bookmarks where sandboxing applies.
- TCC prompts and Full Disk Access guidance.
- NSWorkspace for icons, open-with behavior, file labels, app association.
- Trash APIs or Finder-compatible trash behavior.

### Rust Bridge Policy

- Keep unsafe bridge code isolated in `crates/mac`.
- Prefer `objc2`, `block2`, `core-foundation`, `core-foundation-sys`, `dispatch2`, and narrow C shims only where Rust crates are insufficient.
- Every bridge wrapper must state thread affinity and ownership rules.
- UI-facing APIs return typed Rust values, not raw Objective-C objects.

## GPUI Application Design

### Rendering Philosophy

GPUI gives us a hardware-accelerated surface, but it must be constrained by Finder parity.

- Use GPUI for deterministic custom rendering and low-latency list/grid virtualization.
- Use macOS system rendering concepts for exact colors/materials where possible.
- Avoid novelty UI in default mode.
- Every component is measured against Finder screenshots.

### Core Views

Icon view:

- Virtualized grid.
- Finder-identical icon sizes, text wrapping, truncation, selection, focus, drag behavior.
- Thumbnail/icon pipeline must progressively refine without layout shift.

List view:

- Virtualized table.
- Finder-identical row height, column headers, sort indicators, disclosure triangles, alternating/background behavior if present on target OS.
- Stable sort and grouping.

Column view:

- Horizontally virtualized columns.
- Async child loading.
- Finder-identical preview column behavior.

Gallery view:

- Large preview with filmstrip/list behavior matching Finder.
- Quick Look-backed previews when possible.

Search view:

- Finder-identical search field placement and token behavior.
- Results arrive faster than Finder while the visible shell stays identical.

### Accessibility

- VoiceOver roles should match Finder concepts.
- Keyboard navigation must match Finder.
- Focus order must match Finder.
- Reduced motion and contrast settings must be honored.

## Performance Budgets

### UI

- 120 Hz target where hardware supports it.
- No frame over 8.3 ms on 120 Hz hardware during normal navigation.
- No frame over 16.7 ms on 60 Hz hardware.
- Opening a cached directory: first paint under 16 ms.
- Opening an uncached local APFS directory under 1,000 items: first paint under 50 ms.
- Huge directory: first visible window under 100 ms, then stream.
- Scroll hitch budget: zero visible stalls over 50 ms.

### Search

- Warm filename query: top results under 30 ms.
- Warm mixed query: top results under 50 ms.
- Content query: first partial results under 150 ms.
- Query cancellation: under 5 ms from new keystroke to stale work cancellation signal.
- Index lag after local mutation: target under 250 ms for common operations.

### Indexing

- Initial crawl must be resumable.
- Background indexing should use adaptive I/O budgets.
- Laptop battery and thermal state must reduce background pressure.
- Visible folders and likely user paths get priority:
  - Desktop.
  - Downloads.
  - Documents.
  - Home root.
  - active project directories.
  - mounted working volumes.

### Memory

- UI visible window state must stay small regardless of directory size.
- Index reader should memory-map large immutable segments.
- Hot index target depends on machine size, but the app should remain comfortable under 300 MB resident for normal workloads after indexing.
- Thumbnail cache must be bounded by memory pressure notifications.

## Testing Strategy

### Deterministic Scenario Testing

Per the workspace policy, scenario and regression testing should prefer Fozzy:

- `fozzy doctor --deep --scenario <scenario> --runs 5 --seed <seed> --json`
- `fozzy test --det --strict <scenarios...> --json`
- Record at least one trace:
  - `fozzy run ... --det --record <trace.fozzy> --json`
  - `fozzy trace verify <trace.fozzy> --strict --json`
  - `fozzy replay <trace.fozzy> --json`
  - `fozzy ci <trace.fozzy> --json`

If Fozzy is unavailable in the local environment, record that explicitly and run the nearest deterministic Rust test equivalent until Fozzy is installed.

### UI Parity Tests

- Automated Finder capture harness.
- Automated app capture harness.
- Pixel equality tests per target OS build.
- Component-level geometry assertions.
- Text metrics assertions.
- Accessibility tree diff against expected roles/labels.
- Dark/light mode snapshots.
- View-specific fixtures.

### Filesystem Tests

- Synthetic directory trees from tiny to millions of entries.
- Deep nesting.
- Unicode normalization collisions.
- Case-sensitive and case-insensitive volumes.
- Symlinks and aliases.
- Packages and app bundles.
- Permission-denied trees.
- iCloud placeholders.
- External APFS/HFS/exFAT volumes where available.
- Network mounts where available.
- Concurrent mutation during enumeration.

### Search Tests

- Exact filename.
- Prefix.
- Substring.
- Fuzzy.
- Path component.
- Extension.
- Kind.
- Tags.
- Content.
- Recency ranking.
- Fresh mutation visibility.
- Index crash recovery.
- Segment compaction correctness.
- Corrupt segment recovery.

### Operation Tests

- Same-volume move.
- Cross-volume copy.
- APFS clone duplicate.
- Trash and restore.
- Conflict rename policies.
- Permission failures.
- Network interruption.
- App crash mid-operation.
- Resume after restart.

### Performance Tests

- Microbenchmarks for enumeration, posting-list decode, query parse, ranking, thumbnail cache lookup.
- Macrobenchmarks for opening large directories and searching whole indexes.
- Frame-time traces during scroll, resize, search typing, drag selection, and operation progress.
- I/O pressure tests.
- Memory pressure tests.
- Battery/thermal adaptation tests.

## Milestones

### Phase 0: Reference Capture And Ground Truth

Deliverables:

- Finder screenshot corpus.
- Synthetic filesystem fixture generator.
- Finder UI token database.
- Pixel-diff harness.
- Initial latency benchmark harness.

Exit criteria:

- Can reproduce Finder screenshots for the attached scenario style.
- Can fail a build when a UI token drifts.
- Can measure frame time and input latency locally.

### Phase 1: Native GPUI Shell

Deliverables:

- Rust workspace.
- GPUI window.
- Finder-matching sidebar, toolbar, title/path surface.
- Icon view for current directory.
- Keyboard/mouse basics.
- Native icons via macOS bridge.

Exit criteria:

- Opens a real folder.
- Renders Finder-matching icon view for simple fixtures.
- No blocking filesystem work on UI thread.

### Phase 2: Filesystem Core

Deliverables:

- Volume model.
- Bulk enumeration.
- File identity model.
- Directory cache.
- Permission and hidden-file policy.
- FSEvents watcher.

Exit criteria:

- Large directories stream without UI stalls.
- Visible directory updates after external mutations.
- File identity survives rename/move where possible.

### Phase 3: Finder View Parity

Deliverables:

- Icon, list, column, gallery views.
- Sidebar locations/tags.
- Toolbar modes and search field.
- Context menus and sheets.
- Drag/drop.
- Rename UI.

Exit criteria:

- Zero-pixel-drift snapshots for supported fixture matrix on target OS build.
- Finder keyboard parity for core navigation.

### Phase 4: Search Engine

Deliverables:

- Initial crawl.
- Durable compact index.
- Hot name/path index.
- Metadata index.
- FSEvents-driven incremental updates.
- Streaming search UI.

Exit criteria:

- Warm filename search returns top results under target latency.
- New local files appear quickly.
- Index can recover after app kill.

### Phase 5: Content Indexing

Deliverables:

- Text/code extractors.
- PDF text extraction.
- Document extractor strategy.
- Content postings.
- Query ranking blend.

Exit criteria:

- Content search works without compromising filename search latency.
- Large/binary/corrupt files do not poison indexing.

### Phase 6: File Operations

Deliverables:

- Operation scheduler.
- Copy/move/rename/duplicate/trash.
- APFS clone fast path.
- Progress UI matching Finder.
- Conflict UI matching Finder.
- Durable operation journal.

Exit criteria:

- Operations are correct, recoverable, cancellable, and visibly Finder-compatible.
- Concurrent operations are scheduled intelligently.

### Phase 7: Power Features Behind Parity

Deliverables:

- Optional dual pane.
- Optional command palette.
- Optional queue inspector.
- Optional advanced search syntax.
- Optional archive browsing.
- Optional remote connections.

Exit criteria:

- Default UI remains Finder-identical.
- Power features never alter default screenshots unless explicitly enabled.

### Phase 8: Production Hardening

Deliverables:

- Crash reporting local logs.
- Performance dashboard.
- Full-disk-access onboarding.
- Update strategy.
- Signed/notarized binary.
- Legal/distribution decision.

Exit criteria:

- App is stable as a daily driver.
- All critical performance and parity tests pass.
- Release channel is chosen deliberately.

## Key Technical Bets

### Bet 1: Finder UI Can Be Matched In GPUI

Risk:

- Native macOS vibrancy, font rasterization, symbols, and Finder private metrics may be difficult to reproduce exactly.

Mitigation:

- Use per-build reference capture.
- Use public system fonts/colors/icons.
- Isolate native material rendering.
- Maintain token extraction tools.

### Bet 2: Custom Search Can Beat Finder Without Burning The Machine

Risk:

- Full-machine indexing can become CPU, I/O, privacy, and storage expensive.

Mitigation:

- Hot/warm/cold index tiers.
- Adaptive indexing budgets.
- Explicit exclusions.
- Incremental FSEvents updates.
- Compact immutable segments.
- Local-only telemetry.

### Bet 3: macOS Native Semantics Can Be Correct From Rust

Risk:

- AppKit/Foundation/CoreServices semantics are deep, and Rust bridge mistakes can produce subtle bugs.

Mitigation:

- Keep bridge small.
- Heavily test edge cases.
- Use mature crates where possible.
- Write unsafe code only in isolated modules with ownership/threading notes.

## Open Questions

- Is the first target macOS build Sonoma, Sequoia, Tahoe, or the user's current system only?
- Is this personal-use only, or intended for public distribution?
- Should Full Disk Access be required on first launch, or should the app operate in a degraded mode until granted?
- Should search index content by default, or only filenames/metadata until the user opts into content indexing?
- Should iCloud Drive be indexed aggressively, lazily, or only for downloaded files?
- Should network volumes be indexed persistently, session-only, or never by default?
- Should the app support sandboxing, or is non-App-Store distribution acceptable?

## Immediate Next Steps

1. Initialize the Rust workspace.
2. Pin GPUI version and inspect Zed/GPUI examples.
3. Build the screenshot capture harness first.
4. Build Finder fixture folders matching the attached screenshot style.
5. Implement the minimal GPUI shell with no filesystem engine yet.
6. Implement native icon lookup.
7. Add pixel-diff CI locally.
8. Implement async directory enumeration and visible icon-grid virtualization.
9. Measure before optimizing.
10. Only then begin the full indexer.

## Definition Of Done For The First Real Prototype

The first serious prototype is done when:

- It is a signed native macOS binary.
- It opens to a Finder-matching window.
- It renders the user's Desktop fixture with zero-pixel deviation against the matching Finder baseline on the same OS build.
- It navigates directories without UI thread stalls.
- It has a real async filesystem model.
- It indexes filenames and paths across permitted local volumes.
- Search returns warm top results under the target latency.
- FSEvents keep the index fresh after creates, deletes, renames, and moves.
- It has deterministic tests, screenshot parity tests, and performance traces.
- Every unfinished feature is explicitly outside the prototype scope rather than hidden behind fake code.
