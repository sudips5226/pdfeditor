"""Deterministic P5A release/PDFium smoke; demand only, including an optional 10k PDF."""
from __future__ import annotations
import ctypes as c
import hashlib
import json
import pathlib
import sys
import time
sys.dont_write_bytecode = True
from p4_continuous_smoke import View, Page, Snapshot
from p3_viewport_smoke import Ready as MainReady

class ThumbView(c.Structure):
    _fields_ = [(n,c.c_double) for n in ('offset','extent','width','height','label_height','gap','padding','dpr')] + [('generation',c.c_uint64),('current',c.c_uint32),('overscan',c.c_uint32),('rotation',c.c_uint16)]
class Key(c.Structure):
    _fields_ = [(n,c.c_uint64) for n in ('document_id','revision','page_id','dpr_bits')] + [(n,c.c_uint32) for n in ('width','height','flags')] + [('rotation',c.c_uint16)]
class Item(c.Structure):
    _fields_ = [('key',Key),('recycle',c.c_uint64),('top',c.c_double)] + [(n,c.c_uint32) for n in ('index','current','visible')]
class ThumbSnapshot(c.Structure):
    _fields_ = [('offset',c.c_double),('total',c.c_double),('returned',c.c_uint32),('visible',c.c_uint32)]
class Ready(c.Structure):
    _fields_ = [('lease',c.c_void_p),('key',Key),('generation',c.c_uint64),('recycle',c.c_uint64),('status',c.c_int32),('stride',c.c_uint32),('len',c.c_size_t),('data',c.POINTER(c.c_uint8))]
class Metrics(c.Structure):
    _fields_ = [(n,c.c_uint64) for n in ('hits','misses','renders','recycled','stale','errors')] + [(n,c.c_size_t) for n in ('bytes','queue','ready')] + [(n,c.c_uint32) for n in ('slots','visible','current')]
