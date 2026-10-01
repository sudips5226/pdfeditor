# P5A — virtualized thumbnail navigator

Branch: `p5a/thumbnail-navigator`. Navigation only; no P5B operations, page-plan
mutation, undo, saving, annotations, search, OCR or final chrome. No push or merge.

## Architecture and priority

The P4 WinUI → bridge → C ABI → persistent Rust document → continuous layout →
P3 tile scheduler/cache/leases → native GPU cache → Direct3D → single main
SwapChainPanel path remains. The document canvas renderer is unchanged.

A separate thumbnail navigator feeds a thumbnail lane in the **same render worker**.
The scheduler's original state was factored into two bounded lanes; each has its
own demand, generation, cache, done set, completion queue and metrics. Shared
worker selection always checks main work first:

1. Main visible tiles.
2. Main near-viewport/prefetch tiles.
3. Visible thumbnails.
4. Near-visible thumbnail prefetch.

Main completion backpressure also pauses thumbnails if main rendering is pending.
A full thumbnail completion queue cannot block main work. Updating main demand
does not replace thumbnail demand, and scrolling thumbnails does not replace main
demand. Obsolete queued requests are replaced immediately. An old running
thumbnail can finish and remain cached; it cannot publish into unrelated slots.
A gated fake backend verifies ordering with main and thumbnail requests pending,
including main work arriving during an already running thumbnail.

**Priority is enforced at render-job boundaries.** The retained synchronous PDFium
render/load call cannot be interrupted. A particularly expensive thumbnail page
parse/image decode can delay a subsequently arriving main request until that one
call finishes. The small bounded output prevents large raster allocations, but it
does not bound PDFium's parsing/decoding time or memory. No hard interactive latency
guarantee is claimed on the unavailable permanent acceptance PDF. A progressive
backend or cooperative cancellation would need separate measured work if this
becomes material; this milestone does not redesign the backend.

## Layout, virtualization and identity

`ThumbnailLayout` configures a 144 × 168 logical-pixel image box, 24-pixel number
label, 8-pixel gap, 8-pixel padding and two overscan rows per side. Row pitch is
200 logical pixels. Constant-height placement makes refinement independent of
panel extent and avoids scrollbar jumps:

```
row top = padding + logical index × row pitch
visible range = floor/ceil of the logical viewport divided by row pitch
live range = visible range expanded by overscan, clamped to the logical page count
```

Scroll updates perform constant-time range arithmetic and O(K²) matching over
bounded live slots, where K ≤ 64. They do not scan N document pages or create N
controls. The native clipped Canvas is only the panel viewport, with a double
virtual scrollbar representing the total extent. It is not a full-document tile
canvas or an enormous physical backing surface. Native positions subtract the
panel offset before being assigned to controls.

The Rust navigator reads `PagePlan.get(logical_index)`, keeping stable PageId
separate from source index and logical position. A reordered-plan test proves
thumbnail identity and click mapping follow logical order. P5A adds no mutation
API. Every live item has page identity, logical index, fixed display placement,
current/visible flags and a recycle token. The native card adds loading, ready or
unavailable state and an optional WriteableBitmap. Labels are one-based logical
page numbers. Navigation APIs remain zero-based.

The actual row control pool preserves still-demanded cards by key/token, reuses
removed cards for new rows, and removes excess controls on shrink. Peak live
controls are max(previous K, next K), never N. Native images are retained only
for visible cards; overscan cards may stay as lightweight loading placeholders
while the Rust prefetch cache holds their rasters. Initial load creates only the
visible and overscan range. With a 600-pixel panel, the deterministic 10,000-page
exercise uses at most eight slots, including middle and last-page positions.

## Keys, rendering, caches and ownership

Public `ThumbnailKey` / ABI key contains DocumentId, document revision, PageId,
physical bounding-box width/height, exact DPR bits, additional clockwise quarter
turn and backend flags. Intrinsic rotation is part of the immutable source
PageId/revision, and PDFium applies it before the additional viewer rotation.
Selection and generations are not pixel identity. Width/height are configurable
at the ABI boundary. DPR/rotation changes replace demand and key identity;
`XamlRoot.Changed` detects DPR changes even without a logical canvas size change.

The shared scheduler uses an internal tagged work key to reuse queue/cache
machinery. That tag never crosses the thumbnail ABI or goes to PDFium flags;
main and thumbnail caches, metrics and polls are separate. Thumbnail completions
cannot be mistaken for document-canvas tiles.

