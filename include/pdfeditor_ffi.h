#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef _WIN32
  #ifdef PDFEDITOR_CORE_EXPORTS
    #define PDFEDITOR_API __declspec(dllexport)
  #else
    #define PDFEDITOR_API __declspec(dllimport)
  #endif
#else
  #define PDFEDITOR_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef struct PdfeditorTile {
    uint32_t width;
    uint32_t height;
    uint32_t stride;
    size_t len;
    uint8_t* data;
} PdfeditorTile;

typedef struct PdfeditorDocument PdfeditorDocument;

typedef struct PdfeditorPageGeometry {
    uint64_t page_id;
    double width_points;
    double height_points;
    uint16_t rotation_degrees;
} PdfeditorPageGeometry;

/* ABI v3. PageId belongs to this open document. Scale is logical pixels per
   point; DPR converts to physical pixels. Signed grid indices are measured
   after rotation. Rotation adds 0/90/180/270 clockwise to the source rotation.
   P2 accepts width=height=512 only. Outside-page pixels are opaque white. */
typedef struct PdfeditorTileRequest {
    uint64_t page_id;
    int32_t tile_x;
    int32_t tile_y;
    double scale;
    double device_pixel_ratio;
    uint16_t rotation_degrees;
    uint32_t width;
    uint32_t height;
} PdfeditorTileRequest;

enum {
    PDFEDITOR_OK = 0,
    PDFEDITOR_ERROR_NULL_ARGUMENT = 1,
    PDFEDITOR_ERROR_INTERNAL = 2,
    PDFEDITOR_ERROR_INVALID_UTF8 = 3,
    PDFEDITOR_ERROR_PDFIUM = 4,
    PDFEDITOR_ERROR_INVALID_HANDLE = 5,
    PDFEDITOR_ERROR_INVALID_PAGE = 6,
    PDFEDITOR_ERROR_INVALID_TILE_REQUEST = 7,
    PDFEDITOR_NO_TILE = 8,
    PDFEDITOR_ERROR_VIEWPORT = 9,
    PDFEDITOR_ERROR_CAPACITY = 10,
    PDFEDITOR_ERROR_STALE_GENERATION = 11,
    PDFEDITOR_ERROR_NO_SELECTION = 12,
    PDFEDITOR_ERROR_EDIT_ARGUMENT = 13,
    PDFEDITOR_ERROR_LAST_PAGE = 14,
    PDFEDITOR_ERROR_NO_HISTORY = 15,
    PDFEDITOR_ERROR_OUTPUT = 16,
    PDFEDITOR_ERROR_OUTPUT_BUSY = 17,
    PDFEDITOR_ERROR_SOURCE = 18,
    PDFEDITOR_ERROR_SOURCE_TARGET = 19,
    PDFEDITOR_ERROR_OVERWRITE = 20
};

/* ABI v4. All viewport coordinates/extents are rotated physical page pixels.
   Generation must strictly increase. Invalid updates leave current work intact. */
typedef struct PdfeditorViewport {
    uint64_t page_id;
    double origin_x, origin_y, width, height, scale, device_pixel_ratio;
    uint64_t generation;
    uint16_t rotation_degrees;
} PdfeditorViewport;

/* Scale x DPR is normalized into physical_scale_bits (IEEE f64). */
typedef struct PdfeditorTileKey {
    uint64_t document_id, document_revision, page_id, physical_scale_bits;
    int32_t tile_x, tile_y;
    uint32_t width, height, render_flags;
    uint16_t rotation_degrees;
} PdfeditorTileKey;
typedef struct PdfeditorTileLease PdfeditorTileLease;
typedef struct PdfeditorReadyTile {
    PdfeditorTileLease* lease;
    PdfeditorTileKey key;
    uint64_t generation;
    uint32_t width, height, stride;
    size_t len;
    const uint8_t* data;
} PdfeditorReadyTile;
typedef struct PdfeditorRendererConfig {
    size_t cpu_byte_budget;
    uint32_t queue_capacity, completion_capacity;
} PdfeditorRendererConfig;
typedef struct PdfeditorMetrics {
    uint64_t tile_requests, cache_hits, cache_misses, renders_performed;
    uint64_t stale_renders_discarded, render_errors;
    size_t cpu_cache_bytes, queue_depth, completion_depth, outstanding_leases;
} PdfeditorMetrics;

