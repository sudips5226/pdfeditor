# pdfeditor

Commercial-grade, Windows-first PDF/document platform under active development.

## Development name

The repository name **pdfeditor** is intentionally a development name. Product branding will be decided before commercialization.

## Architecture baseline

- **Windows App SDK / WinUI 3** — desktop application shell
- **Direct3D / DirectX** — document canvas and GPU composition
- **Rust** — document/platform core
- **PDFium** — initial PDF rendering/parsing backend
- **qpdf/libqpdf** — structural PDF operations
- **SQLite** — local persistence where required

The core is designed so UI technology, PDF renderer, and structural backend remain replaceable behind narrow interfaces.

## P5C — duplicate, insert, extract and safe document output

ABI v8 adds independent duplicate PageIds, insert-all after the current page,
and explicit source-page lists in the Rust API. A Rust-owned SourceRegistry
keeps one persistent PDFium document per immutable source. Existing selection,
undo/redo, continuous layout, bounded rendering caches and thumbnail navigation
apply to every logical page. Save As and Extract use immutable snapshots on a
dedicated output worker with pinned libqpdf 12.3.2; page objects and content are
copied structurally, never generated from rasters.

Open PDF, Duplicate, Insert PDF, Extract Selected, Save As and Cancel Output are
available in the temporary WinUI controls. Extract follows current logical
order. Save As records the successful snapshot as the dirty baseline, retaining
any edits made while output runs. Native file pickers select paths; replacing
an existing target requires explicit confirmation. Active sources cannot be
overwritten. A same-directory temporary PDF is verified with qpdf and PDFium
before Windows atomic finalization. Cancellation waits for a native boundary.

Build Rust with `cargo build -p document-ffi --release`, then run
`python scripts/build_winui.py` (or restore/build the vcxproj with MSBuild).
MSBuild runs `scripts/build_qpdf.py`, verifies the official SDK checksum,
compiles the private adapter and deploys its runtime and notices automatically.
This requires Python, the MSVC x64 tools and network access on the first build.
No developer qpdf installation is needed.

Read [P5C design, complete verification report and preservation limitations](docs/architecture/P5C-document-output.md)
before relying on document-level preservation. Page content, resources, boxes,
rotations and ordinary annotations are tested. Document metadata, outlines,
signatures, JavaScript, encryption and full form behavior are not guaranteed.
Encrypted sources are rejected for output; structural rewriting invalidates
signatures. Live WinUI checks cover duplicate/insert and undo/redo, inserted
rendering/editing, Extract, Save As, reopen and overwrite/source guards. The
active-write browsing path is verified by gated worker tests; live 964-page and
11,568-page writes completed before the next UI status observation.

## P0 — architecture proof

The first milestone proves the production-shaped path:

```text
PDF file
  -> DocumentSource
  -> Rust document core
  -> PdfBackend
  -> PDFium
  -> 512x512 tile buffer
  -> C ABI
  -> C++/WinRT bridge
  -> Direct3D texture
  -> WinUI DocumentCanvas
```

P0 established this boundary and is frozen.

## P1 — document foundation

P1 keeps the P0 PDFium → BGRA → Direct3D rendering path and replaces its
one-shot path call with an opened document handle. The C ABI exposes document
open/close, logical page count, page geometry with stable page ID, and rendering
through the open handle. The Rust core owns source, identity, coordinate-space,
and original-page-order primitives; the PDFium backend alone owns PDFium handles.

The initial `LocalFileSource` uses PDFium's file-backed loader. Opening builds a
logical page plan but does not load or render each page. Page geometry is queried
on demand. The open document and its PDFium runtime are released on close; the
runtime is shared while multiple documents are open.

## P2 — tile viewer

The viewer now requests independent 512×512 physical-pixel page regions through
ABI v3. `TileRequest` identifies a stable page, signed tile grid position, scale,
device pixel ratio, additional clockwise rotation, and fixed tile dimensions.
PDFium renders directly into each tile's BGRA buffer using a matrix and clip;
there is no full-page raster or preview upscaling in this path.

The minimal WinUI proof displays four adjacent tiles and offers 2× zoom and
90-degree rotation controls. Each click reuses the open document, renders tiles
synchronously, uploads them at fixed Direct3D canvas coordinates, and presents
once. P0/P1 preview exports and tests remain for compatibility.

