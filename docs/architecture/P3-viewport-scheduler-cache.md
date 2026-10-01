# P3 — viewport, scheduler, and bounded caches

Implemented on `p3/viewport-scheduler-cache`. The established WinUI 3 →
C++/WinRT → C ABI → Rust document platform → persistent document → PDFium
page-region render → BGRA → Direct3D → SwapChainPanel path is preserved.
P3 remains a single-page proof. No P4, continuous multipage scrolling, thumbnails,
search, annotations, OCR, structural editing, disk cache, or navigation redesign.

## Viewport and pixel identity

`document_core::viewport::ViewportState` contains `PageId`, an explicit
`DevicePoint` origin, `DeviceSize` physical viewport extent, logical scale, DPR,
additional clockwise rotation, and a monotonically increasing nonzero generation.
Origin and extent use rotated physical page pixels. Page dimensions remain
`PageSize` in normalized page points, with source crop/intrinsic rotation applied
by the existing P2 backend. Additional rotation follows the P2 contract.

`tile_demand()` intersects the viewport with the transformed page bounds using
geometry alone. It returns visible cells in row-major order, then a clipped
one-cell surrounding prefetch ring. Right/bottom boundaries are exclusive, so
an exact 512-pixel extent does not demand an unnecessary neighbor. Off-page
viewports return no demand. Invalid/overflowing coordinates, dimensions, scales,
DPR, rotations, and generation zero are rejected. Demand above the configured
queue capacity is rejected before allocating/iterating the grid.

`TileKey` includes document identity/revision, stable page identity, signed grid
indices, normalized physical render scale, rotation, width/height, and render
flags. The scale is the IEEE f64 bits of `scale × DPR`; equivalent products reuse
one key and are rendered with this product and DPR 1. No generation, viewport
origin, priority, or UI state enters the key. There is no arbitrary zoom rounding.
Revision and flags are currently zero because documents are read-only and the
existing backend uses fixed flags. Future pixel-changing options must update
these values together with their backend implementation.

## Scheduler, priority, and generations

One lazily initialized Rust worker per open renderer handles all of its tile
work. Requests never spawn tasks. The backend's existing process-wide PDFium
mutex remains authoritative, including across different documents. Workers share
the original backend through `Arc`; no PDFium document is duplicated. No page
handle cache was introduced. Each render still loads/releases its source page.

P0 visible demand precedes P1 prefetch demand; row-major order breaks ties.
The priority enum leaves a clear extension point for later task classes without
implementing them. Stable sorting plus key deduplication coalesces duplicate
requests. Updates replace queued demand, reset current-generation deliveries,
clear obsolete completions, and immediately refill from cache. An overlapping
running tile is reused and tagged with the latest generation. A running tile
that is no longer demanded finishes safely and is discarded. There is no force
cancellation inside PDFium. Old/duplicate generation updates return an error
without replacing current work.

Default demand/work capacity is 256 keys, configurable up to 4096. Default ready
capacity is 16, configurable up to 256. A full completion queue backpressures
the worker; polling resumes it. Refill reserves one completion slot for a
current visible render already running outside the state lock, preventing
cache hits during an update/poll from overflowing the queue. Prefetch successes populate CPU cache without
producing unnecessary native uploads. Render errors are counted and returned
through the poll status. The native proof displays an error and stops polling
instead of silently leaving a failed tile pending. A subsequent update can retry.

## CPU cache and ownership

The configurable development cache budget defaults to 128 MiB. Deterministic LRU
counts each resident tile's actual pixel-vector length. Current visible keys are
pinned, and non-visible entries are evicted first. An oversized tile, or a tile
that cannot fit because all residents are pinned, is not admitted. The ready
queue can still deliver it. The cache never exceeds its byte budget; pinning
does not override that limit. Cache hits avoid PDFium rendering.

The document registry now clones an `Arc<OpenDocument>` and releases its registry
mutex before operating on the document. Hot viewport updates therefore do not
wait behind a worker holding a document-registry lock. Only one page geometry is
retained in the FFI, and the C++ bridge reuses its opened first-page geometry.
First access to previously uncached page geometry remains a synchronous metadata
query. Opening does not load or render all pages.

