# ADR-0001: Commercial document-platform boundaries

Status: Accepted for P0
Repository: pdfeditor

## Decision

The commercial implementation is Windows-first and uses:

- Windows App SDK / WinUI 3 for the application shell.
- Direct3D / DirectX for the document canvas and GPU composition.
- Rust for document state, scheduling, caches, annotations, search, undo/redo, and save coordination.
- PDFium as the initial PDF rendering/parsing backend.
- qpdf/libqpdf for structural PDF transformations.

The application is tile/viewport-native from the beginning. The initial physical tile edge is 512 pixels.

## Dependency direction

    WinUI shell / Direct3D canvas
                |
           narrow C ABI
                |
           document-ffi
                |
           document-core
                |
          backend traits
            /       \
       PDFium       qpdf

Smart P&ID may consume the stable document SDK/ABI but must not depend directly on PDFium, qpdf, WinUI, or the pdfeditor application UI.

## Hard boundaries

The Rust document core must not depend on WinUI/XAML types, DirectX handles, PDFium public handle types, or qpdf C++ types.

The C ABI may expose only fixed-width scalar types, plain C-compatible structs, byte buffers with explicit ownership, and opaque handles where necessary.

No C++ STL container, Rust collection, backend object, or UI object crosses the ABI.

## P0 proof

P0 is complete only when this production-shaped path works:

    PDF file
      -> DocumentSource
      -> Rust document core
      -> PdfBackend
      -> PDFium
      -> 512x512 BGRA tile
      -> C ABI
      -> C++/WinRT bridge
      -> Direct3D texture
      -> WinUI DocumentCanvas

Before PDFium and Direct3D are connected, a synthetic 512x512 tile is permitted solely to validate ABI ownership and pixel transfer.

## Non-goals for P0

No thumbnails, continuous scrolling, annotations, search, OCR, Smart P&ID, structural editing, or GUI beautification.

## Licensing discipline

Only dependencies compatible with intended proprietary commercial distribution may enter the production dependency graph. Every new third-party dependency requires license identification, redistribution/NOTICE review, source-disclosure review, and an inventory entry.

This ADR does not replace legal review before commercial release.
