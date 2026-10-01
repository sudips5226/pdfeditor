"""Real PDFium predictive preparation oracle; GPU residency is simulated, not Direct3D.

Native benchmark CSV is the separate end-to-end GPU/Present measurement.
"""
from __future__ import annotations
import ctypes as c
import json
import pathlib
import sys
import time
sys.dont_write_bytecode = True
from p3_viewport_smoke import Ready, Key, Metrics
from p4_continuous_smoke import View, Page, Snapshot
from p5b_editing_smoke import Status
from p5d_presentation_smoke import Presentation, identity

class Prediction(c.Structure):
    _fields_ = [(n,c.c_double) for n in ('velocity','preparation_us','lead_distance')] + [
        (n,c.c_uint64) for n in ('input_sequence','viewport_requests','keys_generated','cpu_completed',
        'gpu_uploaded','cpu_used','gpu_used','cpu_wasted','gpu_wasted','invalidated','predicted_at',
        'predicted_cpu_at','predicted_gpu_at','requested_at')] + [('direction',c.c_int32)] + [
        (n,c.c_uint32) for n in ('depth','tile_count','cpu_ready','gpu_ready','entry_required','entry_cpu',
        'entry_gpu','entry_predictive_cpu','entry_predictive_gpu','mandatory_rendered','mandatory_uploaded')]

