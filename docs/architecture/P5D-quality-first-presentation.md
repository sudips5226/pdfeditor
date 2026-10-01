# P5D-B — Quality-First Atomic Tile Presentation

Implemented on `p5d/quality-first-rendering`, based on the completed
`p5c/document-output` checkout. No P5D-A shell was present. Nothing was pushed or
merged, and no Split/Merge/P6 work was started.

The main viewer now retains its last complete frame while a newer destination
renders, uploads, and becomes ready. It presents the complete destination once.
Rust owns both requested and displayed viewport state. Tile arrival cannot itself
move the displayed viewport.

**Does a reduced-quality navigation rendering path exist? NO.** Every required
tile is rendered directly at the requested scale, DPR, and orientation. There is
no preview tier, motion-dependent DPI reduction, half-resolution tile, or
low-to-high-quality promotion. Existing P0/P1 compatibility preview exports and
the independent P5A thumbnail renderer remain; neither supplies document-viewer
destination content.

## State and ownership

`document-core::presentation::Presentation` belongs to the existing scheduler.
It records requested and displayed `Destination` values (continuous viewport
plus logical current-page index), the deduplicated exact mandatory TileKeys,
CPU completion facts, GPU residency, visible-geometry readiness, and metrics.

| State | Condition | Visible result |
|---|---|---|
| Stable | Latest generation has been successfully presented and acknowledged | Complete displayed frame |
| Pending | New generation, unresolved geometry, or missing required texture | Last complete displayed frame retained |
| Ready to commit | Current valid generation, exact geometry, and every mandatory key GPU-resident | Old frame retained until composition/Present |
| Commit | Native composition and `Present(1, 0)` succeed, then Rust acknowledges this generation | Requested becomes displayed together with its complete visible set |

The acknowledgement rejects stale, invalidated, incomplete, and duplicate
commits. Editing invalidates pending placement while preserving the previously
presented destination until its replacement succeeds. Requested page labels,
scrollbar position, and thumbnail current-page tracking may advance before the
canvas. Both positions are explicit in developer metrics.

ABI v9 adds `pdfeditor_document_presentation_snapshot`,
`pdfeditor_document_gpu_residency`, `pdfeditor_document_commit_presentation`, and
`pdfeditor_document_navigation_direction`. The snapshot is 256 bytes and includes
both 72-byte continuous viewports, logical indices, counts, state, geometry
readiness, timings, and counters. Prior ABI structures are unchanged. This is
still a narrow C boundary; PDFium, Direct3D, and qpdf types do not cross it.

## Exact mandatory coverage

The continuous layout calculates intersecting pages and their physical visible
tile ranges using the existing 512×512 backend. Only `Priority::Visible` keys are
mandatory. Prefetch rings, directional tiles, overscan pages, and thumbnails are
excluded from the commit requirement.

The keys include source/document identity, document pixel revision, stable
PageId, exact physical scale (`scale * DPR` bits), combined page/edit/view
rotation, signed tile coordinates, dimensions, and render flags. A tile from a
different source, revision, scale, or orientation cannot satisfy readiness.
PagePlan/SourceRegistry resolution makes inserted pages obey the same rule.

Unknown intersecting page sizes or intrinsic rotations block readiness until
the asynchronous geometry worker refines the layout and creates a new demand.
An empty required set is ready only when geometry is known: a genuine document
gap can commit immediately, but unknown geometry cannot produce a blank-page
commit. Paper and gaps are composed deterministically with the complete tile set.

CPU readiness does not imply GPU readiness. Worker completion and CPU cache
hits mark exact keys CPU-ready. Existing matching GPU textures also count as
available content without requiring a redundant CPU render. These CPU counts
describe completion/availability facts, not a promise that every buffer remains
in the current CPU LRU. Native reports its entire actual resident texture-key
set; removing a texture revokes GPU readiness and permits delivery again.
Residency reports are bounded to 512 entries.

## Native commit and retention

`CacheTile` uploads only current-generation mandatory tiles and never calls
Present. `SetViewport` stages requested geometry and keys without clearing the
swap chain. `TryCommitPresentation` reports residency, checks Rust readiness
and native memory limits, and composes only when every required texture exists.

`ComposeViewport` validates the complete set before clearing/drawing a back
buffer, draws paper/gaps and exactly those keys, and calls Present once. After a
successful Present it promotes the native displayed pins; Rust is acknowledged
immediately afterward on the same UI thread. The old cross-scale fallback
composition was removed. Old and new zoom/rotation textures cannot form a
destination patchwork.

ResizeBuffers is delayed until a complete replacement can be composed. During
window movement the existing swap-chain image may be stretched by XAML; this
does not create or admit a lower-quality destination raster. A settled viewport
still requires exact target-quality tiles. No resize debounce was added.

