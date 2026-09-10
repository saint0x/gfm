# Operation Recovery Architecture

GFM treats mutating filesystem work as a durable operation stream, not as an
incidental side effect of UI commands. Copy, move, rename, delete, Trash,
restore, and Empty Trash all pass through the `gfm_ops` engine so recovery,
progress, conflict policy, permission decisions, and retries share one contract.

The central rule is strict: after a crash, process kill, power loss, unmount,
permission denial, cancellation, pause, or transient volume failure, GFM must
leave the filesystem and its own state in an inspectable condition with a
deterministic recovery path. Recovery is part of the operation engine, not a
best-effort cleanup pass bolted onto the app shell.

## Ownership

The operation stack is split by responsibility:

- `crates/ops/src/operation.rs` defines the operation model and the access
  requirements each operation needs before mutation.
- `crates/ops/src/context.rs` carries execution policy: conflict behavior,
  journal path, Trash metadata path, cancellation and pause handles, verification
  policy, access gate, volume copy policy, and retry probe state.
- `crates/ops/src/operator.rs` owns lifecycle orchestration: access checks,
  conflict resolution, journal append, planning, execution, pause/cancel/failure
  status mapping, recovery replay, and retry admission.
- `crates/ops/src/journal.rs` owns the append-only journal format and parsing.
- `crates/ops/src/recovery.rs` converts journal history into replay plans.
- `crates/ops/src/plan.rs` computes exact item and byte totals before execution.
- `crates/ops/src/copy.rs`, `relocate.rs`, and `removal.rs` own the mutating
  filesystem algorithms.
- `crates/ops/src/trashmeta.rs` owns GFM Trash restore metadata persistence.
- `crates/app/src/operation.rs` is an internal operator and deterministic test
  harness surface. It is not a product CLI.

The GPUI shell should never duplicate operation semantics. It should submit
typed operations, display progress and recovery state, and resolve user
decisions. The backend remains the source of truth.

## Journal Contract

The operation journal is append-only TSV. Each entry has exactly seven fields:

1. operation id
2. status
3. timestamp in nanoseconds since the Unix epoch
4. operation kind
5. source or target path
6. destination path
7. optional message

Paths and messages are escaped for tabs, newlines, carriage returns, and
backslashes. A malformed journal is a format error because silently skipping
unknown entries can create unsafe recovery behavior.

Supported statuses are:

- `started`
- `completed`
- `skipped`
- `paused`
- `cancelled`
- `failed`

`completed`, `skipped`, `cancelled`, and non-retryable `failed` entries are
terminal for automatic restart recovery. `started`, `paused`, and explicitly
retryable `failed` entries are candidates for recovery.

The same operation id must be reused when replaying an interrupted operation.
That preserves a single durable history for the user's action instead of
creating a misleading second operation.

## Lifecycle

Normal foreground execution follows this order:

1. Check the operation access gate before touching files.
2. Resolve conflict policy into the operation that will actually run.
3. Append `started`.
4. Plan the operation with cancellation checks.
5. Execute the operation with pause and cancellation checkpoints.
6. Append the terminal status.

Access denial or prompt-required protected paths are still journaled as
`started` followed by a terminal error status. This matters because protected
path failures must be visible to diagnostics and recovery review instead of
disappearing as UI-only failures.

Planning happens after journaling because a crash during planning still means a
user-visible operation was accepted by the engine. Planning itself must be
cancellable, and recursive planning must check cancellation between filesystem
walk steps.

## Recovery Selection

Recovery reads the complete journal and folds entries by operation id. For each
id, the latest status, latest operation payload, latest message, count of
`started` attempts, and latest timestamp are retained. Recovery candidates are
then sorted by latest timestamp and id for deterministic replay order.

An operation is recoverable when:

- its last status is `started`; or
- its last status is `paused`; or
- its last status is `failed`, `retry_failed` is enabled, attempts remain, and
  the failure classifier says the message is retryable.

Paused and retryable failed operations append a fresh `started` entry before
resuming. Interrupted `started` operations resume directly under the original
id. Cancelled operations are not automatically resumed because cancellation is
an explicit user intent. Skipped operations are not resumed because the conflict
decision is a durable terminal outcome.

## Idempotent Resume

