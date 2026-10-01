//! ABI v4: polling only; no callbacks or backend handles cross the boundary.
use super::*;
use document_core::scheduler::{RenderScheduler, SchedulerConfig};
use document_core::viewport::{tile_demand, TileKey, ViewportError, ViewportState};
use document_core::DevicePoint;

pub const PDFEDITOR_NO_TILE: i32 = 8;
pub const PDFEDITOR_ERROR_VIEWPORT: i32 = 9;
pub const PDFEDITOR_ERROR_CAPACITY: i32 = 10;
pub const PDFEDITOR_ERROR_STALE_GENERATION: i32 = 11;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorViewport {
    pub page_id: u64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    pub device_pixel_ratio: f64,
    pub generation: u64,
    pub rotation_degrees: u16,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct PdfeditorTileKey {
    pub document_id: u64,
    pub document_revision: u64,
    pub page_id: u64,
    pub physical_scale_bits: u64,
    pub tile_x: i32,
    pub tile_y: i32,
    pub width: u32,
    pub height: u32,
    pub render_flags: u32,
    pub rotation_degrees: u16,
}
impl From<TileKey> for PdfeditorTileKey {
    fn from(k: TileKey) -> Self {
        Self {
            document_id: k.document_id.0,
            document_revision: k.document_revision,
            page_id: k.page_id.0,
            physical_scale_bits: k.physical_scale_bits,
            tile_x: k.tile_x,
            tile_y: k.tile_y,
            width: k.width,
            height: k.height,
            render_flags: k.render_flags,
            rotation_degrees: k.rotation_degrees,
        }
    }
}
#[repr(C)]
pub struct PdfeditorTileLease {
    _private: [u8; 0],
}
#[repr(C)]
pub struct PdfeditorReadyTile {
    pub lease: *mut PdfeditorTileLease,
    pub key: PdfeditorTileKey,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub len: usize,
    pub data: *const u8,
}
impl Default for PdfeditorReadyTile {
    fn default() -> Self {
        Self {
            lease: ptr::null_mut(),
            key: Default::default(),
            generation: 0,
            width: 0,
            height: 0,
            stride: 0,
            len: 0,
            data: ptr::null(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorRendererConfig {
    pub cpu_byte_budget: usize,
    pub queue_capacity: u32,
    pub completion_capacity: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorMetrics {
    pub tile_requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub renders_performed: u64,
    pub stale_renders_discarded: u64,
    pub render_errors: u64,
    pub cpu_cache_bytes: usize,
    pub queue_depth: usize,
    pub completion_depth: usize,
    pub outstanding_leases: usize,
}

// Bound outstanding external references independently of the CPU cache. A
// misbehaving consumer gets backpressure rather than unbounded retained pixels.
const MAX_LEASES: usize = 64;
static LEASES: OnceLock<Mutex<HashMap<usize, Arc<TileBuffer>>>> = OnceLock::new();
static NEXT_LEASE: AtomicUsize = AtomicUsize::new(1);
fn leases() -> &'static Mutex<HashMap<usize, Arc<TileBuffer>>> {
    LEASES.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(super) fn viewport_error(e: ViewportError) -> i32 {
    match e {
        ViewportError::InvalidInput => PDFEDITOR_ERROR_VIEWPORT,
        ViewportError::Capacity => PDFEDITOR_ERROR_CAPACITY,
        ViewportError::StaleGeneration => PDFEDITOR_ERROR_STALE_GENERATION,
    }
}
pub(super) fn renderer(
    document: &OpenDocument,
    config: SchedulerConfig,
) -> Result<&RenderScheduler, i32> {
    document
        .renderer
        .get_or_init(|| {
            let backend = Arc::clone(&document.backend);
            let plan = document.model.page_plan.clone();
            RenderScheduler::new(config, move |key| {
                let index = plan
                    .source_index_of(key.page_id)
                    .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?;
                backend
                    .render_tile(index, &key.request())
                    .map_err(backend_error)
            })
            .map_err(viewport_error)
        })
        .as_ref()
        .map_err(|code| *code)
}
pub(super) fn ffi_call(f: impl FnOnce() -> Result<(), i32>) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(())) => PDFEDITOR_OK,
        Ok(Err(code)) => code,
        Err(_) => PDFEDITOR_ERROR_INTERNAL,
    }
}

/// Configure once before the first viewport update. Zero fields select defaults.
/// # Safety
/// Config must point to one readable config, separate from any output storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_configure_renderer(
    handle: *mut PdfeditorDocument,
    config: *const PdfeditorRendererConfig,
) -> i32 {
    if config.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let c = unsafe { ptr::read(config) };
    let defaults = SchedulerConfig::default();
    let config = SchedulerConfig {
        cpu_bytes: if c.cpu_byte_budget == 0 {
            defaults.cpu_bytes
        } else {
            c.cpu_byte_budget
        },
        queue_capacity: if c.queue_capacity == 0 {
            defaults.queue_capacity
        } else {
            c.queue_capacity as usize
        },
        completion_capacity: if c.completion_capacity == 0 {
            defaults.completion_capacity
        } else {
            c.completion_capacity as usize
        },
    };
    ffi_call(|| {
        with_document(handle, |d| {
            if d.renderer.get().is_some() {
                return Err(PDFEDITOR_ERROR_VIEWPORT);
            }
            renderer(d, config)?;
            Ok(())
        })
    })
}

/// # Safety
/// Viewport must point to readable scalar storage for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_update_viewport(
    handle: *mut PdfeditorDocument,
    viewport: *const PdfeditorViewport,
) -> i32 {
    if viewport.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let v = unsafe { ptr::read(viewport) };
    ffi_call(|| {
        with_document(handle, |d| {
            let page_id = PageId(v.page_id);
            let index = d
                .model
                .page_plan
                .source_index_of(page_id)
                .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?;
            let size = {
                let mut geometry = d.geometry.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(size) = geometry.get(&page_id) {
                    *size
                } else {
                    let size = d.backend.page_geometry(index).map_err(backend_error)?.size;
                    geometry.clear(); // P3 keeps only the current page geometry.
                    geometry.insert(page_id, size);
                    size
                }
            };
            let state = ViewportState {
                page_id,
                origin: DevicePoint {
                    x: v.origin_x,
                    y: v.origin_y,
                },
                extent: document_core::viewport::DeviceSize {
                    width: v.width,
                    height: v.height,
                },
                scale: v.scale,
                device_pixel_ratio: v.device_pixel_ratio,
                rotation_degrees: v.rotation_degrees,
                generation: v.generation,
            };
            let r = renderer(d, Default::default())?;
            let demand =
                tile_demand(d.model.id, 0, size, state, r.capacity()).map_err(viewport_error)?;
            r.update(v.generation, demand).map_err(viewport_error)
        })
    })
}

/// Poll from the UI/render thread; NO_TILE means empty, CAPACITY means release
/// outstanding leases. Success retains the exact Arc buffer, without copying.
/// # Safety
/// Output must be writable, and must not contain an unreleased lease.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_poll_ready_tile(
    handle: *mut PdfeditorDocument,
    out: *mut PdfeditorReadyTile,
) -> i32 {
    if out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    ffi_call(|| {
        with_document(handle, |d| {
            let mut leases = leases().lock().unwrap_or_else(|p| p.into_inner());
            if leases.len() >= MAX_LEASES {
                return Err(PDFEDITOR_ERROR_CAPACITY);
            }
            let r = renderer(d, Default::default())?;
            let ready = r.poll().ok_or(PDFEDITOR_NO_TILE)?;
            let tile = ready.result?;
            let token = NEXT_LEASE.fetch_add(1, Ordering::Relaxed);
            if token == 0 {
                return Err(PDFEDITOR_ERROR_INTERNAL);
            }
            let output = PdfeditorReadyTile {
                lease: token as *mut PdfeditorTileLease,
                key: ready.key.into(),
                generation: ready.generation,
                width: tile.width,
                height: tile.height,
                stride: tile.stride,
                len: tile.pixels.len(),
                data: tile.pixels.as_ptr(),
            };
            leases.insert(token, tile);
            unsafe {
                ptr::write(out, output);
            }
            Ok(())
        })
    })
}

