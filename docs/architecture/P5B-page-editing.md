# P5B — Page Manager and logical page editing

Implemented on `p5b/page-editing`. P5B changes the in-memory logical document;
it never writes the source PDF. No Save As, qpdf export, physical rewriting,
duplication, insertion, extraction, splitting, merging, annotations, search,
OCR, bookmarks, or P5C functionality is included. The changes are uncommitted
and have not been pushed or merged.

## Ownership and changed files

The existing path remains WinUI 3 → C++/WinRT bridge → narrow C ABI → persistent
Rust document platform → stable DocumentId/PageId → PagePlan → DocumentLayout
→ continuous viewer → scheduler and bounded caches → virtualized thumbnails.
Rust owns selection, validation, edits, history, current identity, and dirty
state. C++ submits commands and displays bounded snapshots.

| Files | Responsibility |
| --- | --- |
| `crates/document-core/src/editing.rs` (new) | PagePlan mutations, Editor, selection, history, dirty state, deterministic editing tests |
| `crates/document-core/src/lib.rs` | Per-entry editing rotation and PageId-to-position lookup |
| `crates/document-core/src/layout.rs` | Edited order/rotation, geometry retained by PageId, linear layout reconstruction |
| `crates/document-core/src/thumbnails.rs` | Per-page rotation in thumbnail pixel keys |
| `crates/document-core/src/scheduler.rs` | Placement invalidation with CPU cache retention and stale-work tests |
| `crates/document-ffi/src/editing.rs` (new) | Editor status, selection, commands, history configuration, error mapping |
| `crates/document-ffi/src/lib.rs` | ABI v7, authoritative Editor, serialized public operations, immutable source lookup |
| `crates/document-ffi/src/{continuous,thumbnails,viewport}.rs` | Edited layout/demand, source-keyed asynchronous geometry, selection projection |
| `include/pdfeditor_ffi.h` | Plain C editor declarations, status, errors, thumbnail selected flag |
| `native/winui/NativeCoreBridge.{h,cpp}` | Bind ABI v7 editor functions and translate errors |
| `native/winui/ThumbnailPanel.{h,cpp}` | Stable-ID/token selection, modifier reads, selected/current styling |
| `native/winui/MainWindow.xaml`, `MainWindow.xaml.{h,cpp}` | Temporary commands, shortcuts, reading-position recovery, development state |
| `native/winui/DocumentCanvasRenderer.{h,cpp}` | Invalidate displayed placement while retaining GPU textures; per-page rotation matching |
| `scripts/p5b_editing_smoke.py` (new) | Real PDFium integration, editing/cache/generation assertions, source hash checks |
| `scripts/{p0_ffi,p2_tile,p3_viewport,p5a_thumbnail}_smoke.py` | ABI expectation and thumbnail padding-field updates; prior coverage retained |
| `.github/workflows/rust-ci.yml` | P5B branch coverage and mixed/10,000-page editing smoke |
| `README.md`, this report | Usage, architecture, semantics, verification and limitations |

The original DocumentModel remains immutable for backend source-index lookup,
including deleted pages that undo can restore. All active logical navigation
and membership checks use the Editor's PagePlan. Opening does not reconstruct
PDFium state after each edit.

## Selection and current page

Selection is a Rust HashSet of stable PageIds, with a stable PageId anchor.
Current page is a separate active PageId. Thumbnail cards only project this
state; recycling or scrolling them cannot erase selection.

* Plain activation replaces selection with the clicked page, sets its anchor,
  and navigates to it.
* Ctrl activation toggles that page without clearing the others, makes it the
  anchor, and navigates to it even when toggling it off.
* Shift activation replaces selection with the inclusive anchor-to-clicked
  range in the **current logical order**, retains the anchor, and navigates to
  the clicked page. Ctrl+Shift uses Shift semantics. A missing/deleted anchor
  resets to the clicked page.
* Main-view scrolling updates current PageId without changing selection.
* Ctrl+A selects all logical IDs without creating controls or rendering pages.
  Selection changes are not undo commands and do not mark the document dirty.

Green card fill denotes selected; a blue border/edge denotes current. A card
can display both. The command area shows selected count and CLEAN/DIRTY.
Delete/Ctrl+Z/Ctrl+Y/Ctrl+A call the same validated Rust commands as the buttons;
shortcuts leave focused text boxes to their normal text-editing behavior.

## Mutation semantics

