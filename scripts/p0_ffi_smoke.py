"""P0 runtime smoke test for the Rust C ABI.

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
    if len(sys.argv) != 2:
        raise SystemExit("usage: p0_ffi_smoke.py <pdfeditor_core.dll>")

    dll_path = pathlib.Path(sys.argv[1]).resolve()
    if not dll_path.is_file():
        raise SystemExit(f"missing native core DLL: {dll_path}")

    core = ctypes.CDLL(str(dll_path))

    core.pdfeditor_abi_version.argtypes = []
    core.pdfeditor_abi_version.restype = ctypes.c_uint32

    core.pdfeditor_render_test_tile.argtypes = [ctypes.POINTER(PdfeditorTile)]
    core.pdfeditor_render_test_tile.restype = ctypes.c_int32

    core.pdfeditor_tile_free.argtypes = [ctypes.POINTER(PdfeditorTile)]
    core.pdfeditor_tile_free.restype = None

    abi_version = core.pdfeditor_abi_version()
    assert abi_version == 5, abi_version

    tile = PdfeditorTile()
    result = core.pdfeditor_render_test_tile(ctypes.byref(tile))
    assert result == 0, result
    assert tile.width == 512, tile.width
    assert tile.height == 512, tile.height
    assert tile.stride == 2048, tile.stride
    assert tile.len == 512 * 512 * 4, tile.len
    assert bool(tile.data)

    # Pixel (1, 2): BGRA = [1, 2, 1 XOR 2, 255].
    offset = 2 * tile.stride + 1 * 4
    sample = [tile.data[offset + channel] for channel in range(4)]
    assert sample == [1, 2, 3, 255], sample

    rendered_width = tile.width
    rendered_height = tile.height

    core.pdfeditor_tile_free(ctypes.byref(tile))
    assert not bool(tile.data)
    assert tile.len == 0

    print(
        f"P0 FFI smoke passed: ABI v{abi_version}, "
        f"{rendered_width}x{rendered_height} tile ownership round-trip"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
