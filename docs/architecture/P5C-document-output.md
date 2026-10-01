# P5C — Duplicate, Insert, Extract and Safe Document Output

Implementation, local automated verification and live WinUI checks on
2026-10-01, branch `p5c/document-output`. No push or merge was performed.
The end-to-end editing and output sequence passed using disposable fixtures.
Live browsing during the brief active native write was not directly observed;
the gated worker tests exercise that concurrency path.

## Boundaries and dependency

WinUI → C++/WinRT bridge → application C ABI v8 → Rust Editor/SourceRegistry
and SaveCoordinator → qpdf-backend Rust crate → private C adapter → libqpdf.
PDFium remains a separate rendering/verification subsystem. qpdf object types
exist only inside `native/qpdf-adapter/adapter.cpp`; they never enter the public
application header, document-core, PagePlan, PDFium backend or WinUI. The bridge
has only picker/command/status translation; it does no structural PDF work.

libqpdf **12.3.2**, official `qpdf-12.3.2-msvc64.zip`, SHA-256:
`8941870a604e7c87ed24566b038d46c24ce76616254d2383c578f60c0677f202`.
`scripts/build_qpdf.py` downloads it into ignored build/, verifies that digest
on every build and compiles a small x64 `/MD` adapter against `qpdf.lib`.
WinUI MSBuild invokes it and deploys `pdfeditor_qpdf.dll`, `qpdf30.dll`, supplied
VC runtime DLLs and notices beside the executable. No qpdf.exe is executed or
deployed. Python/MSVC are build tools only; neither is an export runtime.