ABI v4 adds configure, update, poll, release-lease, and metrics exports. Existing
P0/P1/P2 exports/layouts remain available. Configuration must precede renderer
initialization; zero fields select defaults. All arguments/results are plain C
scalars or opaque checked tokens. No collections or backend handles cross the
ABI. x64 layout assertions cover viewport (72 bytes), key (56), and ready result
(104), alongside retained P2 request assertions.

`poll_ready_tile()` is nonblocking. Success returns an opaque lease, complete key,
generation, dimensions, stride, length, and a const BGRA pointer. An `Arc` retains
the original buffer; there is no extra 1 MiB copy to cross the ABI. The C++ bridge
uploads synchronously and releases through an exception-safe RAII guard. The
cache may keep its own shared reference. Leases survive cache eviction and
document close, and remain valid until explicit release. Stale/double release
is rejected without pointer dereference. There is a process-wide cap of 64
outstanding leases; further polling returns capacity backpressure without
consuming a completion. Poll/update should be coordinated on the native render
thread; consumers must also check generation before use.

CPU-cache bytes are not total application memory: bounded completions, one
running buffer per worker, outstanding leases, source metadata, and PDFium
internals are separate. Shared buffers often overlap these counts. With defaults,
a conservative upper bound for application BGRA ownership with one renderer is
128 MiB cache + 16 MiB ready + 1 MiB running + 64 MiB external leases. PDFium's
internal parsing/decoded-image allocations are not bounded by this cache.

## Native GPU cache and composition

`DocumentCanvasRenderer` retains its D3D11 device and SwapChainPanel. Its cache
uses the same scalar TileKey identity through an explicit field comparator;
padding bytes are never compared. Each BGRA texture is immutable, has one mip,
and has a shader-resource view. Existing keys are reused rather than uploaded
again. Constructor parameters configure byte/count budgets, initially 128 MiB
and 256 textures. Accounted bytes are width × height × 4; driver allocation
overhead, views, and two fixed swap-chain buffers are outside that figure.

Simple LRU evicts old non-current-visible textures. Current visible native-quality
textures are pinned. If a configured budget cannot admit another visible tile,
admission fails explicitly; the proof reports exhaustion instead of exceeding
the limit. Temporary lower-quality fallback textures may be evicted under
pressure so they do not indefinitely prevent current-quality admission.

A small D3D11 textured-quad pipeline composes cached tiles at
`tile_origin × target_scale / tile_scale − viewport_origin`, with viewport
clipping. Exact-quality tiles use their native resolution. Every presented flip
buffer is fully recomposed from retained textures; textures are not recreated
per frame. Same-page/rotation older scales are temporarily drawn first when
current tiles are missing; native target tiles overlay them. Once the visible
current-quality set is complete, fallback textures are no longer drawn.
For a page/rotation change with no compatible fallback transform, the last
presented frame remains until the replacement set is complete. Requested
destination and displayed content therefore stay distinct.

The fixed proof viewport remains 1024 × 1024 physical pixels and compensates
XAML composition scale as in P2. Temporary buttons pan by 256 physical pixels;
checkboxes request 1×/2× zoom and 0°/90° rotation. A weak-captured UI dispatcher
timer polls every 50 ms, uploads at most 16 results per tick, and presents only
when viewport/content changes. Rust never invokes WinUI from worker threads.

## Instrumentation and verification

Metrics expose unique demanded tile requests, hits/misses at viewport update,
backend renders started, stale renders discarded, render errors, CPU cache bytes,
queue depth, ready depth, and process-wide outstanding leases. Hits/misses count
prefetch as well as visible demand; an in-flight coalesced request can count as
a miss without another render. The proof also shows GPU bytes/count, uploads,
and reuse. These are development counters, not telemetry.

Verification on this Windows environment:

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace --all-targets`: 24 tests passed with deployed PDFium
  enabled; all retained P0/P1/P2 tests execute.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo build -p document-ffi --release`: passed.
- WinUI Debug x64 MSBuild: passed with warnings treated as errors. The local
  PATH/Path normalization from P2 was used again.
