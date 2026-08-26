# Agent 2 Remaining Work

Date: 2026-08-26

This is an undone-only handoff list. Do not use it to record completed work. When an item is fully implemented, verified at the stated scope, merged, and pushed, remove it from this file.

GFM is macOS-only. It is a native Rust + GPUI file manager whose default UI must match Finder byte-for-byte for the supported macOS build/profile while replacing Finder internals with lower-latency, deterministic, recoverable systems. Do not build a web app. Do not build a product CLI. Existing `gfm <command>` paths are internal operator/test harness routes.

## Verification Rule

Do not mark anything done from code shape alone. Done requires current evidence:

1. The implementation exists in production code, not only tests, fixtures, docs, or an isolated demo path.
2. The implementation is wired into the downstream product path that users or production workers depend on.
3. The relevant unit, integration, binary/operator, and deterministic scenario tests pass.
4. The verification scope matches the claim. A narrow test cannot close a broad status item.
5. Generated artifacts are reviewed or intentionally retained, then cleaned before commit if they are not source artifacts.
6. The commit is pushed to `origin/main`.
7. `git status --branch --short` is clean after push.

If any evidence is missing, leave the item on this list.

## Primary Assignment: macOS FileProvider And iCloud State

1. Inspect the existing FileProvider, iCloud, placeholder, badge, sidebar, preview, index, and operation policy code.

   Start with:

   ```sh
   rg -n "FileProvider|iCloud|cloud|placeholder|materialize|evict|download|sync|provider|badge|sidebar|preview policy|thumbnail|index policy|operation policy" crates/mac crates/fs crates/preview crates/index crates/ops crates/ui crates/app/src crates/app/tests STATUS.md PLAN.md README.md
   ```

2. Add a typed FileProvider state contract in `crates/mac`.

   Required states:

   1. local materialized
   2. remote placeholder
   3. downloading or materializing
   4. uploading or syncing
   5. offline or unavailable
   6. evictable local file
   7. provider conflict
   8. provider error
   9. unsupported host or unavailable API
   10. inaccessible path or permission denied

3. Implement a safe path-based API that returns the typed FileProvider outcome for a real filesystem path.

   Requirements:

   1. Keep Objective-C, CoreFoundation, and unsafe ownership at the narrow macOS bridge boundary.
   2. Make thread-affinity requirements explicit.
   3. Make host-version support explicit.
   4. Do not silently coerce unsupported or denied states into ordinary local-file success.
   5. Do not make Spotlight the source of truth for FileProvider state.

4. Add deterministic pure mapping tests for every typed FileProvider state.

5. Add a real app/operator route that prints stable TSV for one path.

   Suggested route:

   ```sh
   cargo run -p gfm -- file-provider-state <path>
   ```

   The route is internal diagnostics only. Keep it thin; platform logic belongs in `crates/mac`.

6. Add a binary test in `crates/app/tests/platform.rs` for the route.

   The test must prove stable formatting and explicit unsupported/error handling. If the host cannot provide live FileProvider state in CI, test the unsupported outcome honestly rather than faking success.

7. Wire FileProvider state into preview/thumbnail scheduling policy.

   Required behavior:

   1. Remote placeholders must not be treated as ordinary local bytes.
   2. Offline/unavailable provider state must produce a typed skip/defer/fail policy.
   3. Downloading/syncing state must be visible to scheduling priority and publication policy.
   4. Foreground preview requests must remain cancellable and must not block the UI render/update path.

8. Wire FileProvider state into icon or sidebar badge intent.

   Required badge intents:

   1. cloud-only
   2. downloading
   3. syncing
   4. unavailable
   5. conflict

   Use typed values. Do not duplicate string parsing outside the macOS/platform layer.

9. Add tests proving preview/thumbnail policy and badge policy for every important FileProvider state.

10. Add Fozzy coverage for the operator route if the route is deterministic on the host.

11. Do not remove `STATUS.md` item 28 unless the full item is complete: direct FileProvider.framework/NSFileProviderManager state reads, native download/evict operations, provider progress callbacks, placeholder detection, conflict-resolution UI plumbing, sidebar/icon badge propagation, live invalidation, and captured Finder pixel baselines.

## Secondary Assignment: DiskArbitration Volume Truth

1. Inspect current volume descriptor, copy policy, sidebar, and index scheduling code.

   Start with:

   ```sh
   rg -n "DiskArbitration|volume|mount|unmount|eject|APFS|network|external|removable|readonly|read-only|case-sensitive|slow volume|copy policy|sidebar location|index policy" crates/mac crates/fs crates/ops crates/index crates/ui crates/app/src crates/app/tests STATUS.md PLAN.md README.md
   ```

