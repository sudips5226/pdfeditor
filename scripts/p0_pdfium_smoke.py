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

    core.pdfeditor_render_pdf_preview_utf8.argtypes = [
        ctypes.c_char_p,
        ctypes.c_uint32,
        ctypes.POINTER(PdfeditorTile),
    ]
    core.pdfeditor_render_pdf_preview_utf8.restype = ctypes.c_int32

    core.pdfeditor_tile_free.argtypes = [ctypes.POINTER(PdfeditorTile)]
    core.pdfeditor_tile_free.restype = None

    tile = PdfeditorTile()
    path_bytes = str(pdf_path).encode("utf-8")
    result = core.pdfeditor_render_pdf_preview_utf8(
        path_bytes,
        0,
        ctypes.byref(tile),
    )

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

    print("P0 PDFium smoke passed: real PDF rendered to a 512x512 BGRA tile")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