- Retained synthetic P0 ABI, P0/P1 PDFium, and P2 region smoke scripts: passed.
- New release/PDFium P3 smoke: passed with a 16 MiB CPU budget, 64-key queue,
  and two-result ready queue. It verifies viewport output, overlapping cached
  visible movement without duplicate backend rendering (new prefetch cells
  render independently), identical buffer pointers across leases,
  rapid-generation updates, invalid/stale inputs, memory accounting, 64-lease
  backpressure/recovery, and lease validity after eviction/document close.
  Three final repeat runs passed at exactly the configured 16 MiB CPU cache
  limit; one also recorded an actual stale PDFium render discard.
- Fake-backend deterministic tests cover demand/ring/rotation, invalid inputs,
  priority, coalescing, byte/LRU/pin behavior, cache reuse without renders,
  forced A → B → C stale-running-result rejection, overlapping-running reuse,
  bounded demand/completions, and shared-buffer completion ownership. Lease
  checked-release/layout tests do not require PDFium.
- Live Debug WinUI proof exercised using the computer-use plugin: horizontal
  and vertical pan, cache reuse, pending zoom fallback, completed native zoom,
  pending rotation retention, completed rotation, and return to cached rotation.
  The cached vertical move left renders at 12. Returning to cached rotation
  left renders at 44 and GPU uploads at 27; GPU reuse increased from 10 to 19.
  Observed CPU/GPU accounted bytes were 44 MiB / 27 MiB, with queue/ready depths
  returning to zero. Content remained visible while requests were pending.

The fast vector fixture did not reliably leave an obsolete render running during
UI clicks; the gated fake-backend test proves that case deterministically. CPU
pressure/eviction is tested at small budgets. The live GUI session did not force
GPU-budget exhaustion or driver/device loss. No second-DPR display was available
in this run; existing backend scale/DPR tests remain in place. The interactive
proof was closed after verification.

## Concerns for later measurement

PDFium still loads a page for each tile and globally serializes backend access.
Measure parsing/resource decode and render durations before considering a page
cache or different concurrency. Document close joins its worker and can wait
for one active PDFium call. Per-document worker/cache budgets multiply if a
future caller keeps several renderers open; the current shell uses one.
Rust may render a CPU cache miss even when the matching texture remains only in
the GPU cache; cross-cache residency feedback is deferred. The conservative
float32/2^24 coordinate limits from P2 remain. Device-loss recovery and resizing
the fixed proof canvas are outside this milestone.

## Files changed

- `crates/document-core/src/lib.rs`: expose renderer-independent P3 modules.
- `crates/document-core/src/viewport.rs`: typed viewport/key/demand and tests.
- `crates/document-core/src/scheduler.rs`: worker, bounded CPU cache/completions,
  metrics, and fake-backend tests.
- `crates/document-ffi/src/lib.rs`: shared document ownership, bounded geometry
  metadata, ABI v4; retained synchronous exports.
- `crates/document-ffi/src/viewport.rs`: P3 exports, opaque leases, backpressure,
  layout/release tests.
- `include/pdfeditor_ffi.h`: matching C structs/exports/status/lifetime contract.
- `native/winui/NativeCoreBridge.h`, `NativeCoreBridge.cpp`: ABI binding,
  cached metadata, polling, metrics, RAII release.
- `native/winui/DocumentCanvasRenderer.h`, `DocumentCanvasRenderer.cpp`: bounded
  native texture cache, quad composition, quality fallback/display retention.
- `native/winui/MainWindow.xaml`, `MainWindow.xaml.h`, `MainWindow.xaml.cpp`:
  temporary P3 controls, dispatcher polling, and status metrics.
- `native/winui/PdfEditor.WinUI.vcxproj`: link D3D shader compiler.
- `scripts/p0_ffi_smoke.py`, `scripts/p2_tile_smoke.py`: ABI version assertions.
- `scripts/p3_viewport_smoke.py`: real release/PDFium P3 proof.
- `.github/workflows/rust-ci.yml`: P3 branch and smoke verification.
- `README.md`, this document: milestone contract and verification record.

No push or merge was performed.
