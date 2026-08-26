# Agent 2 Remaining Work

Date: 2026-08-26

This is an undone-only handoff. Remove an item only when the whole item is implemented in production code, verified at the stated scope, committed, pushed to `origin/main`, and followed by a clean `git status --branch --short`.

GFM is macOS-only. It is a native Rust + GPUI Finder-parity file manager with GFM-owned performance-critical internals. Do not build cross-platform abstractions. Do not build a product CLI. Existing `gfm <command>` routes are internal operator/test harness surfaces only.

## Done Bar

1. A claim is done only when the production path exists and is wired into the downstream system that depends on it.
2. Tests must cover the exact claim being closed: pure mapping tests for deterministic policy, crate tests for domain behavior, binary/operator tests for public internal routes, and Fozzy for deterministic host scenarios.
3. Host-backed macOS work must report unsupported, unavailable, denied, offline, and missing states honestly. Do not convert those states into ordinary success.
4. Pixel-parity work is not done from a token, layout constant, or hand-built fixture alone. It requires captured Finder and captured GFM artifacts for the same macOS build/profile, a strict diff, and reviewed baseline provenance.
5. UI-plumbing work is not done until the GPUI surface consumes the typed production state and has tests or captured artifacts proving the state appears in the right Finder-matched surface.
6. Performance work is not done until it has measured latency, memory, cancellation, backpressure, and failure-path evidence for the relevant hot path.
7. Keep `STATUS.md` as the source of truth for global unfinished work. Do not shrink it unless an entire numbered status item is production-complete.

## FileProvider And iCloud Remaining Work

1. Implement direct `NSFileProviderManager` or FileProvider-domain enumeration in the macOS bridge so provider/domain identity is not inferred only from URL resource keys, path shape, xattrs, or fixture names.
2. Add a typed FileProvider domain/manager report that can distinguish iCloud Drive, third-party FileProvider domains, unavailable provider APIs, missing domains, permission-denied paths, and unsupported macOS hosts.
3. Wire the provider/domain report into `FileProviderStateReport` without losing the existing stable TSV behavior for operator routes.
4. Harden materialized-placeholder detection beyond filename and xattr heuristics. Use native resource values and FileProvider domain truth where available, and return a typed unknown/unsupported result when host data is not sufficient.
5. Replace the current FileProvider state-transition report with a live invalidation source that is driven by native provider callbacks or an explicitly owned background observer, then feed those events into icon, preview memory cache, preview disk cache, sidebar rows, and search metadata.
6. Add captured Finder pixel baselines for FileProvider/iCloud icon badges, sidebar rows, progress states, conflict states, and unavailable/offline states.
7. Verify FileProvider work with focused `gfm-mac-sys`, `gfm-mac`, `gfm-preview`, `gfm-ui`, `gfm`, and Fozzy coverage. Leave `STATUS.md` item 28 in place until every FileProvider sub-capability in that item is complete.

## DiskArbitration And Volume Remaining Work

1. Replace remaining marker/path-derived volume classification with direct DiskArbitration, URL resource, mount table, and APFS/container metadata where available.
2. Implement a long-lived DiskArbitration session owned by the macOS/platform layer, with explicit lifecycle, callback threading, cancellation, and teardown behavior.
3. Add native eject, unmount, and mount operations with typed disposition, refusal reasons, permission failures, busy-volume failures, and user-cancelled outcomes.
4. Extend the volume descriptor to include APFS container identity, volume role, case sensitivity, read-only state, network reachability, removable media truth, stable identity, and unavailable API states.
5. Feed real volume descriptors into sidebar location rows, operation copy/chunk fallback policy, and index scheduling invalidation for slow, network, external, offline, and read-only volumes.
6. Add live volume invalidation so sidebar rows, operation policy, and index admission update when mount, unmount, eject, disconnect, reconnect, or reachability changes occur.
7. Add captured Finder pixel baselines for mounted volumes, eject controls, network volumes, offline volumes, disk images, read-only volumes, and volume error sheets.
8. Verify with pure descriptor mapping tests, host-backed operator tests, downstream policy tests, and Fozzy coverage. Leave `STATUS.md` item 29 in place until the full DiskArbitration scope is complete.

## Security, TCC, And Permission Remaining Work

1. Wire the protected-path/security-scoped access contract into the GPUI first-run and just-in-time permission surfaces with Finder-matched sheet presentation.
2. Implement prompt orchestration that separates Full Disk Access guidance, security-scoped bookmark acquisition, denied paths, promptable user-selected locations, and non-promptable failures.
3. Ensure index workers, preview workers, thumbnail workers, extraction workers, and file operations all enforce the same typed permission contract before touching protected paths.
4. Add a durable permission-state invalidation path so UI, workers, and operation preflight update when access is granted, denied, revoked, stale, repaired, or unavailable.
5. Add Finder-parity captured baselines for first-run permission guidance, protected-path denial, bookmark acquisition, operation permission sheets, and Full Disk Access guidance.
6. Verify with deterministic security-policy tests, binary/operator tests that do not trigger unwanted prompts, operation preflight tests, worker admission tests, GPUI contract tests, and Fozzy coverage. Leave `STATUS.md` items 30, 41, and 50 in place until the full UI and worker-enforcement scope is complete.

