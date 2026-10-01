//! ABI v5: bounded snapshots and asynchronous nearby-page geometry acquisition.
use super::viewport::{
    ffi_call, renderer, viewport_error, PDFEDITOR_ERROR_CAPACITY, PDFEDITOR_ERROR_STALE_GENERATION,
    PDFEDITOR_ERROR_VIEWPORT,
};
use super::*;
use document_core::layout::{DocumentLayout, DocumentPoint, DocumentViewport, MAX_FRAME_PAGES};
use document_core::viewport::{DeviceSize, Priority};
use document_core::PageGeometry;
use std::sync::Condvar;
use std::thread::{self, JoinHandle};
use std::time::Instant;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorDocumentViewport {
    pub origin_x: f64,
    pub origin_y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    pub device_pixel_ratio: f64,
    pub page_gap: f64,
    pub generation: u64,
    pub rotation_degrees: u16,
}
impl From<PdfeditorDocumentViewport> for DocumentViewport {
    fn from(v: PdfeditorDocumentViewport) -> Self {
        Self {
            origin: DocumentPoint {
                x: v.origin_x,
                y: v.origin_y,
            },
            extent: DeviceSize {
                width: v.width,
                height: v.height,
            },
            scale: v.scale,
            device_pixel_ratio: v.device_pixel_ratio,
            page_gap: v.page_gap,
            generation: v.generation,
            rotation_degrees: v.rotation_degrees,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorPageLayout {
    pub page_id: u64,
    pub index: u32,
    pub geometry_known: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub page_width: f64,
    pub page_height: f64,
    pub spacing_before: f64,
    pub spacing_after: f64,
    pub intrinsic_rotation: u16,
    pub effective_rotation: u16,
    pub intrinsic_rotation_known: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorLayoutSnapshot {
    pub origin_x: f64,
    pub origin_y: f64,
    pub extent_width: f64,
    pub extent_height: f64,
    pub open_micros: u64,
    pub layout_init_micros: u64,
    pub geometry_micros: u64,
    pub geometry_queries: u64,
    pub metadata_bytes: usize,
    pub page_count: u32,
    pub current_page: u32,
    pub returned_pages: u32,
    pub visible_pages: u32,
    pub visible_tiles: u32,
    pub known_pages: u32,
}
struct GeometryState {
    pending: Vec<(u32, bool)>,
    ready: Vec<(u32, Result<PageGeometry, i32>, bool)>,
    inflight: Option<u32>,
    stop: bool,
    queries: u64,
    micros: u64,
}
struct GeometryShared {
    state: Mutex<GeometryState>,
    wake: Condvar,
}
struct GeometryWorker {
    shared: Arc<GeometryShared>,
    thread: Option<JoinHandle<()>>,
}
impl GeometryWorker {
    fn new(backend: Arc<PdfiumDocument>) -> Self {
        let shared = Arc::new(GeometryShared {
            state: Mutex::new(GeometryState {
                pending: Vec::new(),
                ready: Vec::new(),
                inflight: None,
                stop: false,
                queries: 0,
                micros: 0,
            }),
            wake: Condvar::new(),
        });
        let worker = Arc::clone(&shared);
        let thread = thread::spawn(move || loop {
            let mut s = worker.state.lock().unwrap_or_else(|p| p.into_inner());
            while !s.stop && (s.pending.is_empty() || s.ready.len() > MAX_FRAME_PAGES - 2) {
                s = worker.wake.wait(s).unwrap_or_else(|p| p.into_inner());
            }
            if s.stop {
                break;
            }
            let (index, rotation) = s.pending.remove(0);
            s.inflight = Some(index);
            drop(s);
            let start = Instant::now();
            let result = backend
                .page_size_by_index(index)
                .map(|size| PageGeometry {
                    size,
                    rotation_degrees: 0,
                })
                .map_err(backend_error);
            let failed = result.is_err();
            let mut s = worker.state.lock().unwrap_or_else(|p| p.into_inner());
            s.queries += 1;
            s.micros += start.elapsed().as_micros() as u64;
            s.ready.push((index, result, false));
            let finish_rotation = rotation && !failed && !s.stop;
            if !finish_rotation {
                s.inflight = None;
            }
            drop(s);
            if finish_rotation {
                let start = Instant::now();
                let result = backend.page_geometry(index).map_err(backend_error);
                let mut s = worker.state.lock().unwrap_or_else(|p| p.into_inner());
                s.queries += 1;
                s.micros += start.elapsed().as_micros() as u64;
                s.ready.push((index, result, true));
                s.inflight = None;
            }
        });
        Self {
            shared,
            thread: Some(thread),
        }
    }
    fn request(&self, indices: Vec<(u32, bool)>) {
        let mut s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
        s.pending = indices
            .into_iter()
            .filter(|i| s.inflight != Some(i.0) && !s.ready.iter().any(|r| r.0 == i.0))
            .take(MAX_FRAME_PAGES)
            .collect();
        self.shared.wake.notify_one();
    }
}
impl Drop for GeometryWorker {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stop = true;
        self.shared.wake.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
struct LayoutState {
    layout: DocumentLayout,
    generation: u64,
    known_pages: u32,
}
pub(super) struct ContinuousRenderer {
    state: Mutex<LayoutState>,
    geometry: GeometryWorker,
    init_micros: u64,
}
fn continuous(d: &OpenDocument) -> &ContinuousRenderer {
    d.continuous.get_or_init(|| {
        let start = Instant::now();
        let layout =
            DocumentLayout::new(&d.editor.lock().unwrap_or_else(|p| p.into_inner()).page_plan);
        let init_micros = start.elapsed().as_micros() as u64;
        ContinuousRenderer {
            state: Mutex::new(LayoutState {
                layout,
                generation: 0,
                known_pages: 0,
            }),
            geometry: GeometryWorker::new(Arc::clone(&d.backend)),
            init_micros,
        }
    })
}

pub(super) fn sync_plan(d: &OpenDocument, plan: &document_core::PagePlan) {
    if let Some(c) = d.continuous.get() {
        let mut s = c.state.lock().unwrap_or_else(|p| p.into_inner());
        s.layout.sync_plan(plan);
        s.known_pages = (0..s.layout.len())
            .filter(|i| {
                // All known flags are retained; no backend request here.
                s.layout.geometry_known(*i)
            })
            .count() as u32;
    }
}

/// One atomic generation submits demand for all visible/neighbor pages through
/// the P3 renderer. Unknown geometry produces placeholders, never tile pixels.
/// # Safety
/// Viewport, snapshot, and capacity page entries must be valid separate storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_update_continuous_viewport(
    handle: *mut PdfeditorDocument,
    viewport: *const PdfeditorDocumentViewport,
    snapshot: *mut PdfeditorLayoutSnapshot,
    pages: *mut PdfeditorPageLayout,
    capacity: u32,
) -> i32 {
    if viewport.is_null() || snapshot.is_null() || pages.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let input = unsafe { ptr::read(viewport) };
    unsafe {
        ptr::write(snapshot, Default::default());
    }
    ffi_call(|| {
        with_document(handle, |d| {
            let mut v = DocumentViewport::from(input);
            v.validate().map_err(viewport_error)?;
            if capacity as usize > MAX_FRAME_PAGES {
                return Err(PDFEDITOR_ERROR_CAPACITY);
            }
            let c = continuous(d);
            let mut editor = d.editor.lock().unwrap_or_else(|p| p.into_inner());
            let mut s = c.state.lock().unwrap_or_else(|p| p.into_inner());
            if v.generation <= s.generation {
                return Err(PDFEDITOR_ERROR_STALE_GENERATION);
            }
            // Preserve the page-local top-of-viewport offset when estimates above
            // it refine. Navigation input is interpreted in the previous layout.
            let anchor = s
                .layout
                .page_at_y(v.origin.y, v)
                .and_then(|i| s.layout.page(i, v));
            let anchor_offset = anchor.map(|p| v.origin.y - p.bounds.origin.y);
            let mut geometry = c
                .geometry
                .shared
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let ready = std::mem::take(&mut geometry.ready);
            let geometry_queries = geometry.queries;
            let geometry_micros = geometry.micros;
            drop(geometry);
            c.geometry.shared.wake.notify_one();
            for (source, g, rotation_known) in ready {
                let id = d
                    .model
                    .page_plan
                    .get(source)
                    .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?
                    .id;
                let Some(i) = editor.page_plan.position_of(id) else {
                    continue;
                };
                let g = g?;
                if !s
                    .layout
                    .page(i, v)
                    .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?
                    .known
                {
                    s.known_pages += 1;
                }
                if rotation_known {
                    s.layout.resolve(i, g).map_err(viewport_error)?;
                } else {
                    s.layout.resolve_size(i, g.size).map_err(viewport_error)?;
                }
            }
            if let (Some(p), Some(offset)) = (anchor, anchor_offset) {
                v.origin.y = s.layout.page(p.index as usize, v).unwrap().bounds.origin.y + offset;
            }
            let extent = s.layout.extent(v);
            if !extent.height.is_finite()
                || extent.height > 1e15
                || extent.width * v.physical_scale() > 1e15
            {
                return Err(PDFEDITOR_ERROR_VIEWPORT);
            }
            v.origin.x = v.origin.x.clamp(
                0.0,
                (extent.width - v.extent.width / v.physical_scale()).max(0.0),
            );
            v.origin.y = v.origin.y.clamp(
                0.0,
                (extent.height - v.extent.height / v.physical_scale()).max(0.0),
            );
            let range = s.layout.visible_range(v, 1).map_err(viewport_error)?;
            if range.len() > capacity as usize {
                return Err(PDFEDITOR_ERROR_CAPACITY);
            }
            let r = renderer(d, Default::default())?;
            let demand = s
                .layout
                .demand(d.model.id, v, r.capacity())
                .map_err(viewport_error)?;
            let visible_tiles = demand
                .iter()
                .filter(|t| t.priority == Priority::Visible)
                .count() as u32;
            let mut output = Vec::with_capacity(range.len());
            let mut unknown = Vec::new();
            let mut visible_pages = 0;
            for i in range {
                let p = s.layout.page(i, v).unwrap();
                let visible = DocumentLayout::intersects(p, v);
                visible_pages += u32::from(visible);
                if !p.known || (visible && !p.intrinsic_rotation_known) {
                    unknown.push((
                        !visible,
                        editor.page_plan.get(p.index).unwrap().source_index,
                        visible,
                    ));
                }
                output.push(PdfeditorPageLayout {
                    page_id: p.page_id.0,
                    index: p.index,
                    geometry_known: u32::from(p.known),
                    x: p.bounds.origin.x,
                    y: p.bounds.origin.y,
                    width: p.bounds.size.width,
                    height: p.bounds.size.height,
                    page_width: p.geometry.size.width,
                    page_height: p.geometry.size.height,
                    spacing_before: p.spacing_before,
                    spacing_after: p.spacing_after,
                    intrinsic_rotation: p.geometry.rotation_degrees,
                    effective_rotation: p.effective_rotation,
                    intrinsic_rotation_known: u32::from(p.intrinsic_rotation_known),
                });
            }
            r.update(v.generation, demand).map_err(viewport_error)?;
            s.generation = v.generation;
            unknown.sort();
            c.geometry
                .request(unknown.into_iter().map(|p| (p.1, p.2)).collect());
            let current_page = s.layout.current_page(v).unwrap_or(0) as u32;
            let current_id = editor.page_plan.get(current_page).unwrap().id;
            editor
                .set_current(current_id)
                .map_err(super::editing::edit_error)?;
            let result = PdfeditorLayoutSnapshot {
                origin_x: v.origin.x,
                origin_y: v.origin.y,
                extent_width: extent.width,
                extent_height: extent.height,
                open_micros: d.open_micros,
                layout_init_micros: c.init_micros,
                geometry_micros,
                geometry_queries,
                metadata_bytes: s.layout.metadata_bytes(),
                page_count: s.layout.len() as u32,
                current_page,
                returned_pages: output.len() as u32,
                visible_pages,
                visible_tiles,
                known_pages: s.known_pages,
            };
            unsafe {
                ptr::copy_nonoverlapping(output.as_ptr(), pages, output.len());
                ptr::write(snapshot, result);
            }
            Ok(())
        })
    })
}
/// Nonblocking metadata completion probe; timer may submit a new generation.
/// # Safety
/// Output must be writable scalar storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_layout_needs_refresh(
    handle: *mut PdfeditorDocument,
    out: *mut u32,
) -> i32 {
    if out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, 0);
    }
    ffi_call(|| {
        with_document(handle, |d| {
            let ready = d.continuous.get().is_some_and(|c| {
                !c.geometry
                    .shared
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .ready
                    .is_empty()
            });
            unsafe {
                ptr::write(out, u32::from(ready));
            }
            Ok(())
        })
    })
}
/// Computes an estimated or known unscaled document destination; no PDFium call.
/// # Safety
/// Viewport and two writable f64 outputs must be valid separate storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_go_to_page(
    handle: *mut PdfeditorDocument,
    index: u32,
    viewport: *const PdfeditorDocumentViewport,
    out_x: *mut f64,
    out_y: *mut f64,
) -> i32 {
    if viewport.is_null() || out_x.is_null() || out_y.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let v = DocumentViewport::from(unsafe { ptr::read(viewport) });
    unsafe {
        ptr::write(out_x, 0.0);
        ptr::write(out_y, 0.0);
    }
    ffi_call(|| {
        with_document(handle, |d| {
            v.validate().map_err(viewport_error)?;
            let s = continuous(d)
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let p = s
                .layout
                .go_to_page(index as usize, v)
                .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?;
            unsafe {
                ptr::write(out_x, p.x);
                ptr::write(out_y, p.y);
            }
            Ok(())
        })
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_layout_and_null_validation() {
        assert_eq!(std::mem::size_of::<PdfeditorDocumentViewport>(), 72);
        assert_eq!(std::mem::size_of::<PdfeditorPageLayout>(), 88);
        assert_eq!(std::mem::size_of::<PdfeditorLayoutSnapshot>(), 96);
        assert_eq!(
            unsafe {
                pdfeditor_document_update_continuous_viewport(
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    0,
                )
            },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
    }
}
