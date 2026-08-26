# Agent 2 Production Plan

Date: 2026-08-26

This document is for the second engineer joining GFM. The project goal is not to build a Finder-inspired demo. The goal is a macOS-only native Rust + GPUI file manager that preserves Finder's familiar surface with strict byte-for-byte UI parity while replacing Finder's opaque and slow internals with a deliberately engineered low-latency filesystem, search, preview, operation, and recovery engine.

GFM means Good Fucking Manager. That is the product bar.

## What You Are Joining

GFM is already structured as a multi-crate Rust workspace. The architectural spine is intentionally split by production ownership:

- `crates/app`: native app entrypoint and internal operator/test harness routes.
- `crates/ui`: GPUI shell, Finder-parity surfaces, view models, visual tokens, and screenshot surfaces.
- `crates/mac`: typed macOS integration contracts and direct platform bridges.
- `crates/fs`: filesystem enumeration, identity, metadata, package detection, aliases, symlinks, hidden files, and permission-aware record building.
- `crates/ops`: file operations, copy/move/delete/trash/recovery, journaling, conflict handling, and verification.
- `crates/index`: filesystem crawling, FSEvents state, indexing policy, content indexing, incremental updates, and repair scheduling.
- `crates/search`: query parsing, candidate generation, ranking, cancellation, sessions, and streaming result behavior.
- `crates/store`: mmap archives, compressed postings, dictionaries, sidecars, manifests, compaction, and recovery.
- `crates/preview`: icons, thumbnails, Quick Look contracts, scheduling, invalidation, and preview cache behavior.
- `crates/jobs`: scheduling, fairness, retries, progress, cancellation, worker admission, and durable job metadata.
- `crates/config`: versioned config and Finder parity profiles.
- `crates/telemetry`: local-only telemetry, latency budgets, histograms, frame timing, traces, and diagnostics exports.
- `crates/diagnostics`: operator-grade recovery, inspection, parity, and storage verification surfaces.
- `crates/testkit`: fixtures, benchmarks, screenshot/pixel tooling, and production test helpers.
- `crates/packaging`: app bundle construction, signing/notarization policy, toolchain validation, release checks, and update policy.

Do not introduce a product CLI. Existing `gfm <command>` routes are internal operator and deterministic-test harness surfaces. The user-facing product is the native macOS app.

## Non-Negotiable Product Requirements

1. Mac only. Do not spend time on Windows, Linux, web, Electron, Tauri, cross-platform abstractions, or browser UI.
2. Rust and GPUI. The native binary and UI shell are part of the product definition.
3. Finder parity is strict. The default UI must be a byte-for-byte Finder match on the supported target macOS build/profile.
4. Better than Finder must be mostly inside the engine, not sprayed across the default surface.
5. Search must be machine-wide, low-latency, compact, cancellable, incremental, and progressive.
6. No blocking filesystem, search, extraction, preview, or operation work on the UI render/update path.
7. Performance-critical internals should be owned by GFM from scratch where the project needs control over layout, latency, cancellation, recovery, ranking, scheduling, compaction, or failure semantics.
8. macOS-native integration is required where users can observe the difference: AppKit, Foundation, LaunchServices, Quick Look, Security/TCC, DiskArbitration, FileProvider, Spotlight metadata reconciliation, FSEvents, icons, tags, FinderInfo, packages, aliases, iCloud placeholders, and volumes.
9. Long-running work must be durable, inspectable, cancellable, retryable where appropriate, and recoverable after crash/restart.
10. `STATUS.md` is a numbered living list of unfinished work only. Remove an item only when the entire numbered capability is implemented and verified at production scope.
11. `README.md` is written as the completed-product contract. Keep it true to the intended production standard.
12. Push cleanly to `origin/main` after each verified pass.

## Current Direction

Agent 1 is currently deep in Jobs/Runtime durability and recovery plumbing:

- durable payload catalog integration
- persistent progress snapshots
- restart planning
- fair scheduling
- adaptive pressure deferral
- real content job spec recovery
- Fozzy deterministic traces
- Apple Metal/GPUI build-path verification

Avoid duplicating that exact lane unless coordinated. Your highest-value offload is to take one of the adjacent large production surfaces and push it forward with the same rigor.

## Recommended Workstream A: Pixel Parity Harness

This is the cleanest offload because it can coalesce with all UI work later and does not collide much with runtime plumbing.

### Goal