## Finder Pixel-Parity Harness Remaining Work

1. Implement real Finder screenshot capture for each target macOS build/profile, including appearance, scale factor, color profile, focus state, view mode, window size, fixture root, and surface metadata.
2. Implement deterministic GFM screenshot capture for the identical fixture matrix and profile metadata.
3. Persist baseline artifacts with provenance: macOS build, hardware/display profile, app version, fixture manifest, capture command, timestamp, reviewer, and approved mask set.
4. Fail CI on every unapproved Finder drift for layout, text, icons, toolbar, sidebar, selection, focus, hover, thumbnail, preview, sheet, and menu regions.
5. Add baseline update review bundles containing Finder screenshot, GFM screenshot, visual diff, first unmasked mismatch, per-region summaries, mask justifications, and signer/reviewer metadata.
6. Enforce per-build mask files with tight rectangles and durable reasons. Masks are allowed only for unavoidable OS-owned dynamic pixels, never for GFM-owned layout/text/icon drift.
7. Add tests proving stale baselines, mismatched macOS profiles, missing provenance, empty mask reasons, loose masks, and unapproved drift all fail.
8. Verify with crate tests, binary parity-gate tests, generated review artifacts, and Fozzy scenario coverage. Leave `STATUS.md` items 7 through 14 in place until the whole capture/baseline/diff/CI/review workflow is complete.

## Performance-Critical Work Agent 2 Should Prefer

1. Prioritize code that removes latency from hot paths: provider state reads, volume classification, permission preflight, sidebar invalidation, preview/thumbnail admission, and operation scheduling.
2. Do not spend time on cosmetic docs or broad refactors unless they unlock one of the latency-sensitive production paths above.
3. Keep UI render/update paths free of disk, network, provider, TCC, DiskArbitration, Quick Look, and indexing work. Route those through typed background contracts with cancellation and backpressure.
4. When introducing caches, define key ownership, invalidation source, memory budget, disk budget, eviction order, and corruption behavior in code and tests.
5. When touching macOS bridges, keep unsafe Objective-C/CoreFoundation ownership isolated in `crates/mac-sys`, expose typed safe contracts through `crates/mac`, and test unsupported host behavior explicitly.

## Required Verification

1. Run focused Rust verification for every crate touched:

   ```sh
   cargo fmt --all -- --check
   cargo test -p <changed-crate> <focused-filter> -- --nocapture
   cargo test -p gfm --test platform <focused-filter> -- --nocapture
   cargo clippy -p <changed-crate> --tests -- -D warnings
   git diff --check
   ```

2. Run deterministic host/system verification when an operator route, app path, macOS bridge, or cross-crate policy changes:

   ```sh
   fozzy doctor --deep --scenario tests/scenarios/gfm-cli-host.fozzy.json --runs 5 --seed 424242 --json
   fozzy test --det --strict-verify tests/scenarios/gfm-cli-host.fozzy.json --json
   fozzy run tests/scenarios/gfm-cli-host.fozzy.json --det --record /tmp/gfm-cli-host.trace.fozzy --proc-backend host --fs-backend host --http-backend host --json
   fozzy trace verify /tmp/gfm-cli-host.trace.fozzy --strict --json
   fozzy replay /tmp/gfm-cli-host.trace.fozzy --json
   fozzy ci /tmp/gfm-cli-host.trace.fozzy --json
   fz doctor project . --strict
   fz audit unsafe .
   ```

3. Clean verifier artifacts before committing unless the artifact is an intentional source artifact:

   ```sh
   if [ -d .fz ]; then /bin/rm -r .fz; fi
   if [ -d .fozzy ]; then /bin/rm -r .fozzy; fi
   if [ -e /tmp/gfm-cli-host.trace.fozzy ]; then /bin/rm /tmp/gfm-cli-host.trace.fozzy; fi
   if [ -e /tmp/gfm-cli-host.trace.1.fozzy ]; then /bin/rm /tmp/gfm-cli-host.trace.1.fozzy; fi
   ```

4. Before removing anything from this file or `STATUS.md`, cite the exact verification evidence in the commit body or PR notes: commands, changed crates, host assumptions, and why the evidence covers the full item.

## Commit Rules

1. Keep each commit scoped to one production claim.
2. Do not mix FileProvider, DiskArbitration, Security/TCC, and parity-capture work in one broad commit.
3. Do not update `README.md` unless a real command, contract, or product behavior changed.
4. Do not update `PLAN.md` unless an architectural decision changed.
5. Do not shrink `STATUS.md` unless a full numbered item is complete.
6. Push every verified pass to `origin/main`.
7. Leave the tree clean after push.
