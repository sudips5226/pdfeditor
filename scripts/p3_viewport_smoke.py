"""Release-ABI/PDFium viewport, cache, bounded memory, and lease proof."""
from __future__ import annotations

import ctypes as c
import pathlib
import sys
import time

sys.dont_write_bytecode = True
from p0_pdfium_smoke import PdfeditorPageGeometry


class Viewport(c.Structure):
    _fields_ = [("page_id", c.c_uint64), *[(name, c.c_double) for name in
                ("origin_x", "origin_y", "width", "height", "scale", "device_pixel_ratio")],
                ("generation", c.c_uint64), ("rotation_degrees", c.c_uint16)]


class Key(c.Structure):
    _fields_ = [(name, c.c_uint64) for name in
                ("document_id", "document_revision", "page_id", "physical_scale_bits")] + [
                ("tile_x", c.c_int32), ("tile_y", c.c_int32), ("width", c.c_uint32),
                ("height", c.c_uint32), ("render_flags", c.c_uint32), ("rotation_degrees", c.c_uint16)]


class Ready(c.Structure):
    _fields_ = [("lease", c.c_void_p), ("key", Key), ("generation", c.c_uint64),
                ("width", c.c_uint32), ("height", c.c_uint32), ("stride", c.c_uint32),
                ("len", c.c_size_t), ("data", c.POINTER(c.c_uint8))]


class Config(c.Structure):
    _fields_ = [("cpu_byte_budget", c.c_size_t), ("queue_capacity", c.c_uint32),
                ("completion_capacity", c.c_uint32)]