Replay must be idempotent at operation boundaries. A recovered operation should
validate the destination state before doing additional work, especially for
copy, move, and rename operations whose earlier attempt may have partially
materialized staged files or directories.

Current resume-aware paths use the `resuming` flag passed from `Operator` into
copy and relocate execution. The required behavior is:

- If the destination already matches the intended result, complete without
  duplicating work.
- If a staged destination exists and is valid, continue from that state.
- If a fresh directory or package destination was left by a failed or cancelled
  copy, remove it unless the operation was paused and therefore explicitly
  resumable.
- If replacement staging failed before commit, preserve the existing
  destination.
- If replacement install failed after backup, restore the backup before
  reporting failure.
- If a same-inode replace or hard-link collapse is detected, avoid destructive
  destination deletion.

Resume correctness is more important than optimistic speed, but the common path
must still avoid unnecessary full-file rereads. Size verification is the default
for regular copies; streaming byte comparison remains available for hostile
volumes and diagnostics.

## Copy And Replace Discipline

Copy starts with the fastest safe path:

1. Use native APFS `fclonefileat` for supported regular-file clones.
2. Fall back to bounded GFM-owned streaming byte copy when clone is unsupported
   or cross-device.
3. Preserve sparse holes during byte-copy fallback without turning ordinary dense
   zero-filled files into sparse files.
4. Select checkpoint chunk sizes from volume class: local, external, network, or
   slow.

Replacement never deletes a valid existing destination before the replacement is
ready:

- Regular-file copy replace stages a hidden sibling, verifies it, then commits
  with final rename.
- Symlink replace stages the new symlink before final rename.
- Directory and package replace materializes a complete hidden sibling tree,
  then backs up and installs the replacement with restore-on-failure semantics.
- Move and rename replacement try final rename first and use staged fallback only
  when direct rename is unavailable, such as cross-volume moves.

This discipline is the core difference between an elite file manager and a fast
demo. The user should never lose a correct existing destination because a new
replacement could not be fully produced.

## Metadata Preservation

Copy and move recovery must account for Finder-visible metadata, not only bytes.
The operation layer is responsible for preserving, where the host permits it:

- ownership
- permissions
- access, modified, and birth timestamps
- symlink object timestamps
- copyable extended attributes
- macOS ACLs
- BSD file flags
- recursive hard-link topology

Directory metadata is applied after children are materialized. That avoids
setting final timestamps or flags before recursive writes are finished.

Metadata degradation is not hidden. Degradation is emitted through progress
events so the GPUI progress surface and diagnostics can report what could not be
preserved on the current volume or permission boundary.

## Conflict Outcomes

Conflict policy is resolved before the operation is journaled as the actual
work. This is intentionally durable: if the user selected keep-both and the
engine rewrote the destination to `Report copy.md`, recovery should resume that
specific destination, not rediscover the conflict and produce a new name.

Supported policies are:

- `fail`
- `replace`
- `keep-both`
- `merge`
- `skip`

Directory conflicts may expose merge. File, symlink, and other target conflicts
do not. Skip is a terminal journal outcome. Apply-to-all and per-target choices
are represented by `OperationConflictPlan` so batch execution records the policy
used for each operation outcome.

Finder packages are atomic conflict items. Package merges must not recursively
blend bundle internals unless the package model explicitly allows traversal.

## Trash Recovery

Trash behavior is journaled through the same engine because Trash is still a
destructive operation from the user's point of view.

GFM Trash restore metadata records:

- display name
- original path
- deletion timestamp
- restore capability
- permanent-delete capability
- permission issue text

The metadata file is rewritten atomically through a temporary sibling, flush,
sync, rename, and parent sync. Cancellation checkpoints protect existing
metadata from partial rewrites.

Restore uses metadata-backed original destinations unless the caller provides an
explicit destination. Empty Trash deletes children while preserving the Trash
directory itself. Trash metadata is removed only after permanent deletion
succeeds. If an interrupted Empty Trash run already deleted children before
metadata cleanup, reconciliation removes only entries whose Trash children are
actually gone.

## Permission And Security Scope

Every foreground mutation has explicit access requirements:

- copy, move, rename, and restore require source access plus destination-parent
  access;
- delete, Trash, and Empty Trash require target access.

The access gate returns allow, prompt, or deny decisions before mutation. Prompt
and deny decisions become permission errors with enough reason text for the app
to present a native permission sheet, refresh permission state when necessary,
and retry only after the user has granted access.

