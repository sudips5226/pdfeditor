# P5D-C — Adaptive Full-Quality CPU+GPU Predictive Prefetch

Review point on `p5d/adaptive-predictive-prefetch`, based on completed P5D-B
commit `6c761db`. No P6 work, PDFium concurrency change, push, or merge was
performed. Measurements were taken locally on 2 October 2026.

P5D-C makes likely future document viewports GPU-ready before the user requests
them. It preserves Rust-owned requested/displayed state and the existing atomic
composition gate. Preparing a texture does not present a viewport.

## Explicit acceptance answers

1. **Does any reduced-quality navigation path exist? NO.** Main-view predictive
   and mandatory tiles use identical exact scale/DPR/rotation keys and full
   512×512 rasters. Independent thumbnails and compatibility preview exports
   are not main-view destination content.
2. **Were any partial destinations presented? NO.** Both native 192-movement
   traces recorded zero partial destinations. Deterministic gates and all
   atomic-presentation smokes also passed.
3. **Do predicted tiles reach GPU before becoming mandatory? YES.** The native
   adaptive trace proactively admitted 216 textures; 204 were subsequently
   used as mandatory content. The fake-backend test proves these admissions do
   not change DisplayedViewport or commit count. Native predictive CacheTile
   performs no composition/Present.
4. **GPU readiness after direction establishment:** all mandatory tiles in the
   final 22 cold-forward destinations were GPU-resident on arrival: **100%**,
   versus baseline **73.47%**. Across all 96 cold/slow/medium/fast forward
   requests, coverage was **99.52%**, versus **72.73%**. This is tile-weighted
   coverage, not a claim that every first transition or jump is ready.
5. **Continuous forward latency improvement:** cold-forward median fell from
   **2.262 to 1.724 ms** (23.8%); p95 from **50.742 to 6.442 ms** (87.3%).
   Combining cold/slow/medium/fast forward phases, median fell from **2.767 to
   1.643 ms** and p95 from **49.987 to 6.961 ms**. These are grouped
   request-log→final-commit measurements including geometry-refined generations.
6. **Predictive work never used:** at trace end, **6/86 CPU admissions (6.98%)**
   and **12/216 GPU admissions (5.56%)** had not been consumed. Of those,
   **0 CPU and 6 GPU admissions (2.78% of GPU admissions)** had already been
   evicted unused. The other admissions remained resident for possible later
   use. A finite trace cannot establish that those resident tiles are *never*
   used; do not equate outstanding future content with definite waste.
7. **Is serialized PDFium the dominant remaining bottleneck? Not established
   for ordinary scrolling on this scanned region.** Its CPU preparation kept
   up at all measured cadences. The baseline forward trace already had 100%
   CPU coverage; late GPU upload was the measured obstacle. Complex engineering
   content has substantially longer cold raster preparation, so backend
   throughput can still limit an uncached destination. No capacity/saturation
   study or concurrency redesign was performed.
8. **Must the user still pause between ordinary adjacent pages? No pause was
   needed in the measured established scanned-document traversal.** All 22
   established cold-forward requests and all slow/medium/fast requests arrived
   fully GPU-ready. This does not promise smooth arbitrary jumps, large sudden
   wheel displacements, or movement faster than full-quality preparation.

## Prediction model and exact identity

`document-core::prediction::Predictor` is deterministic and owns a rolling set
of at most **64** predictive keys. Its bookkeeping retains only active keys or
keys still in bounded CPU/GPU caches; it does not accumulate document-page
history. Even direct Rust `set_prediction` callers are deduplicated and capped.

Actual wheel, vertical pan/button input, and user scrollbar changes report
direction, document Y and time. Page/thumbnail jumps explicitly reset the lead.
Geometry-induced programmatic scrollbar corrections retain the existing
direction guard and do not report navigation input.

The first directional input establishes position. A second useful input
establishes velocity; there is no mandatory dwell interval. Document-space
velocity is an EWMA: `0.35 × new sample + 0.65 × previous estimate`. Sample time
is floored at 1 ms. A pause over 500 ms, jump, reversal, displacement exceeding
three viewport heights, or incompatible viewport quality resets confidence.

