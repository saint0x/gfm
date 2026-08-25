# GFM

**GFM** stands for **Good Fucking Manager**.

GFM is a native macOS file manager built in Rust and GPUI. It is designed to be a byte-for-byte Finder UI match on the target macOS build while replacing Finder's slow, opaque internals with a low-latency filesystem engine, compact machine-wide search, explicit operation scheduling, and production-grade recovery.

The point is simple: the familiar macOS file manager surface, engineered the way it should have been engineered.

## Product Standard

GFM is not Finder-inspired. GFM is Finder-compatible.

The default experience matches Finder exactly:

- window chrome
- toolbar layout
- sidebar
- icon view
- list view
- column view
- gallery view
- typography
- spacing
- colors
- materials and vibrancy
- hover, focus, selection, drag, rename, sheet, alert, and context-menu behavior
- keyboard and pointer interactions
- file icons, thumbnails, previews, metadata, tags, labels, and iCloud state

Power features do not contaminate the default surface. Anything beyond Finder parity lives behind explicit modes, commands, or settings.

## Why

Finder is trusted because it is native and familiar. It is also too slow at scale, too dependent on opaque indexing behavior, too casual with long-running operations, and too willing to let filesystem work leak into user-visible stalls.

GFM keeps the trust and replaces the machinery:

- instant directory navigation
- machine-wide filename, path, metadata, and content search
- progressive search results with compact indexes
- APFS-aware copy, clone, move, rename, delete, and Trash behavior
- explicit job queues with pause, resume, retry, cancellation, progress, and crash recovery
- background indexing that never blocks rendering
- durable FSEvents ingestion and dropped-event repair
- native Quick Look, thumbnail, icon, tag, label, comment, package, alias, symlink, volume, iCloud, and permission handling
- deterministic tests, trace replay, pixel parity checks, and measured latency budgets

## Architecture

GFM is a multi-crate Rust workspace with strict ownership boundaries.

- `crates/app`: native GPUI app entrypoint plus internal operator/test harness routing for diagnostics and deterministic verification.
- `crates/ui`: GPUI application startup, production window lifecycle, root surface, titlebar contract, activation, tab grouping, Finder-parity components, visual tokens, layout primitives, virtualized views, and screenshot-test surfaces.
- `crates/mac`: narrow typed bridges to AppKit, Foundation, CoreServices, QuickLook, Spotlight, FSEvents, Security, DiskArbitration, APFS, FileProvider, host support detection, first-run permission readiness, and target matrix policy.
- `crates/fs`: filesystem enumeration, identity, permissions, package detection, aliases, symlinks, hidden files, volume behavior, and metadata reads.
- `crates/ops`: APFS-aware file operations, clone fast paths, volume-classed bounded streaming byte-copy fallback, copy/move/delete/trash semantics, conflict handling, operation journaling, recovery, and retries.
- `crates/index`: initial crawl, FSEvents ingestion, background metadata/content pipelines, per-volume state, and repair scheduling.
- `crates/search`: query parsing, ranking, streaming results, filename/path/content/metadata retrieval, fuzzy matching, snippets, cancellation, supersession, and recency scoring.
- `crates/store`: mmap segment store, dictionaries, compressed postings, appendable content segments, tombstones, merge policy, and compaction.
- `crates/preview`: icons, thumbnails, Quick Look preview policy, memory/disk preview cache, request coalescing, visible-window prioritization, cancellation, security decisions, invalidation, and extraction budgets.
- `crates/jobs`: scheduling, cancellation, prioritization, fairness, progress, volume-isolated worker admission, and backpressure.
- `crates/config`: versioned TOML config, Finder parity profiles, user settings, feature flags, hidden performance controls, diagnostics toggles, validation, and atomic persistence.
- `crates/telemetry`: bounded latency histograms, hard performance budgets, frame timing, UI-thread stall detection, IO/CPU/memory/allocation/queue/compaction summaries, counters, traces, and local-only diagnostics export with privacy review.
- `crates/diagnostics`: operator commands for index rebuilds, privacy-reviewed trace export, parity baseline selection, and persisted storage inspection.
- `crates/testkit`: filesystem fixtures, synthetic trees, repeatable macrobenchmarks, macOS capture harnesses, pixel diffing, and benchmark utilities.
- `crates/packaging`: deterministic macOS `.app` bundle construction, `Info.plist` generation, icon/resource placement, entitlements, ad-hoc or Developer ID signing, hardened-runtime options, Launch Services registration, document associations, release/update/rollback/crash/diagnostics policy, and clean-machine release artifact validation.

No UI render/update path performs blocking filesystem work. No performance-critical search, ranking, scheduling, virtualization, storage, or operation orchestration path is outsourced to a generic black box. Dependencies exist for platform access and standards compliance; GFM owns the contracts.

## Search

Search is designed as a first-class engine, not a text box glued to Spotlight.

GFM indexes:

