# Third-party dependency inventory

This is a development-time inventory, not legal advice. Exact versions, license texts, copyright notices, and redistribution obligations must be captured before release.

| Component | Planned role | License posture | P0 status |
|---|---|---|---|
| Rust toolchain / standard library | Core implementation | Permissive project licensing; verify shipped runtime artifacts | Selected |
| Windows App SDK / WinUI 3 | Windows desktop shell | Track exact redistributed Microsoft packages | Selected |
| DirectX / Direct3D | GPU composition | Windows platform SDK/runtime terms | Selected |
| PDFium | PDF render/parse backend | BSD-style plus bundled third-party notices; audit exact build | Selected; binary pinned via bblanchon.PDFium.Win32 156.0.8066 |
| bblanchon/pdfium-binaries | Reproducible PDFium binary distribution | Apache-2.0 distribution project; underlying PDFium notices still apply | Selected |
| libloading | Rust dynamic-library boundary for PDFium | ISC | Selected |
| qpdf/libqpdf | Structural PDF operations | Apache-2.0 | Selected, not yet integrated |
| SQLite | Local persistence where needed | Public-domain project | Selected, not yet integrated |
| ONNX Runtime | Smart P&ID inference, separate repo | MIT | Not part of pdfeditor P0 |
| MuPDF | Optional future backend only | AGPL/commercial | Excluded from foundational dependency graph |

## Rule

A dependency is not approved merely because it is technically useful. Its license and redistribution terms must be reviewed before it is merged into a shipping dependency path.