Preparation latency starts at 100 ms and uses an EWMA with 0.2 weight for new
samples. Samples include mandatory preparation misses and prediction→GPU
admission time, incorporating serialized rendering and UI upload delay. Samples
are clamped to 10–500 ms. Cached microsecond commits do not erase the estimate
of preparing new content.

The nominal lead is `abs(velocity) × preparation_seconds × 1.5`. Viewport depth
is `ceil(lead / document_viewport_height + 0.5)`, clamped to **1–4** and further
reduced by queue/cache capacity. Reported lead is the resulting whole-viewport
distance. Prediction can be disabled at depth zero by confidence or pressure.

`DocumentLayout::demand_predictive` generates V+1 through V+depth in confirmed
direction, one viewport height apart. It computes each window's exact mandatory
tile coverage and deduplicates against the current mandatory set. V+1 gets
nearest prediction priority; more distant windows get PredictiveFar. The old
ordinary nearby ring remains lower priority. Predictions are bounded by 64 keys
and the existing 256-demand capacity.

Future windows with unknown size or intrinsic rotation first request bounded
lazy geometry: at most four pages per window and four windows. They produce no
predictive render keys until exact geometry is known. Refinement recalculates
placement and generations; pixel reuse still requires exact TileKey identity.
Scale bits, DPR-derived physical scale, source/document revision, stable PageId,
editing/viewer rotation and tile coordinates retain their existing semantics.
Duplicate and externally inserted pages go through the established PagePlan
and SourceRegistry, not assumptions about adjacent source-PDF page indexes.

## Scheduler, CPU preparation, and proactive GPU uploads

There is still one document render worker and serialized PDFium access. The
effective order is:

1. Newest mandatory document rendering.
2. Mandatory UI upload and atomic commit.
3. Nearest exact predictive render/upload.
4. Farther predictive render/upload.
5. Ordinary nearby main-view CPU prefetch.
6. Visible then speculative thumbnail work in its separate bounded lane.

Pending mandatory upload/geometry/presentation prevents new speculative render
or thumbnail work. An already-running PDFium call can finish. Main backpressure
also prevents a full completion queue from being bypassed by thumbnails.

Prediction uses the normal **128 MiB CPU cache**. CPU demand still pins current
mandatory content and bounds queue/ready publication; main completion capacity
remains 16. GPU-resident exact keys skip CPU delivery and backend rendering.

The scheduler publishes active predicted completions with current generation
and separate upload intent. `poll_ready_tile` drains only mandatory content;
the additive `poll_predictive_tile` drains only active predictions and only when
presentation is stable. Both use existing tile leases, with no copied CPU
buffer and the existing 64-lease ABI bound.

A completion callback is invoked **after releasing the scheduler mutex**. The
native bridge callback only posts nonblocking DispatcherQueue work. It does
not enter the core or touch Direct3D from the render thread. Document close
joins the worker before callback context can be destroyed.

The UI pump is coalesced, low dispatcher priority, and checks a deadline before
polling each lease: mandatory work has up to 16 uploads and an 8 ms total target;
predictive work has up to four uploads, a 2 ms target, and shares the 8 ms total
target. Commit is attempted before and after predictive draining. Native
CacheTile admits speculative textures without ComposeViewport, Present, or
commit acknowledgement. The 30 ms timer remains a fallback and services
geometry/thumbnails/output. Follow-up callbacks are posted only when actionable
ready work remains, avoiding pending-geometry and memory-blocked busy loops.

Deadline checks bound admission of further work. A single synchronous
CreateTexture2D/driver call or Present cannot be interrupted; these time targets
are not a hard real-time guarantee. The measured 964-page pump p95/max were
**2.158/5.465 ms**, below the 8 ms target. Logged pump time covers draining and
the first commit attempt; it excludes subsequent status/log/thumbnail work.

For an already GPU-cached destination, UpdateViewport immediately follows the
existing valid composition/commit path. It does not wait for a timer, render a
tile, copy a CPU tile, or upload a texture. The dedicated test uses a backend
that panics if called, and verifies zero publications/renders across cached
movements. The ABI smoke verifies zero mandatory render/upload counters and
microsecond core commits; the native trace separately measures real composition.

## Reversal, pressure, and eviction