- file names
- path components
- extensions
- Finder-visible metadata
- tags, labels, comments, dates, sizes, and kinds
- text/code content
- common document content
- recent filesystem changes

The index is compact and incremental:

- appendable per-volume segments
- default recursive indexing skips generated build/cache trees such as `target`, `.git`, `.fozzy`, and `node_modules`
- per-volume query shards with parallel fanout and deterministic global merge ordering
- front-coded dictionaries for terms, paths, shared path prefixes, extensions, tags, kinds, metadata keys, and comment tokens
- delta-coded postings
- tombstones for deletion and replacement
- background compaction
- mmap immutable readers
- mmap-backed index footprint telemetry with adaptive run/throttle/defer compaction scheduling
- hot mutable buffers
- progressive hot/deep result streaming with stable dedupe
- delete-key fuzzy candidate indexes for typo tolerance without full-record scans
- archive-backed prefix and fuzzy lookup with explicit candidate budgets, adaptive prefix cutoffs, lookup cache telemetry, truncation telemetry, and mmap-resident sidecars instead of hydrated session heaps
- exact phrase and `near:N:alpha,beta` positional content retrieval
- bounded snippets with highlighted content matches
- binary-signature and control-byte classification before content extraction
- bounded PDF text-stream extraction with PDF-specific byte, page, and object budgets
- bounded OOXML extraction for DOCX, XLSX, and PPTX with ZIP entry, XML part, and text-output budgets
- bounded HTML, RTF, email, and ZIP archive-metadata extraction policies
- structured JSON, CSV, XML plist, and binary plist extraction for searchable keys, cells, and values
- explicit ranking accumulator for exact, prefix, substring, fuzzy, path, metadata, kind, tag, content, recency, term-frequency, and user-pinned signals
- user-intent boosts for Applications, Recents, Downloads, Desktop, screenshots, and project folders

Queries return immediately from hot state and refine as deeper metadata/content results arrive through explicit stream batches.

## Operations

File operations are explicit jobs with durable state.

GFM supports:

- copy
- APFS clone
- move
- rename
- delete
- Trash
- restore
- duplicate
- archive/extract
- conflict resolution
- skip conflicts
- apply-to-all and per-target conflict decisions
- pause/resume
- retry
- cancellation
- crash recovery
- network-volume fallbacks
- checksummed verification where needed

The operation engine is journaled. A crash, power loss, unmount, permission denial, cancellation, pause, or network failure leaves an inspectable recovery path instead of mystery state. Foreground mutations are preflighted through the macOS security-scope decision contract before the operation engine touches files, so prompt-required or denied protected paths fail as explicit permission outcomes instead of partial mutations. File copies preserve symlink objects, try the native macOS `fclonefileat` path first for regular files, fall back to a GFM-owned bounded streaming byte-copy path for unsupported or cross-device copies, select fallback checkpoint chunk sizes from local, external, network, and slow volume classes, and bind discovered macOS volume descriptors into the operation copy policy before execution. Copy tests report whether the host used the APFS clone fast path or byte-copy fallback, emit chunk-level byte progress and pause/cancel checkpoints during byte-copy fallback, preserve ownership where the host permits it, preserve permissions, access/modified timestamps, and copyable xattrs, then verify copied regular-file output with the configured size or streaming byte policy before reporting success. Recursive copy, move, rename, delete, trash, and restore operations preflight exact item/byte totals, honor cancellation and pause checkpoints during planning and execution, resolve replace, keep-both, merge-folder, skip, apply-to-all, and per-target conflict decisions before journaling the actual terminal outcome, treat Finder packages as atomic items for merge collisions instead of blending bundle internals, record GFM Trash restore metadata, restore trash entries to their metadata-backed original paths, emit completion-backed progress during execution, replay interrupted started operations from the journal with the original operation id, treat skipped operations as durable terminal outcomes, resume paused copy/move/rename operations through idempotent destination verification, and retry classified transient failed operations only under an explicit capped recovery policy.

Runtime workers admit volume-scoped jobs through explicit per-volume limits. Heavy work on one disk, external drive, iCloud subtree, or network mount cannot consume every worker and starve visible work on another volume.

Foreground copy, move, rename, delete, trash, and restore actions enter the operation engine through the same volume-isolated worker admission path while preserving operation journaling and failure records. The command-line routes are an internal operator/test harness, not the user-facing file manager surface.

Interactive live content extraction/search enters through volume-isolated worker admission before crawling, extracting, and producing snippets, so one expensive search on a large volume cannot starve unrelated visible work.

Quick Look preview and thumbnail generation commands also enter through volume-isolated worker admission before producing preview contracts.

Background content indexing persists its `VolumeId` in the durable job spec and resumes through the same isolated, journaled, capped-retry worker path.

The jobs layer also persists a typed payload catalog for operation, indexing, extraction, thumbnail, preview, and repair jobs. Catalog rows carry job id, payload kind, label, payload path, volume id, and a compact summary so recovery and diagnostics can inspect what a job was meant to do without guessing from logs.