See [P2 transform, ABI, and verification notes](docs/architecture/P2-tile-viewer.md)
for the coordinate contract, resource limits, and manual proof steps.

## P3 — viewport, scheduler, and bounded caches

ABI v4 accepts a typed single-page viewport, schedules visible tiles before a
clipped prefetch ring, and exposes immutable BGRA leases through nonblocking
polling. One worker per renderer respects the existing global PDFium lock.
CPU and native GPU caches default to configurable 128 MiB budgets. Generations
reject obsolete work while overlapping cached tiles are reused. The temporary
WinUI proof has horizontal/vertical pan, zoom, rotation, and development metrics.

See [P3 architecture and verification](docs/architecture/P3-viewport-scheduler-cache.md)
for ownership, limits, test results, and live visual verification.

## P4 — continuous virtualized multi-page viewer

ABI v5 adds a lightweight continuous layout, explicit document coordinates,
asynchronous nearby-page geometry, binary/prefix visible-page lookup, vertical
wheel/scrollbar navigation, anchored zoom, current page, and go-to-page. One
resizable document canvas feeds all visible pages into the existing P3 scheduler
and bounded CPU/GPU caches. A 10,000-page layout uses 560,016 accounted bytes;
unknown page sizes are explicitly estimated and refined lazily.

See [P4 architecture, measurements, verification, and local benchmark setup](docs/architecture/P4-continuous-viewer.md).
The permanent large acceptance PDF is not included. Set `PDFEDITOR_DOCUMENT_PATH`
before launching the Debug viewer to test a local document.

## P5A — virtualized thumbnail navigator

ABI v6 adds a dedicated left thumbnail panel with recycled visible/overscan cards,
current-page highlighting, click navigation through P4, and explicit Show Current.
One render worker prioritizes all main-view work ahead of thumbnails; thumbnail
rasters have a separate configurable 32 MiB cache. Native WriteableBitmap images
remain bounded by visible rows and a separate 32 MiB pixel-payload limit. Thumbnail
scrolling never enumerates or renders the full page list. No page editing is added.

See [P5A architecture, resource bounds, measurements and verification](docs/architecture/P5A-thumbnail-navigator.md).
Run `python scripts/p5a_thumbnail_smoke.py target/release/pdfeditor_core.dll tests/fixtures/p4-mixed-pages.pdf`
with `PDFEDITOR_PDFIUM_PATH` pointing to the deployed PDFium DLL. Optional additional
PDF arguments allow local large-document checks without committing source files.

## P5B — Page Manager and logical editing

ABI v7 adds Rust-owned stable-PageId selection, logical delete/group movement,
per-page editing rotation, bounded undo/redo, and exact structural dirty tracking.
The original PDF stays immutable. Main layout and recycled thumbnails follow the
active PagePlan; unchanged page pixels retain their cache identity across moves.

The temporary Page Manager supports plain/Ctrl/Shift thumbnail activation, Delete,
Move Before (1-based, page count + 1 means end), Rotate Left/Right, Undo/Redo and
Delete/Ctrl+Z/Ctrl+Y/Ctrl+A shortcuts outside text boxes. Selection remains
independent of viewer current-page movement. History defaults to 100 commands and
64 MiB of accounted logical metadata. P5B originally added no output; P5C now provides Save As and Extract.

See [P5B architecture, semantics, verification and measurements](docs/architecture/P5B-page-editing.md).
Run `python scripts/p5b_editing_smoke.py target/release/pdfeditor_core.dll tests/fixtures/p4-mixed-pages.pdf`
with `PDFEDITOR_PDFIUM_PATH` pointing to the deployed PDFium DLL. Optional PDF
arguments exercise local large documents without writing an edited PDF.

## Engineering principles

1. Viewer interaction has absolute priority.
2. Opening a document must not trigger whole-document rendering or scanning.
3. Rendering is tile/viewport-native from the beginning.
4. Memory use is bounded and largely independent of document file size.
5. Document state is owned by the Rust core, not by the UI.
6. Backend-specific types do not cross the public core boundary.
7. Smart P&ID will consume stable document-platform APIs rather than PDF/UI internals.

