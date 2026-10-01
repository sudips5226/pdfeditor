"""Real libqpdf + PDFium workflow, preservation and safe-output integration.

Use PDFEDITOR_QPDF_PATH and PDFEDITOR_PDFIUM_PATH with a release core DLL.
Production export is structural; raster hashes here are an independent oracle.
"""
from __future__ import annotations
import ctypes as c
import hashlib
import json
import pathlib
import sys
import time
sys.dont_write_bytecode=True
from p5b_editing_smoke import Status, digest
from p0_pdfium_smoke import PdfeditorPageGeometry as Geometry, PdfeditorTile as Tile
from p4_continuous_smoke import View, Page, Snapshot
from p5a_thumbnail_smoke import ThumbView, ThumbSnapshot, Item, Ready
from p3_viewport_smoke import Ready as MainReady

class Output(c.Structure):
    _fields_=[(n,c.c_uint64) for n in ('document_id','snapshot_fingerprint','saved_fingerprint','current_revision','snapshot_revision','snapshot_micros','build_write_micros','verification_micros','elapsed_micros','output_bytes')]+[('coordination_bytes',c.c_size_t),('saved_revision',c.c_uint64)]+[(n,c.c_uint32) for n in ('phase','kind','percent','page_count','source_count','registered_sources','temp_exists')]+[('error_code',c.c_int32),('target',c.c_char*1024),('error',c.c_char*2048)]
class Request(c.Structure):
    _fields_=[('page_id',c.c_uint64),('x',c.c_int32),('y',c.c_int32),('scale',c.c_double),('dpr',c.c_double),('rotation',c.c_uint16),('width',c.c_uint32),('height',c.c_uint32)]