The scheduler plans foreground, visible, background, maintenance, and repair work through explicit job classes, dependency edges, and weighted fairness quotas. Foreground and visible work keep first-class latency while background indexing, compaction, and repair queues continue making deterministic progress instead of starving behind endless user-visible churn.

Job progress is persisted through atomic typed snapshots that record job id, class, priority, label, volume id, state, completed units, total units, detail text, and update timestamp. On restart, planned, running, and paused snapshots are restored for user-visible progress surfaces while completed, cancelled, and failed terminal work stays out of the active restoration set.

When `GFM_JOB_PAYLOAD_CATALOG` and `GFM_JOB_PROGRESS_STORE` are configured, shared operation, volume-scoped, adaptive scheduled, and adaptive extraction worker producers publish payload catalog rows and planned/running/terminal progress snapshots directly from the scheduler path. That gives foreground operations, visible preview and repair jobs, adaptive sidecar and persistent-index repair, diagnostics rebuilds, direct and quarantined extraction workers, and thumbnail generation one durable runtime metadata contract.

The retriable background content indexing worker also publishes payload and progress records as it plans, enters retry attempts, and records terminal completion or failure, so machine-wide indexing work can be restored and diagnosed through the same runtime metadata layer.

Adaptive scheduled producers for sidecar repair, persistent-index repair, diagnostics rebuilds, and content maintenance run through the same isolated retriable worker. Transient and offline-volume failures receive bounded backoff and durable attempt journal entries; permission, missing-file, corrupt-file, and permanent failures are terminal. The `jobs-runtime-retry-probe` diagnostic exercises that path deterministically.

Cancellation is structured rather than flat. A parent job token fans out cancellation to children and grandchildren so nested previews, extraction, indexing, and operation subtasks stop quickly, while cancelling one child branch does not poison sibling work or the parent scope.

Job retries classify failures as transient, permission, missing-file, corrupt-file, offline-volume, or permanent before recovery admission. Transient and offline-volume failures receive bounded exponential backoff; permission, missing-file, corrupt-file, and permanent failures are surfaced without retry churn.

Background content indexing also consumes explicit runtime pressure signals: saturated I/O or critical thermal pressure defers the durable job before extraction starts, while elevated pressure, low power, or active user input throttles worker admission.

Background content extraction budgets are derived from file type, size ceilings, volume class, thermal state, battery state, and user activity before extractor reads are admitted.

Resumed background content jobs use the same pressure-aware budget derivation before restored extraction work restarts.

Adaptive direct content search uses the same pressure-aware budget derivation before live extraction and snippet generation.

Adaptive persisted content search uses the same pressure-aware budget derivation before foreground snippet extraction reopens source files.

Adaptive subprocess extraction workers use the same pressure-aware budget derivation before worker-side file reads begin.

Content segment maintenance uses the same adaptive scheduling policy before compaction, manifest promotion, or cleanup publication starts.

Sidecar repair uses the same adaptive scheduling policy before rebuilding derived sidecars or quarantining corrupt sidecar archives.

Persistent index repair uses the same adaptive scheduling policy before rebuilding state, quarantining corrupt record archives, or publishing repaired recovery state.

Diagnostics index rebuild uses the same adaptive scheduling policy before filesystem scans, content extraction, record publication, or content archive publication starts.

Sidecar repair, persistent index repair, and diagnostics index rebuild commands enter through volume-isolated worker admission before scanning, rebuilding, quarantining, or publishing repaired archives.

## UI Parity

GFM treats Finder parity as a testable contract.

Every supported macOS build has a reference matrix covering:

- light and dark appearance
- 1x and 2x display scale
- multiple window sizes
- icon, list, column, and gallery view
- empty, small, medium, and huge directories
- search active/inactive states
- selection, drag, rename, context menu, sheets, alerts, and progress UI
- Desktop, home, Documents, Downloads, Applications, iCloud Drive, external volumes, network mounts, and Trash

CI captures Finder and GFM against the same fixtures and fails on pixel drift. Any mask must be explicit, documented, and forbidden from hiding layout, text, icon, selection, focus, hover, toolbar, thumbnail, or file-content differences.

## Commands

List a directory:

```sh
cargo run -p gfm -- list .
cargo run -p gfm -- package-traversal ~/Desktop opaque
cargo run -p gfm -- finder-metadata ~/Desktop/Report.md
cargo run -p gfm -- app ~/Desktop
cargo run -p gfm -- ui-contract ~/Desktop
cargo run -p gfm -- ui-menu-contract
cargo run -p gfm -- ui-context-menu-contract search-result
cargo run -p gfm -- ui-dialog-contract conflict
cargo run -p gfm -- ui-titlebar-contract ~/Desktop
cargo run -p gfm -- ui-session-contract ~/Desktop
cargo run -p gfm -- ui-toolbar-contract ~/Desktop
cargo run -p gfm -- ui-sidebar-contract ~/Desktop
cargo run -p gfm -- ui-icon-view-contract ~/Desktop 6 4 0
cargo run -p gfm -- ui-virtualization-contract list-rows 250000 32 199990
cargo run -p gfm -- ui-list-view-contract ~/Desktop 12 0
cargo run -p gfm -- ui-column-view-contract ~/Desktop 12 0 Documents
cargo run -p gfm -- ui-gallery-view-contract ~/Desktop 8 0 Screenshot.png
cargo run -p gfm -- ui-search-results-contract ~ "invoice" 12 0
cargo run -p gfm -- ui-trash-view-contract ~/.Trash restore.tsv 12 0
```