def identity(k): return tuple(getattr(k,n) for n,_ in Key._fields_)
def run(dll,path):
    core=c.CDLL(str(pathlib.Path(dll).resolve()))
    sig={
        'pdfeditor_document_open_utf8':[c.c_char_p,c.POINTER(c.c_void_p)],
        'pdfeditor_document_close':[c.c_void_p],
        'pdfeditor_document_page_count':[c.c_void_p,c.POINTER(c.c_uint32)],
        'pdfeditor_document_configure_thumbnails':[c.c_void_p,c.c_size_t],
        'pdfeditor_document_update_thumbnails':[c.c_void_p,c.POINTER(ThumbView),c.POINTER(ThumbSnapshot),c.POINTER(Item),c.c_uint32],
        'pdfeditor_document_poll_ready_thumbnail':[c.c_void_p,c.POINTER(Ready)],
        'pdfeditor_document_thumbnail_metrics':[c.c_void_p,c.POINTER(Metrics)],
        'pdfeditor_document_thumbnail_sync_current':[c.c_void_p,c.c_uint32],
        'pdfeditor_document_thumbnail_show_current':[c.c_void_p,c.POINTER(ThumbView),c.POINTER(c.c_double)],
        'pdfeditor_tile_lease_release':[c.c_void_p],
        'pdfeditor_document_go_to_page':[c.c_void_p,c.c_uint32,c.POINTER(View),c.POINTER(c.c_double),c.POINTER(c.c_double)],
        'pdfeditor_document_update_continuous_viewport':[c.c_void_p,c.POINTER(View),c.POINTER(Snapshot),c.POINTER(Page),c.c_uint32],
        'pdfeditor_document_layout_needs_refresh':[c.c_void_p,c.POINTER(c.c_uint32)],
        'pdfeditor_document_poll_ready_tile':[c.c_void_p,c.POINTER(MainReady)],
    }
    for n,a in sig.items(): getattr(core,n).argtypes,getattr(core,n).restype=a,c.c_int32
    assert (c.sizeof(ThumbView),c.sizeof(Key),c.sizeof(Item),c.sizeof(Ready),c.sizeof(Metrics))==(88,48,80,96,88)
    h=c.c_void_p();assert core.pdfeditor_document_open_utf8(str(pathlib.Path(path).resolve()).encode(),c.byref(h))==0
    count=c.c_uint32();assert core.pdfeditor_document_page_count(h,c.byref(count))==0
    v=ThumbView(0,600,144,168,24,8,8,1,0,0,2,0);snap=ThumbSnapshot();items=(Item*64)();durations=[];max_slots=0;max_queue=0;max_bytes=0
    main=View(0,0,800,600,1,1,24,1,0);main_snap=Snapshot();pages=(Page*64)()
    try:
        budget=32*1048576
        assert core.pdfeditor_document_configure_thumbnails(h,budget)==0
        def metrics():
            nonlocal max_queue,max_bytes
            m=Metrics();assert core.pdfeditor_document_thumbnail_metrics(h,c.byref(m))==0
            assert m.bytes<=budget and m.queue<=64 and m.ready<=16 and m.slots<=8
            max_queue=max(max_queue,m.queue);max_bytes=max(max_bytes,m.bytes);return m
        def update():
            nonlocal max_slots
            v.generation+=1;start=time.perf_counter()
            assert core.pdfeditor_document_update_thumbnails(h,c.byref(v),c.byref(snap),items,64)==0
            durations.append((time.perf_counter()-start)*1000);v.offset=snap.offset
            assert snap.returned<=8 and snap.visible<=4
            max_slots=max(max_slots,snap.returned)
            assert [i.index for i in items[:snap.returned]]==list(range(items[0].index,items[0].index+snap.returned))
            metrics()
        def settle():
            seen={};deadline=time.monotonic()+30
            expected={identity(i.key):i.recycle for i in items[:snap.returned] if i.visible}
            while len(seen)<len(expected) or metrics().queue:
                ready=Ready();code=core.pdfeditor_document_poll_ready_thumbnail(h,c.byref(ready))
                assert code in (0,8),code
                if code==0:
                    try:
                        key=identity(ready.key);assert key in expected and expected[key]==ready.recycle
                        assert ready.generation==v.generation and ready.status==0
                        assert ready.len==ready.key.width*ready.key.height*4
                        pixels=c.string_at(ready.data,ready.len)
                        if pathlib.Path(path).name.startswith('p4-'):
                            assert any(x!=255 for x in pixels), 'all-white fixture thumbnail'
                        seen[key]=(hashlib.sha256(pixels).hexdigest(),c.cast(ready.data,c.c_void_p).value)
                    finally: assert core.pdfeditor_tile_lease_release(ready.lease)==0
                else: time.sleep(.001)
                assert time.monotonic()<deadline,'thumbnail timeout'
            return seen
        def show(index):
            v.current=index;offset=c.c_double();assert core.pdfeditor_document_thumbnail_show_current(h,c.byref(v),c.byref(offset))==0
            v.offset=offset.value;update();assert any(i.index==index and i.visible for i in items[:snap.returned])
        update();a=settle();old=Item.from_buffer_copy(items[0]);a_keys=set(a)
        show(count.value//2);settle();show(count.value-1);settle();show(0);before=metrics().renders;a_return=settle()
        assert a_return==a and metrics().renders==before,'A did not reuse resident buffers'
        assert items[0].recycle!=old.recycle and items[0].key.page_id==old.key.page_id
        assert items[0].current==1
        # Off-screen current sync must not change demand, queue or scroll position.
        before_sync=metrics(); saved_offset=v.offset
        assert core.pdfeditor_document_thumbnail_sync_current(h,count.value-1)==0
        after_sync=metrics()
        assert after_sync.current==count.value-1 and after_sync.renders==before_sync.renders
        assert after_sync.slots==before_sync.slots and v.offset==saved_offset
        # Highlight changes must preserve pixel identity and recycling identity.
        keys=[identity(i.key) for i in items[:snap.returned]];tokens=[i.recycle for i in items[:snap.returned]]
        v.current=min(1,count.value-1);update();settle()
        assert keys==[identity(i.key) for i in items[:snap.returned]] and tokens==[i.recycle for i in items[:snap.returned]]
        # Thumbnail click uses the existing P4 navigation destination immediately.
        target=items[min(1,snap.returned-1)].index;x,y=c.c_double(),c.c_double()
        assert core.pdfeditor_document_go_to_page(h,target,c.byref(main),c.byref(x),c.byref(y))==0
        main.origin_x,main.origin_y=x.value,y.value;main.generation+=1
        assert core.pdfeditor_document_update_continuous_viewport(h,c.byref(main),c.byref(main_snap),pages,64)==0
        # Drain main and refine its lazy geometry while thumbnail generations churn.
        for i in range(100):
            v.offset=(i*37991) % max(1,snap.total-v.extent);update()
            r=MainReady();code=core.pdfeditor_document_poll_ready_tile(h,c.byref(r));assert code in (0,8)
            if code==0: assert core.pdfeditor_tile_lease_release(r.lease)==0
            probe=c.c_uint32();assert core.pdfeditor_document_layout_needs_refresh(h,c.byref(probe))==0
            if probe.value:
                main.generation+=1
                assert core.pdfeditor_document_update_continuous_viewport(h,c.byref(main),c.byref(main_snap),pages,64)==0
        # Clear main demand by draining all tiles; then finish the latest thumbnail generation.
        deadline=time.monotonic()+30
        while True:
            r=MainReady();code=core.pdfeditor_document_poll_ready_tile(h,c.byref(r));assert code in (0,8)
            if code==0: assert core.pdfeditor_tile_lease_release(r.lease)==0
            else: break
            assert time.monotonic()<deadline
        settle()
        v.dpr=2;show(0);dpr=settle();assert not a_keys.intersection(dpr)
        v.rotation=90;update();rotated=settle();assert not set(dpr).intersection(rotated)
        m=metrics()
        assert core.pdfeditor_document_update_thumbnails(h,c.byref(v),c.byref(snap),items,64)==11
        assert snap.returned==0
        print(json.dumps({'fixture':str(path),'pages':count.value,'max_slots':max_slots,'max_queue':max_queue,'max_cache_bytes':max_bytes,'renders':m.renders,'hits':m.hits,'recycled':m.recycled,'stale':m.stale,'max_update_ms':max(durations),'A_return_same_pixels_and_buffers':True}))
    finally: assert core.pdfeditor_document_close(h)==0
if __name__=='__main__':
    if len(sys.argv)<3: raise SystemExit('usage: p5a_thumbnail_smoke.py <core.dll> <pdf> [<pdf>...]')
    for path in sys.argv[2:]: run(sys.argv[1],path)