`pdfium-backend::render_thumbnail` loads only a demanded page, resolves its
normalized dimensions there, aspect-fits it, and renders directly into its
DPR-sized BGRA navigation box. A portrait, landscape, rotated or oversized page
keeps its proportions. There is no main bitmap reuse, high-resolution full-page
allocation or downscale stage. Physical dimensions are capped at 1024 × 1024
before allocation. Only zero render flags are currently supported.

Unknown pages have a fixed loading box; exact aspect fit is acquired lazily in
the render worker. The row position and bounding-box key do not change when
geometry becomes known, so no scroll jump or speculative-raster invalidation is
needed. This follows P4's on-demand geometry principle without requesting exact
metadata for all pages or adding thumbnails to its visible-page geometry queue.
P4 retains its own lazy main-view geometry and anchor behavior unchanged.

The dedicated raster cache defaults to **32 MiB**, configurable before first
thumbnail submission, independently of the main 128 MiB cache. It uses the
existing actual `pixels.len()` accounting, deterministic LRU eviction, visible
key pins and hard admission rejection when pinned/oversized entries cannot fit.
A cache miss never authorizes growth beyond the byte budget. Thumbnail demand
caps at 64 keys, ready completions at 16, and one shared worker can have one
running raster. These ready/in-flight leases are bounded transient memory beyond
the resident-cache budget. Checked immutable Rust Arc leases use the retained
process-wide cap of 64 outstanding consumer leases and survive document close.

Native presentation uses WinUI Image + WriteableBitmap, separate from the main
Direct3D swap chain. Native code copies a validated thumbnail buffer once into a
small bitmap and immediately releases the Rust lease. There is no persistent
D3D resource per document page. Retained native bitmap pixel payload has a
separate 32 MiB admission cap and is released on recycling, invisibility and
removal. Framework/device surface overhead is not included in that pixel count,
but remains bounded by live image count. Main uploads execute first; at most two
thumbnail completions are uploaded per 30 ms timer tick. No separate thumbnail
D3D device/cache was needed.

## Navigation and asynchronous safety

Main current-page changes call highlight-only synchronization. They do not scroll
or replace thumbnail render demand. Off-screen current state is retained for
metrics and Show Current. If the current card is visible, its border/background
updates in place. Manual panel scrolling remains under user control.

Click reads the recycled card's current logical index and calls the existing P4
`GoToPage` bridge, then immediately submits its main viewport. No thumbnail render
wait is involved. The existing P4 renderer retains its previous useful frame
until destination content is available; no second navigation/retention path was
added. P4's viewport-center definition remains authoritative after navigation.

Show Current calculates a centered/clamped panel offset from the current logical
index. It changes only the thumbnail scrollbar/demand, never the main viewport
or selection. Resize refreshes live range and clips, retaining panel scroll offset
except necessary end clamping. Existing main-view resize preserves its document
origin. Zoom does not change thumbnail resolution; viewer rotation does.

New key assignments get monotonic recycle tokens. Rust publishing checks current
key and thumbnail generation; native presentation checks PageId-containing full
key, recycle token, generation and current visibility. Recycled slots clear old
images before reassignment. Compatible running work may be retagged for the same
pixel identity; unrelated old results are rejected or cached without presentation.
Backend errors publish keyed/tokened nonzero status completions without a lease,
so the card shows `Unavailable` and remains clickable. Fake-backend tests cover
failure completion, cache reuse, LRU eviction, queue backpressure and stale work.

## Development instrumentation

The status line reports visible thumbnail count, instantiated slots, queue depth,
cache hits/misses/renders, cache/native bytes, uploads, recycled cards, stale
rejections and current one-based thumbnail number. The ABI additionally exposes
ready depth and backend errors. Rust recycle counts measure reassigned slots;
native counts measure reused card controls. These are local debug counters only.

## Verification and measurements

Final checks passed:

- `cargo fmt --all -- --check`.
- `cargo test --workspace --all-targets`: 39 passing tests, including PDFium-backed
  retained P0–P4 tests with the deployed PDFium DLL enabled.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo build -p document-ffi --release`.
- WinUI Debug x64 MSBuild, warnings treated as errors.
- Retained P0 FFI, P0/P1 PDFium, P2, P3 and P4 smoke tests. P4 also passed on the
  locally generated 10,000-page PDF; main cache remained within 128 MiB.
- New P5A real PDFium ABI smoke on mixed, synthetic 10,000-page and local real
  964-page PDFs. Tests cover first/middle/last ranges, 100 rapid generations,
  slot tokens, current sync, logical navigation, Show Current, DPR and rotation
  key changes, independent main polling, capacities and scroll-away/return reuse.
- CI now includes the P5A branch pattern and generates/tests an ignored 10k fixture.

A final smoke run with a 600-logical-pixel thumbnail panel recorded:

| Measurement | Mixed 12 pages | Synthetic 10,000 pages | Local 964 pages / 268 MB |
|---|---:|---:|---:|
| Maximum live slots | 8 | 8 | 8 |
| Maximum observed queue | 5 | 8 | 8 |
| Peak resident thumbnail bytes | 5,031,936 | 7,160,832 | 5,902,848 |
| Thumbnail renders, including DPR/rotation | 22 | 44 | 35 |
| Cache hits | 744 | 15 | 25 |
| Recycled Rust slots | 96 | 812 | 812 |
| Stale running results rejected | 0 | 9 | 5 |
| Maximum demand-update time | 0.033 ms | 0.026 ms | 0.020 ms |
| A → B → A reused identical pixels and buffer addresses | Yes | Yes | Yes |

These timings are observations on warm local runs, not guarantees. Render counts
vary with worker timing during rapid generation churn. No test renders all 10,000
thumbnails. The cache-eviction and hard-budget fake tests deliberately use a tiny
32-byte budget to force admission/eviction deterministically.

Live native checks used the Computer Use skill on the mixed fixture at display
DPR 2. Portrait/landscape/intrinsic rotations were proportional. Clicking page 2
moved the main viewport to Y=890. Rapid thumbnail scroll to the last rows left
main Y=890 unchanged. Show Current moved panel offset 2066→133 while main stayed
at Y=890; resident thumbnails were re-uploaded from cache. Main wheel scroll
moved to page 3/Y=1790 with the panel still at 133. Zoom 1→1.25 left panel pixels
and offset unchanged. Additional 90° rotation regenerated thumbnails with correct
orientation. Maximizing resized main demand from 8 to 15 visible tiles, live
thumbnail slots from 5 to 6, while preserving main Y=1858 and panel offset 133.
No wrong-page thumbnail was observed after recycling. Loading placeholders and
retained main content were observed during pending work.

The final rebuilt app was also checked with the synthetic 10,000-page document.
It opened with four live thumbnail slots and two visible images. Jumping the
thumbnail scrollbar to offset 999808 displayed pages 5000/5001 with six live
slots while the main viewer stayed on page 1. Clicking thumbnail 5000 navigated
the main viewer to Y=4,329,158; only five main page sizes were known at that point.
Moving the panel near the end (offset 1,999,600) reused five card controls and kept
the main viewer on page 5000. Show Current returned to that page's thumbnail
without changing main Y, with resident thumbnail hits. No 10,000-control/raster
creation occurred. The 8,726-page / ~1.27 GB acceptance
PDF was not found in the checked local locations and was not verified. The local
964-page engineering PDF was read in place and not copied into the repository.
Physical display switching/device loss and pixel-perfect error-placeholder visual
injection remain unverified; DPR key behavior and fake-backend failure are tested.

## Changed files

- `crates/document-core/src/thumbnails.rs`: geometry/ranges/identity/recycling/tests.
- `crates/document-core/src/scheduler.rs`: independent bounded thumbnail lane,
  shared absolute-priority worker and fake-backend regressions.
- `crates/document-core/src/lib.rs`: module and logical reordered-plan mapping test.
- `crates/document-ffi/src/thumbnails.rs`: ABI v6 demand/snapshot/poll/current/metrics.
- `crates/document-ffi/src/{lib,viewport}.rs`: persistent thumbnail state, shared
  lease helpers and backend dispatch. Existing ABI struct layouts retained.
- `crates/pdfium-backend/src/lib.rs`: direct bounded thumbnail rasterization.
- `include/pdfeditor_ffi.h`: v6 contracts and x64-compatible structs.
- `native/winui/ThumbnailPanel.{h,cpp}`: bounded recycled cards/bitmap presentation.
- `native/winui/NativeCoreBridge.{h,cpp}`: v6 bindings and checked leases.
- `native/winui/MainWindow.xaml{,.h,.cpp}`: left panel, Show Current, synchronization,
  clipping, DPR refresh and development status.
- `native/winui/PdfEditor.WinUI.vcxproj`: compile the dedicated native panel.
- `scripts/p5a_thumbnail_smoke.py`: bounded real-backend stress and cache proof.
- `scripts/{p0_ffi,p2_tile,p3_viewport}_smoke.py`: ABI version assertions only (and
  P2 status text); retained behavioral checks.
- `.github/workflows/rust-ci.yml`: P5A branch and thumbnail smoke.
- `README.md` and this report: architecture/verification notes.

No new dependency or lockfile change is required. Generated PDFs, logs and runtime
outputs remain ignored under `artifacts/`. The main document canvas implementation
has no P5A changes.
