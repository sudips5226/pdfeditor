"""P2 ABI smoke against the release DLL and real PDFium. No product Python dependency."""

from __future__ import annotations

import ctypes
import pathlib
import sys

sys.dont_write_bytecode = True

from p0_pdfium_smoke import PdfeditorPageGeometry, PdfeditorTile


class PdfeditorTileRequest(ctypes.Structure):
    _fields_ = [
        ("page_id", ctypes.c_uint64),
        ("tile_x", ctypes.c_int32),
        ("tile_y", ctypes.c_int32),
        ("scale", ctypes.c_double),
        ("device_pixel_ratio", ctypes.c_double),
        ("rotation_degrees", ctypes.c_uint16),
        ("width", ctypes.c_uint32),
        ("height", ctypes.c_uint32),
    ]


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: p2_tile_smoke.py <pdfeditor_core.dll> <p2-tile-regions.pdf>")
    core = ctypes.CDLL(str(pathlib.Path(sys.argv[1]).resolve()))
    core.pdfeditor_abi_version.restype = ctypes.c_uint32
    assert core.pdfeditor_abi_version() == 7
    assert ctypes.sizeof(PdfeditorTileRequest) == 48
    assert PdfeditorTileRequest.width.offset == 36
    core.pdfeditor_document_open_utf8.argtypes = [ctypes.c_char_p, ctypes.POINTER(ctypes.c_void_p)]
    core.pdfeditor_document_open_utf8.restype = ctypes.c_int32
    core.pdfeditor_document_close.argtypes = [ctypes.c_void_p]
    core.pdfeditor_document_close.restype = ctypes.c_int32
    core.pdfeditor_document_page_geometry.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(PdfeditorPageGeometry)]
    core.pdfeditor_document_page_geometry.restype = ctypes.c_int32
    core.pdfeditor_document_render_tile.argtypes = [ctypes.c_void_p, ctypes.POINTER(PdfeditorTileRequest), ctypes.POINTER(PdfeditorTile)]
    core.pdfeditor_document_render_tile.restype = ctypes.c_int32
    core.pdfeditor_tile_free.argtypes = [ctypes.POINTER(PdfeditorTile)]
    core.pdfeditor_tile_free.restype = None
    handle = ctypes.c_void_p()
    path = str(pathlib.Path(sys.argv[2]).resolve()).encode("utf-8")
    assert core.pdfeditor_document_open_utf8(path, ctypes.byref(handle)) == 0
    try:
        geometry = PdfeditorPageGeometry()
        assert core.pdfeditor_document_page_geometry(handle, 0, ctypes.byref(geometry)) == 0
        for scale in (1.0, 2.0):
            regions = []
            for y in range(2):
                for x in range(2):
                    request = PdfeditorTileRequest(geometry.page_id, x, y, scale, 1.0, 0, 512, 512)
                    tile = PdfeditorTile()
                    assert core.pdfeditor_document_render_tile(handle, ctypes.byref(request), ctypes.byref(tile)) == 0
                    try:
                        assert (tile.width, tile.height, tile.stride, tile.len) == (512, 512, 2048, 1048576)
                        pixels = ctypes.string_at(tile.data, tile.len)
                        assert all(alpha == 255 for alpha in pixels[3::4])
                        regions.append(pixels)
                        if scale == 1.0 and (x, y) == (1, 1):
                            assert pixels[300 * 2048 + 400 * 4:300 * 2048 + 400 * 4 + 4] == b"\xff" * 4
                    finally:
                        core.pdfeditor_tile_free(ctypes.byref(tile))
                    assert not tile.data and tile.len == 0
            assert len(set(regions)) == 4, "expected four distinct page regions"
        request.scale = 0.0
        assert core.pdfeditor_document_render_tile(handle, ctypes.byref(request), ctypes.byref(tile)) == 7
        assert not tile.data
        request.scale = 1.0
        request.page_id = 0
        assert core.pdfeditor_document_render_tile(handle, ctypes.byref(request), ctypes.byref(tile)) == 6
    finally:
        assert core.pdfeditor_document_close(handle) == 0
    assert core.pdfeditor_document_render_tile(handle, ctypes.byref(request), ctypes.byref(tile)) == 5
    print("P2 tile smoke passed: release ABI v7, independent regions at 1x/2x, edge fill, ownership and invalid requests")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