Reversal retires forward prediction and replaces pending demand with the latest
viewport. Native SetViewport replaces predictive preferences immediately. Old
textures may remain as ordinary reusable cache entries, without predictive
protection. An in-flight obsolete call may finish, but its old publication
cannot commit. Reverse-side work is rebuilt after actual reverse cadence is
established, and mandatory content precedes it.

GPU limits remain **128 MiB / 256 entries normal**, plus the existing bounded
mandatory presentation reserve of **128 MiB / 128 additional entries**, with
the existing absolute residency cap of 512. Predictive uploads can use **only
normal capacity**. Displayed and pending exact mandatory sets remain hard
protected. Prediction is a soft eviction preference: ordinary unprotected LRU
entries are sacrificed first, then predictive entries; mandatory pins are never
evicted to accommodate speculation. A failed speculative admission is skipped,
without marking mandatory presentation memory-blocked.

The predictor receives normal capacity after accounting for the
displayed/pending union. It uses the minimum CPU/GPU byte capacity, subtracts
mandatory tiles, caps speculation at 64, and limits depth by the number of
complete mandatory-sized windows that fit. Insufficient capacity makes depth
zero. Queue depth above 24 limits look-ahead to two windows; above 48 limits it
to one. Each new generation replaces demand rather than appending a backlog.

Tests exercise a 4 MiB CPU cache, 100 rapid destinations while a predictive
render is blocked, a bounded nine-key rolling queue, latest mandatory priority,
stale rejection and latest commit. Native shared eviction-policy tests shrink
capacity and prove predictive textures are sacrificed before displayed/pending
pins, with real resource lifetime and hard accounting checks.

Zoom, DPR, viewport-size and viewer rotation changes reset prediction confidence.
Structural/rotation edits invalidate placement and prediction via the existing
PagePlan path. Old exact textures may remain ordinary entries; they are never
readiness for a different-quality request.

## Native comparison protocol and measurements

Source: local **964-page** scanned document
`C:\Users\sudip\Desktop\9800-101-IEX-DZ-ORP-0001-015_edited.pdf`.
Both comparison processes were fresh WinUI Debug x64 instances, same default
1069×615 captured window, scale 1 and DPR 2. The trace starts at page 200 and
jumps to page 750 for the final phase. Movement is 240 physical pixels
(120 document units) per tick, 24 inputs per phase:

| Phase | Cadence | Baseline median/p95 ms | Adaptive median/p95 ms | Baseline/adaptive GPU tile coverage | Baseline/adaptive missing-GPU arrivals |
|---|---:|---:|---:|---:|---:|
| Cold forward | 180 ms | 2.262 / 50.742 | 1.724 / 6.442 | 73.58% / 98.11% | 12 / 1 |
| Immediate reverse | 180 ms | 1.168 / 1.476 | 1.222 / 1.651 | 100% / 100% | 0 / 0 |
| Warm forward | 180 ms | 1.141 / 1.357 | 1.326 / 1.729 | 100% / 100% | 0 / 0 |
| Slow forward | 450 ms | 2.181 / 50.934 | 1.490 / 5.187 | 71.70% / 100% | 12 / 0 |
| Medium forward | 180 ms | 2.351 / 48.419 | 1.684 / 8.704 | 74% / 100% | 12 / 0 |
| Fast forward | 70 ms | 3.772 / 36.928 | 1.766 / 6.939 | 71.70% / 100% | 13 / 0 |
| Direction reversal | 180 ms | 1.170 / 1.364 | 1.420 / 1.724 | 100% / 100% | 0 / 0 |
| Distant jump, then forward | 180 ms | 4.976 / 37.900 | 1.727 / 77.124 | 73.47% / 97.96% | 13 / 1 |

Each mode committed all 192 input sequences. Durations were 41.70 s baseline
and 42.39 s adaptive. Both had one initial unknown-geometry arrival at the
distant jump, in addition to the missing-GPU arrival counts above. That jump
still held the old complete frame; it was not predicted. Across the entire
trace, missing-GPU arrivals fell **62→2**, longest tracked hold **119.957→107.327
ms**. The longest hold was a jump, so it should not be represented as ordinary
adjacent scroll latency. Cold-forward maximum tracked hold fell **54.198→4.256
ms**. A tracked hold includes preparation/composition elapsed time even for
microsecond-ready requests; it is not an independently measured human-visible
freeze duration.