Initial open has no earlier complete document frame to retain. Subsequent
requests, including edits and opening another document, retain the last useful
frame until their replacement succeeds. A backend failure leaves the last frame
visible and reports the existing error; it cannot grant readiness.

## Bounded cache and pressure policy

The CPU tile cache remains 128 MiB. The GPU texture cache retains its normal
128 MiB/256-entry LRU target. Thumbnails retain their separate 32 MiB bounds.
Existing bounded completion queues and CPU leases remain in use.

The union of displayed and pending required texture keys is pinned. Unrelated
textures are evicted oldest-first before allocating destination textures. A
fixed presentation reserve permits at most an additional **128 MiB and 128
entries**, with an absolute 512-entry cap. With the default budgets the hard
texture-payload bound is 256 MiB and 384 entries; 1 MiB 512×512 BGRA tiles normally
hit the byte bound first. Both admission and allocation enforce the hard limits.

After commit, old displayed keys become ordinary LRU candidates and the cache
is trimmed toward its normal target. A displayed set larger than the normal
target can use the reserve while pinned. Shared tile keys are accounted once.

If the old/new union exceeds the hard bound, `MemoryBlocked` deterministically
holds the existing frame and exposes a diagnostic. A smaller viewport/request
can recover. The implementation does not drop the old frame, allocate without
limit, or render a cheaper substitute. Extremely large viewport/DPR combinations
may therefore remain pending until the user changes the request. The texture
metric excludes swap-chain buffers and driver overhead; it is not total device
memory. Existing canvas dimension limits remain 16384 pixels per axis.

## Scheduling and interaction

Each new generation replaces queued obsolete demand. Running serialized
PDFium work can finish and enter the bounded CPU cache; it can be delivered for
the latest demand only when its exact key is still useful. It cannot commit a
superseded destination. Hold timing spans successive coalesced requests until a
complete replacement is finally committed.

Priority is newest mandatory visible rendering, mandatory upload/commit,
directional full-quality prefetch, ordinary nearby main-view prefetch, then
thumbnails. While atomic presentation is pending, the worker pauses speculative
and thumbnail jobs once mandatory CPU work finishes, giving upload and commit
the next opportunity. Existing primitive scheduler/ABI smoke clients that do
not opt into GPU reporting retain their scheduling contract.

Direction comes from actual wheel, navigation, pan, thumbnail activation, and
scrollbar input. Programmatic scrollbar refinement is guarded; zoom and viewer
rotation reset direction. One viewport ahead is anticipated at the exact target
quality, with at most 32 promoted directional tiles and four ahead-page geometry
queries. Reversal replaces obsolete queued speculation. Directional rasters
prepare the CPU cache; the native compositor uploads them when they become
mandatory. PDFium serialization and backend concurrency are unchanged.

Fully GPU-cached scrolling commits in the input path without PDFium work or
texture upload. CPU-only cached scrolling still waits for upload. Uncached
scrolling, page jumps, zoom, and rotation update requested state immediately,
retain displayed state, and commit only complete coverage. The dispatcher polls
asynchronously; it never waits for PDFium. Upload batches admit up to 16 tiles
per tick with an 8 ms work bound on the existing 30 ms timer. This replaced the
four-upload limit after live measurements showed avoidable upload delay.

## Instrumentation

The Debug status area exposes requested/displayed page, document Y, scale,
generation; required/CPU/GPU/missing counts; request-to-first-render,
request-to-all-CPU, request-to-all-GPU, request-to-commit, previous-frame hold;
commit/stale/coalesced/partial counters; and memory-pressure blocking.

Debug builds append each acknowledged native commit to
`pdfeditor-presentation.csv` beside the executable. `PDFEDITOR_PRESENTATION_LOG`
can override the path and enable recording in other configurations. There is no
CSV header; its 15 columns are:

```text
generation,page_zero_based,scale,requested_at,render_started_at,cpu_ready_at,gpu_ready_at,committed_at,hold_us,required_count,partial_count,document_y,rotation,coalesced,document_id
```

Timestamps are monotonic microseconds relative to the presentation instance;
zero means that event did not occur. A GPU-cached request has no render start.
For reused in-flight mandatory work, start records availability of that running
work at the new request boundary. Request latency measures the final committed
generation, including layout refinements; hold measures the entire pending
chain. The CPU interval includes scheduling/backend work and cannot isolate
pure PDFium CPU execution. GPU-ready-to-commit also includes composition and
Present return; these are not scan-out timestamps.