2. Add or extend a typed volume descriptor that can represent:

   1. local internal APFS
   2. external removable
   3. network volume
   4. read-only
   5. ejectable
   6. unmountable
   7. offline or unreachable
   8. case-sensitive
   9. stable volume identity where available
   10. unsupported host or unavailable API

3. Implement real host-backed volume lookup for a path or mounted volume.

4. Keep DiskArbitration session ownership isolated and documented at the unsafe/platform boundary.

5. Add deterministic descriptor mapping tests independent from host enumeration.

6. Add a stable app/operator route.

   Suggested route:

   ```sh
   cargo run -p gfm -- volume-state <path>
   ```

7. Feed the descriptor into at least two downstream policies:

   1. sidebar location row state
   2. operation copy chunk/fallback policy
   3. index scheduling policy for slow, network, external, or offline volumes

8. Add binary/operator tests and focused downstream policy tests.

9. Do not remove `STATUS.md` item 29 unless the full item is complete: long-lived DiskArbitration callbacks, native eject/unmount/mount operations, APFS/container metadata, network-volume reachability, sidebar propagation, live index policy invalidation, and captured Finder pixel baselines.

## Tertiary Assignment: Security And TCC Readiness

1. Inspect current security-scope, permission, protected-path, first-run onboarding, index admission, preview admission, and operation preflight code.

   Start with:

   ```sh
   rg -n "Security|TCC|Full Disk|security-scoped|bookmark|permission|protected|privacy|onboarding|prompt|denied|operation preflight|index admission|preview admission" crates/mac crates/fs crates/ops crates/index crates/preview crates/ui crates/app/src crates/app/tests STATUS.md PLAN.md README.md
   ```

2. Add or extend a typed protected-path readiness contract.

   Required outcomes:

   1. allowed
   2. requires Full Disk Access
   3. security-scoped access available
   4. promptable
   5. denied
   6. unsupported host or unknown

3. Feed the readiness contract into at least two downstream policies:

   1. first-run permission onboarding
   2. index worker admission
   3. preview worker admission
   4. operation preflight
   5. GPUI permission sheet contract

4. Add deterministic tests for every outcome.

5. Add a stable app/operator route if it can report without triggering unwanted prompts.

6. Do not remove `STATUS.md` item 30 or 50 unless the full GPUI shell, prompt behavior, worker enforcement, and captured Finder baselines are complete.

## Parity Harness Remaining Work

1. Connect the existing parity gate to real Finder screenshot capture.

2. Connect the existing parity gate to deterministic GFM screenshot capture.

3. Add baseline artifact storage keyed by macOS build, appearance, scale factor, color profile, focus state, view mode, fixture root, and surface.

4. Add CI enforcement that fails on every unapproved drift.

5. Add baseline update review artifacts with signer/reviewer metadata.

6. Add per-build mask approval files with durable reason strings and tight rectangles only.

7. Add tests proving stale or mismatched baseline provenance fails.

8. Do not remove `STATUS.md` items 7 through 14 unless the entire capture, baseline, diff, CI, review, and per-build profile workflow is complete.

## Required Verification Commands

Run the focused commands for the crates you touch:

```sh
cargo fmt --all -- --check
cargo test -p gfm-mac <filter> -- --nocapture
cargo test -p gfm-preview <filter> -- --nocapture
cargo test -p gfm-fs <filter> -- --nocapture
cargo test -p gfm-index <filter> -- --nocapture
cargo test -p gfm-ops <filter> -- --nocapture
cargo test -p gfm-ui <filter> -- --nocapture
cargo test -p gfm --test platform <filter> -- --nocapture
cargo clippy -p <changed-crate> --tests -- -D warnings
git diff --check
```

For app/operator/system behavior, also run:

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

Clean verifier artifacts before committing:

```sh
if [ -d .fz ]; then /bin/rm -r .fz; fi
if [ -d .fozzy ]; then /bin/rm -r .fozzy; fi
if [ -e /tmp/gfm-cli-host.trace.fozzy ]; then /bin/rm /tmp/gfm-cli-host.trace.fozzy; fi
if [ -e /tmp/gfm-cli-host.trace.1.fozzy ]; then /bin/rm /tmp/gfm-cli-host.trace.1.fozzy; fi
```

## Commit Rules

1. Keep each commit scoped to one production claim.
2. Do not mix FileProvider, DiskArbitration, Security/TCC, and parity-capture work in one broad commit.
3. Do not update `README.md` unless a real command, contract, or production behavior changed.
4. Do not update `PLAN.md` unless an architectural decision changed.
5. Do not shrink `STATUS.md` unless a full numbered item is production-complete and verified.
6. Push every verified pass to `origin/main`.
7. Leave the tree clean after push.