/// Checked opaque token; no dereference. Pixels survive document close until
/// release. Releasing a stale token returns INVALID_HANDLE.
#[no_mangle]
pub extern "C" fn pdfeditor_tile_lease_release(lease: *mut PdfeditorTileLease) -> i32 {
    ffi_call(|| {
        if lease.is_null() {
            return Err(PDFEDITOR_ERROR_NULL_ARGUMENT);
        }
        leases()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&(lease as usize))
            .ok_or(PDFEDITOR_ERROR_INVALID_HANDLE)?;
        Ok(())
    })
}

/// # Safety
/// Output must point to writable metrics storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_renderer_metrics(
    handle: *mut PdfeditorDocument,
    out: *mut PdfeditorMetrics,
) -> i32 {
    if out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    ffi_call(|| {
        with_document(handle, |d| {
            let m = renderer(d, Default::default())?.metrics();
            let outstanding_leases = leases().lock().unwrap_or_else(|p| p.into_inner()).len();
            unsafe {
                ptr::write(
                    out,
                    PdfeditorMetrics {
                        tile_requests: m.tile_requests,
                        cache_hits: m.cache_hits,
                        cache_misses: m.cache_misses,
                        renders_performed: m.renders_performed,
                        stale_renders_discarded: m.stale_renders_discarded,
                        render_errors: m.render_errors,
                        cpu_cache_bytes: m.cpu_cache_bytes,
                        queue_depth: m.queue_depth,
                        completion_depth: m.completion_depth,
                        outstanding_leases,
                    },
                );
            }
            Ok(())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lease_lifetime_and_checked_release() {
        let tile = Arc::new(TileBuffer::new_bgra(512, 512));
        let weak = Arc::downgrade(&tile);
        let token = NEXT_LEASE.fetch_add(1, Ordering::Relaxed);
        leases().lock().unwrap().insert(token, tile);
        let lease = token as *mut PdfeditorTileLease;
        assert!(weak.upgrade().is_some());
        assert_eq!(pdfeditor_tile_lease_release(lease), PDFEDITOR_OK);
        assert!(weak.upgrade().is_none());
        assert_eq!(
            pdfeditor_tile_lease_release(lease),
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
        assert_eq!(
            pdfeditor_tile_lease_release(ptr::null_mut()),
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
    }
    #[test]
    fn abi_layout_and_invalid_outputs() {
        assert_eq!(std::mem::size_of::<PdfeditorViewport>(), 72);
        assert_eq!(std::mem::size_of::<PdfeditorTileKey>(), 56);
        assert_eq!(std::mem::size_of::<PdfeditorReadyTile>(), 104);
        let mut ready = PdfeditorReadyTile::default();
        assert_eq!(
            unsafe {
                pdfeditor_document_poll_ready_tile(123usize as *mut PdfeditorDocument, &mut ready)
            },
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
        assert!(ready.data.is_null());
        assert_eq!(
            unsafe { pdfeditor_document_update_viewport(ptr::null_mut(), ptr::null()) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
    }
}