Search directly:

```sh
cargo run -p gfm -- search . PLAN
cargo run -p gfm -- search . 'PLAN kind:file ext:md size:>1b'
cargo run -p gfm -- search . 'PLAN modified:>=2026-01-01'
cargo run -p gfm -- search . '(PLAN OR README) NOT draft kind:file'
cargo run -p gfm -- search . 'tag:Important kind:file'
cargo run -p gfm -- search . 'README @desktop'
cargo run -p gfm -- search-content . "Finder parity"
cargo run -p gfm -- search-content . '"Finder parity"'
cargo run -p gfm -- search-content . "near:6:finder,parity"
cargo run -p gfm -- search-content-adaptive . "Finder parity" elevated serious low active
```

Manage the versioned config store:

```sh
cargo run -p gfm -- config-path
cargo run -p gfm -- config-init ~/Library/Application\ Support/GFM/config.toml
cargo run -p gfm -- config-check ~/Library/Application\ Support/GFM/config.toml
cargo run -p gfm -- config-dump ~/Library/Application\ Support/GFM/config.toml
```

Internal performance controls live in the config file but are inert unless both `features.internal_power_mode` and `performance.enabled` are explicitly set. The default Finder-parity surface does not expose them.

Run operator diagnostics:

```sh
cargo run -p gfm -- diagnostics-index-rebuild . /tmp/gfm.gfmidx /tmp/gfm.gfmcontent
cargo run -p gfm -- diagnostics-index-rebuild-adaptive . /tmp/gfm.gfmidx saturated nominal ac idle /tmp/gfm.gfmcontent
cargo run -p gfm -- resume-content-background-adaptive content.job jobs.journal elevated serious low active
cargo run -p gfm -- diagnostics-storage-inspect /tmp/gfm.gfmidx
cargo run -p gfm -- diagnostics-storage-inspect /tmp/gfm.gfmcontent
cargo run -p gfm -- diagnostics-trace-export /tmp/gfm-diagnostics.json
cargo run -p gfm -- diagnostics-parity-baseline ~/Library/Application\ Support/GFM/config.toml tests/parity/baselines 25A354
cargo run -p gfm -- mac-bridges
cargo run -p gfm -- native-icon ~/Desktop/GFM.app
cargo run -p gfm -- fileprovider-state ~/Library/Mobile\ Documents/com~apple~CloudDocs/Report.md
cargo run -p gfm -- volume-discovery
cargo run -p gfm -- volume-index-policy opt-in opt-in opt-in:/Volumes/Work /Volumes/Work /Volumes/TeamShare
cargo run -p gfm -- spotlight-reconcile ~/Desktop/Report.md
```

Check the current host against the supported macOS and hardware matrix:

```sh
cargo run -p gfm -- support-check
```

Inspect first-run permission readiness without forcing Full Disk Access:

```sh
cargo run -p gfm -- permission-onboarding
cargo run -p gfm -- security-scope ~/Documents/Plan.md read
```

Inspect preview security and invalidation policy:

```sh
cargo run -p gfm -- preview-check /tmp/example.app quick-look
cargo run -p gfm -- quicklook-session /tmp/example.pdf
cargo run -p gfm -- thumbnail-generation /tmp/example.png
cargo run -p gfm -- preview-schedule
```

Inspect the validated release policy:

```sh
cargo run -p gfm -- release-policy
```

Run repeatable macrobenchmarks:

```sh
cargo run -p gfm -- macrobench /tmp/gfm-bench smoke
cargo run -p gfm -- macrobench /tmp/gfm-bench standard
cargo run -p gfm -- macrobench-fixture /tmp/gfm-bench million
cargo run -p gfm -- parity-fixture /tmp/gfm-parity smoke
cargo run -p gfm -- pixel-diff expected.rgba actual.rgba 3024 1890 masks.tsv
cargo run -p gfm -- pixel-threshold-check toolbar expected.rgba actual.rgba 3024 1890 masks.tsv
cargo run -p gfm -- parity-gate parity-gate.tsv
cargo run -p gfm -- parity-review parity-gate.tsv /tmp/gfm-parity-review
cargo run -p gfm -- parity-profile 25A354 dark 2x display-p3
cargo run -p gfm -- regression-gate /tmp/gfm-bench smoke
cargo run -p gfm -- large-sidecar-gate /tmp/gfm-bench 1000000
cargo run -p gfm -- diagnostics-index-recovery-plan /tmp/root records.gfmidx state.gfmstate quarantine
cargo run -p gfm -- diagnostics-index-recover /tmp/root records.gfmidx state.gfmstate quarantine
cargo run -p gfm -- diagnostics-index-recover-adaptive /tmp/root records.gfmidx state.gfmstate saturated nominal ac idle quarantine
cargo run -p gfm -- content-manifest-recovery-plan content.gfmmanifest hot:content.gfmcontent
cargo run -p gfm -- content-manifest-recover content.gfmmanifest quarantine hot:content.gfmcontent
cargo run -p gfm -- content-manifest-promotion-recovery-plan content.gfmmanifest
cargo run -p gfm -- content-manifest-promotion-recover content.gfmmanifest
cargo run -p gfm -- sidecar-recovery-plan records.gfmidx columns.gfmcols metadata.gfmmeta prefixes.gfmprefix fuzzy.gfmfuzzy dictionary.gfmdict
cargo run -p gfm -- sidecar-recover records.gfmidx quarantine columns.gfmcols metadata.gfmmeta prefixes.gfmprefix fuzzy.gfmfuzzy dictionary.gfmdict
cargo run -p gfm -- sidecar-recover-adaptive records.gfmidx quarantine saturated nominal ac idle columns.gfmcols metadata.gfmmeta prefixes.gfmprefix fuzzy.gfmfuzzy dictionary.gfmdict
cargo run -p gfm -- archive-schema records records.gfmidx
cargo run -p gfm -- archive-schema prefixes prefixes.gfmprefix
cargo run -p gfm -- archive-rebuild-plan records.gfmidx columns.gfmcols metadata.gfmmeta prefixes.gfmprefix fuzzy.gfmfuzzy dictionary.gfmdict content.gfmcontent content.gfmmanifest hot:content.gfmcontent
cargo run -p gfm -- records-migration-plan records.gfmidx
cargo run -p gfm -- records-migrate records.gfmidx quarantine
cargo run -p gfm -- content-migration-plan content.gfmcontent
cargo run -p gfm -- content-migrate content.gfmcontent quarantine
cargo run -p gfm -- metadata-migration-plan metadata.gfmmeta
cargo run -p gfm -- metadata-migrate metadata.gfmmeta quarantine
cargo run -p gfm -- columns-rebuild-plan records.gfmidx columns.gfmcols
cargo run -p gfm -- columns-rebuild records.gfmidx columns.gfmcols quarantine
cargo run -p gfm -- derived-sidecar-rebuild-plan records.gfmidx prefixes prefixes.gfmprefix
cargo run -p gfm -- derived-sidecar-rebuild records.gfmidx prefixes prefixes.gfmprefix quarantine
```

`macrobench-fixture` materializes real filesystem benchmark trees for developer projects, documents, media, iCloud-shaped files, external-volume-shaped files, network-volume-shaped files, huge directories, and nested trees, then writes a manifest with exact file and directory counts; the `million` scale materializes a one-million-file fixture without running the full benchmark loop.
`regression-gate` materializes benchmark indexes and real prefix/fuzzy sidecar archives, then fails on latency, index-density, prefix lookup, fuzzy lookup, cache-path, and sidecar-truncation drift.
`large-sidecar-gate` synthesizes realistic record distributions, writes real prefix/fuzzy sidecars, verifies bounded repeated lookup behavior at million-entry scale, skips digit-run-heavy tokens in fuzzy sidecars, probes full sidecars with a bounded live record set, and retains `thresholds.tsv` plus `gfm-large-sidecar-history.tsv` artifacts using the `production-macos-million-v1` calibration profile.
`diagnostics-index-rebuild-adaptive` defers filesystem scans, content extraction, record publication, and content archive publication under saturated host pressure.
`diagnostics-index-recovery-plan`, `diagnostics-index-recover`, and `diagnostics-index-recover-adaptive` classify persistent record/state health, rebuild missing or stale state, and quarantine corrupt record archives before rebuilding; the adaptive path defers before mutating recovery state under saturated host pressure.
`content-manifest-recovery-plan` and `content-manifest-recover` classify content manifest health, prune invalid archives, and quarantine corrupt manifests before rebuilding from mmap-validated archives.
`content-manifest-promotion-recovery-plan` and `content-manifest-promotion-recover` complete or clean up interrupted content manifest promotions from the durable promotion journal so compaction cannot strand a valid new content archive behind a stale manifest.
`sidecar-recovery-plan` and `sidecar-recover` validate, quarantine, and rebuild search sidecars from the durable record archive.
`archive-schema` classifies record, column, metadata, prefix, fuzzy, dictionary, content, and content-manifest archives as current, legacy, unsupported, missing, or unreadable while validating known schemas through the production readers used by search and diagnostics.
`archive-rebuild-plan` emits one deterministic preflight over records, columns, metadata, prefixes, fuzzy, dictionary, content, and content-manifest state, selecting the concrete ready/migrate/rebuild/recover/blocked route for each surface before any bytes are mutated.
`records-migration-plan` and `records-migrate` rewrite legacy record archives into the current checksummed schema after preserving a byte backup for operator rollback and forensic inspection.
`content-migration-plan` and `content-migrate` rewrite legacy sequential content archives into the current indexed and checksummed content schema after preserving a byte backup.
`metadata-migration-plan` and `metadata-migrate` rewrite legacy metadata archives into the current checksummed metadata schema after preserving a byte backup.
`columns-rebuild-plan` and `columns-rebuild` preserve legacy, unsupported, or unreadable column bytes and regenerate the current checksummed column mmap archive from the durable record archive.
`derived-sidecar-rebuild-plan` and `derived-sidecar-rebuild` apply the same durable-record rebuild contract to metadata, prefix, fuzzy, dictionary, and column sidecars.

