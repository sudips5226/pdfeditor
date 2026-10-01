"""ABI v5 real PDFium continuous viewer proof; optional local 10k/benchmark PDF."""
from __future__ import annotations
import ctypes as c
import json
import math
import pathlib
import sys
import time

sys.dont_write_bytecode = True
from p3_viewport_smoke import Ready, Config, Metrics


class View(c.Structure):
    _fields_ = [(n, c.c_double) for n in ("origin_x", "origin_y", "width", "height", "scale", "device_pixel_ratio", "page_gap")] + [("generation", c.c_uint64), ("rotation_degrees", c.c_uint16)]


class Page(c.Structure):
    _fields_ = [("page_id", c.c_uint64), ("index", c.c_uint32), ("geometry_known", c.c_uint32)] + [(n, c.c_double) for n in ("x", "y", "width", "height", "page_width", "page_height", "spacing_before", "spacing_after")] + [("intrinsic_rotation", c.c_uint16), ("effective_rotation", c.c_uint16), ("intrinsic_rotation_known", c.c_uint32)]


class Snapshot(c.Structure):
    _fields_ = [(n, c.c_double) for n in ("origin_x", "origin_y", "extent_width", "extent_height")] + [(n, c.c_uint64) for n in ("open_micros", "layout_init_micros", "geometry_micros", "geometry_queries")] + [("metadata_bytes", c.c_size_t)] + [(n, c.c_uint32) for n in ("page_count", "current_page", "returned_pages", "visible_pages", "visible_tiles", "known_pages")]