Security-scoped bookmark acquisition and persistence live below the app shell's
permission UX, but the operation engine must keep enforcing the typed access
contract. Missing bookmarks must not degrade into partial filesystem mutation.

## Retry Policy

Retry is explicit and capped. The default recovery policy does not retry failed
operations. When retry is enabled, recovery checks:

- how many `started` attempts already exist for the operation id;
- whether attempts remain under `max_attempts`;
- whether the failure message classifies as retryable.

Retryable failures receive bounded backoff from the jobs retry policy. Backoff
sleep is cancellation-aware in small chunks so a user stop request does not wait
behind a long retry delay.

Permission, missing-file, corrupt-file, and permanent failures should surface
without retry churn. Transient and offline-volume failures are the only classes
that should be admitted for automatic retry.

## Progress And Control

Progress is not decorative. It is part of recovery correctness.

Recursive operations preflight exact item and byte totals before mutation.
Execution emits progress as work is completed. Byte-copy fallback emits
chunk-level byte progress and throughput classification. Pause and cancellation
are checked during planning, between recursive steps, and during streaming byte
copy.

Pause maps to `paused`; cancellation maps to `cancelled`. They must never be
collapsed into generic failure because recovery policy treats them differently.

The UI layer may show Finder-compatible Pause, Resume, and Stop controls, but
those controls must bind to the shared `OperationPause` and
`OperationCancellation` contracts rather than introducing UI-local state.

## Startup And Diagnostics

Startup recovery should call `Operator::recover_interrupted` or
`Operator::recover_with_policy` with the runtime journal and Trash metadata
paths. The resulting `OperationRecoveryReport` is the compact diagnostic summary
for recovered operations.

Operator diagnostics may expose recovery through commands such as
`ops-recover`, but these routes exist for deterministic testing, CI, and
operator inspection. They are not part of the user-facing product surface.

Recovery diagnostics should make these states visible:

- journal parse failure;
- no recoverable work;
- started operation replayed;
- paused operation resumed;
- failed operation admitted for retry;
- retry refused because attempts are exhausted;
- retry refused because failure is not retryable;
- replay produced completed, paused, cancelled, or failed status;
- Trash metadata reconciliation changed state.

## Invariants

Production changes to `gfm_ops` must preserve these invariants:

1. No mutation without an access-gate decision.
2. No accepted operation without a `started` journal entry.
3. No terminal outcome without a terminal journal entry when append is possible.
4. No conflict policy rediscovery during replay after a concrete destination was
   chosen.
5. No destination delete before replacement materialization and verification.
6. No recursive directory/package replacement without backup restore on install
   failure.
7. No cancellation reported as generic failure.
8. No pause reported as generic failure.
9. No cancelled operation automatically resumed.
10. No skipped operation automatically resumed.
11. No failed operation retried without explicit capped policy.
12. No Trash metadata removal before the corresponding delete succeeds.
13. No partial Trash metadata rewrite replacing a valid previous metadata file.
14. No full-file verification on the common fast path unless policy requires it.
15. No UI-only operation state that cannot be reconstructed from durable backend
    state.

## Verification

Operation recovery changes require focused Rust tests plus deterministic system
verification. At minimum, test coverage should include:

- journal round trips for every operation kind and status;
- malformed journal rejection;
- recoverable selection for started, paused, cancelled, skipped, failed, and
  retryable failed histories;
- original operation id reuse during replay;
- cancellation during planning;
- pause and cancellation during execution;
- retry exhaustion and retry admission;
- replace staging preserving an existing destination on pre-commit failure;
- directory/package backup restoration on install failure;
- same-inode no-op or hard-link collapse behavior;
- Trash metadata atomicity under cancellation;
- Empty Trash stale metadata reconciliation;
- protected-path prompt and deny outcomes before mutation.

System verification should record and replay deterministic traces for the active
operation scenario using Fozzy strict deterministic mode, then verify the trace
under strict replay. The trace should exercise at least one real operation path,
not only a parse-only diagnostic.

## Open Production Work

This document records the intended operation recovery architecture and the
contracts already visible in the codebase. Remaining production work is tracked
only in `STATUS.md`; completed and verified work should be removed from that
living list rather than duplicated here.