def run(dll,path,external=None):
    core=c.CDLL(str(pathlib.Path(dll).resolve()))
    signatures={
        'open_utf8':[c.c_char_p,c.POINTER(c.c_void_p)],'close':[c.c_void_p],
        'update_continuous_viewport':[c.c_void_p,c.POINTER(View),c.POINTER(Snapshot),c.POINTER(Page),c.c_uint32],
        'presentation_snapshot':[c.c_void_p,c.POINTER(Presentation),c.POINTER(Key),c.c_uint32],
        'prediction_snapshot':[c.c_void_p,c.POINTER(Prediction),c.POINTER(Key),c.c_uint32],
        'prediction_configure':[c.c_void_p,c.c_uint32,c.c_uint64,c.c_uint32],
        'navigation_input':[c.c_void_p,c.c_int32,c.c_double,c.c_uint32],
        'poll_ready_tile':[c.c_void_p,c.POINTER(Ready)],'poll_predictive_tile':[c.c_void_p,c.POINTER(Ready)],
        'gpu_residency':[c.c_void_p,c.POINTER(Key),c.c_uint32],
        'commit_presentation':[c.c_void_p,c.c_uint64],
        'layout_needs_refresh':[c.c_void_p,c.POINTER(c.c_uint32)],
        'renderer_metrics':[c.c_void_p,c.POINTER(Metrics)],
        'insert_source':[c.c_void_p,c.c_char_p,c.c_uint32,c.POINTER(c.c_uint32),c.c_uint32,c.POINTER(Status)],
    }
    for name,args in signatures.items():
        f=getattr(core,'pdfeditor_document_'+name);f.argtypes=args;f.restype=c.c_int32
    core.pdfeditor_tile_lease_release.argtypes=[c.c_void_p]
    assert core.pdfeditor_abi_version()==10 and c.sizeof(Prediction)==184
    h=c.c_void_p();assert core.pdfeditor_document_open_utf8(str(pathlib.Path(path).resolve()).encode(),c.byref(h))==0
    v=View(0,0,1024,768,1,1,24,0,0);s=Snapshot();pages=(Page*64)();resident={};records=[]
    def residency():
        # A bounded simulated cache, keeping current mandatory textures protected.
        p,keys=presentation()
        protected={identity(k) for k in keys}
        while len(resident)>128:
            victim=next(k for k in resident if k not in protected);del resident[victim]
        arr=(Key*len(resident))(*resident.values());assert core.pdfeditor_document_gpu_residency(h,arr,len(arr))==0
    def presentation():
        p=Presentation();keys=(Key*4096)()
        assert core.pdfeditor_document_presentation_snapshot(h,c.byref(p),keys,4096)==0
        assert p.partial_presentations==0
        return p,keys[:p.required_count]
    def prediction():
        q=Prediction();keys=(Key*64)();assert core.pdfeditor_document_prediction_snapshot(h,c.byref(q),keys,64)==0
        assert q.depth<=4 and q.tile_count<=64
        return q,keys[:q.tile_count]
    def update():
        residency();v.generation+=1
        assert core.pdfeditor_document_update_continuous_viewport(h,c.byref(v),c.byref(s),pages,64)==0
        v.origin_x,v.origin_y=s.origin_x,s.origin_y
        assert s.metadata_bytes<=s.page_count*128+16 and s.returned_pages<=64
    def upload(predictive=False):
        t=Ready();f=core.pdfeditor_document_poll_predictive_tile if predictive else core.pdfeditor_document_poll_ready_tile
        code=f(h,c.byref(t))
        if code==8:return False
        assert code==0,code
        p,_=presentation();displayed=p.displayed.generation
        assert t.generation==v.generation
        assert t.key.physical_scale_bits==c.c_uint64.from_buffer_copy(c.c_double(v.scale*v.device_pixel_ratio)).value
        resident[identity(t.key)]=Key.from_buffer_copy(t.key)
        assert core.pdfeditor_tile_lease_release(t.lease)==0
        residency();assert presentation()[0].displayed.generation==displayed
        return True
    def prepare(predicted=True):
        end=time.monotonic()+60
        while True:
            refresh=c.c_uint32();assert core.pdfeditor_document_layout_needs_refresh(h,c.byref(refresh))==0
            if refresh.value:update()
            p,_=presentation()
            if p.state==2:assert core.pdfeditor_document_commit_presentation(h,v.generation)==0
            if not upload():upload(True)
            p,_=presentation();q,_=prediction()
            m=Metrics();core.pdfeditor_document_renderer_metrics(h,c.byref(m))
            if p.state==0 and (not predicted or (q.gpu_ready==q.tile_count and m.queue_depth==0 and m.completion_depth==0)):
                return q
            assert time.monotonic()<end,('timeout',p.required_count,p.cpu_count,p.gpu_count,q.tile_count,q.gpu_ready)
            time.sleep(.001)
    def movement(delta):
        v.origin_y=max(0,v.origin_y+delta)
        assert core.pdfeditor_document_navigation_input(h,1 if delta>0 else -1,v.origin_y,0)==0
        update()
    try:
        assert core.pdfeditor_document_prediction_configure(h,1,128<<20,256)==0
        if external:
            st=Status();assert core.pdfeditor_document_insert_source(h,str(pathlib.Path(external).resolve()).encode(),1,None,0,c.byref(st))==0
        update();prepare()
        movement(120);time.sleep(.025);movement(120);time.sleep(.025);movement(120);q=prepare()
        assert q.tile_count>0 and q.gpu_uploaded>0
        # Future exact textures are prepared without moving displayed content.
        p,_=presentation();assert p.displayed.generation==v.generation
        for _ in range(6):
            old_renders=Metrics();core.pdfeditor_document_renderer_metrics(h,c.byref(old_renders))
            movement(120);p,_=presentation();q,_=prediction()
            assert q.entry_gpu==q.entry_required, (q.entry_gpu,q.entry_required)
            assert p.state==2
            assert core.pdfeditor_document_commit_presentation(h,v.generation)==0
            p,_=presentation()
            assert q.mandatory_rendered==0 and q.mandatory_uploaded==0
            records.append({'entry_gpu':q.entry_gpu,'required':q.entry_required,'predictive_gpu':q.entry_predictive_gpu,
                            'commit_us':p.committed_at-p.requested_at})
            prepare()
        assert prediction()[0].gpu_used>0
        movement(-120);assert prediction()[0].depth==0;prepare(False)
        time.sleep(.025);movement(-120);prepare()
        assert prediction()[0].direction==-1 and prediction()[0].invalidated>0
        v.scale=2;update();assert prediction()[0].depth==0;prepare(False)
        v.rotation_degrees=90;update();assert prediction()[0].depth==0;prepare(False)
        # Mandatory pressure shrinks prediction instead of consuming reserve.
        assert core.pdfeditor_document_prediction_configure(h,1,1<<20,1)==0
        time.sleep(.025);movement(40);time.sleep(.025);movement(40)
        assert prediction()[0].depth==0;prepare(False)
        q,_=prediction();p,_=presentation()
        result={'fixture':str(path),'gpu':'simulated bounded exact-key residency','cached_adjacent':records,
                'predictive_cpu_completed':q.cpu_completed,'predictive_gpu_uploaded':q.gpu_uploaded,
                'cpu_used':q.cpu_used,'gpu_used':q.gpu_used,'cpu_wasted':q.cpu_wasted,'gpu_wasted':q.gpu_wasted,
                'invalidated':q.invalidated,'partial':p.partial_presentations,'known_pages':s.known_pages,'pages':s.page_count}
        print(json.dumps(result,indent=2))
    finally:assert core.pdfeditor_document_close(h)==0

if __name__=='__main__':run(*sys.argv[1:])