`PagePlan::delete_pages`, `move_pages`, and `rotate_pages` accept stable-ID sets.
They validate the entire input before changing the plan. Editor methods wrap
them as one transaction per user action. Empty selection, stale identity,
invalid destination, invalid rotation, and unavailable history fail without
partially changing state. Debug assertions and tests enforce unique active
IDs, valid immutable source references, quarter-turn rotations, an active
current page, and active selected/anchor IDs.

**Delete:** remove the selected entries and preserve survivor order. Clear
selection and a deleted anchor. Keep current PageId if it survives; otherwise
use the old current logical index clamped to the new final index. Thus deleting
C from A B C D while C is current selects D; deleting the final current page
selects the preceding survivor. Deleting every page, including the only page
of a one-page document, returns “At least one page must remain.” Multi-page
deletion is one undo step. Native code preserves the previous page-local
vertical offset approximately, then applies normal viewport clamping.

**Move:** the ABI argument is a boundary in the original order: zero means
before the first page, page count means end. The UI's Move Before is 1-based;
page count + 1 means end. Extract selected entries in their existing logical
order, subtract the number removed before the original boundary, and insert
at that normalized position. For A B C D E F, moving B/D/E to end produces
A C F B D E. A boundary inside an already contiguous selected group leaves
the plan unchanged. For a non-contiguous group, it gathers the group at the
normalized boundary. An unchanged plan creates no history entry, does not
increase structural revision, and does not clear redo. Selection, anchor,
current PageId, and source identity survive movement; logical numbers change.

**Rotate:** persistent application-owned additional clockwise rotation is
stored in each PagePlanEntry, normalized to 0/90/180/270. UI left/right apply
−90/+90. The core accepts signed ±90/±180/±270; other arguments fail. The
whole-view 90-degree checkbox remains a separate temporary view transform.

PDFium owns intrinsic source rotation. Source geometry is already normalized
for that intrinsic rotation. Layout and pixel keys apply editing rotation plus
temporary viewer rotation; reported effective rotation includes intrinsic
rotation as well. This avoids applying intrinsic rotation twice. The P3 and
continuous APIs add editing rotation when generating render demand. The P2
low-level tile API retains its explicit *total additional rotation* contract;
callers submitting a TileKey's rotation must not add editor rotation again.
The legacy preview API remains a source preview rather than an export of the
edited logical document.

## History and dirty state

Each successful structural command stores before/after lightweight logical
snapshots: entries, selected IDs, anchor, and current ID. No source bytes,
PDFium objects, CPU pixels, GPU textures, or thumbnail bitmaps enter history.
Undo and redo restore all of that logical state, including the original IDs
of deleted pages, then synchronize layout and demand. A new changed command
clears redo; failed commands and no-op moves preserve it.

The default combined undo/redo bound is 100 commands and 64 MiB of accounted
metadata. The ABI accepts 1–1,000 commands and 1 byte–256 MiB. Accounting uses
entry capacity plus conservative HashSet capacity/overhead estimates; it is
not an operating-system heap measurement. Oldest undo commands are evicted
first; if only redo remains, farthest-future redo entries are evicted first.
A command larger than the configured budget can be applied with no retained
undo entry. History limits are explicit and inspectable through status.

The immutable initial entry vector is the dirty baseline. After a mutation or
history restoration, compare order, identity/source references, and rotation
against that baseline and cache the result. Returning exactly to the original
plan is CLEAN, including inverse edits. Selection/current movement never
affects dirty state. History eviction can make the baseline unreachable via
undo, but does not discard the baseline or make dirty permanently true.
Per-frame status reads do not compare all document entries.

## Layout, generations, thumbnails, and caches

Public operations on a document are serialized so command commit, layout
synchronization, and placement invalidation cannot interleave with another
public call. Existing render workers do not take this operation gate.

Structural edits rebuild lightweight layout metadata and Fenwick sums in
O(N). Known geometry is retained by PageId, including deleted pages restored
by undo. Nearby asynchronous geometry requests/results use immutable source
indices, then resolve to the current logical position; deleted results are
ignored. Layout count, bounds, extent, navigation, and current lookup follow
the active plan. Rotation swaps dimensions where appropriate. Geometry stays
lazy; no edit scans or renders the source document's pages.

PageId-to-position maps avoid an O(N) scan for current lookup during each
viewport update. The native UI captures the current page-local offset,
invalidates the old displayed frame, navigates to the returned current index,
and submits fresh demand. Cached GPU textures remain available; invalidating
the frame prevents a deleted or moved page from persisting at its old location.

