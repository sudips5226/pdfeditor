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

## Engineering principles

1. Viewer interaction has absolute priority.
2. Opening a document must not trigger whole-document rendering or scanning.
3. Rendering is tile/viewport-native from the beginning.
4. Memory use is bounded and largely independent of document file size.
5. Document state is owned by the Rust core, not by the UI.
6. Backend-specific types do not cross the public core boundary.
7. Smart P&ID will consume stable document-platform APIs rather than PDF/UI internals.