Build, sign, and register the native app bundle:

```sh
cargo build -p gfm --release
cargo run -p gfm -- bundle-app target/release/gfm assets/GFM.icns dist --ad-hoc
cargo run -p gfm -- notarize-app dist/GFM.app dist --keychain-profile gfm-release
cargo run -p gfm -- release-validate dist/GFM.app
cargo run -p gfm -- register-app dist/GFM.app
```

Notarization also accepts Apple ID credentials or App Store Connect API key credentials:

```sh
cargo run -p gfm -- notarize-app dist/GFM.app dist --apple-id you@example.com --team-id TEAMID --password app-specific-password
cargo run -p gfm -- notarize-app dist/GFM.app dist --api-key AuthKey_KEYID.p8 --key-id KEYID --issuer ISSUERID
```

Build and query record indexes:

```sh
cargo run -p gfm -- index . /tmp/gfm.gfmidx
cargo run -p gfm -- index-state . /tmp/gfm.gfmidx /tmp/gfm.gfmstate
cargo run -p gfm -- index-state-inspect /tmp/gfm.gfmstate
cargo run -p gfm -- scan-progress . /tmp/gfm.gfmidx /tmp/gfm.gfmprogress
cargo run -p gfm -- scan-progress-inspect /tmp/gfm.gfmprogress
cargo run -p gfm -- fair-scan . 8 ~/Desktop ~/Documents
cargo run -p gfm -- extract-report ~/Desktop/Report.pdf
cargo run -p gfm -- extract-worker-adaptive ~/Desktop/Report.pdf elevated serious low active
cargo run -p gfm -- extract-worker-cancel-adaptive ~/Desktop/Report.pdf elevated serious low active
cargo run -p gfm -- extract-worker-quarantine-adaptive ~/Desktop/Report.pdf /tmp/gfm.gfmquarantine elevated serious low active
cargo run -p gfm -- extract-cache ~/Desktop/Report.pdf
cargo run -p gfm -- extract-quarantine ~/Desktop/Report.pdf /tmp/gfm.gfmquarantine timeout 2
cargo run -p gfm -- rename-correlation /tmp/OldName.md /tmp/NewName.md
cargo run -p gfm -- metadata-update /tmp/Report.md ' appended bytes'
cargo run -p gfm -- event-backpressure 4096 8 250000 32
cargo run -p gfm -- fsevents-cursor-checkpoint /tmp/gfm.gfmstate /tmp/gfm.gfmcursor 12345
cargo run -p gfm -- fsevents-cursor-resume /tmp/gfm.gfmstate /tmp/gfm.gfmcursor
cargo run -p gfm -- fsevents-repair-schedule /tmp/gfm.gfmstate /tmp/gfm.gfmcursor 12346,12350 kernel-dropped ~/Documents
cargo run -p gfm -- search-index /tmp/gfm.gfmidx PLAN
cargo run -p gfm -- search-index-mmap /tmp/gfm.gfmidx PLAN
cargo run -p gfm -- search-index-columns /tmp/gfm.gfmidx /tmp/gfm.gfmcols PLAN
cargo run -p gfm -- search-index-sidecars /tmp/gfm.gfmidx /tmp/gfm.gfmcols /tmp/gfm.gfmmeta /tmp/gfm.gfmprefix /tmp/gfm.gfmfuzzy /tmp/gfm.gfmcontent PLAN
cargo run -p gfm -- search-index-sidecars-budget /tmp/gfm.gfmidx /tmp/gfm.gfmcols /tmp/gfm.gfmmeta /tmp/gfm.gfmprefix /tmp/gfm.gfmfuzzy /tmp/gfm.gfmcontent 4096 96 512 4096 PLAN
cargo run -p gfm -- index-footprint /tmp/gfm.gfmidx /tmp/gfm.gfmcols /tmp/gfm.gfmmeta /tmp/gfm.gfmprefix /tmp/gfm.gfmfuzzy /tmp/gfm.gfmmanifest /tmp/gfm-*.gfmseg
cargo run -p gfm -- index-compaction-plan /tmp/gfm.gfmidx /tmp/gfm.gfmmanifest elevated serious battery active /tmp/gfm-*.gfmseg
cargo run -p gfm -- archive-schema records /tmp/gfm.gfmidx
cargo run -p gfm -- archive-schema content-manifest /tmp/gfm.gfmmanifest
cargo run -p gfm -- records-migration-plan /tmp/gfm.gfmidx
cargo run -p gfm -- records-migrate /tmp/gfm.gfmidx /tmp/gfm-migration-backups
cargo run -p gfm -- archive-rebuild-plan /tmp/gfm.gfmidx /tmp/gfm.gfmcols /tmp/gfm.gfmmeta /tmp/gfm.gfmprefix /tmp/gfm.gfmfuzzy /tmp/gfm.gfmdict /tmp/gfm.gfmcontent /tmp/gfm.gfmmanifest hot:/tmp/gfm.gfmcontent
cargo run -p gfm -- content-migration-plan /tmp/gfm.gfmcontent
cargo run -p gfm -- content-migrate /tmp/gfm.gfmcontent /tmp/gfm-migration-backups
cargo run -p gfm -- metadata-migration-plan /tmp/gfm.gfmmeta
cargo run -p gfm -- metadata-migrate /tmp/gfm.gfmmeta /tmp/gfm-migration-backups
cargo run -p gfm -- columns-rebuild-plan /tmp/gfm.gfmidx /tmp/gfm.gfmcols
cargo run -p gfm -- columns-rebuild /tmp/gfm.gfmidx /tmp/gfm.gfmcols /tmp/gfm-migration-backups
cargo run -p gfm -- derived-sidecar-rebuild-plan /tmp/gfm.gfmidx prefixes /tmp/gfm.gfmprefix
cargo run -p gfm -- derived-sidecar-rebuild /tmp/gfm.gfmidx prefixes /tmp/gfm.gfmprefix /tmp/gfm-migration-backups
cargo run -p gfm -- records-verify /tmp/gfm.gfmidx
cargo run -p gfm -- index-columns /tmp/gfm.gfmidx /tmp/gfm.gfmcols
cargo run -p gfm -- columns-verify /tmp/gfm.gfmcols
cargo run -p gfm -- columns-lookup /tmp/gfm.gfmcols 1 1
cargo run -p gfm -- index-metadata /tmp/gfm.gfmidx /tmp/gfm.gfmmeta
cargo run -p gfm -- metadata-ids-mmap /tmp/gfm.gfmmeta tag Important
cargo run -p gfm -- metadata-id-block-mmap /tmp/gfm.gfmmeta tag Important 0
cargo run -p gfm -- metadata-verify /tmp/gfm.gfmmeta
cargo run -p gfm -- index-dictionary /tmp/gfm.gfmidx /tmp/gfm.gfmdict
cargo run -p gfm -- dictionary-lookup /tmp/gfm.gfmdict Important
cargo run -p gfm -- dictionary-verify /tmp/gfm.gfmdict
cargo run -p gfm -- index-prefixes /tmp/gfm.gfmidx /tmp/gfm.gfmprefix
cargo run -p gfm -- prefix-ids-mmap /tmp/gfm.gfmprefix Pro
cargo run -p gfm -- prefix-id-block-mmap /tmp/gfm.gfmprefix Pro 0
cargo run -p gfm -- prefix-verify /tmp/gfm.gfmprefix
cargo run -p gfm -- index-fuzzy /tmp/gfm.gfmidx /tmp/gfm.gfmfuzzy
cargo run -p gfm -- fuzzy-terms-mmap /tmp/gfm.gfmfuzzy Pln
cargo run -p gfm -- fuzzy-verify /tmp/gfm.gfmfuzzy
```