The baseline is the **same instrumented native executable with Debug Ctrl+P
baseline mode**: P5D-B one-window CPU-only directional demand, speculative GPU
uploads off, completion-driven pump off, and the 30 ms upload timer. It is a
controlled comparison of preparation policies, not an independent historical
binary benchmark. Ctrl+B runs the eight phases through real Rust layout,
PDFium, Direct3D texture creation, composition and Present. It generates timed
navigation inputs, not injected physical wheel/scrollbar events. Actual wheel
forward/reverse checks were performed separately. The distant jump uses the
same core jump/reset path as Go rather than a synthetic scrollbar drag.

This is **one paired local Debug trace**, not a multi-run confidence estimate.
Requests are grouped by actual input sequence, so geometry-refined generations
do not inflate sample counts. First request-log→last commit-log elapsed time
includes refinement, while per-generation core latency is recorded separately.
The first log occurs after request setup, so neither is a complete physical
input-to-photon measurement. Percentiles use nearest rank. Fully cached native
composition remains around 1–2 ms; warm-forward median/p95 increased by
0.185/0.372 ms in this single instrumented trial. That small increase is
reported rather than hidden; the no-render/no-upload/no-timer invariant passed.

| Resource/counter over trace | Baseline | Adaptive |
|---|---:|---:|
| Main renders | 231 | 231 |
| Native GPU uploads | 213 | 225 |
| Main CPU peak | 128 MiB | 128 MiB |
| GPU texture peak | 129 MiB | 129 MiB |
| Upload-pump p95 / maximum | 3.181 / 6.931 ms | 2.158 / 5.465 ms |
| Partial destinations | 0 | 0 |
| Uncommitted input sequences | 0 | 0 |
| Predictive CPU completed / consumed | 0 / 0 | 86 / 80 |
| Predictive GPU admitted / consumed | 0 / 0 | 216 / 204 |
| Predictive CPU/GPU evicted unused | 0 / 0 | 0 / 6 |
| Retired predictive key memberships | 0 | 360 |

The 129 MiB GPU peak uses the unchanged mandatory reserve; prediction does not
expand the normal cache. Retired memberships count rolling-window exits and
reversals, including already useful keys, not 360 wasted renders. CPU usefulness
is **93.02%**, GPU usefulness **94.44%**, and definite GPU eviction waste
**2.78%**. Predictive GPU admission exceeds predictive CPU completions because
it also uploads exact CPU tiles already prepared by ordinary prefetch.

The baseline had 100% CPU coverage for all 96 forward movements. That is direct
evidence that proactive GPU admission, rather than additional render threads,
addresses the measured recurring hold. Adaptive depth was one on this region
because measured preparation was short enough; deterministic velocity/latency
tests exercise contraction and depths two through four.

The fast phase consumed 39 new predictive GPU admissions over approximately
2.02 s between its first and last input (about 19.3 tiles/s). Rendering started
42 main tiles in the same interval (about 20.8 tiles/s), with zero GPU lead
deficit at the 24 arrivals. These are demand-observed rates, not backend maximum
throughput. Whole-trace rendering averaged 5.45 tiles/s because it includes
cached reversal and slow phases. Saturation and higher-quality-scale throughput
remain unmeasured; holds are still correct when consumption outruns preparation.

Raw local evidence lives in ignored `artifacts/p5dc/native-baseline.csv`,
`native-adaptive.csv` and `native-comparison.json`. Reconstruct with:

```powershell
python scripts/p5dc_analyze_trace.py artifacts/p5dc/native-baseline.csv artifacts/p5dc/native-adaptive.csv artifacts/p5dc/native-comparison.json
```

## Live sources and validation

The native scanned trace crossed new adjacent page boundaries without a
one-second pause. The 22 established cold-forward destinations were fully
resident on arrival. A later single 600-unit wheel movement into new content
after a long pause correctly reset confidence and incurred a cold hold; reversing
that wheel movement committed the cached exact frame in approximately 1.8 ms.
This separates ordinary steady preparation from unpredicted discontinuities.