Build the production pixel parity harness that can compare Finder and GFM screenshots from the same deterministic fixture matrix, with explicit baseline manifests, RGBA diffing, masks for OS-owned dynamic pixels, and review artifacts.

### Why It Matters

The user explicitly requires byte-for-byte Finder UI parity with zero deviation. That cannot be managed by eyeballing. We need a ruthless, automated, baseline-governed parity loop:

- capture Finder
- render GFM
- diff pixels
- fail on unapproved drift
- publish artifacts humans can review
- keep per-macOS-build profiles separate

Without this, UI parity becomes vibes. That is not acceptable here.

### Authoritative Existing Anchors

Start by inspecting:

- `STATUS.md`, items 7 through 14
- `PLAN.md`, pixel parity and Finder UI sections
- `crates/testkit`
- `crates/config`
- `crates/ui`
- `crates/app` parity-related routes
- any existing pixel diff, threshold, baseline, screenshot, fixture, or manifest code

Use `rg` first:

```sh
rg -n "pixel|parity|baseline|screenshot|rgba|mask|threshold|Finder|fixture" crates tests PLAN.md STATUS.md README.md
```

### Done Looks Like

This workstream is done only when all of these are true:

1. A deterministic Finder/GFM fixture manifest format exists and is versioned.
2. The manifest encodes target macOS build/profile, appearance, scale factor, window size, focus state, view mode, fixture root, and allowed dynamic masks.
3. The harness can ingest captured Finder PNGs and GFM PNGs for the same fixture.
4. RGBA diffing operates on real image bytes, not placeholder text.
5. Diff output includes exact dimensions, changed-pixel counts, max channel delta, per-region summaries, and failure thresholds.
6. Masks are explicit and governed. No broad fuzzy masks.
7. Review artifacts are generated in a stable directory structure.
8. CI/operator route exits nonzero for unapproved drift.
9. Tests cover at least one pass, one fail, one dimension mismatch, and one masked dynamic region.
10. `STATUS.md` items should shrink only if the entire relevant numbered item is finished and verified at the broad scope stated there.

### Quality Bar

- No fake screenshots.
- No tolerance-based hand-waving that would hide real layout drift.
- No "close enough" thresholds for surfaces where byte parity is required.
- No generated SVG approximations of Finder icons as production proof.
- No UI screenshots without manifest provenance.
- No masks without a durable reason string and region.
- No test that only checks that a file was created.

### Suggested Implementation Shape

Prefer a small, explicit module split:

- `crates/testkit/src/parity.rs` for manifest/report orchestration.
- `crates/testkit/src/pixel.rs` for pure RGBA diff logic if not already present.
- `crates/testkit/src/artifact.rs` for review bundle writing if needed.
- `crates/app/src` route only as a thin operator/test harness entrypoint.

Keep pure diffing independent from app/UI/macOS capture so it can be tested fast.

## Recommended Workstream B: Finder UI Calibration Surface

Take this if you are strongest in GPUI and native macOS surface fidelity.

### Goal

Turn the current native shell pieces into a calibrated Finder-parity surface that can be driven by the pixel harness.

### Start Here

Inspect:

- `crates/ui/src`
- `crates/config/src`
- `crates/mac/src`
- `STATUS.md`, items 1 through 6 and 15 through 22
- the attached Finder screenshot from the original request as a human reference, but treat captured baselines as authoritative once the harness exists

Use:

```sh
find crates/ui/src -type f -maxdepth 2 -print | sort
rg -n "toolbar|sidebar|titlebar|icon|list|column|gallery|Finder|parity|token|vibrancy|selection|rename|context" crates/ui/src crates/config/src crates/mac/src
```

### Done Looks Like

For any single UI surface you take, done means:

1. It is implemented in GPUI as a real native app surface.
2. It uses Finder-calibrated tokens/profiles, not invented spacing or colors.
3. It has deterministic screenshot fixtures.
4. It passes the pixel harness for the target profile, or produces explicit failing artifacts documenting remaining drift.
5. Interaction behavior is wired, not just drawn.
6. Accessibility roles/focus behavior are present where applicable.
7. Tests cover state changes, not just construction.

### Quality Bar

- The first viewport must look like Finder, not a redesigned file manager.
- Do not add decorative product flourishes.
- Do not make a marketing page.
- Do not create a "nice but different" design.
- Finder exactness beats personal taste in the default surface.
- Extra GFM power features must be hidden behind explicit modes/settings.

