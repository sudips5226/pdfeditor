//! ABI v9: native residency acknowledgment and successful Present acknowledgment.
use super::continuous::PdfeditorDocumentViewport;
use super::viewport::{ffi_call, renderer, viewport_error, PdfeditorTileKey};
use super::*;
use document_core::presentation::Destination;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorPresentationSnapshot {
    pub requested: PdfeditorDocumentViewport,
    pub displayed: PdfeditorDocumentViewport,
    pub requested_at: u64,
    pub render_started_at: u64,
    pub cpu_ready_at: u64,
    pub gpu_ready_at: u64,
    pub committed_at: u64,
    pub hold_micros: u64,
    pub commit_count: u64,
    pub stale_destinations: u64,
    pub coalesced_requests: u64,
    pub partial_presentations: u64,
    pub requested_page: u32,
    pub displayed_page: u32,
    pub required_count: u32,
    pub cpu_count: u32,
    pub gpu_count: u32,
    pub missing_count: u32,
    /// 0 stable, 1 pending, 2 ready to commit. Only successful Present commits.
    pub state: u32,
    pub geometry_ready: u32,
}
fn viewport(d: Option<Destination>) -> PdfeditorDocumentViewport {
    d.map(|d| {
        let v = d.viewport;
        PdfeditorDocumentViewport {
            origin_x: v.origin.x,
            origin_y: v.origin.y,
            width: v.extent.width,
            height: v.extent.height,
            scale: v.scale,
            device_pixel_ratio: v.device_pixel_ratio,
            page_gap: v.page_gap,
            generation: v.generation,
            rotation_degrees: v.rotation_degrees,
        }
    })
    .unwrap_or_default()
}
/// Report actual native textures before submission and after uploads/evictions.
/// # Safety
/// Keys must reference count readable entries (null allowed for zero); count <=512.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_gpu_residency(
    handle: *mut PdfeditorDocument,
    keys: *const PdfeditorTileKey,
    count: u32,
) -> i32 {
    if count > 512 {
        return viewport::PDFEDITOR_ERROR_CAPACITY;
    }
    if count != 0 && keys.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let keys = if count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(keys, count as usize) }
    };
    ffi_call(|| {
        with_document(handle, |d| {
            renderer(d, Default::default())?
                .set_gpu_residency(keys.iter().copied().map(Into::into).collect())
                .map_err(viewport_error)
        })
    })
}
/// Snapshot and exact mandatory keys; no PDFium call, no tile copies or waits.
/// # Safety
/// Snapshot and capacity keys must be writable separate storage; capacity <=4096.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_presentation_snapshot(
    handle: *mut PdfeditorDocument,
    out: *mut PdfeditorPresentationSnapshot,
    keys: *mut PdfeditorTileKey,
    capacity: u32,
) -> i32 {
    if out.is_null() || (capacity != 0 && keys.is_null()) {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    if capacity > 4096 {
        return viewport::PDFEDITOR_ERROR_CAPACITY;
    }
    ffi_call(|| {
        with_document(handle, |d| {
            renderer(d, Default::default())?.presentation(|p| {
                if p.required.len() > capacity as usize {
                    return Err(viewport::PDFEDITOR_ERROR_CAPACITY);
                }
                let m = &p.metrics;
                let snapshot = PdfeditorPresentationSnapshot {
                    requested: viewport(p.requested),
                    displayed: viewport(p.displayed),
                    requested_at: m.requested_at,
                    render_started_at: m.render_started_at,
                    cpu_ready_at: m.cpu_ready_at,
                    gpu_ready_at: m.gpu_ready_at,
                    committed_at: m.committed_at,
                    hold_micros: m.hold_micros,
                    commit_count: m.commit_count,
                    stale_destinations: m.stale_destinations,
                    coalesced_requests: m.coalesced_requests,
                    partial_presentations: m.partial_presentations,
                    requested_page: p.requested.map_or(0, |d| d.current_page),
                    displayed_page: p.displayed.map_or(0, |d| d.current_page),
                    required_count: p.required.len() as u32,
                    cpu_count: p.cpu.len() as u32,
                    gpu_count: p.gpu.len() as u32,
                    missing_count: (p.required.len() - p.gpu.len()) as u32,
                    state: if p.ready() { 2 } else { u32::from(p.pending()) },
                    geometry_ready: u32::from(p.geometry_ready),
                };
                for (i, key) in p.required.iter().enumerate() {
                    unsafe {
                        ptr::write(keys.add(i), (*key).into());
                    }
                }
                unsafe {
                    ptr::write(out, snapshot);
                }
                Ok(())
            })
        })
    })
}
/// Call on the coordinated UI/render thread only AFTER successful complete Present.
#[no_mangle]
pub extern "C" fn pdfeditor_document_commit_presentation(
    handle: *mut PdfeditorDocument,
    generation: u64,
) -> i32 {
    ffi_call(|| {
        with_document(handle, |d| {
            renderer(d, Default::default())?
                .commit_presentation(generation)
                .map_err(viewport_error)
        })
    })
}
/// Actual user travel direction: -1 up, 0 no speculation, +1 down. Layout
/// correction and geometry refresh must not change this signal.
#[no_mangle]
pub extern "C" fn pdfeditor_document_navigation_direction(
    handle: *mut PdfeditorDocument,
    direction: i32,
) -> i32 {
    ffi_call(|| {
        with_document(handle, |d| {
            renderer(d, Default::default())?
                .set_direction(direction)
                .map_err(viewport_error)
        })
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_and_nulls() {
        assert_eq!(std::mem::size_of::<PdfeditorPresentationSnapshot>(), 256);
        assert_eq!(
            unsafe { pdfeditor_document_gpu_residency(ptr::null_mut(), ptr::null(), 1) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        assert_eq!(
            unsafe {
                pdfeditor_document_presentation_snapshot(
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    0,
                )
            },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
    }
}