Adaptive subprocess extraction workers derive pressure-aware byte budgets before worker-side file reads, run inside a macOS Seatbelt wrapper that denies filesystem mutation when `sandbox-exec` is available, are supervised with timeout-bounded process-group termination, honor the shared jobs-layer cancellation token, and persist worker timeout/crash failures into the extraction quarantine store.

Build and query content indexes:

```sh
cargo run -p gfm -- index-content . /tmp/gfm.gfmidx /tmp/gfm.gfmcontent
cargo run -p gfm -- search-content-index /tmp/gfm.gfmidx /tmp/gfm.gfmcontent "performance-critical"
cargo run -p gfm -- search-content-index-adaptive /tmp/gfm.gfmidx /tmp/gfm.gfmcontent "performance-critical" elevated serious low active
cargo run -p gfm -- search-content-index-set /tmp/gfm.gfmidx "performance-critical" /tmp/gfm-hot.gfmcontent /tmp/gfm-warm.gfmcontent
cargo run -p gfm -- search-content-index /tmp/gfm.gfmidx /tmp/gfm.gfmcontent '"performance-critical systems"'
cargo run -p gfm -- search-content-index /tmp/gfm.gfmidx /tmp/gfm.gfmcontent "near:8:performance,systems"
cargo run -p gfm -- content-ids /tmp/gfm.gfmcontent "performance-critical"
cargo run -p gfm -- content-ids-mmap /tmp/gfm.gfmcontent "performance-critical"
cargo run -p gfm -- content-ids-mmap-set "performance-critical" /tmp/gfm-hot.gfmcontent /tmp/gfm-warm.gfmcontent
cargo run -p gfm -- content-id-block-mmap /tmp/gfm.gfmcontent "performance-critical" 0
cargo run -p gfm -- content-verify /tmp/gfm.gfmcontent
```

