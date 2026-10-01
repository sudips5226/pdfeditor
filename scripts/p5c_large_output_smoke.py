"""Large structural output with lazy viewer checks and app-owned memory metrics.

Hashes files by streaming, never loads entire sources into app coordination RAM.
"""
import ctypes as c
import json
import pathlib
import sys
import time
sys.dont_write_bytecode=True
from p5c_output_smoke import Output
from p5b_editing_smoke import Status, digest
from p0_pdfium_smoke import PdfeditorPageGeometry as Geometry
from p4_continuous_smoke import View, Page, Snapshot
from p3_viewport_smoke import Ready
from p5a_thumbnail_smoke import ThumbView, ThumbSnapshot, Item, Ready as ThumbReady

def run(dll, source, external, target):
    source,external,target=[pathlib.Path(p).resolve() for p in (source,external,target)]
    hashes=[digest(p) for p in (source,external)]
    core=c.CDLL(str(pathlib.Path(dll).resolve()))
    signatures={'open_utf8':[c.c_char_p,c.POINTER(c.c_void_p)],'close':[c.c_void_p],
      'page_geometry':[c.c_void_p,c.c_uint32,c.POINTER(Geometry)],'editor_status':[c.c_void_p,c.POINTER(Status)],
      'select_page':[c.c_void_p,c.c_uint64,c.c_uint64,c.c_uint32], 'edit':[c.c_void_p,c.c_uint32,c.c_int32,c.POINTER(Status)],
      'insert_source':[c.c_void_p,c.c_char_p,c.c_uint32,c.POINTER(c.c_uint32),c.c_uint32,c.POINTER(Status)],
      'output_start':[c.c_void_p,c.c_char_p,c.c_uint32,c.c_uint32], 'output_status':[c.c_void_p,c.POINTER(Output)],
      'update_continuous_viewport':[c.c_void_p,c.POINTER(View),c.POINTER(Snapshot),c.POINTER(Page),c.c_uint32],
      'go_to_page':[c.c_void_p,c.c_uint32,c.POINTER(View),c.POINTER(c.c_double),c.POINTER(c.c_double)],
      'poll_ready_tile':[c.c_void_p,c.POINTER(Ready)],
      'update_thumbnails':[c.c_void_p,c.POINTER(ThumbView),c.POINTER(ThumbSnapshot),c.POINTER(Item),c.c_uint32],
      'poll_ready_thumbnail':[c.c_void_p,c.POINTER(ThumbReady)],
    }
    for name,args in signatures.items():f=getattr(core,'pdfeditor_document_'+name);f.argtypes,f.restype=args,c.c_int32
    core.pdfeditor_tile_lease_release.argtypes=[c.c_void_p]
    def status(h):
        s=Status();assert core.pdfeditor_document_editor_status(h,c.byref(s))==0;return s
    def output(h):
        s=Output();assert core.pdfeditor_document_output_status(h,c.byref(s))==0;return s
    def edit(h,command,arg=0):
        s=Status();assert core.pdfeditor_document_edit(h,command,arg,c.byref(s))==0
    h=c.c_void_p();assert core.pdfeditor_document_open_utf8(str(source).encode(),c.byref(h))==0
    try:
        original_count=status(h).page_count;g=Geometry();assert core.pdfeditor_document_page_geometry(h,original_count//2,c.byref(g))==0
        assert core.pdfeditor_document_select_page(h,g.page_id,0,0)==0
        edit(h,7);edit(h,2,0);edit(h,3,90)
        s=Status();assert core.pdfeditor_document_insert_source(h,str(external).encode(),original_count//2,None,0,c.byref(s))==0
        expected_count=status(h).page_count
        # Source B rendering demand through main viewer and thumbnail lanes.
        v=View(0,0,800,600,.5,1,24,1,0);x,y=c.c_double(),c.c_double()
        assert core.pdfeditor_document_go_to_page(h,original_count//2,c.byref(v),c.byref(x),c.byref(y))==0
        v.origin_x,v.origin_y=x.value,y.value
        pages=(Page*64)();snapshot=Snapshot()
        tv=ThumbView(8+(original_count//2)*200,600,144,168,24,8,8,1,1,original_count//2,2,0);items=(Item*64)();ts=ThumbSnapshot()
        begin=time.perf_counter();assert core.pdfeditor_document_output_start(h,str(target).encode(),1,1)==0
        max_api_ms=0;viewport_updates=0;main_tiles=0;thumb_tiles=0;peak_rss=0
        class Memory(c.Structure):
            _fields_=[('cb',c.c_uint32),('faults',c.c_uint32)]+[(n,c.c_size_t) for n in ('peak','working','poolpeakpaged','poolpaged','poolpeaknonpaged','poolnonpaged','pagefile','peakpagefile')]
        c.windll.kernel32.GetCurrentProcess.restype=c.c_void_p
        c.windll.psapi.GetProcessMemoryInfo.argtypes=[c.c_void_p,c.POINTER(Memory),c.c_uint32]
        memory=Memory();memory.cb=c.sizeof(memory)
        deadline=time.monotonic()+180
        while True:
            o=output(h)
            if o.phase in (7,8,10):assert o.phase==7,(o.phase,o.error.decode());break
            v.generation+=1
            start=time.perf_counter();assert core.pdfeditor_document_update_continuous_viewport(h,c.byref(v),c.byref(snapshot),pages,64)==0
            max_api_ms=max(max_api_ms,(time.perf_counter()-start)*1000);viewport_updates+=1
            v.origin_x,v.origin_y=snapshot.origin_x,snapshot.origin_y
            tv.generation+=1;assert core.pdfeditor_document_update_thumbnails(h,c.byref(tv),c.byref(ts),items,64)==0;tv.offset=ts.offset
            for _ in range(8):
                r=Ready();code=core.pdfeditor_document_poll_ready_tile(h,c.byref(r));assert code in (0,8)
                if code==0:main_tiles+=1;assert core.pdfeditor_tile_lease_release(r.lease)==0
                t=ThumbReady();code=core.pdfeditor_document_poll_ready_thumbnail(h,c.byref(t));assert code in (0,8)
                if code==0:assert t.status==0;thumb_tiles+=1;assert core.pdfeditor_tile_lease_release(t.lease)==0
            assert c.windll.psapi.GetProcessMemoryInfo(c.windll.kernel32.GetCurrentProcess(),c.byref(memory),c.sizeof(memory))
            peak_rss=max(peak_rss,memory.working)
            assert time.monotonic()<deadline;time.sleep(.01)
        assert o.page_count==expected_count and not status(h).structural_dirty
        reopened=c.c_void_p();assert core.pdfeditor_document_open_utf8(str(target).encode(),c.byref(reopened))==0
        try:
            assert status(reopened).page_count==expected_count
            for index in [0,expected_count//2,expected_count-1]:assert core.pdfeditor_document_page_geometry(reopened,index,c.byref(g))==0
        finally:assert core.pdfeditor_document_close(reopened)==0
        assert [digest(p) for p in (source,external)]==hashes
        assert not list(target.parent.glob('.pdfeditor-*.tmp'))
        print(json.dumps({'source':str(source),'source_bytes':source.stat().st_size,'original_pages':original_count,'output_pages':o.page_count,'snapshot_us':o.snapshot_micros,'coordination_bytes':o.coordination_bytes,'mapping_bytes':o.page_count*12,'build_write_ms':o.build_write_micros/1000,'verify_ms':o.verification_micros/1000,'total_ms':o.elapsed_micros/1000,'output_bytes':o.output_bytes,'viewer_updates_during_output':viewport_updates,'main_completions':main_tiles,'thumbnail_completions':thumb_tiles,'max_viewport_api_ms':max_api_ms,'python_and_native_working_set_peak':peak_rss,'source_hashes_unchanged':True}))
    finally:assert core.pdfeditor_document_close(h)==0
if __name__=='__main__':run(*sys.argv[1:])