/* ABI v5. Document origins/gap/extents use unscaled f64 points. Viewport
   width/height are physical pixels. Estimated geometry is marked explicitly. Size-known pages may temporarily
   have unknown intrinsic rotation metadata; PDFium still renders it correctly.
   All P0-P3 exports remain available with their original struct layouts. */
typedef struct PdfeditorDocumentViewport {
    double origin_x, origin_y, width, height, scale, device_pixel_ratio, page_gap;
    uint64_t generation;
    uint16_t rotation_degrees;
} PdfeditorDocumentViewport;
typedef struct PdfeditorPageLayout {
    uint64_t page_id;
    uint32_t index, geometry_known;
    double x, y, width, height, page_width, page_height, spacing_before, spacing_after;
    uint16_t intrinsic_rotation, effective_rotation;
    uint32_t intrinsic_rotation_known;
} PdfeditorPageLayout;
typedef struct PdfeditorLayoutSnapshot {
    double origin_x, origin_y, extent_width, extent_height;
    uint64_t open_micros, layout_init_micros, geometry_micros, geometry_queries;
    size_t metadata_bytes;
    uint32_t page_count, current_page, returned_pages, visible_pages, visible_tiles, known_pages;
} PdfeditorLayoutSnapshot;
/* ABI v9. Requested and displayed are authoritative Rust state. Timestamps are
   monotonic microseconds since presentation initialization, zero=not reached.
   Native coordinates update immediately; Present waits for exact visible GPU
   coverage AND resolved visible geometry. Existing ABI layouts are unchanged. */
typedef struct PdfeditorPresentationSnapshot {
    PdfeditorDocumentViewport requested, displayed;
    uint64_t requested_at, render_started_at, cpu_ready_at, gpu_ready_at, committed_at;
    uint64_t hold_micros, commit_count, stale_destinations, coalesced_requests, partial_presentations;
    uint32_t requested_page, displayed_page, required_count, cpu_count, gpu_count, missing_count;
    uint32_t state, geometry_ready;
} PdfeditorPresentationSnapshot;
/* Report complete actual resident texture keys before requests/after uploads.
   Count <=512; null keys allowed for count=0. Coordinate on one UI/render thread. */
PDFEDITOR_API int32_t pdfeditor_document_gpu_residency(PdfeditorDocument*, const PdfeditorTileKey*, uint32_t count);
/* Capacity <=4096; returns required_count exact mandatory keys. Output clears on
   failure; separate input/output storage. State 0 stable, 1 pending, 2 ready. */
PDFEDITOR_API int32_t pdfeditor_document_presentation_snapshot(PdfeditorDocument*, PdfeditorPresentationSnapshot*, PdfeditorTileKey*, uint32_t capacity);
/* Successful complete native Present must precede this acknowledgment. */
PDFEDITOR_API int32_t pdfeditor_document_commit_presentation(PdfeditorDocument*, uint64_t generation);
PDFEDITOR_API int32_t pdfeditor_document_navigation_direction(PdfeditorDocument*, int32_t direction);
/* Bounded visible + one neighboring page snapshot; capacity <= 64.
   Generation strictly increases. Unknown pages schedule metadata only.
   Poll/update calls must be coordinated on one UI/render thread. No PDFium
   calls or waits for rendering occur in these exports. Output storage must
   not alias inputs. Snapshot is cleared on errors. Page output is valid only
   on success, for returned_pages entries. Current page is zero-based; the
   viewport center selects the preceding page when it lies in a gap.
   Extent is provisional until geometry is known; returned origin preserves
   the local top-of-viewport offset during refinement, then clamps to extent. */
PDFEDITOR_API int32_t pdfeditor_document_update_continuous_viewport(
    PdfeditorDocument*, const PdfeditorDocumentViewport*, PdfeditorLayoutSnapshot*,
    PdfeditorPageLayout* pages, uint32_t capacity);
PDFEDITOR_API int32_t pdfeditor_document_layout_needs_refresh(PdfeditorDocument*, uint32_t*);
PDFEDITOR_API int32_t pdfeditor_document_go_to_page(PdfeditorDocument*, uint32_t index,
    const PdfeditorDocumentViewport*, double* out_x, double* out_y);

/* Configure before update; zero fields use 128 MiB / 256 queued / 16 ready.
   Reconfiguration after renderer initialization is rejected. */
PDFEDITOR_API int32_t pdfeditor_document_configure_renderer(
    PdfeditorDocument* document, const PdfeditorRendererConfig* config);
PDFEDITOR_API int32_t pdfeditor_document_update_viewport(
    PdfeditorDocument* document, const PdfeditorViewport* viewport);
