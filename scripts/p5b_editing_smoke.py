"""P5B in-memory release/PDFium integration; never writes an output PDF."""
from __future__ import annotations
import ctypes as c
import hashlib
import json
import pathlib
import sys
import time
sys.dont_write_bytecode = True
from p5a_thumbnail_smoke import ThumbView, ThumbSnapshot, Item, Ready, Metrics, identity
from p4_continuous_smoke import View, Page, Snapshot
from p3_viewport_smoke import Ready as MainReady
from p0_pdfium_smoke import PdfeditorPageGeometry as Geometry

class Status(c.Structure):
    _fields_ = [(n,c.c_uint64) for n in ('revision','selection_revision','current_page_id')] + [('history_bytes',c.c_size_t)] + [(n,c.c_uint32) for n in ('page_count','current_index','selected_count','undo_depth','redo_depth','structural_dirty')]

def digest(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def run(dll, path):
    source_hash = digest(path)
    core = c.CDLL(str(pathlib.Path(dll).resolve()))
    sig = {
        'open_utf8': [c.c_char_p,c.POINTER(c.c_void_p)], 'close':[c.c_void_p],
        'page_count':[c.c_void_p,c.POINTER(c.c_uint32)],
        'page_geometry':[c.c_void_p,c.c_uint32,c.POINTER(Geometry)],
        'editor_status':[c.c_void_p,c.POINTER(Status)],
        'select_page':[c.c_void_p,c.c_uint64,c.c_uint64,c.c_uint32],
        'edit':[c.c_void_p,c.c_uint32,c.c_int32,c.POINTER(Status)],
        'configure_history':[c.c_void_p,c.c_uint32,c.c_size_t],
        'update_thumbnails':[c.c_void_p,c.POINTER(ThumbView),c.POINTER(ThumbSnapshot),c.POINTER(Item),c.c_uint32],
        'poll_ready_thumbnail':[c.c_void_p,c.POINTER(Ready)],
        'thumbnail_metrics':[c.c_void_p,c.POINTER(Metrics)],
        'update_continuous_viewport':[c.c_void_p,c.POINTER(View),c.POINTER(Snapshot),c.POINTER(Page),c.c_uint32],
        'go_to_page':[c.c_void_p,c.c_uint32,c.POINTER(View),c.POINTER(c.c_double),c.POINTER(c.c_double)],
        'poll_ready_tile':[c.c_void_p,c.POINTER(MainReady)],
    }
    for name,args in sig.items():
        f=getattr(core,'pdfeditor_document_'+name);f.argtypes,f.restype=args,c.c_int32
    core.pdfeditor_tile_lease_release.argtypes=[c.c_void_p]
    assert core.pdfeditor_abi_version()==8 and c.sizeof(Status)==56
    h=c.c_void_p(); assert core.pdfeditor_document_open_utf8(str(pathlib.Path(path).resolve()).encode(),c.byref(h))==0
    tv=ThumbView(0,600,144,168,24,8,8,1,0,0,2,0); ts=ThumbSnapshot(); items=(Item*64)()
    v=View(0,0,800,600,1,1,24,0,0); ls=Snapshot(); pages=(Page*64)(); durations=[]
    def status():
        out=Status(); assert core.pdfeditor_document_editor_status(h,c.byref(out))==0; return out
    def geometry(index):
        g=Geometry(); assert core.pdfeditor_document_page_geometry(h,index,c.byref(g))==0;return g
    def select(id,mode=0,token=0):
        assert core.pdfeditor_document_select_page(h,id,token,mode)==0
    def edit(command,arg=0,error=0):
        out=Status(); start=time.perf_counter(); code=core.pdfeditor_document_edit(h,command,arg,c.byref(out));durations.append((time.perf_counter()-start)*1000)
        assert code==error,(command,arg,code,error); return out
    def thumbs(index=0):
        tv.current=index;tv.offset=max(0,8+index*200-200);tv.generation+=1
        assert core.pdfeditor_document_update_thumbnails(h,c.byref(tv),c.byref(ts),items,64)==0
        tv.offset=ts.offset; assert ts.returned<=8
        return [Item.from_buffer_copy(i) for i in items[:ts.returned]]
    def main(index=None):
        if index is not None:
            x,y=c.c_double(),c.c_double()
            # go-to-page requires a valid positive generation, even before initial submission
            v.generation=max(1,v.generation)
            assert core.pdfeditor_document_go_to_page(h,index,c.byref(v),c.byref(x),c.byref(y))==0
            v.origin_x,v.origin_y=x.value,y.value
        v.generation+=1
        assert core.pdfeditor_document_update_continuous_viewport(h,c.byref(v),c.byref(ls),pages,64)==0
        v.origin_x,v.origin_y=ls.origin_x,ls.origin_y
        assert ls.page_count==status().page_count
        for p in pages[:ls.returned_pages]: assert p.page_id==geometry(p.index).page_id
    def settle():
        seen={}; expected={identity(i.key):i.recycle for i in items[:ts.returned] if i.visible};deadline=time.monotonic()+30
        while len(seen)<len(expected):
            r=MainReady();code=core.pdfeditor_document_poll_ready_tile(h,c.byref(r));assert code in (0,8)
            if code==0:
                assert r.generation==v.generation; assert core.pdfeditor_tile_lease_release(r.lease)==0
            t=Ready();code=core.pdfeditor_document_poll_ready_thumbnail(h,c.byref(t)); assert code in (0,8)
            if code==0:
                try:
                    assert t.generation==tv.generation and expected[identity(t.key)]==t.recycle and t.status==0
                    seen[identity(t.key)]=(hashlib.sha256(c.string_at(t.data,t.len)).hexdigest(),c.cast(t.data,c.c_void_p).value)
                finally: assert core.pdfeditor_tile_lease_release(t.lease)==0
            else:time.sleep(.001)
            assert time.monotonic()<deadline,'render timeout'
        return seen
    try:
        count=status().page_count;assert count>=6 and not status().structural_dirty
        original=[geometry(i).page_id for i in range(6)]
        initial=thumbs(); main(0); raster=settle(); key0=identity(initial[0].key)
        select(original[1]);select(original[3],1);assert status().selected_count==2
        main(0);assert status().selected_count==2 # current movement is independent
        selected=thumbs();assert {i.key.page_id for i in selected if i.selected}=={original[1],original[3]}
        edit(2,count+1,error=13);edit(3,45,error=7);assert status().undo_depth==0
        deleted=edit(1);assert deleted.page_count==count-2 and deleted.undo_depth==1 and deleted.structural_dirty
        assert core.pdfeditor_document_update_continuous_viewport(h,c.byref(v),c.byref(ls),pages,64)==11
        assert core.pdfeditor_document_update_thumbnails(h,c.byref(tv),c.byref(ts),items,64)==11
        r=MainReady();assert core.pdfeditor_document_poll_ready_tile(h,c.byref(r))==8
        t=Ready();assert core.pdfeditor_document_poll_ready_thumbnail(h,c.byref(t))==8
        assert core.pdfeditor_document_select_page(h,original[1],0,0)==6
        assert core.pdfeditor_document_select_page(h,initial[0].key.page_id,initial[0].recycle,0)==6
        assert [geometry(i).page_id for i in range(4)]==[original[i] for i in (0,2,4,5)]
        main(deleted.current_index);thumbs();settle()
        edit(4);assert not status().structural_dirty and status().selected_count==2
        assert [geometry(i).page_id for i in range(6)]==original
        edit(5);assert status().page_count==count-2; edit(4)
        moved=edit(2,count);assert moved.page_count==count
        assert geometry(count-2).page_id==original[1] and geometry(count-1).page_id==original[3]
        main(count-1);tail=thumbs(count-1);assert {i.key.page_id for i in tail if i.selected}=={original[1],original[3]};settle()
        edit(4);assert not status().structural_dirty
        # Moving page A changes placement, while retaining its original raster identity/buffer.
        select(original[0]);edit(2,count);thumbs(count-1);main(count-1);moved_raster=settle();assert moved_raster[key0]==raster[key0]
        edit(4);select(original[0]);edit(3,90);main(0);rot=thumbs();rot_raster=settle()
        assert rot[0].key.rotation==90 and identity(rot[0].key)!=key0
        # Resolve real geometry and check only additional edit rotation is applied.
        geometry_deadline=time.monotonic()+30
        while True:
            main(0)
            first=next(p for p in pages[:ls.returned_pages] if p.index==0)
            if first.geometry_known:break
            assert time.monotonic()<geometry_deadline,'geometry timeout'
            time.sleep(.005)
        g=geometry(0);assert first.width==g.height_points and first.height==g.width_points
        assert first.effective_rotation==(first.intrinsic_rotation+90)%360
        edit(4);assert not status().structural_dirty;thumbs();main(0);assert settle()[key0]==raster[key0]
        edit(5);edit(4);edit(3,-90);assert status().redo_depth==0;edit(4)
        select(original[2]);select(original[4],2);assert status().selected_count==3
        edit(6);edit(1,error=14);assert status().page_count==count
        select(original[0]);edit(1);assert status().current_page_id==original[1];edit(4)
        # Delete current last page -> preceding logical page.
        last=geometry(count-1).page_id;previous=geometry(count-2).page_id;select(last);edit(1);assert status().current_page_id==previous;edit(4)
        if count>=10000:
            a=geometry(4999).page_id;b=geometry(6999).page_id
            select(a);select(b,2);assert status().selected_count==2001;edit(3,90);edit(1)
            assert status().page_count==count-2001;edit(4);edit(4);assert not status().structural_dirty
            select(last);edit(2,0);assert geometry(0).page_id==last;edit(4)
        assert core.pdfeditor_document_configure_history(h,3,64*1048576)==0
        select(original[0])
        for _ in range(8):edit(3,90)
        assert status().undo_depth==3 and status().history_bytes<=64*1048576 and not status().structural_dirty
        m=Metrics();assert core.pdfeditor_document_thumbnail_metrics(h,c.byref(m))==0
        assert m.bytes<=32*1048576 and m.slots<=8
        report={'fixture':str(path),'pages':count,'max_edit_ms':max(durations),'thumbnail_cache_bytes':m.bytes,'move_reused_same_pixels_and_buffer':True,'history_bytes':status().history_bytes,'source_sha256_unchanged':True}
    finally:
        assert core.pdfeditor_document_close(h)==0
        assert digest(path)==source_hash,'source PDF was changed'
    print(json.dumps(report))

if __name__=='__main__':
    if len(sys.argv)<3:raise SystemExit('usage: p5b_editing_smoke.py <core.dll> <pdf> [<pdf>...]')
    for path in sys.argv[2:]:run(sys.argv[1],path)