def run(dll, primary, external, outdir):
    root=pathlib.Path(outdir).resolve();root.mkdir(parents=True,exist_ok=True)
    paths=[pathlib.Path(p).resolve() for p in (primary,external)]
    hashes=[digest(p) for p in paths]
    def unchanged():assert [digest(p) for p in paths]==hashes
    core=c.CDLL(str(pathlib.Path(dll).resolve()))
    signatures={
      'open_utf8':[c.c_char_p,c.POINTER(c.c_void_p)],'close':[c.c_void_p],
      'page_geometry':[c.c_void_p,c.c_uint32,c.POINTER(Geometry)],
      'editor_status':[c.c_void_p,c.POINTER(Status)],'select_page':[c.c_void_p,c.c_uint64,c.c_uint64,c.c_uint32],
      'edit':[c.c_void_p,c.c_uint32,c.c_int32,c.POINTER(Status)],
      'insert_source':[c.c_void_p,c.c_char_p,c.c_uint32,c.POINTER(c.c_uint32),c.c_uint32,c.POINTER(Status)],
      'output_start':[c.c_void_p,c.c_char_p,c.c_uint32,c.c_uint32],
      'output_status':[c.c_void_p,c.POINTER(Output)],'output_cancel':[c.c_void_p],
      'render_tile':[c.c_void_p,c.POINTER(Request),c.POINTER(Tile)],
      'update_continuous_viewport':[c.c_void_p,c.POINTER(View),c.POINTER(Snapshot),c.POINTER(Page),c.c_uint32],
      'update_thumbnails':[c.c_void_p,c.POINTER(ThumbView),c.POINTER(ThumbSnapshot),c.POINTER(Item),c.c_uint32],
      'poll_ready_thumbnail':[c.c_void_p,c.POINTER(Ready)],'poll_ready_tile':[c.c_void_p,c.POINTER(MainReady)],
      'go_to_page':[c.c_void_p,c.c_uint32,c.POINTER(View),c.POINTER(c.c_double),c.POINTER(c.c_double)],
    }
    for name,args in signatures.items():
        f=getattr(core,'pdfeditor_document_'+name);f.argtypes,f.restype=args,c.c_int32
    core.pdfeditor_tile_free.argtypes=[c.POINTER(Tile)]
    core.pdfeditor_tile_lease_release.argtypes=[c.c_void_p]
    assert core.pdfeditor_abi_version()==8 and c.sizeof(Output)==3200
    def open(path):
        h=c.c_void_p(); assert core.pdfeditor_document_open_utf8(str(path).encode(),c.byref(h))==0;return h
    def status(h):
        s=Status();assert core.pdfeditor_document_editor_status(h,c.byref(s))==0;return s
    def geometry(h,i):
        g=Geometry();assert core.pdfeditor_document_page_geometry(h,i,c.byref(g))==0;return g
    def select(h,ids):
        for i,id in enumerate(ids): assert core.pdfeditor_document_select_page(h,id,0,0 if i==0 else 1)==0
    def edit(h,command,arg=0):
        s=Status();assert core.pdfeditor_document_edit(h,command,arg,c.byref(s))==0;return s
    def output(h):
        s=Output();assert core.pdfeditor_document_output_status(h,c.byref(s))==0;return s
    def start(h,path,kind=1,overwrite=0,expected=0):
        code=core.pdfeditor_document_output_start(h,str(path).encode(),kind,overwrite);assert code==expected,(code,expected)
    def wait(h,expected=7):
        deadline=time.monotonic()+120
        while True:
            s=output(h)
            if s.phase in (7,8,10):assert s.phase==expected,(s.phase,s.error.decode());assert not s.temp_exists;return s
            assert time.monotonic()<deadline,'output timeout';time.sleep(.003)
    def pixels(h,i,rotation=0):
        g=geometry(h,i);r=Request(g.page_id,0,0,.5,1,rotation,512,512);tile=Tile()
        assert core.pdfeditor_document_render_tile(h,c.byref(r),c.byref(tile))==0
        try:return hashlib.sha256(c.string_at(tile.data,tile.len)).hexdigest()
        finally:core.pdfeditor_tile_free(c.byref(tile))
    h=open(paths[0]);ext=open(paths[1])
    try:
        count=status(h).page_count;assert count>=6
        original=[geometry(h,i).page_id for i in range(count)]
        # Reference independent pixels before editing; rotations are additional.
        
        select(h,[original[4]]);edit(h,2,0) # move source 4 to front
        select(h,[original[2]]);edit(h,1) # delete source 2
        select(h,[original[1]]);edit(h,3,90)
        e=edit(h,7);duplicate=[geometry(h,i).page_id for i in range(e.page_count) if geometry(h,i).page_id not in original]
        unchanged()
        assert len(duplicate)==1 and e.selected_count==1
        edit(h,4);e=edit(h,5);assert duplicate[0] in [geometry(h,i).page_id for i in range(e.page_count)]
        s=Status();assert core.pdfeditor_document_insert_source(h,str(paths[1]).encode(),2,None,0,c.byref(s))==0
        inserted=[geometry(h,i).page_id for i in range(2,5)];assert s.selected_count==3
        unchanged()
        edit(h,4);edit(h,5);assert [geometry(h,i).page_id for i in range(2,5)]==inserted
        # Reuse SourceId, explicit list, independent IDs, then undo second import.
        indices=(c.c_uint32*1)(1)
        assert core.pdfeditor_document_insert_source(h,str(paths[1]).encode(),0,indices,1,c.byref(s))==0
        assert output(h).registered_sources==2;edit(h,4)
        select(h,[inserted[0]]);edit(h,3,90)
        edit(h,2,status(h).page_count)
        mapping=[(0,4,0),(0,0,0),(1,1,0),(1,2,0),(0,1,90),(0,1,90),(0,3,0),(0,5,0),(1,0,90)]
        if count>6: mapping=mapping[:-1]+[(0,i,0) for i in range(6,count)]+mapping[-1:]
        ids=[geometry(h,i).page_id for i in range(status(h).page_count)]
        select(h,[ids[-1],duplicate[0],inserted[1]])
        selected=[i for i in range(len(ids)) if ids[i] in (ids[-1],duplicate[0],inserted[1])]
        before=status(h); extracted=root/'extracted.pdf';start(h,extracted,2,1);ex=wait(h)
        unchanged()
        after=status(h);assert (before.revision,before.undo_depth,before.structural_dirty)==(after.revision,after.undo_depth,after.structural_dirty)
        extra_outputs=[]
        for name,order in [('single',[0]),('contiguous',[1,2,3])]:
            select(h,[ids[i] for i in reversed(order)])
            path=root/f'extracted-{name}.pdf';start(h,path,2,1);result=wait(h)
            assert result.page_count==len(order)
            assert (before.revision,before.undo_depth,before.structural_dirty)==(status(h).revision,status(h).undo_depth,status(h).structural_dirty)
            extra_outputs.append((path,order));unchanged()
        saved=root/'saved.pdf';start(h,saved,1,1);sv=wait(h);assert not status(h).structural_dirty
        unchanged()
        assert sv.snapshot_revision==sv.saved_revision and sv.snapshot_fingerprint==sv.saved_fingerprint
        # Reopen: compare boxes/intrinsic+edit rotation and actual PDFium pixels.
        for path,order in [(saved,list(range(len(ids)))),(extracted,selected),*extra_outputs]:
            reopened=open(path)
            try:
                assert status(reopened).page_count==len(order)
                for i,logical in enumerate(order):
                    source,index,rotation=mapping[logical];source_h=h if source==0 else ext
                    if source==0:
                        oracle=open(paths[0])
                        try:g=geometry(oracle,index);expected=pixels(oracle,index,rotation)
                        finally:core.pdfeditor_document_close(oracle)
                    else:g=geometry(ext,index);expected=pixels(ext,index,rotation)
                    actual=geometry(reopened,i)
                    assert actual.rotation_degrees==(g.rotation_degrees+rotation)%360,(path,i,actual.rotation_degrees,g.rotation_degrees,rotation)
                    dimensions=(g.height_points,g.width_points) if rotation in (90,270) else (g.width_points,g.height_points)
                    assert (actual.width_points,actual.height_points)==dimensions
                    assert pixels(reopened,i)==expected,(path,i,'pixels differ')
                # Optional independent structural oracle, required in CI.
                import pypdf
                result=pypdf.PdfReader(path)
                original_pdfs=[pypdf.PdfReader(p) for p in paths]
                assert len({p.indirect_reference.idnum for p in result.pages})==len(order)
                annotations=[]
                for i,logical in enumerate(order):
                    source,index,rotation=mapping[logical]
                    actual=result.pages[i];original_page=original_pdfs[source].pages[index]
                    assert list(actual.mediabox)==list(original_page.mediabox)
                    assert list(actual.cropbox)==list(original_page.cropbox)
                    assert actual.rotation==(original_page.rotation+rotation)%360
                    assert actual.extract_text()==original_page.extract_text()
                    original_annotations=original_page.get('/Annots',[])
                    assert len(actual.get('/Annots',[]))==len(original_annotations)
                    for anno,old in zip(actual.get('/Annots',[]),original_annotations):
                        assert anno.get_object().get('/Contents')==old.get_object().get('/Contents')
                        annotations.append(anno.idnum)
                assert len(annotations)==len(set(annotations)), 'logical copies must have private annotation objects'
            finally:core.pdfeditor_document_close(reopened)
        # Never write primary/inserted source, including canonical aliases/hardlinks.
        for path in paths:start(h,path,expected=19)
        alias=root/'source-hardlink.pdf'
        if alias.exists():alias.unlink()
        alias.hardlink_to(paths[0]);start(h,alias,expected=19);alias.unlink()
        start(h,saved,expected=20);start(h,root/'missing-directory'/'bad.pdf',expected=16)
        assert [digest(p) for p in paths]==hashes
        assert not list(root.glob('.pdfeditor-*.tmp'))
        print(json.dumps({'fixture':str(paths[0]),'saved_pages':sv.page_count,'extract_pages':ex.page_count,'snapshot_us':sv.snapshot_micros,'build_write_us':sv.build_write_micros,'verify_us':sv.verification_micros,'coordination_bytes':sv.coordination_bytes,'output_bytes':sv.output_bytes,'source_hashes_unchanged':True}))
    finally:
        assert core.pdfeditor_document_close(h)==0;assert core.pdfeditor_document_close(ext)==0
if __name__=='__main__':run(*sys.argv[1:])
