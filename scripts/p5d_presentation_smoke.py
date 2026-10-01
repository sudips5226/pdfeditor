"""Real PDFium + ABI v9 atomicity oracle with simulated GPU residency.

Native WinUI CSV measurements are separate: this script does not claim to test
Direct3D or visible frame timing. Every tile is rendered at final requested scale.
"""
from __future__ import annotations
import ctypes as c
import json
import pathlib
import sys
import time
sys.dont_write_bytecode = True
from p3_viewport_smoke import Ready, Key as TileKey, Metrics
from p4_continuous_smoke import View, Page, Snapshot
from p5b_editing_smoke import Status


class Presentation(c.Structure):
    _fields_ = [('requested', View), ('displayed', View)] + [(n, c.c_uint64) for n in (
        'requested_at', 'render_started_at', 'cpu_ready_at', 'gpu_ready_at', 'committed_at',
        'hold_micros', 'commit_count', 'stale_destinations', 'coalesced_requests', 'partial_presentations')
    ] + [(n, c.c_uint32) for n in ('requested_page', 'displayed_page', 'required_count',
                                  'cpu_count', 'gpu_count', 'missing_count', 'state', 'geometry_ready')]


def identity(k):
    return tuple(getattr(k, name) for name, _ in TileKey._fields_)


