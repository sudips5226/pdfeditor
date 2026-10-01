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

## Engineering principles

1. Viewer interaction has absolute priority.
2. Opening a document must not trigger whole-document rendering or scanning.
3. Rendering is tile/viewport-native from the beginning.
4. Memory use is bounded and largely independent of document file size.
5. Document state is owned by the Rust core, not by the UI.
6. Backend-specific types do not cross the public core boundary.
7. Smart P&ID will consume stable document-platform APIs rather than PDF/UI internals.

