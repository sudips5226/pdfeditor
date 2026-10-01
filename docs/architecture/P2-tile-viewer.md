# P2 — tile viewer

P1 document identity, source, geometry, page plan, persistent opaque handle, and
PDFium lifetime remain in place. The P2 viewer replaces its preview call with
independent region requests. The legacy preview and synthetic exports remain
available for P0/P1 regression checks. No P3 viewer machinery is included.

## Request and coordinate contract

`document_core::TileRequest` has `page_id: PageId`, signed `tile_x/tile_y: i32`,
`scale/device_pixel_ratio: f64`, `rotation_degrees: u16`, and `width/height: u32`.
P2 requires dimensions of 512×512. Scale means logical pixels per PDF point;
the caller may use 96/72 logical pixels per point for a physical-size convention.
The proof UI uses 1 and 2 logical pixels per point. DPR multiplies scale, and
grid indices always measure physical pixels after viewer rotation.

Normalized application page space has a top-left origin and a downward y-axis,
with the source crop and intrinsic PDF rotation already applied. The request's
rotation is **additional** clockwise rotation: 0, 90, 180, or 270 degrees.
`PageGeometry.size` contains the source's display dimensions, including its
intrinsic rotation; the request does not apply that rotation twice.

Let `s = scale × DPR`, `tx = tile_x × width`, `ty = tile_y × height`, and let
`W,H` be the normalized page size. The page-to-tile transform is:

| Additional rotation | Tile x | Tile y |
| --- | --- | --- |
| 0 | s x − tx | s y − ty |
| 90 | s (H − y) − tx | s x − ty |
| 180 | s (W − x) − tx | s (H − y) − ty |
| 270 | s y − tx | s (W − x) − ty |

`TileRequest::page_to_tile()` returns the explicit affine coefficients and the
inverse-mapped page rectangle. The rectangle is intentionally not clipped to
page bounds; it identifies the complete requested region. Adjacent grid cells
share the same boundary, without independently rounding whole-page dimensions.

## PDFium implementation and memory

The backend wraps the returned Rust pixel vector with `FPDFBitmap_CreateEx`,
using an external 512×512 BGRA bitmap with a 2048-byte stride. It initializes all
pixels to opaque white and calls `FPDF_RenderPageBitmapWithMatrix` with the
page-to-tile affine transform and the intersection of tile bounds and transformed
page bounds. Wholly outside tiles return the same-sized white buffer.

PDFium's matrix API first applies its base display transform, which normalizes
PDF y-up coordinates, nonzero crop origins, and intrinsic rotation, then applies
the supplied transform. This behavior is defined in the
[PDFium implementation](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/fpdfsdk/fpdf_view.cpp)
and [page display transform](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/core/fpdfapi/page/cpdf_page.cpp).
Integration tests verify this composition against the pinned 156.0.8066 DLL.

There is one 1,048,576-byte application pixel allocation per render. No full-page
bitmap is allocated, cropped, or scaled to produce a tile. Page loading and
PDFium's internal resource decoding remain backend overhead. This milestone does
not establish a bound on PDFium's internal memory for arbitrary source content.

Stable PageId resolution uses an index within the existing PagePlan, constructed
alongside its original-page entries. Tile requests do not scan all document pages
or load their geometry. Source index resolution stays inside Rust; PDFium types
remain private to the backend.

## ABI and native presentation

ABI v3 adds `PdfeditorTileRequest`, `pdfeditor_document_render_tile`, and
`PDFEDITOR_ERROR_INVALID_TILE_REQUEST = 7`. Existing exports and output layouts
remain intact. The request contains only C-compatible scalar fields, mirrored
by `#[repr(C)]` Rust storage. Its x64 size is 48 bytes; the width offset is 36.
Rust, C++, and ctypes checks validate that layout.

The API rejects null/stale handles, unknown or foreign-document PageIds, invalid
scale/DPR, unsupported rotation, unsupported dimensions, and coordinate overflow.
Errors clear the output. Rust allocates the pixels, the C++ consumer uploads them
synchronously, and its RAII guard calls the Rust free export even if consumption
throws. Returned pixels remain valid independently of document close until freed.
Input and output storage must not alias.