Build appendable content segments and compact them:

```sh
cargo run -p gfm -- index-content-segment . /tmp/gfm.gfmseg
cargo run -p gfm -- compact-content /tmp/gfm.gfmcontent /tmp/gfm.gfmseg
cargo run -p gfm -- compact-content-tiered /tmp/gfm.gfmcontent /tmp/gfm-*.gfmseg
cargo run -p gfm -- content-manifest-write /tmp/gfm.gfmmanifest hot:/tmp/gfm-hot.gfmcontent warm:/tmp/gfm-warm.gfmcontent
cargo run -p gfm -- content-maintain-segments /tmp/gfm.gfmmanifest /tmp/gfm-next.gfmcontent /tmp/gfm-*.gfmseg
cargo run -p gfm -- content-manifest-promote /tmp/gfm.gfmmanifest warm:/tmp/gfm-next.gfmcontent /tmp/gfm-hot.gfmcontent
cargo run -p gfm -- content-manifest-promotion-recovery-plan /tmp/gfm.gfmmanifest
cargo run -p gfm -- content-manifest-promotion-recover /tmp/gfm.gfmmanifest
cargo run -p gfm -- content-cleanup-plan /tmp/gfm.gfmmanifest 1 0 64 /tmp/gfm-hot.gfmcontent
cargo run -p gfm -- content-manifest-cleanup /tmp/gfm.gfmmanifest /tmp/gfm-hot.gfmcontent
cargo run -p gfm -- content-maintain-segments-adaptive /tmp/gfm.gfmmanifest /tmp/gfm-next.gfmcontent saturated nominal ac idle /tmp/gfm-*.gfmseg
cargo run -p gfm -- search-content-index-manifest /tmp/gfm.gfmidx /tmp/gfm.gfmmanifest "performance-critical"
```

Run the background content indexing pipeline:

```sh
cargo run -p gfm -- index-content-background . /tmp/gfm-segments /tmp/gfm.gfmidx /tmp/gfm.gfmcontent
cargo run -p gfm -- index-content-background . /tmp/gfm-segments /tmp/gfm.gfmidx /tmp/gfm.gfmcontent saturated nominal ac idle
cargo run -p gfm -- jobs-recover /tmp/gfm-jobs.journal
cargo run -p gfm -- jobs-payload-catalog /tmp/gfm-jobs.gfmjobs
cargo run -p gfm -- jobs-fairness-plan
cargo run -p gfm -- jobs-progress-snapshot /tmp/gfm-jobs.gfmprogress
cargo run -p gfm -- jobs-cancel-tree
```

Run internal operator/test harness file-operation checks:

```sh
cargo run -p gfm -- watch-once .
cargo run -p gfm -- copy ./PLAN.md /tmp/PLAN.copy.md
cargo run -p gfm -- move /tmp/PLAN.copy.md /tmp/PLAN.moved.md
cargo run -p gfm -- trash /tmp/PLAN.moved.md
```

## Verification

Rust gates:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Deterministic Fozzy gates:

```sh
fozzy validate tests/scenarios/gfm-engine.fozzy.json --json
fozzy doctor --deep --scenario tests/scenarios/gfm-engine.fozzy.json --runs 5 --seed 424242 --json
fozzy test tests/scenarios/gfm-engine.fozzy.json --det --strict-verify --json
fozzy validate tests/scenarios/gfm-cli-host.fozzy.json --json
fozzy run tests/scenarios/gfm-cli-host.fozzy.json --proc-backend host --fs-backend host --http-backend host --json
```

Trace evidence:

```sh
trace="artifacts/gfm-engine-$(date +%Y%m%d%H%M%S).trace.fozzy"
fozzy run tests/scenarios/gfm-engine.fozzy.json --det --record "$trace" --json
fozzy trace verify "$trace" --strict --json
fozzy replay "$trace" --json
fozzy ci "$trace" --json
```

Project doctor:

```sh
fz doctor project . --strict
```

## Rule

If a change makes GFM less Finder-compatible, less native, less deterministic, less recoverable, less observable, or slower on a hot path, it is not an improvement.

Good Fucking Manager means the boring details are done properly.
