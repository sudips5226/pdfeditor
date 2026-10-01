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
    PDFEDITOR_ERROR_STALE_GENERATION = 11
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

#ifdef __cplusplus
}
#endif