Debug-only Ctrl+1/Ctrl+2/Ctrl+4 provide exact 100%/200%/400% checks outside text
inputs. No product quality preference or progressive A/B mode was added; the
optional legacy mode would have retained a second composition path to maintain.

## Measurements and live observations

These are local observations, not throughput targets or matched benchmark
comparisons. Real Direct3D measurements use the final Debug WinUI executable
with the release Rust core. The large live source has **964 pages**; the earlier
P5C output derived from it has 968 pages and is a separate file.

| Live scanned-PDF action | Mandatory tiles | Request to all CPU | Request to all GPU | Request to commit |
|---|---:|---:|---:|---:|
| Go to page 528, 100% | — | — | — | 52.243 ms |
| Exact 200% | 15 | 229.844 ms | 264.472 ms | 267.035 ms |
| Exact 400% | 18 | 234.660 ms | 271.895 ms | 273.664 ms |
| Viewer 90° at 400% | 10 | 173.191 ms | 176.177 ms | 177.200 ms |
| Distant thumbnail return, 400%/90° | 3 | — | — | 56.209 ms |
| Scrollbar drag, page 597 to 840, 100%/90° | 8 | 125.579 ms | 139.352 ms | 140.926 ms |
| Adjacent page 840 to 841, cached | 4 | 0.004 ms | 3.136 ms | 3.968 ms |

A saved final-run sample of 63 scanned-document commits had minimum 0.663 ms,
median 40.130 ms, maximum 273.664 ms, maximum hold 747.541 ms, and 838 coalesced
requests. A saved earlier mixed-vector run of 274 commits had median 10.984 ms,
maximum 215.453 ms, and maximum hold 469.561 ms. These snapshots precede the
additional scrollbar/adjacent-page checks; ongoing logs have more records.
Interactive runs included user activity as well as automated actions.

Real-PDFium ABI smoke measurements below use **simulated exact-key GPU
residency**, and consequently do not measure Direct3D upload/Present latency:

| Source / action | Request to simulated commit |
|---|---:|
| Mixed simple vector fixture, initial 100% | 2.355 ms |
| Mixed vector, exact 200% / 400% | 2.974 / 3.437 ms |
| Local engineering P&ID sheet, initial 100% | 153.575 ms |
| Engineering sheet, exact 200% / 400% | 187.311 / 185.625 ms |
| Engineering sheet, rotated 400% | 148.602 ms |
| Engineering sheet, cached requests | 0.217 / 0.040 ms |

The engineering source was `Annexure-3-PID_page_1.pdf`. It demonstrates real
backend rendering of an engineering sheet, but this run does not establish a
worst-case heavy CAD workload. Private acceptance PDFs are not committed.

**Partial destination presentation count: 0** in deterministic tests, ABI
smokes, and the captured native commit records. The counter is an invariant
counter, not an independent optical detector: the commit path refuses missing
coverage, so zero must be interpreted alongside tests and visual observations.

Live checks covered ordinary/rapid wheel, reversal, scroll away/return, scrollbar
drag, page-number jump, Show Current, adjacent next/previous destinations through
Go, distant thumbnail click, exact 100%/200%/400%, rotation, and maximize/restore.
The existing proof shell has no dedicated next/previous-page buttons. Sampled
frames showed the previous complete content while requested controls advanced,
then a complete replacement. One pending snapshot had seven CPU-ready required
tiles but only four GPU textures; the old frame remained visible. Zoom snapshots
similarly showed 200% requested/100% displayed and 400% requested/200% displayed.
No destination assembly, blank leading edge, or mixed-scale patchwork was
observed in those samples. Input continued to update during pending rendering.

The existing Python HeavyPDF window was inspected directly with the same large
source filename and scrolled at its 50% view. Both applications showed coherent
complete page content during the sampled interactions. The Rust retained-frame
behavior now follows the Python quality-first principle. This was a qualitative
comparison: viewport sizes, zoom, document position, and possible unsaved Python
state were not matched, so it establishes neither pixel equivalence nor a speed
ranking. No high-speed video or independent human comfort study was performed.

## Verification

All final Rust checks passed with real PDFium/qpdf runtime paths configured:

```text
cargo fmt --all -- --check
cargo test --workspace --all-targets    # 54 core + 16 FFI tests passed
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p document-ffi --release
```

WinUI Debug x64 built successfully, including the final separate output at
`artifacts/p5d-final-winui/pdfeditor.exe`. This location allowed final validation
without closing another actively used viewer. Native policy tests compiled with
C++20 `/W4 /WX` and passed.