class Metrics(c.Structure):
    _fields_ = [(name, c.c_uint64) for name in ("tile_requests", "cache_hits", "cache_misses",
                "renders_performed", "stale_renders_discarded", "render_errors")] + [
                (name, c.c_size_t) for name in ("cpu_cache_bytes", "queue_depth",
                "completion_depth", "outstanding_leases")]


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: p3_viewport_smoke.py <core.dll> <p2-fixture.pdf>")
    core = c.CDLL(str(pathlib.Path(sys.argv[1]).resolve()))
    assert c.sizeof(Viewport) == 72 and c.sizeof(Key) == 56 and c.sizeof(Ready) == 104
    signatures = {
        "pdfeditor_document_open_utf8": [c.c_char_p, c.POINTER(c.c_void_p)],
        "pdfeditor_document_close": [c.c_void_p],
        "pdfeditor_document_page_geometry": [c.c_void_p, c.c_uint32, c.POINTER(PdfeditorPageGeometry)],
        "pdfeditor_document_configure_renderer": [c.c_void_p, c.POINTER(Config)],
        "pdfeditor_document_update_viewport": [c.c_void_p, c.POINTER(Viewport)],
        "pdfeditor_document_poll_ready_tile": [c.c_void_p, c.POINTER(Ready)],
        "pdfeditor_document_renderer_metrics": [c.c_void_p, c.POINTER(Metrics)],
        "pdfeditor_tile_lease_release": [c.c_void_p],
    }
    for name, args in signatures.items():
        fn = getattr(core, name)
        fn.argtypes, fn.restype = args, c.c_int32
    core.pdfeditor_abi_version.restype = c.c_uint32
    assert core.pdfeditor_abi_version() == 5
    handle = c.c_void_p()
    assert core.pdfeditor_document_open_utf8(str(pathlib.Path(sys.argv[2]).resolve()).encode(), c.byref(handle)) == 0
    leases = []
    try:
        config = Config(16 * 1048576, 64, 2)
        assert core.pdfeditor_document_configure_renderer(handle, c.byref(config)) == 0
        geometry = PdfeditorPageGeometry()
        assert core.pdfeditor_document_page_geometry(handle, 0, c.byref(geometry)) == 0
        viewport = Viewport(geometry.page_id, 0, 0, 1024, 1024, 2, 1, 1, 0)

        def metrics():
            m = Metrics()
            assert core.pdfeditor_document_renderer_metrics(handle, c.byref(m)) == 0
            assert m.cpu_cache_bytes <= config.cpu_byte_budget
            assert m.queue_depth <= config.queue_capacity
            assert m.completion_depth <= config.completion_capacity
            return m

        def update(generation, x=0, y=0, scale=2):
            viewport.generation, viewport.origin_x, viewport.origin_y, viewport.scale = generation, x, y, scale
            assert core.pdfeditor_document_update_viewport(handle, c.byref(viewport)) == 0

        def drain(expected, retain=False):
            seen = {}
            deadline = time.monotonic() + 10
            while set(seen) != expected:
                tile = Ready()
                result = core.pdfeditor_document_poll_ready_tile(handle, c.byref(tile))
                if result == 8:
                    assert time.monotonic() < deadline, "tile timeout"
                    time.sleep(0.001)
                    continue
                assert result == 0, result
                leases.append(tile.lease)
                assert tile.generation == viewport.generation
                assert tile.key.page_id == geometry.page_id
                assert (tile.width, tile.height, tile.stride, tile.len) == (512, 512, 2048, 1048576)
                coordinate = (tile.key.tile_x, tile.key.tile_y)
                assert coordinate in expected and coordinate not in seen
                seen[coordinate] = c.cast(tile.data, c.c_void_p).value
                if not retain:
                    assert core.pdfeditor_tile_lease_release(leases.pop()) == 0
                metrics()
            return seen

        update(1)
        first = drain({(0, 0), (1, 0), (0, 1), (1, 1)}, retain=True)
        deadline = time.monotonic() + 10
        while metrics().cpu_cache_bytes != 9 * 1048576:
            assert time.monotonic() < deadline
            time.sleep(0.001)
        before = metrics().renders_performed
        update(2, 512)
        moved = drain({(1, 0), (2, 0), (1, 1), (2, 1)})
        assert moved[(1, 0)] == first[(1, 0)]  # No ABI pixel copy.
        # This movement also exposes a new prefetch column at the right page
        # edge. Exactly its three cells may render; visible cells must reuse.
        deadline = time.monotonic() + 10
        while metrics().cpu_cache_bytes != 12 * 1048576:
            assert time.monotonic() < deadline
            time.sleep(0.001)
        assert metrics().renders_performed == before + 3
        assert metrics().cache_hits >= 9
        # Rapid A -> B -> C, with new quality, must publish only C.
        update(3, 0, 0, 3)
        update(4, 1024, 512, 3)
        update(5, 512, 512, 3)
        drain({(x, y) for x in (1, 2) for y in (1, 2)})
        # Unreleased consumers receive backpressure at the process-wide cap.
        for generation in range(6, 21):
            update(generation)
            drain({(0, 0), (1, 0), (0, 1), (1, 1)}, retain=True)
        assert len(leases) == 64
        update(21)
        blocked = Ready()
        assert core.pdfeditor_document_poll_ready_tile(handle, c.byref(blocked)) == 10
        assert not blocked.lease and not blocked.data
        assert core.pdfeditor_tile_lease_release(leases.pop()) == 0
        assert core.pdfeditor_document_poll_ready_tile(handle, c.byref(blocked)) == 0
        leases.append(blocked.lease)
        assert blocked.generation == 21
        assert core.pdfeditor_document_update_viewport(handle, c.byref(viewport)) == 11
        viewport.generation += 1
        viewport.scale = float("nan")
        assert core.pdfeditor_document_update_viewport(handle, c.byref(viewport)) == 9
        # Leased pixels from generation 1 survive eviction and document close.
        address = first[(0, 0)]
        sample = c.string_at(address, 16)
        final = metrics()
        assert core.pdfeditor_document_close(handle) == 0
        handle = c.c_void_p()
        assert c.string_at(address, 16) == sample
        for lease in leases:
            assert core.pdfeditor_tile_lease_release(lease) == 0
            assert core.pdfeditor_tile_lease_release(lease) == 5
        leases.clear()
        print(f"P3 smoke passed: native tiles, visible overlap reuse without duplicate render/copy, current-generation results, "
              f"leases survive close; CPU={final.cpu_cache_bytes}, renders={final.renders_performed}, "
              f"stale={final.stale_renders_discarded}")
    finally:
        for lease in leases:
            core.pdfeditor_tile_lease_release(lease)
        if handle:
            core.pdfeditor_document_close(handle)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
