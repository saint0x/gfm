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

- `crates/app`: native application startup, GPUI composition, windows, menus, and command routing.
- `crates/ui`: Finder-parity components, visual tokens, layout primitives, virtualized views, and screenshot-test surfaces.
- `crates/mac`: narrow typed bridges to AppKit, Foundation, CoreServices, QuickLook, Spotlight, FSEvents, Security, DiskArbitration, APFS, and FileProvider.
- `crates/fs`: filesystem enumeration, identity, permissions, package detection, aliases, symlinks, hidden files, volume behavior, and metadata reads.
- `crates/ops`: APFS-aware file operations, clone fast paths, copy/move/delete/trash semantics, conflict handling, operation journaling, recovery, and retries.
- `crates/index`: initial crawl, FSEvents ingestion, background metadata/content pipelines, per-volume state, and repair scheduling.
- `crates/search`: query parsing, ranking, streaming results, filename/path/content/metadata retrieval, fuzzy matching, cancellation, supersession, and recency scoring.
- `crates/store`: mmap segment store, dictionaries, compressed postings, appendable content segments, tombstones, merge policy, and compaction.
- `crates/preview`: icons, thumbnails, Quick Look previews, preview cache, and extraction budgets.
- `crates/jobs`: scheduling, cancellation, prioritization, fairness, progress, and backpressure.
- `crates/config`: Finder parity profiles, settings, feature flags, and per-macOS-build baselines.
- `crates/telemetry`: latency histograms, counters, traces, and local diagnostics.
- `crates/testkit`: filesystem fixtures, synthetic trees, macOS capture harnesses, pixel diffing, and benchmark utilities.

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
- dictionary-compressed terms
- delta-coded postings
- tombstones for deletion and replacement
- background compaction
- mmap immutable readers
- hot mutable buffers
- progressive result streaming
- ranking that separates exact, prefix, substring, fuzzy, metadata, content, and recency signals

Queries return immediately from hot state and refine as deeper metadata/content results arrive.

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