## Recommended Workstream C: macOS Integration Bridges

Take this if you are strongest in Objective-C/runtime, CoreFoundation ownership, and unsafe boundary design.

### Goal

Harden direct macOS bridges so the app can read and act on the same platform truth Finder uses: icons, UTTypes, FileProvider state, volumes, security-scoped access, Quick Look, Spotlight metadata, FSEvents, and FinderInfo.

### Start Here

Inspect:

- `crates/mac`
- `crates/mac-sys`
- `crates/fs`
- `crates/preview`
- `crates/packaging`
- `STATUS.md`, items 23 through 30 and 49 through 50

Use:

```sh
rg -n "AppKit|Foundation|CoreServices|LaunchServices|QuickLook|FileProvider|DiskArbitration|Security|Spotlight|FSEvents|FinderInfo|NS|CF|objc|unsafe" crates/mac crates/mac-sys crates/fs crates/preview crates/packaging
```

### Done Looks Like

Pick one platform bridge and take it to production:

1. The bridge is isolated behind a narrow safe Rust API.
2. All unsafe/CoreFoundation/Objective-C ownership rules are documented near the boundary.
3. Thread-affinity requirements are explicit and enforced.
4. Host-version gates exist where APIs differ by macOS version.
5. Errors map into typed GFM outcomes.
6. Tests use real macOS host behavior where feasible and deterministic fixtures where host state is unstable.
7. The app/operator route proves the bridge with real paths.
8. The bridge feeds downstream UI/search/preview/operation state, not just a standalone report.

### Quality Bar

- No broad unsafe in app/UI code.
- No stringly typed platform state when a typed enum is feasible.
- No silent fallback that hides missing permissions or unsupported host behavior.
- No dependency on Spotlight as the primary search engine.
- No pretending FileProvider/iCloud placeholder state is a normal local file.

## Recommended Workstream D: Search Latency And Memory

Take this if you are strongest in indexing, data structures, memory mapping, ranking, and benchmark-driven tuning.

### Goal

Push the machine-wide search engine toward the user's core demand: instant search across the Mac with compact indexes and deterministic low-latency behavior.

### Start Here

Inspect:

- `crates/search`
- `crates/store`
- `crates/index`
- `crates/content`
- `crates/telemetry`
- `STATUS.md`, items 31 through 36 and search sections in `PLAN.md`/`README.md`

Use:

```sh
rg -n "QuerySession|Sidecar|mmap|posting|candidate|rank|top-k|cancel|budget|telemetry|latency|compact|content|phrase|fuzzy|substring|metadata" crates/search crates/store crates/index crates/content crates/telemetry
```

### Done Looks Like

Pick a measurable hot path and improve it end to end:

1. Identify the target query path and current data flow.
2. Add or use an existing latency budget.
3. Remove avoidable allocations, scans, duplicate hydration, or lock contention.
4. Preserve deterministic ranking.
5. Preserve cancellation and supersession behavior.
6. Add focused tests for correctness and a benchmark/telemetry path for latency.
7. Validate on a materialized fixture large enough to make the improvement meaningful.

### Quality Bar

- No "fast" claims without measurement.
- No unbounded candidate expansion for short or fuzzy queries.
- No full-record scans when an indexed candidate source can bound the work.
- No global locks around search-as-you-type hot paths.
- No cache poisoning panics on foreground search paths.
- No memory-heavy structures that duplicate mmap-resident indexes without a clear budget.

## Recommended Workstream E: File Operations UI Binding

Take this if you are strongest at marrying engine correctness to native UX.

### Goal

Bind operation engine state into Finder-parity GPUI progress surfaces, conflict sheets, pause/resume/stop controls, Trash restore flows, and permission prompts.

### Start Here

Inspect:

- `crates/ops`
- `crates/jobs`
- `crates/ui`
- `crates/mac`
- `crates/app/src/operation.rs`
- `STATUS.md`, items 37 through 42

Use:

```sh
rg -n "copy|move|rename|delete|trash|restore|conflict|journal|pause|resume|progress|permission|security|sheet|operation" crates/ops crates/jobs crates/ui crates/mac crates/app/src
```

### Done Looks Like

1. UI surfaces subscribe to real operation/job progress, not fake local state.
2. Pause/resume/stop actions call the real operation/job control path.
3. Conflict sheets are backed by the actual conflict state machine.
4. Permission prompts are driven by macOS security-scope outcomes.
5. Trash/Put Back flows preserve platform identity and metadata.
6. Failure and cancellation states remain recoverable after restart.
7. Pixel parity harness covers the visible sheets/progress surfaces.

