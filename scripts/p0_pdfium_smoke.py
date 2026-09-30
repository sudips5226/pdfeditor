"""P0 runtime smoke test for the real PDFium rendering path.

This script is CI-only tooling. Python is not part of the product runtime.
"""

from __future__ import annotations

import ctypes
import pathlib
import sys


class PdfeditorTile(ctypes.Structure):
    _fields_ = [
        ("width", ctypes.c_uint32),
        ("height", ctypes.c_uint32),
        ("stride", ctypes.c_uint32),
        ("len", ctypes.c_size_t),
        ("data", ctypes.POINTER(ctypes.c_uint8)),
    ]


class PdfeditorPageGeometry(ctypes.Structure):
    _fields_ = [
        ("page_id", ctypes.c_uint64),
        ("width_points", ctypes.c_double),
        ("height_points", ctypes.c_double),
        ("rotation_degrees", ctypes.c_uint16),
    ]


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit(
            "usage: p0_pdfium_smoke.py <pdfeditor_core.dll> <fixture.pdf>"
        )

    core_path = pathlib.Path(sys.argv[1]).resolve()
    pdf_path = pathlib.Path(sys.argv[2]).resolve()
    if not core_path.is_file():
        raise SystemExit(f"missing native core DLL: {core_path}")
    if not pdf_path.is_file():
        raise SystemExit(f"missing PDF fixture: {pdf_path}")

    core = ctypes.CDLL(str(core_path))

    core.pdfeditor_document_open_utf8.argtypes = [ctypes.c_char_p, ctypes.POINTER(ctypes.c_void_p)]
    core.pdfeditor_document_open_utf8.restype = ctypes.c_int32
    core.pdfeditor_document_close.argtypes = [ctypes.c_void_p]
    core.pdfeditor_document_close.restype = ctypes.c_int32
    core.pdfeditor_document_page_count.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32)]
    core.pdfeditor_document_page_count.restype = ctypes.c_int32
    core.pdfeditor_document_page_geometry.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(PdfeditorPageGeometry)]
    core.pdfeditor_document_page_geometry.restype = ctypes.c_int32
    core.pdfeditor_document_render_page_preview.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(PdfeditorTile)]
    core.pdfeditor_document_render_page_preview.restype = ctypes.c_int32

    core.pdfeditor_tile_free.argtypes = [ctypes.POINTER(PdfeditorTile)]
    core.pdfeditor_tile_free.restype = None

    handle = ctypes.c_void_p()
    path_bytes = str(pdf_path).encode("utf-8")
    assert core.pdfeditor_document_open_utf8(path_bytes, ctypes.byref(handle)) == 0
    assert handle.value
    count = ctypes.c_uint32()
    assert core.pdfeditor_document_page_count(handle, ctypes.byref(count)) == 0
    assert count.value == 1
    geometry = PdfeditorPageGeometry()
    assert core.pdfeditor_document_page_geometry(handle, 0, ctypes.byref(geometry)) == 0
    assert geometry.page_id > 0
    assert geometry.width_points > 0 and geometry.height_points > 0
    assert core.pdfeditor_document_page_geometry(handle, 1, ctypes.byref(geometry)) == 6

    tile = PdfeditorTile()
    result = core.pdfeditor_document_render_page_preview(handle, 0, ctypes.byref(tile))
    assert result == 0, f"PDFium render returned {result}"
    assert tile.width == 512, tile.width
    assert tile.height == 512, tile.height
    assert tile.stride == 2048, tile.stride
    assert tile.len == 512 * 512 * 4, tile.len
    assert bool(tile.data)

    pixels = ctypes.string_at(tile.data, tile.len)
    assert any(value != 255 for value in pixels), "PDFium tile is still all white"

    core.pdfeditor_tile_free(ctypes.byref(tile))
    assert not bool(tile.data)
    assert tile.len == 0
    assert core.pdfeditor_document_close(handle) == 0
    assert core.pdfeditor_document_close(handle) == 5

    print("P0 PDFium smoke passed: real PDF rendered to a 512x512 BGRA tile")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