/* Output is cleared on errors. NO_TILE is normal nonblocking empty polling.
   Poll and viewport updates should be coordinated on the same UI/render thread.
   Success lends immutable pixels until release, even after document close.
   Maximum 64 outstanding leases process-wide; further polls return CAPACITY.
   Input/output storage must not alias; never overwrite an unreleased lease. */
PDFEDITOR_API int32_t pdfeditor_document_poll_ready_tile(
    PdfeditorDocument* document, PdfeditorReadyTile* out_tile);
PDFEDITOR_API int32_t pdfeditor_tile_lease_release(PdfeditorTileLease* lease);
PDFEDITOR_API int32_t pdfeditor_document_renderer_metrics(
    PdfeditorDocument* document, PdfeditorMetrics* out_metrics);

/* ABI v6: fixed logical thumbnail rows, explicit overscan, independent generation.
   Keys contain physical bounding dimensions and DPR. Intrinsic rotation belongs
   to immutable PageId/revision; rotation adds the viewer quarter turn.
   At most 64 slots; 32 MiB default dedicated raster cache; one shared render worker.
   Only visible completions publish; prefetch populates the cache. No backend calls
   occur during update/show-current. Old exports/layouts are retained. */
typedef struct PdfeditorThumbnailViewport {
    double offset, extent, width, height, label_height, gap, padding, dpr;
    uint64_t generation;
    uint32_t current, overscan;
    uint16_t rotation;
} PdfeditorThumbnailViewport;
typedef struct PdfeditorThumbnailKey {
    uint64_t document_id, revision, page_id, dpr_bits;
    uint32_t width, height, flags;
    uint16_t rotation;
} PdfeditorThumbnailKey;
typedef struct PdfeditorThumbnailItem {
    PdfeditorThumbnailKey key;
    uint64_t recycle;
    double top;
    uint32_t index, current, visible, selected;
} PdfeditorThumbnailItem;
typedef struct PdfeditorThumbnailSnapshot {
    double offset, total;
    uint32_t returned, visible;
} PdfeditorThumbnailSnapshot;
typedef struct PdfeditorReadyThumbnail {
    PdfeditorTileLease* lease;
    PdfeditorThumbnailKey key;
    uint64_t generation, recycle;
    int32_t status;
    uint32_t stride;
    size_t len;
    const uint8_t* data;
} PdfeditorReadyThumbnail;
typedef struct PdfeditorThumbnailMetrics {
    uint64_t hits, misses, renders, recycled, stale, errors;
    size_t bytes, queue, ready;
    uint32_t slots, visible, current;
} PdfeditorThumbnailMetrics;
PDFEDITOR_API int32_t pdfeditor_document_configure_thumbnails(PdfeditorDocument*, size_t byte_budget);
/* Output snapshot is cleared on error. Capacity <=64. Items valid only on success.
   Coordinate update and polling on one UI thread. Generation strictly increases. */
PDFEDITOR_API int32_t pdfeditor_document_update_thumbnails(PdfeditorDocument*, const PdfeditorThumbnailViewport*,
    PdfeditorThumbnailSnapshot*, PdfeditorThumbnailItem*, uint32_t capacity);
/* NO_TILE means empty. Nonzero completion status is a clickable error placeholder
   with key/generation/recycle and no lease. Success pixels use the existing checked
   lease release and 64 outstanding process cap. Never overwrite a held lease. */
PDFEDITOR_API int32_t pdfeditor_document_poll_ready_thumbnail(PdfeditorDocument*, PdfeditorReadyThumbnail*);
PDFEDITOR_API int32_t pdfeditor_document_thumbnail_show_current(PdfeditorDocument*, const PdfeditorThumbnailViewport*, double*);
PDFEDITOR_API int32_t pdfeditor_document_thumbnail_sync_current(PdfeditorDocument*, uint32_t current);
PDFEDITOR_API int32_t pdfeditor_document_thumbnail_metrics(PdfeditorDocument*, PdfeditorThumbnailMetrics*);

PDFEDITOR_API uint32_t pdfeditor_abi_version(void);
PDFEDITOR_API int32_t pdfeditor_render_test_tile(PdfeditorTile* out_tile);
/* On success, close the returned handle once. Null/stale handles return an error. */
PDFEDITOR_API int32_t pdfeditor_document_open_utf8(
    const char* pdf_path_utf8,
    PdfeditorDocument** out_document);