Structural revision and selection revision are separate monotonic counters.
An edit clears scheduler placement demand/queues/completions/pins in both
lanes, and immediately invalidates Rust thumbnail slot tokens. It retains
generation floors, so previously submitted viewport/thumbnail generations
are rejected. The next bounded thumbnail update assigns new tokens and
projects current IDs, logical labels, selection, and current highlighting.
Native cards remain recycled and bounded by visible/overscan slots.

Tile/thumbnail pixel identity includes DocumentId, PageId, raster parameters,
and additional rotation, rather than logical position. Delete and move retain
CPU tile and thumbnail caches; unchanged surviving pixels remain reusable.
Rotation requests distinct keys; undo can reuse the earlier orientation.
Native GPU textures are also retained within their existing budget. Bounded
card bitmap payloads may be cleared on token reassignment and reuploaded from
the retained thumbnail cache. No whole-cache flush is needed for movement.

A render already executing at edit time can finish. Its result is cached or
published only when the same pixel key is valid in newly submitted demand;
otherwise it cannot paint into post-edit placement. Deterministic gated-worker
tests cover this race. Main-view work keeps priority over thumbnails.

## ABI v7

New exports are `pdfeditor_document_editor_status`,
`pdfeditor_document_select_page`, `pdfeditor_document_edit`, and
`pdfeditor_document_configure_history`. No Rust collections cross the ABI.

The 56-byte x64 EditorStatus contains structural/selection revisions, current
PageId, history bytes, logical count/current index, selected count, undo/redo
depth, and structural dirty flag. Command codes are delete=1, move=2, rotate=3,
undo=4, redo=5, select all=6. Selection mode is plain=0, toggle=1, range=2.
Selection token zero supports direct stable-ID callers; nonzero tokens must
match a current Rust thumbnail assignment. Stale tokens fail safely.

ThumbnailItem adds `selected` in previously unused tail padding, preserving its
80-byte x64 size. Existing continuous/thumbnail/raster structures retain their
sizes; native code requires ABI v7. New named errors are no selection=12,
invalid edit argument=13, last page=14, and no history=15. Existing invalid
page=6 and invalid render/rotation argument=7 remain in use. Output status is
zeroed on error. C header and native static assertions verify these contracts.

## Automated verification

The final checks passed locally:

* `cargo fmt --all -- --check`
* `cargo test --workspace --all-targets`: 51 tests, 41 core and 10 FFI; zero
  failures. PDFium-backed FFI tests ran with the deployed PDFium DLL.
* `cargo clippy --workspace --all-targets -- -D warnings`
* `cargo build -p document-ffi --release`
* WinUI Debug x64 build through `python scripts/build_winui.py`.
* All retained P0–P5A ABI/PDFium/tile/viewport/continuous/thumbnail smokes.
* New P5B smoke on mixed 12 pages, synthetic 10,000 pages, and the local
  964-page PDF. All source SHA-256 comparisons passed; no output PDF was written.

Core tests cover plain/toggle/range/anchor semantics and current independence;
first/last/current, contiguous/non-contiguous and all-but-one deletion; move
direction/group order/inside-boundary/no-op; left/right/group rotation, keys,
intrinsic rotation, dimensions and geometry restoration; transaction history,
redo clearing, memory/count bounds, exact dirty baseline; layout and thumbnail
order after edits; and a 10,000-page sequence around page 5,000, deleting 2,001
pages and moving near-end pages to the beginning with undo/redo.

Scheduler tests cover stale completion cancellation, old generation rejection,
same-Arc cache reuse for moved pixels, new rotated pixels, and a render that
was already running during invalidation. Real PDFium smoke additionally checks
ABI sizes, immediate old-token rejection, empty old completion polls, source
identity restoration, current recovery, rotated layout/key correctness,
same-buffer thumbnail reuse, large selection, and bounded history.

### Local release measurements

These are observed maximum durations of the exercised edit ABI calls on this
machine, including Rust history/layout/invalidation work. They exclude later
raster completion and native frame/upload time. They are local warm-run
measurements, not latency guarantees.

| Document | Original pages | Maximum edit call | Thumbnail cache bytes at sampled end | History bytes at sampled end |
| --- | ---: | ---: | ---: | ---: |
| Mixed fixture | 12 | 0.0170 ms | 1,064,448 | 4,608 |
| Synthetic fixture | 10,000 | 1.0852 ms | 1,161,216 | 1,648,896 |
| Local engineering PDF, 268,459,200 bytes | 964 | 0.0825 ms | 1,064,448 | 437,376 |

