# P4 — continuous virtualized multi-page viewer

Branch: `p4/continuous-viewer`. Existing uncommitted P3 changes were preserved;
the working-tree diff includes that baseline. No push or merge was performed.

The WinUI 3 → C++/WinRT → narrow C ABI → persistent Rust document → typed
viewport → P3 tile demand/scheduler → bounded CPU cache → shared leases →
completion polling → bounded native GPU cache → Direct3D → one SwapChainPanel
architecture remains. No thumbnails, search, annotations, OCR, structural/page
editing, bookmarks, selection, qpdf, or P5 features were added.

## Layout and coordinates

`document_core::layout::DocumentLayout` stores one metadata record per logical
page and two Fenwick prefix-sum arrays. It stores no pixels or backend/UI/GPU
handles. `PageLayout` is a derived frame record containing stable PageId,
zero-based index, normalized dimensions, intrinsic/additional/effective rotation,
geometry-known flags, document bounds, and spacing before/after.

`DocumentPoint` and `DocumentRect` are distinct from PagePoint and DevicePoint.
Document space is unscaled PDF points, top-left, y-down. Normalized page space
already incorporates source crop/intrinsic rotation, following P2. Page↔document
transforms apply only additional viewer rotation; applying intrinsic rotation
again would be incorrect. Additional 90°/270° swaps the normalized dimensions.

Pages are left-aligned with 24-point outside margins and a configurable gap,
defaulting to 24 unscaled points:

```
page origin = (24, 24 + prefix(page heights, index) + index × gap)
```

For 90°/270°, the width prefix replaces the height prefix. One geometry refinement
costs O(log N). Zoom, DPR, gap, and viewer rotation do not rebuild the model or
prefix arrays. Each page can have independently varying geometry.

Vertical extent is the prefix total plus gaps/margins. Unknown sizes initially
use 595×842 points, so extent is explicitly provisional. Horizontal extent is
conservative: largest encountered width (or height when rotated), including the
initial estimate, plus margins. Extra neutral background can remain after a
narrower refinement; individual page bounds remain correct.

The transform chain is normalized page point → additional quarter turn → page
document origin → subtract viewport document origin in f64 → scale × DPR →
physical viewport pixels. Native tile placement is:

```
(page_document_origin − viewport_document_origin) × target_physical_scale
  + (tile_grid × 512) × target_physical_scale / tile_physical_scale
```

Large document Y never crosses PDFium's float32 page-coordinate boundary or
becomes a native integer directly. Demand is clipped after origin subtraction,
in local physical coordinates. This avoids cancellation at exact tile boundaries
when adding small viewport heights to large document Y. Native page rectangles
clamp before LONG conversion. Each tile is scissored to its own page, preventing
outside-page padding from painting over gaps/adjacent pages.

## Lazy geometry

Open retains the P1 file-backed PDFium document and creates only its lightweight
logical plan. First continuous-view use initializes estimated layout metadata
in O(N), without PDFium geometry enumeration or rendering all pages.

A lazily created metadata worker has a replaceable queue of at most 64 nearby
requests, at most 64 ready results, and one active request. Visible requests
precede neighbors; new viewport updates replace pending requests. It is not a
rendering scheduler. All rendering remains in the existing P3 RenderScheduler.

The first query is `FPDF_GetPageSizeByIndexF`, obtaining normalized crop/rotation-
aware dimensions without `FPDF_LoadPage`. Size-known pages can immediately drive
layout and tile demand. A visible page additionally receives the existing loaded-
page geometry query for exact intrinsic rotation metadata. Neighbor-only pages
use size-only queries until visible. `intrinsic_rotation_known` makes this
progressive state explicit; PDFium rendering always handles source rotation.

PDFium's [size-by-index implementation](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/fpdfsdk/fpdf_view.cpp)
constructs geometry from the page dictionary without parsing page content. Its
float32 dimensions are promoted to f64. The narrow API does not return the
intrinsic rotation angle, which is why visible pages retain a loaded-page pass.
No complex PDFium page-handle cache was added.

Queries honor the existing process-wide PDFium mutex. Viewport updates, navigation,
and metadata probes perform no PDFium calls and do not wait for rendering; only
short Rust state locks are involved. Completed metadata is applied on a new UI
generation. The page-local offset at viewport top is preserved as geometry above
it refines, then origin is clamped to extent. Errors surface through ABI/native
status. Close stops metadata acquisition before joining the render worker; an
active PDFium call can still delay close.

## Lookup, scrolling, navigation, and zoom

Visible lookup binary-searches page tops using prefix sums: O(log² N), with no
O(N) frame scan. It visits intersecting pages plus one neighbor on either side.
Snapshots cap at 64 entries. Extreme viewports/zoom that exceed page/tile limits
return capacity errors before unbounded allocation or grid iteration.