The locally available engineering source was
`C:\Users\sudip\Desktop\Annexure-3-PID_page_1.pdf`. Native full-quality opening
resolved one page and presented exact content with CPU-ready around 223 ms and
commit around 230 ms for the final geometry-refined generation (whole tracked
cold hold about 356 ms). The viewport showed sharp vector drawing lines and
labels when panned into the drawing. Cached pan remained immediate; no partial
destination or reduced-quality path appeared. This is a within-page engineering
check, not a multi-page engineering A/B. A repeated-run developer timer issue
was corrected by creating a fresh timer per benchmark; the affected engineering
timed traversal was excluded from the comparison. The two scanned A/B runs
were first runs in separate processes and were unaffected.

The real-PDFium ABI predictive smoke passed on mixed vector pages with an
inserted external source, the 10,000-page architecture fixture, the 964-page
source, and the engineering sheet. Its GPU set is **explicitly simulated
bounded exact-key residency**; its microsecond commits are correctness
measurements, not Direct3D performance numbers. It verifies advance admission,
six cached adjacent movements with zero mandatory render/upload work,
reversal/zoom/rotation reset, depth zero with a 1 MiB budget, exact leases and
zero partial destinations. Native traces provide the real GPU measurements.

Final checks passed:

- `cargo fmt --all -- --check`.
- `cargo test --workspace --all-targets`: **60 core + 17 FFI tests**, zero failures,
  with deployed PDFium/qpdf configured for backend tests.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo build -p document-ffi --release`.
- WinUI Debug x64 build, isolated output/intermediate directories to avoid
  concurrent Visual Studio compiler-PDB collisions.
- P0 FFI/PDFium, P2 tile, P3 viewport/cache, P4 continuous/10,000 pages,
  P5A thumbnails/10,000 pages, P5B editing/10,000 pages, P5C structural output,
  native GPU pinning policy, P5D-B atomic presentation with inserted source and
  10,000 pages, and new P5D-C smokes on all four sources.
- P5C large output retained: 10,000→10,004 pages, about 1.89 s total output,
  177 concurrent viewer updates, source hashes unchanged.
- `git diff --check`; no source PDFs or generated output were added to Git.

Regression output is in ignored `artifacts/p5dc/validation.log`; the final
native build log is `artifacts/p5dc/native-build-final.log`. The review executable
and runtime are in `artifacts/p5dc-final/`.

## Metrics, ABI, and changed files

ABI **10** adds a 184-byte prediction snapshot, actual-navigation input,
prediction capacity/configuration, worker ready notification and predictive tile
polling. Existing structures retain their layouts. The native bridge validates
the version and struct size; scripts' ABI assertions were updated.

Developer metrics now expose direction, velocity, estimated preparation latency,
lead/depth, active predictive tiles/CPU/GPU coverage, mandatory entry prediction
hits, CPU/GPU consumed and unused-eviction counts and rates, predictive GPU bytes,
and upload maximum. Debug `prediction-events.csv` records request/commit/pump
events, actual input sequence, generation, counters, preparation/request timing,
lead and coverage, wall time, benchmark step and policy mode. Prediction/CPU/GPU
times share the predictor's monotonic epoch; the request snapshot records the
latest relevant tile times, not a per-tile latency distribution. Counters permit
aggregate analysis without unbounded in-memory per-page records. Existing
presentation CSV remains. No production telemetry was added.

Changed files:

- Core: new `prediction.rs`; `layout.rs`, `scheduler.rs`, `viewport.rs`, `lib.rs`.
- FFI: new `prediction.rs`; `continuous.rs`, `viewport.rs`, `lib.rs`;
  `include/pdfeditor_ffi.h`.
- WinUI: `NativeCoreBridge.{h,cpp}`, `DocumentCanvasRenderer.{h,cpp}`,
  `GpuPresentationPolicy.h`, `MainWindow.xaml.{h,cpp}`.
- Validation: new `scripts/p5dc_prediction_smoke.py` and
  `scripts/p5dc_analyze_trace.py`; `tests/gpu_presentation_policy.cpp`;
  ABI assertions in P0/P2/P3/P5B/P5C scripts; CI predictive smokes.
- Documentation: this report and the README milestone entry.

Remaining limitations are cold jumps, unknown geometry, nonpreemptible ongoing
PDFium/driver work, and higher consumption than full-quality preparation can
sustain. The scanned-region result supports review of this preparation policy;
it does not justify changing PDFium serialization or beginning P6.
