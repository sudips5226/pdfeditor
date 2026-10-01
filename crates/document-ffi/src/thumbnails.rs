//! ABI v6: dedicated thumbnail snapshots, completions, and bounded image leases.
use super::viewport::{
    ffi_call, leases, renderer, viewport_error, PdfeditorTileLease, MAX_LEASES, NEXT_LEASE,
    PDFEDITOR_ERROR_CAPACITY, PDFEDITOR_ERROR_STALE_GENERATION, PDFEDITOR_NO_TILE,
};
use super::*;
use document_core::thumbnails::{
    ThumbnailKey, ThumbnailLayout, ThumbnailNavigator, MAX_THUMBNAIL_SLOTS,
};
use document_core::viewport::{Priority, TileDemand};

#[derive(Default)]
pub(super) struct ThumbnailState {
    pub(super) navigator: ThumbnailNavigator,
    generation: u64,
    stale: u64,
    current: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorThumbnailViewport {
    pub offset: f64,
    pub extent: f64,
    pub width: f64,
    pub height: f64,
    pub label_height: f64,
    pub gap: f64,
    pub padding: f64,
    pub dpr: f64,
    pub generation: u64,
    pub current: u32,
    pub overscan: u32,
    pub rotation: u16,
}
impl PdfeditorThumbnailViewport {
    fn layout(self) -> ThumbnailLayout {
        ThumbnailLayout {
            width: self.width,
            height: self.height,
            label_height: self.label_height,
            gap: self.gap,
            padding: self.padding,
            overscan: self.overscan as usize,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct PdfeditorThumbnailKey {
    pub document_id: u64,
    pub revision: u64,
    pub page_id: u64,
    pub dpr_bits: u64,
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    pub rotation: u16,
}
impl From<ThumbnailKey> for PdfeditorThumbnailKey {
    fn from(k: ThumbnailKey) -> Self {
        Self {
            document_id: k.document_id.0,
            revision: k.revision,
            page_id: k.page_id.0,
            dpr_bits: k.dpr_bits,
            width: k.width,
            height: k.height,
            flags: k.flags,
            rotation: k.rotation,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorThumbnailItem {
    pub key: PdfeditorThumbnailKey,
    pub recycle: u64,
    pub top: f64,
    pub index: u32,
    pub current: u32,
    pub visible: u32,
    pub selected: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorThumbnailSnapshot {
    pub offset: f64,
    pub total: f64,
    pub returned: u32,
    pub visible: u32,
}
#[repr(C)]
pub struct PdfeditorReadyThumbnail {
    pub lease: *mut PdfeditorTileLease,
    pub key: PdfeditorThumbnailKey,
    pub generation: u64,
    pub recycle: u64,
    pub status: i32,
    pub stride: u32,
    pub len: usize,
    pub data: *const u8,
}
impl Default for PdfeditorReadyThumbnail {
    fn default() -> Self {
        Self {
            lease: ptr::null_mut(),
            key: Default::default(),
            generation: 0,
            recycle: 0,
            status: 0,
            stride: 0,
            len: 0,
            data: ptr::null(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorThumbnailMetrics {
    pub hits: u64,
    pub misses: u64,
    pub renders: u64,
    pub recycled: u64,
    pub stale: u64,
    pub errors: u64,
    pub bytes: usize,
    pub queue: usize,
    pub ready: usize,
    pub slots: u32,
    pub visible: u32,
    pub current: u32,
}
/// Configure before first thumbnail submission. Budget is independent of document tiles.
#[no_mangle]
pub extern "C" fn pdfeditor_document_configure_thumbnails(
    h: *mut PdfeditorDocument,
    budget: usize,
) -> i32 {
    ffi_call(|| {
        with_document(h, |d| {
            renderer(d, Default::default())?
                .configure_thumbnails(budget)
                .map_err(viewport_error)
        })
    })
}
/// # Safety
/// Input, snapshot, and capacity item entries must be valid separate storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_update_thumbnails(
    h: *mut PdfeditorDocument,
    v: *const PdfeditorThumbnailViewport,
    out: *mut PdfeditorThumbnailSnapshot,
    items: *mut PdfeditorThumbnailItem,
    capacity: u32,
) -> i32 {
    if v.is_null() || out.is_null() || items.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let v = unsafe { ptr::read(v) };
    unsafe {
        ptr::write(out, Default::default());
    }
    ffi_call(|| {
        with_document(h, |d| {
            let editor = d.editor.lock().unwrap_or_else(|p| p.into_inner());
            let layout = v.layout();
            layout
                .validate(v.offset, v.extent, v.dpr, v.rotation)
                .map_err(viewport_error)?;
            let count = editor.page_plan.entries().len();
            let total = layout.total(count);
            let offset = v.offset.clamp(0.0, (total - v.extent).max(0.0));
            let range = layout.overscanned(count, layout.visible(count, offset, v.extent));
            if capacity as usize > MAX_THUMBNAIL_SLOTS || range.len() > capacity as usize {
                return Err(PDFEDITOR_ERROR_CAPACITY);
            }
            let mut s = d.thumbnails.lock().unwrap_or_else(|p| p.into_inner());
            if v.generation == 0 || v.generation <= s.generation {
                return Err(PDFEDITOR_ERROR_STALE_GENERATION);
            }
            s.navigator
                .update(
                    &editor.page_plan,
                    d.model.id,
                    layout,
                    offset,
                    v.extent,
                    v.dpr,
                    v.rotation,
                    v.current,
                )
                .map_err(viewport_error)?;
            let demand = s
                .navigator
                .slots
                .iter()
                .map(|slot| TileDemand {
                    key: slot.key.work_key(),
                    priority: if slot.visible {
                        Priority::Visible
                    } else {
                        Priority::Prefetch
                    },
                })
                .collect();
            renderer(d, Default::default())?
                .update_thumbnails(v.generation, demand)
                .map_err(viewport_error)?;
            s.generation = v.generation;
            s.current = v.current;
            let output: Vec<_> = s
                .navigator
                .slots
                .iter()
                .map(|slot| PdfeditorThumbnailItem {
                    key: slot.key.into(),
                    recycle: slot.recycle,
                    top: layout.top(slot.index as usize),
                    index: slot.index,
                    current: u32::from(slot.current),
                    visible: u32::from(slot.visible),
                    selected: u32::from(editor.selected(slot.key.page_id)),
                })
                .collect();
            unsafe {
                ptr::copy_nonoverlapping(output.as_ptr(), items, output.len());
                ptr::write(
                    out,
                    PdfeditorThumbnailSnapshot {
                        offset,
                        total,
                        returned: output.len() as u32,
                        visible: output.iter().filter(|i| i.visible != 0).count() as u32,
                    },
                );
            }
            Ok(())
        })
    })
}
/// # Safety
/// Output must be writable and contain no unreleased lease. Error placeholders
/// are successful completions with nonzero status, an expected key/token, and no pixels.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_poll_ready_thumbnail(
    h: *mut PdfeditorDocument,
    out: *mut PdfeditorReadyThumbnail,
) -> i32 {
    if out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    ffi_call(|| {
        with_document(h, |d| {
            let mut held = leases().lock().unwrap_or_else(|p| p.into_inner());
            if held.len() >= MAX_LEASES {
                return Err(PDFEDITOR_ERROR_CAPACITY);
            }
            let ready = renderer(d, Default::default())?
                .poll_thumbnail()
                .ok_or(PDFEDITOR_NO_TILE)?;
            let key = ThumbnailKey::from_work(ready.key);
            let mut s = d.thumbnails.lock().unwrap_or_else(|p| p.into_inner());
            let slot = s
                .navigator
                .slots
                .iter()
                .find(|slot| slot.key == key)
                .copied();
            let Some(slot) = slot.filter(|_| ready.generation == s.generation) else {
                s.stale += 1;
                return Err(PDFEDITOR_NO_TILE);
            };
            let mut output = PdfeditorReadyThumbnail {
                key: key.into(),
                generation: ready.generation,
                recycle: slot.recycle,
                ..Default::default()
            };
            match ready.result {
                Ok(raster) => {
                    let token = NEXT_LEASE.fetch_add(1, Ordering::Relaxed);
                    if token == 0 {
                        return Err(PDFEDITOR_ERROR_INTERNAL);
                    }
                    output.lease = token as *mut PdfeditorTileLease;
                    output.stride = raster.stride;
                    output.len = raster.pixels.len();
                    output.data = raster.pixels.as_ptr();
                    held.insert(token, raster);
                }
                Err(code) => output.status = code,
            }
            unsafe {
                ptr::write(out, output);
            }
            Ok(())
        })
    })
}
/// Explicit positioning action; performs no document navigation or backend call.
/// # Safety
/// Viewport and writable output must be valid separate storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_thumbnail_show_current(
    h: *mut PdfeditorDocument,
    v: *const PdfeditorThumbnailViewport,
    out: *mut f64,
) -> i32 {
    if v.is_null() || out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let v = unsafe { ptr::read(v) };
    unsafe {
        ptr::write(out, 0.0);
    }
    ffi_call(|| {
        with_document(h, |d| {
            let editor = d.editor.lock().unwrap_or_else(|p| p.into_inner());
            let l = v.layout();
            l.validate(v.offset, v.extent, v.dpr, v.rotation)
                .map_err(viewport_error)?;
            if v.current as usize >= editor.page_plan.entries().len() {
                return Err(PDFEDITOR_ERROR_INVALID_PAGE);
            }
            unsafe {
                ptr::write(
                    out,
                    l.show_current(
                        editor.page_plan.entries().len(),
                        v.current as usize,
                        v.extent,
                    ),
                );
            }
            Ok(())
        })
    })
}
/// Highlight-only synchronization; does not scroll, replace demand, or change pixel identity.
#[no_mangle]
pub extern "C" fn pdfeditor_document_thumbnail_sync_current(
    h: *mut PdfeditorDocument,
    current: u32,
) -> i32 {
    ffi_call(|| {
        with_document(h, |d| {
            let editor = d.editor.lock().unwrap_or_else(|p| p.into_inner());
            if current as usize >= editor.page_plan.entries().len() {
                return Err(PDFEDITOR_ERROR_INVALID_PAGE);
            }
            let mut s = d.thumbnails.lock().unwrap_or_else(|p| p.into_inner());
            s.current = current;
            s.navigator.sync_current(current);
            Ok(())
        })
    })
}
/// # Safety
/// Output must point to writable metrics storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_thumbnail_metrics(
    h: *mut PdfeditorDocument,
    out: *mut PdfeditorThumbnailMetrics,
) -> i32 {
    if out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    ffi_call(|| {
        with_document(h, |d| {
            let s = d.thumbnails.lock().unwrap_or_else(|p| p.into_inner());
            let m = renderer(d, Default::default())?.thumbnail_metrics();
            let slots = &s.navigator.slots;
            unsafe {
                ptr::write(
                    out,
                    PdfeditorThumbnailMetrics {
                        hits: m.cache_hits,
                        misses: m.cache_misses,
                        renders: m.renders_performed,
                        recycled: s.navigator.recycled,
                        stale: s.stale + m.stale_renders_discarded,
                        errors: m.render_errors,
                        bytes: m.cpu_cache_bytes,
                        queue: m.queue_depth,
                        ready: m.completion_depth,
                        slots: slots.len() as u32,
                        visible: slots.iter().filter(|s| s.visible).count() as u32,
                        current: s.current,
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
    fn abi_sizes_and_invalid_outputs() {
        assert_eq!(std::mem::size_of::<PdfeditorThumbnailViewport>(), 88);
        assert_eq!(std::mem::size_of::<PdfeditorThumbnailKey>(), 48);
        assert_eq!(std::mem::size_of::<PdfeditorThumbnailItem>(), 80);
        assert_eq!(std::mem::size_of::<PdfeditorReadyThumbnail>(), 96);
        assert_eq!(std::mem::size_of::<PdfeditorThumbnailMetrics>(), 88);
        let mut out = PdfeditorReadyThumbnail::default();
        assert_eq!(
            unsafe {
                pdfeditor_document_poll_ready_thumbnail(
                    123usize as *mut PdfeditorDocument,
                    &mut out,
                )
            },
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
        assert!(out.data.is_null());
        assert_eq!(
            unsafe { pdfeditor_document_poll_ready_thumbnail(ptr::null_mut(), ptr::null_mut()) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
    }
}