def run(dll, path):
    core = c.CDLL(str(pathlib.Path(dll).resolve()))
    signatures = {
        "pdfeditor_document_open_utf8": [c.c_char_p, c.POINTER(c.c_void_p)],
        "pdfeditor_document_close": [c.c_void_p],
        "pdfeditor_document_configure_renderer": [c.c_void_p, c.POINTER(Config)],
        "pdfeditor_document_update_continuous_viewport": [c.c_void_p, c.POINTER(View), c.POINTER(Snapshot), c.POINTER(Page), c.c_uint32],
        "pdfeditor_document_layout_needs_refresh": [c.c_void_p, c.POINTER(c.c_uint32)],
        "pdfeditor_document_go_to_page": [c.c_void_p, c.c_uint32, c.POINTER(View), c.POINTER(c.c_double), c.POINTER(c.c_double)],
        "pdfeditor_document_poll_ready_tile": [c.c_void_p, c.POINTER(Ready)],
        "pdfeditor_document_renderer_metrics": [c.c_void_p, c.POINTER(Metrics)],
        "pdfeditor_tile_lease_release": [c.c_void_p],
    }
    for name, args in signatures.items():
        getattr(core, name).argtypes, getattr(core, name).restype = args, c.c_int32
    assert (c.sizeof(View), c.sizeof(Page), c.sizeof(Snapshot)) == (72, 88, 96)
    h = c.c_void_p()
    assert core.pdfeditor_document_open_utf8(str(pathlib.Path(path).resolve()).encode(), c.byref(h)) == 0
    v, snap, pages = View(0, 0, 1024, 1024, 1, 1, 24, 0, 0), Snapshot(), (Page * 64)()
    update_times = []
    try:
        config = Config(128 * 1048576, 256, 16)
        assert core.pdfeditor_document_configure_renderer(h, c.byref(config)) == 0

        def update():
            v.generation += 1
            start = time.perf_counter()
            result = core.pdfeditor_document_update_continuous_viewport(h, c.byref(v), c.byref(snap), pages, 64)
            update_times.append((time.perf_counter() - start) * 1000)
            assert result == 0, result
            v.origin_x, v.origin_y = snap.origin_x, snap.origin_y
            assert snap.returned_pages <= 64 and snap.visible_pages <= snap.returned_pages
            assert snap.metadata_bytes <= snap.page_count * 64 + 16

        def metrics():
            m = Metrics()
            assert core.pdfeditor_document_renderer_metrics(h, c.byref(m)) == 0
            assert m.cpu_cache_bytes <= config.cpu_byte_budget
            assert m.queue_depth <= 256 and m.completion_depth <= 16
            return m

        def expected():
            keys = set()
            s = v.scale * v.device_pixel_ratio
            for p in pages[:snap.returned_pages]:
                left, top = max(0, (v.origin_x - p.x) * s), max(0, (v.origin_y - p.y) * s)
                right, bottom = min(p.width * s, (v.origin_x - p.x) * s + v.width), min(p.height * s, (v.origin_y - p.y) * s + v.height)
                if right <= left or bottom <= top:
                    continue
                assert p.geometry_known and p.intrinsic_rotation_known
                assert p.effective_rotation == (p.intrinsic_rotation + v.rotation_degrees) % 360
                for y in range(math.floor(top / 512), math.ceil(bottom / 512)):
                    for x in range(math.floor(left / 512), math.ceil(right / 512)):
                        keys.add((p.page_id, x, y))
            assert len(keys) == snap.visible_tiles, (len(keys), snap.visible_tiles, v.origin_y, [(p.index,p.y,p.height) for p in pages[:snap.returned_pages]], keys)
            return keys

        def settle():
            seen = {}
            deadline = time.monotonic() + 15
            idle_since = None
            while True:
                probe = c.c_uint32()
                assert core.pdfeditor_document_layout_needs_refresh(h, c.byref(probe)) == 0
                if probe.value:
                    update(); seen.clear(); idle_since = None
                tile = Ready()
                code = core.pdfeditor_document_poll_ready_tile(h, c.byref(tile))
                if code == 0:
                    assert tile.generation == v.generation
                    seen[(tile.key.page_id, tile.key.tile_x, tile.key.tile_y)] = c.cast(tile.data, c.c_void_p).value
                    assert core.pdfeditor_tile_lease_release(tile.lease) == 0
                else:
                    assert code == 8, code
                m = metrics()
                known = all(p.geometry_known and p.intrinsic_rotation_known for p in pages[:snap.returned_pages]
                            if (p.y - v.origin_y) * v.scale * v.device_pixel_ratio < v.height
                            and (p.y + p.height - v.origin_y) * v.scale * v.device_pixel_ratio > 0)
                if known and set(seen) == expected() and m.queue_depth == 0 and m.completion_depth == 0:
                    if idle_since is None: idle_since = time.monotonic()
                    if time.monotonic() - idle_since > 0.05: return seen
                else: idle_since = None
                assert time.monotonic() < deadline, ("timeout", snap.known_pages, snap.visible_tiles, len(seen))
                time.sleep(0.001)

        def go(index):
            x, y = c.c_double(), c.c_double()
            assert core.pdfeditor_document_go_to_page(h, index, c.byref(v), c.byref(x), c.byref(y)) == 0
            v.origin_x, v.origin_y = x.value, y.value
            update()

        update()
        first = settle()
        initial = {n: getattr(snap, n) for n in ("page_count", "open_micros", "layout_init_micros", "geometry_queries", "geometry_micros", "metadata_bytes", "visible_pages", "visible_tiles")}
        assert snap.known_pages < snap.page_count if snap.page_count > 4 else True
        # Stale/invalid requests must leave renderer generation intact.
        bad = View.from_buffer_copy(v)
        assert core.pdfeditor_document_update_continuous_viewport(h, c.byref(bad), c.byref(Snapshot()), pages, 64) == 11
        bad.generation += 1; bad.origin_y = float("nan")
        assert core.pdfeditor_document_update_continuous_viewport(h, c.byref(bad), c.byref(Snapshot()), pages, 64) == 9
        for index in [min(3, snap.page_count - 1), snap.page_count // 2, snap.page_count - 1]:
            go(index); settle()
            assert abs(snap.current_page - index) <= 1
        # Return reuses identical page-specific buffers, without new render work.
        renders = metrics().renders_performed
        go(0); returned = settle()
        assert first == returned
        assert metrics().renders_performed == renders
        # A viewport across the page gap has independent page-local tile grids.
        first_page = pages[0]
        v.origin_y = first_page.y + first_page.height - 128
        update(); settle()
        assert snap.visible_pages >= 2
        # Exercise normalized DPR, viewer rotation and a large document Y.
        v.scale, v.device_pixel_ratio, v.rotation_degrees = 2, 1.5, 90
        go(snap.page_count - 1); settle()
        for i in range(30): go((i * 977) % snap.page_count)
        go(0); settle()
        final = metrics()
        print(json.dumps({"document": pathlib.Path(path).name, "initial": initial,
            "final_geometry_queries": snap.geometry_queries, "final_geometry_us": snap.geometry_micros,
            "known_pages": snap.known_pages, "renders": final.renders_performed,
            "cache_hits": final.cache_hits, "cpu_cache_bytes": final.cpu_cache_bytes,
            "max_update_ms": round(max(update_times), 3)}, indent=2))
    finally:
        assert core.pdfeditor_document_close(h) == 0


if __name__ == "__main__":
    for path in sys.argv[2:]: run(sys.argv[1], path)
