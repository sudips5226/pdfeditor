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
| qpdf/libqpdf 12.3.2 | Structural page copying, output and verification | Apache-2.0; embedded notices apply | Integrated in P5C through a private library adapter |
| SQLite | Local persistence where needed | Public-domain project | Selected, not yet integrated |
| ONNX Runtime | Smart P&ID inference, separate repo | MIT | Not part of pdfeditor P0 |
| MuPDF | Optional future backend only | AGPL/commercial | Excluded from foundational dependency graph |

## Rule

A dependency is not approved merely because it is technically useful. Its license and redistribution terms must be reviewed before it is merged into a shipping dependency path.

## P5C deterministic qpdf dependency

`scripts/build_qpdf.py` downloads the official
[qpdf 12.3.2 MSVC x64 SDK](https://github.com/qpdf/qpdf/releases/tag/v12.3.2).
The archive `qpdf-12.3.2-msvc64.zip` must have SHA-256
`8941870a604e7c87ed24566b038d46c24ce76616254d2383c578f60c0677f202`.
It is verified on every adapter build, including cached SDKs. Sources are not
vendored, and SDK binaries remain under ignored `build/`.

The product uses `qpdf.lib`/`qpdf30.dll` and our `pdfeditor_qpdf.dll`, compiled
with the release MSVC runtime even for a Debug UI. No qpdf executable is copied
into the application or invoked. MSBuild deploys the adapter, SDK runtime DLLs,
and `qpdf-notices/`. `third_party/qpdf/LICENSE.txt` and `NOTICE.md` are unmodified
upstream v12.3.2 files and include the Rijndael/public-domain and embedded SHA-2
notices. Upstream SDK manuals accompany them. The SDK also bundles Microsoft
VC runtime redistributables; their applicable Microsoft terms continue to apply.
The SDK's exported static link interface lists zlib, JPEG, OpenSSL and platform
libraries; these are dependencies of the upstream binary, not separate runtime
qpdf processes. Unmodified JPEG 9f, OpenSSL 3.6.0 and zlib 1.3.1 license texts
from the preceding official external-libs source release are checked in and
deployed too; `third_party/qpdf/DEPENDENCIES.md` records their archive checksum
and distinguishes build provenance from an embedded binary SBOM. Shipping
license review should verify that upstream inventory against the final package.

`pypdf==6.14.2` (BSD-3-Clause) is an independent **test-only** structural oracle
installed in CI; it is not a product runtime dependency. Production uses no
Python, pypdf, shell or subprocess for Save As/Extract.