### Quality Bar

- No optimistic UI that says an operation succeeded before durable completion.
- No destructive operation without a recoverable journal/state path.
- No modal/sheet that diverges from Finder order, spacing, focus, or button semantics.
- No conflict logic duplicated in UI.

## Coordination Rules

1. Work from `main`; pull before starting if needed.
2. Keep changes scoped to one production slice.
3. Do not rewrite Agent 1's runtime plumbing unless you coordinate first.
4. Do not remove `STATUS.md` entries unless the whole numbered item is complete and verified.
5. Keep `PLAN.md` and `README.md` aligned only when architectural truth changes.
6. Avoid broad refactors unless they unlock a real production path.
7. Add tests at the layer where the behavior lives.
8. Use deterministic host tests when touching app/operator surfaces.
9. Use Fozzy for scenario/system verification.
10. Push clean commits to `origin/main` after each verified pass.

## Verification Standard

Use the narrowest meaningful Cargo tests first, then broaden.

Expected baseline commands for most Rust code passes:

```sh
cargo fmt --all -- --check
cargo test -p <changed-crate> <focused-filter> -- --nocapture
cargo clippy -p <changed-crate> --tests -- -D warnings
git diff --check
```

For app/operator/system behavior, include:

```sh
cargo test -p gfm --test cli <focused-filter> -- --nocapture
fozzy doctor --deep --scenario tests/scenarios/gfm-cli-host.fozzy.json --runs 5 --seed 424242 --json
fozzy test --det --strict-verify tests/scenarios/gfm-cli-host.fozzy.json --json
fozzy run tests/scenarios/gfm-cli-host.fozzy.json --det --record /tmp/gfm-cli-host.trace.fozzy --proc-backend host --fs-backend host --http-backend host --json
fozzy trace verify /tmp/gfm-cli-host.trace.fozzy --strict --json
fozzy replay /tmp/gfm-cli-host.trace.fozzy --json
fozzy ci /tmp/gfm-cli-host.trace.fozzy --json
fz doctor project . --strict
fz audit unsafe .
```

Clean generated verifier artifacts before committing:

```sh
if [ -d .fz ]; then /bin/rm -r .fz; fi
if [ -d .fozzy ]; then /bin/rm -r .fozzy; fi
if [ -e /tmp/gfm-cli-host.trace.fozzy ]; then /bin/rm /tmp/gfm-cli-host.trace.fozzy; fi
```

Run `cargo clean` after a pushed pass if disk is tight. This workspace has been operating with low free disk, so keep an eye on `df -h .`.

## Merge-Friendly Slice Ideas

These are good first slices because they are valuable, testable, and unlikely to collide with Agent 1:

1. Harden one macOS bridge in `crates/mac` behind a typed safe API.
2. Add Finder custom icon or badge descriptor tests feeding `crates/preview`.
3. Split an oversized file only when the split follows a real ownership boundary and all tests stay green.
4. Wire one GPUI progress/operation surface to real job progress state.

## What Not To Do

- Do not build a web UI.
- Do not add a product CLI.
- Do not create mock business paths and call them architecture.
- Do not depend on Spotlight as the real search engine.
- Do not hide performance work behind generic libraries where GFM needs ownership.
- Do not create placeholder screenshots or fake Finder baselines.
- Do not use "close enough" UI language.
- Do not delete or soften requirements in `STATUS.md`.
- Do not claim an item is complete because a narrow test passed.
- Do not introduce new broad dependencies unless the production tradeoff is explicit.

## Final Definition Of High-Quality Engineering

High-quality GFM work has these properties:

1. It makes the final requested product more true.
2. It is macOS-native where user-observable behavior depends on macOS truth.
3. It is deterministic under test.
4. It has bounded latency, memory, IO, and failure behavior.
5. It is crash/restart aware when it touches long-running work.
6. It keeps UI rendering free of blocking filesystem work.
7. It separates platform bridges, domain logic, persistence, scheduling, and UI.
8. It is measurable when it claims performance.
9. It is pixel-verifiable when it claims Finder parity.
10. It can be understood and extended by another strong engineer without a rewrite.

If you are unsure what to take, take Workstream A: Pixel Parity Harness. It is the leverage point that lets all Finder UI work become objectively shippable.