`NativeCoreBridge::OpenPdf()` reuses the persistent handle.
`NativeCoreBridge::RenderTile()` calls the new export. `DocumentCanvasRenderer`
retains its device/swap-chain architecture and expands the fixed surface to
1024×1024. `BeginFrame()` clears the current buffer, `UploadBgra()` writes each
tile into a checked `D3D11_BOX`, and `EndFrame()` presents once. Flip buffers are
fully populated every frame. The panel's composition scale is compensated using
an inverse swap-chain matrix, following
[Microsoft's composition API](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_3/nf-dxgi1_3-idxgiswapchain2-setmatrixtransform).

## Verification

The vector fixture has four colored, labeled regions, fine grid strokes, diagonal
landmarks, intrinsic 90/180/270-degree page variants, and a nonzero crop-origin
variant. The WinUI proof uses its first page only.

Rust tests retain all P0/P1 checks and add region coordinates, adjacent boundaries,
negative and edge tiles, invalid requests, affine rotations, persistent-handle
rendering, exact 512×512 dimensions and ownership. Real PDFium tests check color
landmarks at 1×, 2×, fractional scale with DPR 1.5, and 16×; a 0.25-point stroke
is checked at 16×. Intrinsic rotations and crop origins are compared pixel-for-pixel.
They run when `PDFEDITOR_PDFIUM_PATH` is set; CI repeats the Rust suite with the
deployed DLL after the WinUI build so these tests do not silently skip in CI.

`scripts/p2_tile_smoke.py` validates the actual release DLL's C ABI, four distinct
tiles at 1× and 2×, edge fill, request rejection, and Rust allocation/free ownership.

For manual WinUI verification, run the Debug x64 executable and click **Render
2 x 2 tiles**. Resize the window to expose the fixed canvas if needed. Confirm
different regions and continuous grid/diagonal landmarks across tile seams.
Enable **2x zoom**, render again, and confirm enlarged content from the same open
document. Enable **90 degrees clockwise** and render again. Check a second display
scale if available. These are manual GPU/XAML checks; passing core/ABI tests and a
native build does not verify live SwapChainPanel presentation.

## Limits and concerns

PDFium uses float32 matrices. Requests whose page extents, tile origins, or affine
translations exceed the 2^24 physical-pixel coordinate range are rejected rather
than allowing loss of single-pixel addressability. Scales must also produce a
finite, normal float32 determinant. Extremely large zoom ranges may require a
future backend precision strategy; this limit is explicit, not a memory limit.

Rendering remains synchronous and serialized through the established PDFium
runtime mutex. Each request loads/releases its source page, while keeping the
document open. Repeated page parsing and complex source-image decoding should be
measured in a later performance milestone. There is no tile cache, prefetch,
background rendering, scheduler, navigation, or multipage scrolling in P2.

## Changed files

- `crates/document-core/src/lib.rs`: request validation, explicit transform,
  indexed stable page lookup, coordinate tests.
- `crates/pdfium-backend/src/lib.rs`: tile-sized matrix rendering and clipping.
- `crates/document-ffi/src/lib.rs`: ABI v3 request/export/errors and integration tests.
- `include/pdfeditor_ffi.h`: matching plain C contract.
- `native/winui/NativeCoreBridge.h` and `NativeCoreBridge.cpp`: persistent open,
  tile calls, allocation guards, ABI/layout checks.
- `native/winui/DocumentCanvasRenderer.h` and `DocumentCanvasRenderer.cpp`:
  frame lifecycle, fixed tile placements, composition scale.
- `native/winui/MainWindow.xaml` and `MainWindow.xaml.cpp`: minimal 2×2 proof,
  zoom/rotation controls and status.
- `native/winui/PdfEditor.WinUI.vcxproj`: deploy the P2 fixture.
- `tests/fixtures/p2-tile-regions.pdf`: vector region/rotation/crop test fixture.
- `scripts/p2_tile_smoke.py`: release ABI verification.
- `scripts/p0_ffi_smoke.py`: expected ABI version updated to v3.
- `.github/workflows/rust-ci.yml`: P2 branch and deployed-PDFium test coverage.
- `README.md`: P2 description and coordinate-contract link.
- `Cargo.lock`: resolved workspace dependency lock for reproducible application builds.
- `docs/architecture/P2-tile-viewer.md`: implementation contract and verification notes.

Local verification passed all four requested Cargo commands, all 12 Rust tests
with the actual PDFium DLL enabled, both retained P0/P1 smoke scripts, the new P2
release ABI smoke script, and WinUI Debug x64. The initial native build failed
because its inherited environment contained duplicate `PATH`/`Path` keys; passing
a normalized environment to MSBuild resolved it without project changes.

Local ignored artifacts `artifacts/p2-tile-proof-1x.png` and
`artifacts/p2-tile-proof-2x.png` were assembled from independent release-ABI tile
outputs and visually inspected. They verify backend output, not live GPU/XAML
presentation. The manual checks above remain required.