Deterministic tests cover single/N−1 completion, all CPU with missing GPU,
one successful complete commit, retained slow-page frame, A/B/C coalescing,
stale uploads/commits, resident cached demand with no render/delivery, GPU loss,
edit invalidation, unknown geometry versus real gaps, every key identity/quality
component, upload priority, reversal, and bounded directional demand on 10,000
pages. The C++ test exercises the same eviction/admission helper used by the
real texture cache with a one-tile normal budget; it verifies both pin sets,
unrelated LRU eviction, hard caps, and release of old resource references.

Existing P0 FFI/PDFium, P2 tiles, P3 viewport, P4 continuous, P5A thumbnails,
P5B editing, and P5C structural-output smokes passed. P4/P5A/P5B also passed on
the 10,000-page fixture. New atomic smokes passed on mixed vector pages, 10,000
synthetic pages, the scanned 964-page source, and the engineering sheet. The
mixed atomic run includes an inserted external-source page. P5C tests preserve
duplicate/insert, selection, delete/move/rotate, undo/redo, multisource rendering,
Save As, extraction, output worker responsiveness, thumbnail synchronization,
and source immutability. These retained editing/output checks are automated;
they are not a repeat of every command through live dialogs.

An additional 10,000-page structural save produced 10,004 pages in 1,250.88 ms,
including 1,053.63 ms build/write and 187.261 ms verification. It allowed 119
viewer updates during output; maximum measured viewer API time was 0.1269 ms.
Source hashes stayed unchanged. The retained layout baseline is 560,016
accounted bytes for 10,000 pages. Presentation allocates only viewport demand
and bounded cache sets, never per-page textures or UI controls.

Evidence remains locally in `artifacts/p5d/final-rust-checks.log`,
`final-smokes.log`, `large-output.log`, `atomic-*.json`, `live-initial.csv`,
`live-final.csv`, and `latency-summary.json`. Artifacts are ignored build/test
outputs. Run the added checks with:

```text
python scripts/p5d_gpu_policy_smoke.py
python scripts/p5d_presentation_smoke.py target/release/pdfeditor_core.dll tests/fixtures/p4-mixed-pages.pdf tests/fixtures/p5c-external.pdf
python scripts/p5d_presentation_smoke.py target/release/pdfeditor_core.dll artifacts/p4-synthetic-10000.pdf
python scripts/build_winui.py
```

Set `PDFEDITOR_PDFIUM_PATH` to the deployed PDFium DLL for runtime smokes and
`PDFEDITOR_QPDF_PATH` for output checks. Existing fixture-generation procedures
are unchanged. CI now includes the native policy and mixed/10,000-page atomic
smokes, and triggers on `p5d/**` branches.

## Changed files

| Area | Files |
|---|---|
| Rust authoritative state | `crates/document-core/src/presentation.rs` (new), `lib.rs`, `scheduler.rs` |
| Coverage and direction | `crates/document-core/src/layout.rs`, `viewport.rs`; `crates/document-ffi/src/continuous.rs` |
| ABI v9 | `crates/document-ffi/src/presentation.rs` (new), `viewport.rs`, `lib.rs`; `include/pdfeditor_ffi.h` |
| Bridge | `native/winui/NativeCoreBridge.h`, `.cpp` |
| Composition and memory policy | `native/winui/DocumentCanvasRenderer.h`, `.cpp`; `GpuPresentationPolicy.h` (new) |
| Input, commit loop, diagnostics | `native/winui/MainWindow.xaml`, `.xaml.h`, `.xaml.cpp`; `PdfEditor.WinUI.vcxproj` |
| New verification | `scripts/p5d_presentation_smoke.py`, `p5d_gpu_policy_smoke.py`; `tests/gpu_presentation_policy.cpp` |
| ABI regression/CI | `scripts/p0_ffi_smoke.py`, `p2_tile_smoke.py`, `p3_viewport_smoke.py`, `p5b_editing_smoke.py`, `p5c_output_smoke.py`; `.github/workflows/rust-ci.yml` |
| Documentation | `README.md`; this report |

## Remaining experience limits

Expensive destination rendering still produces a deliberate visible hold. Rapid
input can extend that hold across many superseded destinations; the latest
request wins, but this does not guarantee immediate motion. The 30 ms polling
timer, bounded upload batches, geometry refinements, and serialized PDFium work
contribute to latency. The 200%/400% native samples spent most of their measured
interval before all CPU content became available, with additional upload delay.
No backend concurrency change was made.

The worst-case heavy CAD latency, continuous resize distortion, driver/device
failure recovery, and independent subjective comfort remain unqualified by
these measurements. Very large viewports can trigger the documented bounded
memory hold. CSV writing and Present remain small synchronous UI-thread work;
this is a development proof, not a latency guarantee. The complete-frame rule
is implemented and verified, while these timing and acceptance limits remain
visible for follow-up work.
