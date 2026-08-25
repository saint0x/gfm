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

- `crates/app`: native binary entrypoint, command routing, and operator-facing inspection commands.
- `crates/ui`: GPUI application startup, production window lifecycle, root surface, titlebar contract, activation, tab grouping, Finder-parity components, visual tokens, layout primitives, virtualized views, and screenshot-test surfaces.
- `crates/mac`: narrow typed bridges to AppKit, Foundation, CoreServices, QuickLook, Spotlight, FSEvents, Security, DiskArbitration, APFS, FileProvider, host support detection, first-run permission readiness, and target matrix policy.
- `crates/fs`: filesystem enumeration, identity, permissions, package detection, aliases, symlinks, hidden files, volume behavior, and metadata reads.
- `crates/ops`: APFS-aware file operations, clone fast paths, copy/move/delete/trash semantics, conflict handling, operation journaling, recovery, and retries.
- `crates/index`: initial crawl, FSEvents ingestion, background metadata/content pipelines, per-volume state, and repair scheduling.
- `crates/search`: query parsing, ranking, streaming results, filename/path/content/metadata retrieval, fuzzy matching, snippets, cancellation, supersession, and recency scoring.
- `crates/store`: mmap segment store, dictionaries, compressed postings, appendable content segments, tombstones, merge policy, and compaction.
- `crates/preview`: icons, thumbnails, Quick Look preview policy, memory/disk preview cache, request coalescing, visible-window prioritization, cancellation, security decisions, invalidation, and extraction budgets.
- `crates/jobs`: scheduling, cancellation, prioritization, fairness, progress, and backpressure.
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
- dictionary-compressed terms
- delta-coded postings
- tombstones for deletion and replacement
- background compaction
- mmap immutable readers
- hot mutable buffers
- progressive hot/deep result streaming with stable dedupe
- delete-key fuzzy candidate indexes for typo tolerance without full-record scans
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
- pause/resume
- retry
- cancellation
- crash recovery
- network-volume fallbacks
- checksummed verification where needed

The operation engine is journaled. A crash, power loss, unmount, permission denial, or network failure leaves an inspectable recovery path instead of mystery state.

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
cargo run -p gfm -- diagnostics-storage-inspect /tmp/gfm.gfmidx
cargo run -p gfm -- diagnostics-storage-inspect /tmp/gfm.gfmcontent
cargo run -p gfm -- diagnostics-trace-export /tmp/gfm-diagnostics.json
cargo run -p gfm -- diagnostics-parity-baseline ~/Library/Application\ Support/GFM/config.toml tests/parity/baselines 25A354
```

Check the current host against the supported macOS and hardware matrix:

```sh
cargo run -p gfm -- support-check
```

Inspect first-run permission readiness without forcing Full Disk Access:

```sh
cargo run -p gfm -- permission-onboarding
```

Inspect preview security and invalidation policy:

```sh
cargo run -p gfm -- preview-check /tmp/example.app quick-look
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
cargo run -p gfm -- parity-fixture /tmp/gfm-parity smoke
cargo run -p gfm -- pixel-diff expected.rgba actual.rgba 3024 1890 masks.tsv
cargo run -p gfm -- pixel-threshold-check toolbar expected.rgba actual.rgba 3024 1890 masks.tsv
cargo run -p gfm -- parity-gate parity-gate.tsv
cargo run -p gfm -- parity-review parity-gate.tsv /tmp/gfm-parity-review
cargo run -p gfm -- parity-profile 25A354 dark 2x display-p3
cargo run -p gfm -- regression-gate /tmp/gfm-bench smoke
```

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
cargo run -p gfm -- search-index /tmp/gfm.gfmidx PLAN
```

Build and query content indexes:

```sh
cargo run -p gfm -- index-content . /tmp/gfm.gfmidx /tmp/gfm.gfmcontent
cargo run -p gfm -- search-content-index /tmp/gfm.gfmidx /tmp/gfm.gfmcontent "performance-critical"
cargo run -p gfm -- search-content-index /tmp/gfm.gfmidx /tmp/gfm.gfmcontent '"performance-critical systems"'
cargo run -p gfm -- search-content-index /tmp/gfm.gfmidx /tmp/gfm.gfmcontent "near:8:performance,systems"
cargo run -p gfm -- content-ids /tmp/gfm.gfmcontent "performance-critical"
```

Build appendable content segments and compact them:

```sh
cargo run -p gfm -- index-content-segment . /tmp/gfm.gfmseg
cargo run -p gfm -- compact-content /tmp/gfm.gfmcontent /tmp/gfm.gfmseg
```

Run the background content indexing pipeline:

```sh
cargo run -p gfm -- index-content-background . /tmp/gfm-segments /tmp/gfm.gfmidx /tmp/gfm.gfmcontent
cargo run -p gfm -- jobs-recover /tmp/gfm-jobs.journal
```

Watch and operate on files:

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
