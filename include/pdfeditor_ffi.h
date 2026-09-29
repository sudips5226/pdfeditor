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

enum {
    PDFEDITOR_OK = 0,
    PDFEDITOR_ERROR_NULL_ARGUMENT = 1,
    PDFEDITOR_ERROR_INTERNAL = 2
};

PDFEDITOR_API uint32_t pdfeditor_abi_version(void);
PDFEDITOR_API int32_t pdfeditor_render_test_tile(PdfeditorTile* out_tile);
PDFEDITOR_API void pdfeditor_tile_free(PdfeditorTile* tile);

#ifdef __cplusplus
}
#endif