def run(dll, path, external=None):
    core = c.CDLL(str(pathlib.Path(dll).resolve()))
    signatures = {
        'open_utf8': [c.c_char_p, c.POINTER(c.c_void_p)], 'close': [c.c_void_p],
        'update_continuous_viewport': [c.c_void_p, c.POINTER(View), c.POINTER(Snapshot), c.POINTER(Page), c.c_uint32],
        'layout_needs_refresh': [c.c_void_p, c.POINTER(c.c_uint32)],
        'poll_ready_tile': [c.c_void_p, c.POINTER(Ready)],
        'go_to_page': [c.c_void_p, c.c_uint32, c.POINTER(View), c.POINTER(c.c_double), c.POINTER(c.c_double)],
        'gpu_residency': [c.c_void_p, c.POINTER(TileKey), c.c_uint32],
        'presentation_snapshot': [c.c_void_p, c.POINTER(Presentation), c.POINTER(TileKey), c.c_uint32],
        'commit_presentation': [c.c_void_p, c.c_uint64],
        'navigation_direction': [c.c_void_p, c.c_int32],
        'renderer_metrics': [c.c_void_p, c.POINTER(Metrics)],
        'insert_source': [c.c_void_p, c.c_char_p, c.c_uint32, c.POINTER(c.c_uint32), c.c_uint32, c.POINTER(Status)],
    }
    for name, args in signatures.items():
        f = getattr(core, 'pdfeditor_document_' + name)
        f.argtypes, f.restype = args, c.c_int32
    core.pdfeditor_tile_lease_release.argtypes = [c.c_void_p]
    assert c.sizeof(Presentation) == 256
    h = c.c_void_p()
    assert core.pdfeditor_document_open_utf8(str(pathlib.Path(path).resolve()).encode(), c.byref(h)) == 0
    view, layout, pages = View(0, 0, 1024, 768, 1, 1, 24, 0, 0), Snapshot(), (Page * 64)()
    resident, records = {}, []
    committed = 0

    def report_residency():
        arr = (TileKey * len(resident))(*resident.values())
        assert core.pdfeditor_document_gpu_residency(h, arr, len(arr)) == 0

    def presentation():
        p, keys = Presentation(), (TileKey * 4096)()
        assert core.pdfeditor_document_presentation_snapshot(h, c.byref(p), keys, 4096) == 0
        assert p.partial_presentations == 0
        assert p.required_count == len({identity(k) for k in keys[:p.required_count]})
        assert p.gpu_count <= p.required_count and p.cpu_count <= p.required_count
        return p, keys[:p.required_count]

    def update():
        report_residency()
        view.generation += 1
        assert core.pdfeditor_document_update_continuous_viewport(h, c.byref(view), c.byref(layout), pages, 64) == 0
        view.origin_x, view.origin_y = layout.origin_x, layout.origin_y
        p, _ = presentation()
        assert p.requested.generation == view.generation
        assert p.displayed.generation == committed
        assert layout.returned_pages <= 64 and layout.metadata_bytes <= layout.page_count * 128 + 16

    def go(index):
        x, y = c.c_double(), c.c_double()
        assert core.pdfeditor_document_navigation_direction(h, 1 if index > layout.current_page else -1) == 0
        assert core.pdfeditor_document_go_to_page(h, index, c.byref(view), c.byref(x), c.byref(y)) == 0
        view.origin_x, view.origin_y = x.value, y.value
        update()

    def settle():
        deadline = time.monotonic() + 60
        while True:
            refresh = c.c_uint32()
            assert core.pdfeditor_document_layout_needs_refresh(h, c.byref(refresh)) == 0
            if refresh.value:
                update()
            p, keys = presentation()
            if p.state == 2:
                assert {identity(k) for k in keys} <= set(resident)
                return p
            tile = Ready()
            code = core.pdfeditor_document_poll_ready_tile(h, c.byref(tile))
            if code == 0:
                assert tile.generation == view.generation
                expected_bits = c.c_uint64.from_buffer_copy(c.c_double(view.scale * view.device_pixel_ratio)).value
                assert tile.key.physical_scale_bits == expected_bits
                before, _ = presentation()
                assert before.displayed.generation == committed
                # Even all CPU completions cannot commit until their actual GPU
                # availability is reported. Never count arbitrary completions.
                if before.state != 2:
                    assert core.pdfeditor_document_commit_presentation(h, view.generation) == 9
                resident[identity(tile.key)] = TileKey.from_buffer_copy(tile.key)
                assert core.pdfeditor_tile_lease_release(tile.lease) == 0
                report_residency()
            else:
                assert code == 8, code
                time.sleep(0.001)
            assert time.monotonic() < deadline, ('timeout', p.required_count, p.cpu_count, p.gpu_count, p.geometry_ready)

    def commit():
        nonlocal committed
        p = settle()
        assert core.pdfeditor_document_commit_presentation(h, view.generation) == 0
        committed = view.generation
        p, _ = presentation()
        assert p.displayed.generation == committed and p.state == 0
        assert core.pdfeditor_document_commit_presentation(h, committed) == 9
        records.append({'generation': committed, 'page': p.requested_page + 1, 'scale': view.scale,
                        'required': p.required_count, 'request_commit_us': p.committed_at - p.requested_at,
                        'hold_us': p.hold_micros, 'partial': p.partial_presentations})

    try:
        update(); commit()
        # Wait for the existing near-page speculation to finish before measuring
        # cached navigation. Its independent renders must not contaminate the
        # mandatory-destination fast-path assertion on slow raster documents.
        deadline = time.monotonic() + 60
        while True:
            refresh = c.c_uint32()
            core.pdfeditor_document_layout_needs_refresh(h, c.byref(refresh))
            if refresh.value:
                update(); commit()
            m = Metrics(); core.pdfeditor_document_renderer_metrics(h, c.byref(m))
            if m.queue_depth == 0 and all(p.geometry_known for p in pages[:layout.returned_pages]):
                break
            assert time.monotonic() < deadline
            time.sleep(0.001)
        # Identical GPU-resident destination commits immediately with no backend,
        # upload publication, or geometry refresh (cached-region fast path).
        before = Metrics(); core.pdfeditor_document_renderer_metrics(h, c.byref(before))
        update(); p, _ = presentation(); assert p.state == 2
        commit()
        after = Metrics(); core.pdfeditor_document_renderer_metrics(h, c.byref(after))
        assert after.renders_performed == before.renders_performed
        if layout.page_count > 1:
            go(layout.page_count - 1); stale_a = view.generation
            go(layout.page_count // 2); stale_b = view.generation
            go(0)
            assert core.pdfeditor_document_commit_presentation(h, stale_a) == 11
            assert core.pdfeditor_document_commit_presentation(h, stale_b) == 11
            commit()
            go(layout.page_count - 1); commit()
            go(0); commit()
        for scale in (2, 4):
            view.scale = scale; update(); commit()
        view.rotation_degrees = 90; update(); commit()
        view.rotation_degrees, view.scale = 0, 1; update(); commit()
        if external:
            editor = Status()
            assert core.pdfeditor_document_insert_source(h, str(pathlib.Path(external).resolve()).encode(), 1, None, 0, c.byref(editor)) == 0
            go(1); commit()
        p, _ = presentation()
        print(json.dumps({'fixture': str(path), 'gpu': 'simulated exact-key residency',
                          'commits': p.commit_count, 'coalesced': p.coalesced_requests,
                          'partial_presentations': p.partial_presentations, 'records': records}, indent=2))
    finally:
        assert core.pdfeditor_document_close(h) == 0


if __name__ == '__main__':
    run(*sys.argv[1:])
