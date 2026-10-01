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
    PDFEDITOR_ERROR_INVALID_TILE_REQUEST = 7
};

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
