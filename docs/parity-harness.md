# Parity Harness Architecture

GFM's default UI must match Finder byte-for-byte on the target macOS build. That
requirement cannot live as taste, screenshots in a chat, or manual opinion. It
needs a deterministic capture, diff, mask, review, and CI harness.

This document is the architecture contract for that harness. It intentionally
does not specify UI implementation details; Agent 2 owns current UI work.

## Ownership

`crates/testkit` owns parity fixtures, screenshots, profiles, pixel diffing,
thresholds, gates, review bundles, and macrobench support.

`crates/ui` owns rendering a Finder-parity surface from the same fixture state.

`crates/mac` owns platform metadata needed to make fixtures and rendered state
match Finder: icons, packages, tags, iCloud state, volume metadata, permission
state, localized kind strings, and host profile.

`crates/diagnostics` owns operator-facing routes for baseline selection,
inspection, and review artifacts.

## Ground Truth

Finder is the reference for the default surface. For every supported macOS
build, the harness needs captured Finder baselines for:

- light and dark appearance;
- 1x and 2x display scale;
- relevant color profiles;
- active and inactive window focus;
- canonical window sizes;
- icon, list, column, and gallery views;
- Desktop, home, Documents, Downloads, Applications, iCloud Drive, external
  volumes, network volumes, and Trash;
- empty, small, medium, huge, and mixed-content directories;
- search active and inactive states;
- selection, hover, focus, rename, drag, context menu, sheet, alert, and
  progress states.

The harness should compare Finder and GFM against the same fixture manifest on
the same host profile. Cross-host screenshot comparison is not authoritative
unless profiles match exactly.

## Fixture Contract

Parity fixtures must be deterministic and rich enough to expose Finder behavior:

- ordinary files and folders;
- long names and truncation cases;
- hidden files;
- packages and app bundles;
- aliases and symlinks;
- custom icons;
- tags and label colors;
- Finder comments;
- thumbnails and generic icons;
- iCloud/FileProvider downloaded and evicted states;
- Trash restore metadata;
- external and network volume descriptors;
- permission-denied/protected-scope cases;
- mixed timestamp and size metadata;
- large directories for virtualization and scroll behavior.

Fixture generation should write a manifest with enough provenance for another
host to know whether it can reuse the baseline.

## Capture Contract

`ParityScreenshotCaptureOptions` describes a single capture. It records:

- target app: Finder or GFM;
- fixture root;
- output PNG path;
- provenance TSV path;
- scenario name;
- view mode;
- macOS build;
- hardware profile;
- display profile;
- app version;
- capture timestamp and optional expiry;
- reviewer and signer;
- approved mask set;
- appearance;
- display scale;
- color profile;
- focus state;
- window origin and size;
- GFM app path when capturing GFM.

Capture should produce both image output and provenance. A screenshot without
provenance is not a durable baseline.

## Provenance

Parity provenance must be strict. The current validation requires non-empty:

- macOS build;
- hardware profile;
- display profile;
- app version;
- fixture manifest;
- UTC second-precision capture timestamp;
- capture command;
- reviewer;
- signer;
- approved mask set;
- fixture root.

The approved mask set must match the macOS build. Appearance must be resolved to
light or dark, not system. Window dimensions must be positive. Expiry, when
present, must be a valid UTC second-precision timestamp after capture time.

## Pixel Diff Contract

Pixel diffing operates over exact RGBA buffers. `PixelSize` validates dimensions
and byte length. `PixelDiffReport` records:

- total pixels;
- mismatched pixels;
- unmasked mismatches;
- masked mismatches;
- max channel delta;
- masks;
- region summaries;
- first unmasked mismatch.

A strict Finder parity diff passes only when unmasked mismatches are zero.
Surface thresholds can be evaluated per layout, text, icon, sidebar, selection,
focus, hover, toolbar, thumbnail, preview, sheet, and menu region.

## Mask Governance

Masks are allowed only for OS-owned dynamic pixels that cannot be made stable.
They are not a way to hide imperfect layout, typography, icon, selection,
toolbar, thumbnail, preview, sheet, or menu work.

Every governed mask needs:

- x;
- y;
- width;
- height;
- durable reason.

Mask rectangles must fit inside the captured pixel size. Overlap and excessive
masked mismatch counts should be treated as review signals. A mask set should be
approved per macOS build, not globally.

## Gate Contract

`ParityGateInput` binds a surface, expected Finder PNG, actual GFM PNG, pixel
size, optional mask path, and optional provenance. `run_parity_gate` must:

1. Validate inputs and provenance.
2. Decode Finder and GFM images.
3. Read governed masks when provided.
4. Run strict RGBA diff.
5. Evaluate the surface threshold.
6. Produce a pass/fail report with violations.

The gate should fail closed when inputs are missing, image sizes disagree,
provenance is invalid, masks are malformed, or unmasked drift exceeds the
surface threshold.

## Review Bundles

Human review artifacts should be generated for baseline updates and failures.
The review bundle should include:

- summary report;
- entry manifest;
- violations;
- first mismatch;
- region summaries;
- mask justifications;
- provenance;
- visual diff images;
- copied source artifacts;
- bundle manifest.

Review bundles are not a substitute for CI failure. They exist to make
approvals and debugging fast.

## CI Rules

Parity CI should run captured Finder/GFM comparisons for each supported profile
matrix. A CI pass means:

- fixture manifest matches the baseline;
- macOS build/profile matches the baseline;
- Finder baseline provenance is valid;
- GFM capture provenance is valid;
- masks are governed and approved for the build;
- unmasked mismatch count is zero for strict surfaces;
- threshold checks pass for every surface;
- review artifacts are produced on failure and baseline update paths.

If a target macOS build lacks baselines, CI should report missing coverage rather
than treating parity as unknown-but-passing.

## Interaction Timing

Byte-for-byte static pixels are necessary but not enough. Finder parity also
requires timing and interaction checks for:

- hover transitions;
- focus rings;
- selection changes;
- inline rename field activation;
- context menu presentation;
- sheet presentation;
- drag-image capture;
- scroll behavior;
- progress updates.

Timing profiles belong in parity profiles and deterministic tests. UI code
should not hardcode timing values that cannot be traced back to a captured
profile.

## Non-Goals

The parity harness should not:

- bless visual approximations;
- compare against screenshots from a different macOS build as authoritative;
- hide layout drift with masks;
- depend on raw query text, private paths, or user documents;
- require product users to run diagnostics;
- replace performance budgets or interaction tests.

## Verification

Parity harness changes should prove:

1. Valid manifests parse and invalid manifests fail closed.
2. Provenance rejects empty fields, mismatched mask sets, unresolved appearance,
   invalid timestamps, expired baselines when expiry is enforced, and zero-size
   windows.
3. RGBA diff detects exact mismatch counts.
4. Masked and unmasked mismatch accounting is correct.
5. Governed mask parsing requires reason text.
6. Threshold evaluation fails on unapproved drift.
7. Review bundles include all expected files.
8. Capture planning emits deterministic Finder and GFM command sets.
9. CI routes produce artifacts when parity fails.

The harness exists so the strict UI requirement can survive engineering scale.
If a pixel differs, GFM should either fix it or carry an explicit, reviewed,
build-scoped reason.