PDFEDITOR_API int32_t pdfeditor_document_close(PdfeditorDocument* document);
PDFEDITOR_API int32_t pdfeditor_document_page_count(
    PdfeditorDocument* document,
    uint32_t* out_count);
PDFEDITOR_API int32_t pdfeditor_document_page_geometry(
    PdfeditorDocument* document,
    uint32_t page_index,
    PdfeditorPageGeometry* out_geometry);
PDFEDITOR_API int32_t pdfeditor_document_render_page_preview(
    PdfeditorDocument* document,
    uint32_t page_index,
    PdfeditorTile* out_tile);
/* Request and output storage must be separate. Output is cleared on failure.
   On success Rust owns the pixels until pdfeditor_tile_free is called. */
PDFEDITOR_API int32_t pdfeditor_document_render_tile(
    PdfeditorDocument* document,
    const PdfeditorTileRequest* request,
    PdfeditorTile* out_tile);
/* Frees tile pixels and clears the tile; safe to call again on the cleared tile. */
PDFEDITOR_API void pdfeditor_tile_free(PdfeditorTile* tile);

/* ABI v7: logical editing only. Source PDF remains immutable.
   Errors: 12 no selection, 13 invalid command/destination, 14 last page,
   15 no undo/redo; stale PageId/token uses existing error 6. */
typedef struct PdfeditorEditorStatus {
    uint64_t revision, selection_revision, current_page_id;
    size_t history_bytes;
    uint32_t page_count, current_index, selected_count, undo_depth, redo_depth, structural_dirty;
} PdfeditorEditorStatus;
PDFEDITOR_API int32_t pdfeditor_document_editor_status(PdfeditorDocument*, PdfeditorEditorStatus*);
/* mode: 0 plain, 1 Ctrl toggle, 2 Shift range; recycle=0 skips card validation. */
PDFEDITOR_API int32_t pdfeditor_document_select_page(PdfeditorDocument*, uint64_t page_id, uint64_t recycle, uint32_t mode);
/* 1 delete, 2 move before original zero-based boundary (count=end),
   3 rotate signed degrees (-270/-180/-90/90/180/270), 4 undo, 5 redo, 6 select all.
   One successful structural action is one bounded history transaction. */
PDFEDITOR_API int32_t pdfeditor_document_edit(PdfeditorDocument*, uint32_t command, int32_t argument, PdfeditorEditorStatus*);
PDFEDITOR_API int32_t pdfeditor_document_configure_history(PdfeditorDocument*, uint32_t count, size_t bytes);


/* ABI v8. Rust owns snapshots, one output worker/document and source lifetime.
   Kind Save As=1, Extract=2. Output never writes into an active source.
   Phases idle=0, snapshot=1, opening=2, building=3, writing=4, verifying=5,
   finalizing=6, succeeded=7, failed=8, cancel requested=9, cancelled=10.
   Cancellation takes effect at native-operation boundaries, before finalization.
   Status strings are bounded, NUL-terminated UTF-8 (possibly truncated).
   Save success changes only the saved baseline; Extract changes no editor state. */
typedef struct PdfeditorOutputStatus {
    uint64_t document_id, snapshot_fingerprint, saved_fingerprint, current_revision;
    uint64_t snapshot_revision, snapshot_micros, build_write_micros, verification_micros;
    uint64_t elapsed_micros, output_bytes;
    size_t coordination_bytes;
    uint64_t saved_revision;
    uint32_t phase, kind, percent, page_count, source_count, registered_sources, temp_exists;
    int32_t error_code;
    char target_utf8[1024], error_utf8[2048];
} PdfeditorOutputStatus;
/* Insert after a UI-chosen page using an explicit zero-based boundary.
   count=0 inserts all source pages, otherwise indices contains count source
   indices. Redo restores the same PageIds. Duplicate is edit command 7. */
PDFEDITOR_API int32_t pdfeditor_document_insert_source(PdfeditorDocument*, const char* path_utf8,
    uint32_t boundary, const uint32_t* indices, uint32_t count, PdfeditorEditorStatus*);
PDFEDITOR_API int32_t pdfeditor_document_output_start(PdfeditorDocument*, const char* target_utf8,
    uint32_t kind, uint32_t explicit_overwrite);
PDFEDITOR_API int32_t pdfeditor_document_output_status(PdfeditorDocument*, PdfeditorOutputStatus*);
PDFEDITOR_API int32_t pdfeditor_document_output_cancel(PdfeditorDocument*);

#ifdef __cplusplus
}
#endif