All three runs reused exactly the same thumbnail pixels **and buffer pointer**
after moving a page. Retained P5A smoke had at most eight live slots, queue
maxima of five/eight for mixed/synthetic documents, and thumbnail cache maxima
of 5,031,936/7,064,064 bytes. Retained P4 smoke CPU tile cache peaks were
95,420,416/103,809,024 bytes, within the existing 128 MiB budget. Selection does
not add thumbnail demand or card count.

Local ignored evidence files include `artifacts/p5b-rust-tests.log`,
`artifacts/p5b-final-build.log`, `artifacts/p5b-smoke-results.jsonl`, and
`artifacts/p5b-source-hash-before.json`. These are local evidence, not committed
fixtures or portable benchmark guarantees.

## Live WinUI verification

Dedicated test deployments were used; the pre-existing P5A session was left
alone. The mixed fixture showed:

* Pointer single selection; Ctrl toggling and Shift range selection through
  focused thumbnail activation; selected/current combinations visible.
* Main wheel scrolling changed current from logical page 3 to 4 while retaining
  the two-page selection. Delete renumbered 12 pages to 10 and preserved the
  surviving current PageId; undo restored 12, selection, position, and CLEAN;
  redo restored the edited state.
* Moving selected source page 2 before page 1 changed logical labels and current
  index while keeping its content. Render count stayed 38 and GPU uploads 24
  across that move, confirming reuse in the native viewer.
* Rotate Right changed both main content and thumbnail orientation/extent.
  Ctrl+Z/Ctrl+Y restored/reapplied it. Zoom 1→1.25, Show Current, and resizing
  retained the correct stable current page and selection. No wrong-page flash
  was observed during these actions.

The 964-page PDF was exercised in the ABI smoke directly from its original
path. For live UI tests a byte-identical local fixture copy was placed in the
dedicated test deployment. On that live document:

* Go to logical 500 and Show Current used seven live cards and three visible
  cards; initial main geometry remained lazy.
* Deleting selected current PageId 500 produced 963 pages and current PageId
  501 at index 499, retaining document Y approximately 432,106.68. Undo
  restored PageId 500, 964 pages, selection, and CLEAN.
* Moving PageId 500 before page 1 produced index 0 and Y=24. CPU renders stayed
  48 and GPU uploads 18. Show Current displayed its new page-1 card, using four
  live slots. Rotate Left changed orientation and extent; undo rotation and
  undo move restored original order/current page and CLEAN.
* Ctrl+A selected 964 IDs with four live cards and no selection-triggered
  renders. Delete was rejected with the explicit last-page message, leaving
  count, current, dirty state, and history unchanged.

The UI automation API cannot hold a modifier while issuing a pointer click.
Actual plain pointer clicks were checked; Ctrl+Space and Shift+Space on focused
thumbnail buttons exercised the same Click handler and live modifier reads.
Raw held-modifier pointer gestures therefore remain a manual verification
gap, despite deterministic core tests and live modified activation checks.
The permanent approximately 1.27 GB acceptance PDF was not available locally;
its live behavior remains unverified. The available 268 MB, 964-page document
and synthetic 10,000-page integration were tested instead.

## Architectural and performance limits

Structural mutation, history snapshots, exact dirty comparison, and layout
reconstruction are O(N) lightweight work, as allowed for P5B. Snapshots store
the full logical entry vector before and after each command. Count/byte caps
bound retained history, but larger documents can retain fewer than 100
commands and temporarily allocate a command before trimming. A future delta
history could reduce this cost if measurements justify it.

Selection is O(N) lightweight IDs at worst. Geometry retention is bounded by
original source-page population, including undo-restorable deleted IDs;
metadata is therefore document-sized, while raster caches, work queues,
native controls, and bitmap payloads keep their existing explicit bounds.
Reported metadata/history accounting excludes some allocator/container
overhead and is not a process working-set measurement.

Public edits execute synchronously. The observed 10,000-page release calls
were short, but very large metadata populations could justify asynchronous
command orchestration later. No O(N²) editing loop was introduced. Viewport
lookup and bounded thumbnail projection remain independent of selected count
apart from constant-time selected-ID lookup; status history accounting scans
only the bounded command list.

The existing single worker/global PDFium serialization remains. A synchronous
PDFium render already in progress cannot be interrupted; invalidation prevents
incorrect publication, and main demand has priority when choosing the next
job. This is a pre-existing responsiveness boundary, not a source rewrite or
an unbounded render queue. Logical edits disappear when the document closes;
persistence belongs to a later milestone.