The adapter's private ABI version is 1. Rust loads it with DLL-directory search
flags and explicit plain arrays/scalars, bounded errors and a synchronous
worker-only progress callback. `PDFEDITOR_QPDF_PATH` selects an absolute test
adapter path; default production resolution is beside the application exe.
Official [qpdf page APIs](https://github.com/qpdf/qpdf/blob/v12.3.2/include/qpdf/QPDFPageDocumentHelper.hh)
and [annotation-copy APIs](https://github.com/qpdf/qpdf/blob/v12.3.2/include/qpdf/QPDFAcroFormDocumentHelper.hh)
define the copying semantics used here. Dependency inventory/provenance is in
THIRD_PARTY.md. pypdf 6.14.2 is a pinned CI-only independent test oracle.

## Identity, sources and persistent rendering

DocumentId identifies the editor session. SourceId identifies an immutable PDF
within that session (primary=0). PageId identifies a logical occurrence. Each
PagePlanEntry has PageId, SourceId, source_index and persistent extra rotation;
`source_ref()` returns a typed `SourcePageRef { source_id, source_page_index }`.
The existing source_index field spelling remains for compatibility with core
callers; it is never used alone to route multi-source work.

The application SourceRegistry owns canonical paths, SourceIds, page counts,
Arc-owned persistent PDFium documents, and lightweight identity guards: size,
modification/creation metadata and Windows volume/file ID. Registration does no
whole-file read/hash. Reinserted paths/file identities reuse their source and
backend after validating the guard. Source records remain alive for the entire
session, including sources referenced only by undo/redo; history pruning cannot
break redo. This deliberately postpones reference-counted source eviction.

Active PageId → SourcePageRef routing is synchronized at command commit in a
separate shared map. Render workers take short routing/registry reads, copy a
backend Arc and source index, release those locks and render through the correct
persistent PDFium document. No source is opened per tile/thumbnail. Nearby
geometry work is keyed by PageId, so asynchronous results reach the correct
logical occurrence despite moves, duplicates and mixed sources. Deleted results
are ignored. Existing lazy P4 geometry refinement handles different sizes/crops
and intrinsic rotation. Insert does not enumerate geometries or render pages.

Logical edits invalidate placement/generations/tokens while retaining existing
PageId-based CPU, thumbnail and GPU caches. Save/Extract/completion do not flush
caches or alter viewer pixels. Duplicates can rerender under their new IDs;
future source-content cache keys could share those pixels without changing IDs.

## Commands, history and dirty state

Duplicate inserts each copy immediately after its selected original in logical
order, retaining SourcePageRef and edit rotation. Every copy receives a new
PageId. Copies become selected, the first new copy is the deterministic anchor,
and current PageId remains stable. One command captures before/after plan,
selection, anchor and current. Undo removes copies; redo restores the same IDs.

Insert's Rust core accepts SourceId, an explicit source-index list and a logical
boundary in 0..=count. The application ABI registers/reuses a PDF; count=0 means
all pages, otherwise indices selects an explicit list (including repeated source
indices if desired). The minimal native picker inserts all pages after current.
Inserted occurrences receive fresh PageIds and edit rotation 0, become selected
with the first as anchor, and preserve the existing current PageId. Insert is
one history entry; undo/redo restore exactly the same IDs/references and states.
Selection, current membership, unique IDs, source bounds and normalized rotations
are asserted. Existing command-count/64 MiB metadata history bounds remain.

Extract resolves selected PageIds by scanning current logical order, not click
order. Its snapshot includes current order, duplicates, external sources and
edit rotations. It makes no plan mutation, history entry or baseline change.

The exact last-successfully-saved entry vector is the authoritative dirty
baseline, independent of history depth/pruning. A cached FNV development
fingerprint and saved snapshot revision are exposed; hash equality is **not**
the dirty correctness oracle. Selection/navigation and output status reads do
not scan the plan. Successful Save As updates the baseline to its captured
snapshot; live edits after capture remain dirty. Undo/redo or inverse edits that
exactly match the saved vector return to CLEAN. Failures/cancellation/Extract
leave the baseline unchanged.

## Snapshot and worker

An immutable ExportSnapshot contains DocumentId, revision, ordered entries,
required Arc-owned sources, a compact source/index/rotation mapping, validated
target, operation kind and overwrite permission. Snapshot capture does only
logical metadata copying, mapping and filesystem identity/path checks, then
releases the Editor. A named dedicated worker runs one job per document;
another job gets OUTPUT_BUSY. Native output never locks the Editor, render
scheduler or PDFium runtime for its duration. Browsing, zoom, thumbnails and
structural edits remain available. Opening a different document is disabled in
the UI while output is active to avoid a blocking close/join gesture.

Only final path revalidation, atomic finalization and baseline commit take a
short public operation gate. This prevents Insert from registering an output
destination as a new active source between the safety check and finalization.
A regression gates writing, inserts that destination, and confirms the worker
fails safely without modifying it. Closing marks the handle closed, rejects
pending operations, requests cancellation and joins the output worker before
releasing source ownership. Tests cover closing a gated native operation.

Development status includes document ID, kind, target, phase/progress/error,
snapshot and saved fingerprints/revisions, current revision, source/logical
snapshot counts, snapshot/build-write/verification/total timings, accounted
Rust coordination bytes, final file size and temporary-file existence. Live
EditorStatus supplies current logical count and dirty state. Target/error UTF-8
buffers are bounded at 1024/2048 bytes, with safe truncation and NUL termination.
The native UI polls phase-level progress plus qpdf writer percentages.

## Structural construction and preservation

The adapter starts with `emptyPDF()`. Every required QPDF source remains alive
until `QPDFWriter::write()` returns; streams remain file-backed/lazy. It pushes
inherited page attributes, uses supported `shallowCopyPage()` and `addPage()`,
then applies only persistent additional clockwise rotation relative to the
source's intrinsic rotation. Temporary whole-view rotation never enters the
snapshot. Each output occurrence has an independent indirect page object.
`fixCopiedAnnotations()` gives copied pages private annotation objects and
uses qpdf's field-copy machinery; no manual page-tree aliases/raw PDF parser
or raster-to-PDF path exists. The writer streams to the temporary file and
generates object streams. The editor does not retain output bytes in a Vec.

Tested preservation: original vector/text visual content, page fonts/resources,
MediaBox/CropBox, mixed dimensions, intrinsic rotation plus edit rotation,
current order, repeated occurrence count, inserted-source content and ordinary
text annotations (including distinct annotation objects on duplicate pages).
Small fixtures compare exact PDFium tile hashes and use pypdf to inspect page
and annotation object IDs, boxes, rotations, visible marker text and annotation
contents. These rasters are test oracles only.

Document-level preservation is intentionally limited. Primary /Info, metadata,
outlines/bookmarks, article threads, catalog JavaScript, arbitrary catalog
extensions and document-level page labels are not imported into the empty
destination. Full AcroForm behavior, XFA and cross-page interactions are not
certified; helper use does not constitute a form editor or a complete form
preservation guarantee. Ordinary annotations are tested, every subtype is not.
Existing digital signatures become invalid on structural rewrite; signature
detection/warnings and signature preservation are not implemented. Encryption
UI is absent; qpdf rejects encrypted sources even when an empty-password source
can be viewed by PDFium, so encryption is not silently stripped. Output is
unencrypted. qpdf recovery is disabled and structural warnings fail output,
so some PDFs that PDFium can display may require a future repair workflow.

## Safety, verification and cancellation

Before output the engine rejects targets canonically equal to any registered
source, including hardlink/file-ID aliases, and requires an explicit overwrite
flag for existing files. It rechecks before finalization. Sources are opened
with read-only sharing (Windows share-mode READ), denying concurrent writes
and delete/replacement throughout native output, and their guards are checked
before construction and again before finalization. Missing/changed sources
produce bounded errors rather than silently exporting different bytes.

Temporary files use exclusive create-new in the destination directory:
`.pdfeditor-<process>-<monotonic-counter>.tmp`. The qpdf writer is closed before
verification. qpdf reopens without recovery, reads the page tree, validates the
expected occurrence count and every page's numeric positive MediaBox, and
rejects warnings. PDFium independently opens the file, checks count and queries
first/middle/last geometry, without rendering the document. Rust then flushes
the temporary file with sync_all, closes it, revalidates source/target policy
and finalizes with Windows `MoveFileExW(WRITE_THROUGH | optional REPLACE_EXISTING)`.
Same-directory placement avoids cross-volume copy. No destination predelete
or write-in-place occurs; denied replacement leaves an existing target intact.
This is Windows/filesystem-supported atomic replacement, not a guarantee about
every remote filesystem's crash/power-loss behavior.

An RAII temporary owner attempts cleanup on every exit, including panics.
Normal injected failures/cancellation leave no temps. Status reports whether a
temp remains if OS cleanup fails; no startup orphan-scavenging is implemented.
Cancellation records a request, allows the active native write/verification to
return to a safe boundary and discards output before finalization. It does not
force-kill qpdf. Once atomic finalization commits, cancellation cannot turn a
successful committed job into CANCELLED. Large native calls can therefore delay
cancellation/close completion; this granularity is an explicit limitation.

## API v8 and temporary UI

Retained ABI structs/commands keep their sizes. EditorStatus remains 56 bytes;
OutputStatus is 3200 bytes on x64, checked by Rust, C++ and ctypes. New operations:

* `pdfeditor_document_edit`, new command 7 = Duplicate Selected.
* `pdfeditor_document_insert_source`: UTF-8 PDF, explicit boundary/list, status.
* `pdfeditor_document_output_start`: kind Save As=1 or Extract=2, overwrite 0/1.
* `pdfeditor_document_output_status` and `pdfeditor_document_output_cancel`.

New errors are OUTPUT=16, OUTPUT_BUSY=17, SOURCE=18, SOURCE_TARGET=19,
OVERWRITE=20. Existing selection/edit/handle errors retain their values. Output
phases are IDLE=0, SNAPSHOTTING=1, OPENING=2, BUILDING=3, WRITING=4, VERIFYING=5,
FINALIZING=6, SUCCEEDED=7, FAILED=8, CANCEL_REQUESTED=9, CANCELLED=10. No Rust
collections, qpdf objects, PDFium handles, STL or filesystem types cross the C
application ABI. Pickers return paths; an additional explicit native Replace
dialog provides overwrite consent. Sync picker/start errors remain visible in
EditMessage and asynchronous job errors in the output area.

## Automated verification and measurements

Passed locally: cargo fmt --all -- --check; cargo test --workspace --all-targets
(60 tests: 45 core + 15 FFI, deployed PDFium enabled); cargo clippy --workspace
--all-targets -- -D warnings; cargo build -p document-ffi --release; private
qpdf adapter build `/W4 /WX` (upstream headers external/W0); WinUI Debug x64
build; all retained P0/P1/P2/P3/P4/P5A/P5B smokes on mixed and 10,000 pages;
P5C real mixed-source Save/Extract and Unicode-path smoke; large outputs below.
Windows CI is updated with deterministic adapter/deployment checks, P5C tests
and pinned pypdf, but a hosted CI run was not triggered or observed.

Core tests cover edge/contiguous/non-contiguous/rotated duplicates, inserted
duplicates, unique IDs/references, selection/current semantics and stable redo;
explicit insert lists at first/middle/end, source bounds, errors, exact dirty
restoration after pruning, extraction order and 10,301-entry mapping creation.
Fake gated-backend tests cover concurrent edits/save baseline, actual thumbnail
completion and continuous viewport updates during writing, busy rejection,
cancelled output, source missing/changed, temp-creation failure, native write
failure, verification failure, locked destination/finalization failure, denied
overwrite, invalid targets/IDs, output destination becoming an active source,
close/join, destination bytes preserved and reusable worker state. The native
backend failures are injected; a corrupt libqpdf internal state is not forced.

Small real workflow: reorder/delete/rotate A, duplicate, insert B/reuse its
source, undo/redo, move/rotate an inserted page, Extract in click-independent
logical order, then Save As/reopen. Output: 9 complete pages and 3 selected
pages, with additional single/contiguous Extract checks. Both original source
SHA-256 values remain unchanged after duplicate, insert, each extract, Save As
and invalid output requests. Hardlink active-source targets are rejected.

Observed local release results (one run; not latency guarantees):

| Source | Output pages | Snapshot | Rust snapshot/mapping/records | Mapping | Build/write | Verify | Total job | Output bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Synthetic 10,000 pages / 4,520,378 bytes | 10,004 | 12,355 µs | 360,560 B | 120,048 B | 948.731 ms | 181.006 ms | 1141.173 ms | 2,329,541 |
| Local 964 pages / 268,459,200 bytes | 968 | 303 µs | 35,264 B | 11,616 B | 629.062 ms | 70.216 ms | 1141.559 ms | 267,968,408 |

Both large tests duplicated, moved/rotated a page and inserted three external
pages, materialized full plans, reopened and checked output counts/representative
geometry. 10,000-page output admitted 109 continuous updates, 321 main tile and
324 thumbnail completions while writing; maximum viewport API call 0.0755 ms.
Local output admitted 110 updates, 321 main and 327 thumbnail completions;
maximum viewport API call 0.1050 ms. These are API/scheduler checks, not observed
native pointer/scrollbar behavior. Both sources' streaming SHA-256 comparisons
passed. Large source/output files remain ignored and were not committed.

Sampled Python+native working sets peaked at 153,292,800 B (synthetic) and
111,046,656 B (local). These include the test harness, qpdf, PDFium and raster
caches; they are not isolated qpdf or application heap measurements. Rust
coordination accounting covers entry/mapping capacities and Source record
capacity, excluding allocator overhead, strings, thread stack, editor/history
and native qpdf/adapter allocations. The debug core 10,301-entry snapshot and
generic mapping took 234 µs, with 247,224 B in each of those vectors. Production
mapping uses a more compact 12-byte entry. No whole-source bytes or output bytes
are retained by Rust coordination. Native QPDF internals remain unbounded by
the raster cache budget and should be profiled further on complex large PDFs.
Snapshot timing includes filesystem path/file-ID safety checks: another local
10,000-page run measured 542 µs, illustrating filesystem latency variability.

The 8,726-page / 1.27 GB acceptance PDF was not available. The local 964-page
PDF was used for large live Save As and the automated large workflow.

## Live WinUI verification

The Debug x64 application was operated through its actual native controls and
Windows file pickers. With the six-page primary fixture, selecting page 1 and
Duplicate grew the plan to seven pages and marked it dirty; Undo restored six
pages and CLEAN; Redo restored seven. Inserting all three pages of the external
fixture after current grew the plan to ten pages with two registered sources;
Undo returned to seven and Redo to ten. The first inserted page appeared in
both the continuous viewer and its thumbnail. Rotating it right and moving it
before page 1 updated the displayed page and thumbnail to the 90-degree
`EXTERNAL 1` occurrence.

Extract Selected through the native Save picker succeeded as one page, kept the
ten-page editor dirty, and an independent pypdf reopen found `EXTERNAL 1` with
`/Rotate 90`. Save As wrote all ten pages, reported success and made the editor
CLEAN. Independent reopen found the expected sequence: external 1, primary 1,
external 2–3, duplicate primary 1, then primary 2–6, with source and edit
rotations. Reopening that output in WinUI showed ten pages and the rotated first
page. A live regression exposed that clean-to-clean document opening retained
old thumbnail cards when editor revision counters matched. The Open handler now
rebuilds the thumbnail panel after a successful document change; a rebuilt app
displayed the reopened document's own first-page thumbnail and main page.

The native Save picker displayed its existing-file prompt, followed by the
application's explicit Replace dialog. Cancelling the latter left the existing
destination byte-for-byte unchanged (SHA-256
`9B0C6DE3ABDA400E291D855FE84B32FB986A2C3CBB52B1E6DE87DB3A15092CCA`).
Choosing the currently opened PDF as Save As destination reached the engine's
source guard and surfaced `Output cannot replace an active source PDF`; that
document remained readable. The live 964-page Save As produced a parsable
964-page, 267,968,236-byte PDF with no temporary files left behind. A second
live attempt selected and duplicated the 964-page source into an 11,568-page
logical plan. Save As produced a parsable 11,568-page, 268,121,832-byte PDF
and left no temporary files. Even this job completed before the next UI status
observation, so browsing *during* its active native write and the Cancel Output
button were not directly verified in UI.
The gated backend tests did complete actual tile/thumbnail polling and viewport
updates during an active write, tested edit-after-snapshot dirty state, and
exercised cancellation/failure cleanup. Held Ctrl/Shift pointer selection was
also not exercised through the UI driver; P5B selection behavior remains covered
by retained regression tests and P5C extract ordering by integration tests.

Final live source hashes matched their pre-check values: primary fixture
`A4F24F2DE8DDBC00E7DDAD7C5F9168B8E14DB6A9F0B64E82239048CD5EC5B8CE`,
external fixture `AD6273A1F90A489ECE9B3BAD9004ED43BF95860DBBE82A4BB12CAC0052FFA865`,
and local 964-page source
`C76D601C409997FD510783E619F61FE603DA8A67BFA154442E27DDCAF0D624D4`.
The live outputs are in ignored `artifacts/p5c-live/`. The WinUI Debug x64 build
passed again after the thumbnail fix.

Local evidence: artifacts/p5c-tests.log, p5c-retained-smokes.log,
p5c-final-build.log, p5c-local-result.jsonl, p5c-output/ and p5c-unicode/.

## Complete changed-file inventory

* Cargo.toml and Cargo.lock: workspace/backend dependency.
* crates/document-core/src/lib.rs, editing.rs, p5c_tests.rs: source references,
  duplicate/insert, saved baseline/fingerprint and core tests.
* crates/document-ffi/Cargo.toml; src/lib.rs, continuous.rs, viewport.rs,
  editing.rs, sources.rs, output.rs, output_tests.rs: ABI v8, multi-source
  routing/lazy geometry, source ownership, output coordinator and tests.
* crates/qpdf-backend/Cargo.toml and src/lib.rs: private dynamic structural ABI.
* native/qpdf-adapter/adapter.cpp: qpdf page construction/write/verification.
* include/pdfeditor_ffi.h: narrow application declarations/status/errors.
* native/winui/NativeCoreBridge.h/.cpp, MainWindow.xaml/.xaml.h/.xaml.cpp,
  PdfEditor.WinUI.vcxproj: command bindings, native pickers, status/consent and
  deterministic adapter build/deployment.
* scripts/build_qpdf.py, generate_p5c_fixture.py, p5c_output_smoke.py,
  p5c_large_output_smoke.py: pinned dependency build, fixtures and integration.
* scripts/p0_ffi_smoke.py, p2_tile_smoke.py, p3_viewport_smoke.py,
  p5b_editing_smoke.py: ABI expectations advanced; coverage retained.
* tests/fixtures/p5c-primary.pdf and p5c-external.pdf: tiny deterministic sources.
* .github/workflows/rust-ci.yml: P5C branch, deployment and smoke coverage.
* third_party/qpdf/LICENSE.txt, NOTICE.md, DEPENDENCIES.md,
  LICENSE-JPEG-9f.txt, LICENSE-OpenSSL-3.6.0.txt and LICENSE-zlib-1.3.1.txt:
  upstream license/notices and embedded dependency provenance.
* README.md, THIRD_PARTY.md, this document: workflow, boundary, preservation,
  results and remaining acceptance gaps. README's legacy dash bytes were
  normalized to valid UTF-8 while retaining their characters.

No Split/general Merge/Replace Pages/P6 feature was added. Further concerns are
source eviction, delta history, content-key raster sharing, document-level
preservation, finer safe cancellation and source-handle replacement for a future
in-place Save. The present application only writes separate outputs.