Each size-known visible page produces a clipped page-local P3 ViewportState;
neighbor pages request edge strips as prefetch. All pages' demand is merged into
one generation submitted once to P3. Visible work globally precedes prefetch.
TileKeys retain their original page-specific identity and do not include viewport
position/generation. Scroll updates replace stale queued rendering demand and
preserve caches. Obsolete running results cannot replace current completions.

WinUI has one document canvas and a double-valued vertical virtual scrollbar.
Wheel events support fractional deltas, horizontal pan, and Ctrl+wheel cursor
zoom. Temporary buttons provide pan, zoom, rotation, and one-based page entry.
The canvas and two swap-chain buffers follow actual visible window dimensions.
Uploads are capped at four tiles per 30 ms polling tick.

Current page is the page whose top is last at or before viewport center: the
containing page, or the preceding page in a gap. Ends clamp to first/last page.
The ABI uses zero-based indices; the UI shows one-based numbers.

`go_to_page(index)` computes the known/estimated page top from prefix positions,
without PDFium calls, then native code submits the destination immediately.
Clamping can position the last page above its requested top. A short page in a
tall viewport can have its following page selected by the center-based indicator.
Geometry refinement preserves the destination's local offset.

Zoom buttons preserve the document point at viewport center; Ctrl+wheel preserves
the point under the cursor. The temporary UI clamps scale to 0.125–8. Document
edge clamping takes precedence over exact anchor stability. Rotation preserves
the current logical page and navigates to its new top. Extent, visible pages, and
tile demand recompute without rebuilding the model.

Compatible cached tiles compose immediately. Older scale tiles draw first, with
current-quality tiles overlaying them. A distant/rotation destination with no
compatible tile keeps the previous presented frame until useful destination
content arrives. Requested position and displayed content remain distinct.
Estimated paper placeholders and arriving tiles fill progressively.

## Virtualization and metrics

On x64, 10,000 pages use 560,016 accounted layout bytes: 40 bytes per metadata
record, 16 bytes per page in sum arrays, and two array sentinels. This excludes
the original PagePlan and its scheduler clone, bounded metadata queues, stacks,
PDFium allocations, and swap-chain buffers.

Expensive resources scale with demand/budgets, not page count. Defaults remain
128 MiB CPU cache, 256 demanded keys, 16 ready tiles; native defaults remain
128 MiB / 256 textures. Shared leases retain P3's 64-outstanding process cap.
There is one rendering worker, one native device/swap chain/SwapChainPanel,
with no per-page controls/surfaces/full bitmaps or persistent textures. Rust
receives no D3D handles.

ABI v5 preserves all P0–P3 exports/struct layouts and adds continuous update with
bounded snapshot, metadata-ready probe, and navigation. x64 assertions cover
new viewport (72 bytes), page layout (88), and snapshot (96), plus P3 layouts.

Instrumentation exposes document-open/layout-initialization microseconds,
geometry query count/elapsed microseconds, known pages/layout bytes, extent,
current page/document position, visible pages/tiles, CPU/GPU bytes, queue/ready
depths, renders/hits/stale results, and GPU uploads/reuse. Geometry time is separate
from rendering but includes shared-PDFium-lock contention. Open time includes
runtime/document open and plan initialization. Layout time measures estimated
arrays, excluding worker startup. These are development metrics, not telemetry.

## Verification and measurements

- Rust formatting, workspace/all-target tests, strict Clippy, release DLL build:
  passed. 32 tests pass with deployed PDFium enabled; P0/P1/P2/P3 tests execute.
- WinUI Debug x64 MSBuild with warnings as errors: passed. `build_winui.py`
  canonicalizes environment key casing for the host PATH/Path collision.
- P0 FFI, P0/P1 PDFium, P2 tile, and P3 cache/lease smoke tests: passed. P3 recorded
  a real stale render discard and exactly 16 MiB accounted CPU cache use.
- New P4 release/PDFium smoke: passed on the 12-page mixed fixture and ignored,
  locally generated 10,000-page PDF. It checks lazy geometry, rotations/DPR,
  page-local demand, navigation, gaps, stale/invalid updates, rapid generations,
  and identical buffers after returning without new backend renders. It asserts
  cache/queue/snapshot limits.
- Non-rendering tests cover mixed vertical layout, gaps, two-page viewport,
  lookup/current-page/go-to-page, lazy refinement, all rotations in page→tile /
  document→viewport transforms, DPR/zoom anchoring, 10,000 pages, extent >u32,
  and exact local clipping when global physical Y exceeds P2's float limit.
  A gated fake backend proves cross-page stale rejection and scroll-return reuse.

One measured smoke run, initially 1024×1024 physical pixels at scale/DPR 1:

| Measurement | 12-page vector fixture | 10,000-page generated PDF |
|---|---:|---:|
| Document open | 3.218 ms | 102.936 ms |
| Layout initialization | 0.006 ms | 3.422 ms |
| Initial metadata calls | 5 | 5 |
| Initial geometry elapsed | 2.978 ms | 8.900 ms |
| Layout bytes | 688 | 560,016 |
| Initial visible pages / tiles | 2 / 6 | 2 / 6 |
| Maximum viewport update, including initialization | 0.150 ms | 3.873 ms |
| Geometry calls after jumps/rapid updates | 24 | 23 |
| Known sizes after that exercise | 12 | 13 |

The 10,000-page run finished at about 99 MiB CPU cache with 99 tile renders;
it did not enumerate/render 10,000 pages. These synthetic vector timings do not
represent the permanent oversized/scanned acceptance document. A later warm
run measured 9.085 ms open, 0.350 ms layout initialization, and 0.440 ms maximum
viewport update for 10,000 pages; geometry calls/known sizes after rapid scrolling
varied to 25/14 due worker timing. Cache warming, runtime initialization, disk
access and scheduling affect these observations; no latency guarantee is implied.

Live Debug WinUI checks at display DPR 2 observed native page content, wheel
scroll to Y=600 showing two differently sized pages and a 24-point gap, center
zoom 1→1.25 with Y=600→641.4, go-to-page 3 at Y=1756, intrinsic 90° rotation,
and additional 90° rotation with corresponding page top Y=1858. Completed frames
showed no tile leakage; pending frames retained useful content and filled
progressively. Actual window-sized canvas composition was built and verified.
After the temporary low-battery-modal interruption, the final rebuilt viewer
completed the remaining live checks. Wheel scroll away to Y=600 and return to
Y=0 left renders at 28 and GPU uploads at 16; GPU reuse rose 0→6 and CPU hits
35→54. Rapid +6000 wheel input moved the requested viewport to page 8 (global
Y adjusted to 5753 as nearby estimates refined), with destination tiles filling
progressively while the queue was pending. CPU/GPU remained within their limits.
The automated 30-generation stress and gated fake backend additionally prove
latest-generation/stale behavior, since the small vector PDF renders quickly.
The final scrollbar ValueChanged path navigated to Y=1756/page 3 and completed
without blocking. Maximizing the window resized the single canvas and increased
visible demand from 6 to 9 tiles while preserving Y=1756 and correct placement.

## Local benchmark and concerns

The 1.27 GB / 8,726-page acceptance PDF is not in the repository. Set
`PDFEDITOR_DOCUMENT_PATH` before launching the viewer to select it locally.
The small mixed fixture is default. To generate an ignored synthetic PDF:

```
python scripts/generate_p4_fixture.py artifacts/p4-synthetic-10000.pdf 10000
```

P4 smoke accepts additional local PDF paths; `PDFEDITOR_PDFIUM_PATH` selects the
deployed PDFium DLL. No benchmark upload or repository addition is needed.

Extent/navigation across unvisited mixed-size pages is estimated and converges
with lazy acquisition/anchor correction. Exact global extent up front would
require progressive background size enumeration or persisted metadata; neither
is hidden in open. Visible rotation queries and every P3 tile render still load
PDFium pages. Measure uncontended size-query, loaded-page parse/decode, and render
time on the real benchmark before considering a bounded page-handle cache.
PDFium image/parsing memory remains outside tile budgets. Active native calls can
delay close. Device-loss recovery, a second physical-DPR display, sustained GPU
budget exhaustion, and the permanent acceptance document remain unverified.
The inherited P2 per-page float coordinate limit and explicit capacities remain.

## P4 files

- `crates/document-core/src/layout.rs`, `src/lib.rs`: layout/transforms/demand/tests.
- `crates/document-core/src/scheduler.rs`: cross-page stale/reuse regression.
- `crates/document-ffi/src/continuous.rs`: metadata worker/snapshot/navigation;
  `src/lib.rs`: lifetime/timing/v5; `src/viewport.rs`: shared P3 renderer helpers.
- `crates/pdfium-backend/src/lib.rs`: narrow size-by-index query.
- `include/pdfeditor_ffi.h`: v5 structs/contracts.
- `native/winui/NativeCoreBridge.{h,cpp}`: v5 binding/open/navigation/local fixture.
- `native/winui/DocumentCanvasRenderer.{h,cpp}`: placement/clipping/retention/resize.
- `native/winui/MainWindow.xaml{,.h,.cpp}`: temporary controls and metrics;
  `PdfEditor.WinUI.vcxproj`: deploy mixed fixture.
- `scripts/generate_p4_fixture.py`, `tests/fixtures/p4-mixed-pages.pdf`: fixture.
- `scripts/p4_continuous_smoke.py`: real multi-page/large-document ABI proof.
- `scripts/build_winui.py`: local build environment normalization.
- `scripts/p0_ffi_smoke.py`, `scripts/p2_tile_smoke.py`,
  `scripts/p3_viewport_smoke.py`: retained assertions updated to ABI v5.
- `.github/workflows/rust-ci.yml`, `README.md`, this report: verification/docs.
