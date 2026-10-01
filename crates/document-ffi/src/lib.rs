//! Narrow C ABI with opaque, checked document handles and explicit tile ownership.

use document_core::{
    render_test_tile, DocumentModel, LocalFileSource, PageId, TileBuffer, TileRequest,
};
use pdfium_backend::{PdfiumDocument, PdfiumError};
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

mod continuous;
mod editing;
mod thumbnails;
mod viewport;

pub const PDFEDITOR_ABI_VERSION: u32 = 7;
pub const PDFEDITOR_OK: i32 = 0;
pub const PDFEDITOR_ERROR_NULL_ARGUMENT: i32 = 1;
pub const PDFEDITOR_ERROR_INTERNAL: i32 = 2;
pub const PDFEDITOR_ERROR_INVALID_UTF8: i32 = 3;
pub const PDFEDITOR_ERROR_PDFIUM: i32 = 4;
pub const PDFEDITOR_ERROR_INVALID_HANDLE: i32 = 5;
pub const PDFEDITOR_ERROR_INVALID_PAGE: i32 = 6;
pub const PDFEDITOR_ERROR_INVALID_TILE_REQUEST: i32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorTileRequest {
    pub page_id: u64,
    pub tile_x: i32,
    pub tile_y: i32,
    pub scale: f64,
    pub device_pixel_ratio: f64,
    pub rotation_degrees: u16,
    pub width: u32,
    pub height: u32,
}

impl From<PdfeditorTileRequest> for TileRequest {
    fn from(request: PdfeditorTileRequest) -> Self {
        Self {
            page_id: PageId(request.page_id),
            tile_x: request.tile_x,
            tile_y: request.tile_y,
            scale: request.scale,
            device_pixel_ratio: request.device_pixel_ratio,
            rotation_degrees: request.rotation_degrees,
            width: request.width,
            height: request.height,
        }
    }
}

/// Opaque C type. Values are monotonic tokens, never dereferenced pointers.
#[repr(C)]
pub struct PdfeditorDocument {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorPageGeometry {
    pub page_id: u64,
    pub width_points: f64,
    pub height_points: f64,
    pub rotation_degrees: u16,
}

#[repr(C)]
pub struct PdfeditorTile {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub len: usize,
    pub data: *mut u8,
}

impl Default for PdfeditorTile {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            stride: 0,
            len: 0,
            data: ptr::null_mut(),
        }
    }
}

struct OpenDocument {
    backend: Arc<PdfiumDocument>,
    // Immutable identity/source lookup for backend jobs, including undo-restorable pages.
    model: DocumentModel,
    editor: Mutex<document_core::editing::Editor>,
    operation: Mutex<()>,
    _source: LocalFileSource,
    // Stop metadata acquisition before joining the render worker on close.
    continuous: OnceLock<continuous::ContinuousRenderer>,
    renderer: OnceLock<Result<document_core::scheduler::RenderScheduler, i32>>,
    geometry: Mutex<HashMap<PageId, document_core::PageSize>>,
    thumbnails: Mutex<thumbnails::ThumbnailState>,
    open_micros: u64,
}

static DOCUMENTS: OnceLock<Mutex<HashMap<usize, Arc<OpenDocument>>>> = OnceLock::new();
static NEXT_HANDLE: AtomicUsize = AtomicUsize::new(1);

fn documents() -> &'static Mutex<HashMap<usize, Arc<OpenDocument>>> {
    DOCUMENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_document<T>(
    handle: *mut PdfeditorDocument,
    operation: impl FnOnce(&OpenDocument) -> Result<T, i32>,
) -> Result<T, i32> {
    if handle.is_null() {
        return Err(PDFEDITOR_ERROR_NULL_ARGUMENT);
    }
    let guard = documents()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let document = guard
        .get(&(handle as usize))
        .cloned()
        .ok_or(PDFEDITOR_ERROR_INVALID_HANDLE)?;
    drop(guard);
    let _gate = document.operation.lock().unwrap_or_else(|p| p.into_inner());
    operation(&document)
}

fn backend_error(error: PdfiumError) -> i32 {
    match error {
        PdfiumError::InvalidPageIndex(_) => PDFEDITOR_ERROR_INVALID_PAGE,
        PdfiumError::InvalidTileRequest(_) => PDFEDITOR_ERROR_INVALID_TILE_REQUEST,
        _ => PDFEDITOR_ERROR_PDFIUM,
    }
}

fn into_ffi_tile(tile: TileBuffer) -> PdfeditorTile {
    let boxed = tile.pixels.into_boxed_slice();
    let len = boxed.len();
    let data = Box::into_raw(boxed) as *mut u8;
    PdfeditorTile {
        width: tile.width,
        height: tile.height,
        stride: tile.stride,
        len,
        data,
    }
}

#[no_mangle]
pub extern "C" fn pdfeditor_abi_version() -> u32 {
    PDFEDITOR_ABI_VERSION
}

/// Opens a local PDF. On success, close the returned handle exactly once.
///
/// # Safety
/// `path_utf8` must point to a valid NUL-terminated string; `out_document`
/// must point to writable storage for one pointer. Outputs are zeroed on error.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_open_utf8(
    path_utf8: *const c_char,
    out_document: *mut *mut PdfeditorDocument,
) -> i32 {
    if out_document.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out_document, ptr::null_mut());
    }
    if path_utf8.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let path = match unsafe { CStr::from_ptr(path_utf8) }.to_str() {
        Ok(path) => path,
        Err(_) => return PDFEDITOR_ERROR_INVALID_UTF8,
    };
    match std::panic::catch_unwind(|| {
        let started = std::time::Instant::now();
        let source = LocalFileSource::new(Path::new(path));
        let backend = PdfiumDocument::open(&source).map_err(backend_error)?;
        let model = DocumentModel::new(backend.page_count());
        let token = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        if token == 0 {
            return Err(PDFEDITOR_ERROR_INTERNAL);
        }
        documents()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(
                token,
                Arc::new(OpenDocument {
                    _source: source,
                    editor: Mutex::new(document_core::editing::Editor::new(
                        model.page_plan.clone(),
                    )),
                    operation: Mutex::new(()),
                    model,
                    backend: Arc::new(backend),
                    renderer: OnceLock::new(),
                    geometry: Mutex::new(HashMap::new()),
                    continuous: OnceLock::new(),
                    thumbnails: Mutex::new(Default::default()),
                    open_micros: started.elapsed().as_micros() as u64,
                }),
            );
        Ok(token as *mut PdfeditorDocument)
    }) {
        Ok(Ok(handle)) => {
            unsafe {
                ptr::write(out_document, handle);
            }
            PDFEDITOR_OK
        }
        Ok(Err(code)) => code,
        Err(_) => PDFEDITOR_ERROR_INTERNAL,
    }
}

/// Closes a handle. Null and stale handles are rejected without dereferencing.
#[no_mangle]
pub extern "C" fn pdfeditor_document_close(handle: *mut PdfeditorDocument) -> i32 {
    if handle.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    match std::panic::catch_unwind(|| {
        let document = documents()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&(handle as usize));
        match document {
            Some(document) => {
                drop(document);
                PDFEDITOR_OK
            }
            None => PDFEDITOR_ERROR_INVALID_HANDLE,
        }
    }) {
        Ok(code) => code,
        Err(_) => PDFEDITOR_ERROR_INTERNAL,
    }
}

/// # Safety
/// `out_count` must point to writable storage for one u32.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_page_count(
    handle: *mut PdfeditorDocument,
    out_count: *mut u32,
) -> i32 {
    if out_count.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out_count, 0);
    }
    match std::panic::catch_unwind(|| {
        with_document(handle, |document| {
            Ok(document
                .editor
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .page_plan
                .entries()
                .len() as u32)
        })
    }) {
        Ok(Ok(count)) => {
            unsafe {
                ptr::write(out_count, count);
            }
            PDFEDITOR_OK
        }
        Ok(Err(code)) => code,
        Err(_) => PDFEDITOR_ERROR_INTERNAL,
    }
}

/// Page index means current logical position. Geometry includes stable PageId.
///
/// # Safety
/// `out_geometry` must point to writable storage for one geometry value.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_page_geometry(
    handle: *mut PdfeditorDocument,
    page_index: u32,
    out_geometry: *mut PdfeditorPageGeometry,
) -> i32 {
    if out_geometry.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out_geometry, PdfeditorPageGeometry::default());
    }
    match std::panic::catch_unwind(|| {
        with_document(handle, |document| {
            let entry = document
                .editor
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .page_plan
                .get(page_index)
                .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?;
            let geometry = document
                .backend
                .page_geometry(entry.source_index)
                .map_err(backend_error)?;
            let mut cache = document.geometry.lock().unwrap_or_else(|p| p.into_inner());
            cache.clear();
            cache.insert(entry.id, geometry.size);
            Ok(PdfeditorPageGeometry {
                page_id: entry.id.0,
                width_points: geometry.size.width,
                height_points: geometry.size.height,
                rotation_degrees: geometry.rotation_degrees,
            })
        })
    }) {
        Ok(Ok(geometry)) => {
            unsafe {
                ptr::write(out_geometry, geometry);
            }
            PDFEDITOR_OK
        }
        Ok(Err(code)) => code,
        Err(_) => PDFEDITOR_ERROR_INTERNAL,
    }
}

/// Renders an exact page region through an already-open document.
///
/// # Safety
/// `request` must point to one readable request. `out_tile` must point to writable,
/// separate storage. Outputs are cleared on failure; release successful pixels
/// exactly once with `pdfeditor_tile_free`.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_render_tile(
    handle: *mut PdfeditorDocument,
    request: *const PdfeditorTileRequest,
    out_tile: *mut PdfeditorTile,
) -> i32 {
    if out_tile.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out_tile, PdfeditorTile::default());
    }
    if request.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let request = TileRequest::from(unsafe { ptr::read(request) });
    match std::panic::catch_unwind(|| {
        with_document(handle, |document| {
            request
                .validate()
                .map_err(|_| PDFEDITOR_ERROR_INVALID_TILE_REQUEST)?;
            let source_index = document
                .editor
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .page_plan
                .source_index_of(request.page_id)
                .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?;
            document
                .backend
                .render_tile(source_index, &request)
                .map_err(backend_error)
        })
    }) {
        Ok(Ok(tile)) => {
            unsafe {
                ptr::write(out_tile, into_ffi_tile(tile));
            }
            PDFEDITOR_OK
        }
        Ok(Err(code)) => code,
        Err(_) => PDFEDITOR_ERROR_INTERNAL,
    }
}

/// Legacy full-page preview retained for P0/P1 compatibility tests.
///
/// # Safety
/// `out_tile` must point to writable storage for one tile. Release its pixels
/// once with `pdfeditor_tile_free` after a successful call.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_render_page_preview(
    handle: *mut PdfeditorDocument,
    page_index: u32,
    out_tile: *mut PdfeditorTile,
) -> i32 {
    if out_tile.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out_tile, PdfeditorTile::default());
    }
    match std::panic::catch_unwind(|| {
        with_document(handle, |document| {
            let entry = document
                .editor
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .page_plan
                .get(page_index)
                .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?;
            document
                .backend
                .render_page_preview(entry.source_index)
                .map_err(backend_error)
        })
    }) {
        Ok(Ok(tile)) => {
            unsafe {
                ptr::write(out_tile, into_ffi_tile(tile));
            }
            PDFEDITOR_OK
        }
        Ok(Err(code)) => code,
        Err(_) => PDFEDITOR_ERROR_INTERNAL,
    }
}

/// # Safety
/// `out_tile` must point to writable storage for one tile.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_render_test_tile(out_tile: *mut PdfeditorTile) -> i32 {
    if out_tile.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out_tile, PdfeditorTile::default());
    }
    match std::panic::catch_unwind(render_test_tile) {
        Ok(tile) => {
            unsafe {
                ptr::write(out_tile, into_ffi_tile(tile));
            }
            PDFEDITOR_OK
        }
        Err(_) => PDFEDITOR_ERROR_INTERNAL,
    }
}

/// # Safety
/// `tile` must be null or point to a tile initialized by this ABI. Call once
/// for each returned pixel buffer; repeated calls on the cleared tile are safe.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_tile_free(tile: *mut PdfeditorTile) {
    if tile.is_null() {
        return;
    }
    let tile_ref = unsafe { &mut *tile };
    if !tile_ref.data.is_null() && tile_ref.len != 0 {
        let raw = ptr::slice_from_raw_parts_mut(tile_ref.data, tile_ref.len);
        unsafe {
            drop(Box::from_raw(raw));
        }
    }
    *tile_ref = PdfeditorTile::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn tile_api_null_and_stale_handles_clear_output() {
        // Mirrored by the native bridge and ctypes smoke test on x64.
        assert_eq!(std::mem::size_of::<PdfeditorTileRequest>(), 48);
        assert_eq!(std::mem::offset_of!(PdfeditorTileRequest, width), 36);
        let request = PdfeditorTileRequest {
            width: 512,
            height: 512,
            scale: 1.0,
            device_pixel_ratio: 1.0,
            ..Default::default()
        };
        let mut tile = PdfeditorTile {
            width: 9,
            ..Default::default()
        };
        assert_eq!(
            unsafe { pdfeditor_document_render_tile(ptr::null_mut(), &request, ptr::null_mut()) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        assert_eq!(
            unsafe { pdfeditor_document_render_tile(ptr::null_mut(), ptr::null(), &mut tile) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        assert_eq!(tile.width, 0);
        assert_eq!(
            unsafe {
                pdfeditor_document_render_tile(
                    123usize as *mut PdfeditorDocument,
                    &request,
                    &mut tile,
                )
            },
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
        assert!(tile.data.is_null());
    }

    #[test]
    fn persistent_handle_renders_regions_zoom_edges_crop_and_rotations() {
        if std::env::var_os("PDFEDITOR_PDFIUM_PATH").is_none() {
            return;
        }
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/p2-tile-regions.pdf");
        let path = CString::new(fixture.to_str().unwrap()).unwrap();
        let mut handle = ptr::null_mut();
        assert_eq!(
            unsafe { pdfeditor_document_open_utf8(path.as_ptr(), &mut handle) },
            PDFEDITOR_OK
        );
        let geometry = |position| {
            let mut g = PdfeditorPageGeometry::default();
            assert_eq!(
                unsafe { pdfeditor_document_page_geometry(handle, position, &mut g) },
                PDFEDITOR_OK
            );
            g
        };
        let base = PdfeditorTileRequest {
            page_id: geometry(0).page_id,
            tile_x: 0,
            tile_y: 0,
            scale: 1.0,
            device_pixel_ratio: 1.0,
            rotation_degrees: 0,
            width: 512,
            height: 512,
        };
        let render = |request: PdfeditorTileRequest| {
            let mut tile = PdfeditorTile::default();
            assert_eq!(
                unsafe { pdfeditor_document_render_tile(handle, &request, &mut tile) },
                PDFEDITOR_OK
            );
            assert_eq!(
                (tile.width, tile.height, tile.stride, tile.len),
                (512, 512, 2048, 1_048_576)
            );
            let pixels = unsafe { std::slice::from_raw_parts(tile.data, tile.len) }.to_vec();
            unsafe {
                pdfeditor_tile_free(&mut tile);
                pdfeditor_tile_free(&mut tile);
            }
            assert!(tile.data.is_null());
            pixels
        };
        let sample = |pixels: &[u8], x: usize, y: usize| -> [u8; 4] {
            pixels[y * 2048 + x * 4..y * 2048 + x * 4 + 4]
                .try_into()
                .unwrap()
        };
        for (scale, dpr) in [(1.0, 1.0), (2.0, 1.0), (1.25, 1.5), (16.0, 1.0)] {
            for rotation in [0, 90, 180, 270] {
                for (x, y, color) in [
                    (83.0, 123.0, [0, 0, 255, 255]),
                    (583.0, 123.0, [0, 255, 0, 255]),
                    (83.0, 523.0, [255, 0, 0, 255]),
                    (583.0, 523.0, [0, 255, 255, 255]),
                ] {
                    // Independent expected clockwise rotation, in normalized page points.
                    let (rx, ry) = match rotation {
                        0 => (x, y),
                        90 => (700.0 - y, x),
                        180 => (800.0 - x, 700.0 - y),
                        270 => (y, 800.0 - x),
                        _ => unreachable!(),
                    };
                    let px = (rx * scale * dpr) as i32;
                    let py = (ry * scale * dpr) as i32;
                    let pixels = render(PdfeditorTileRequest {
                        tile_x: px / 512,
                        tile_y: py / 512,
                        scale,
                        device_pixel_ratio: dpr,
                        rotation_degrees: rotation,
                        ..base
                    });
                    assert_eq!(
                        sample(&pixels, (px % 512) as usize, (py % 512) as usize),
                        color,
                        "rotation={rotation}, scale={scale}, DPR={dpr}, point=({x},{y})"
                    );
                }
            }
        }
        let edge = render(PdfeditorTileRequest {
            tile_x: 1,
            tile_y: 1,
            ..base
        });
        // A 0.25-point vector stroke becomes four physical pixels at 16x.
        // Samples straddling its center prove target-resolution rendering.
        let fine = render(PdfeditorTileRequest {
            tile_x: 12,
            tile_y: 3,
            scale: 16.0,
            ..base
        });
        assert_eq!(sample(&fine, 255, 432), [0, 0, 0, 255]);
        assert_eq!(sample(&fine, 256, 432), [0, 0, 0, 255]);
        assert_eq!(sample(&fine, 263, 432), [0, 255, 0, 255]);
        assert_eq!(sample(&edge, 188, 88), [0, 255, 255, 255]);
        assert_eq!(sample(&edge, 400, 88), [255; 4]);
        assert_eq!(sample(&edge, 188, 300), [255; 4]);
        for (tile_x, tile_y) in [(-1, 0), (0, -1), (2, 2)] {
            assert!(render(PdfeditorTileRequest {
                tile_x,
                tile_y,
                ..base
            })
            .iter()
            .all(|b| *b == 255));
        }
        let normal = render(base);
        assert_eq!(
            normal,
            render(PdfeditorTileRequest {
                page_id: geometry(4).page_id,
                ..base
            })
        );
        for (position, rotation) in [(1, 90), (2, 180), (3, 270)] {
            let g = geometry(position);
            assert_eq!(g.rotation_degrees, rotation);
            assert_eq!(
                render(PdfeditorTileRequest {
                    page_id: g.page_id,
                    ..base
                }),
                render(PdfeditorTileRequest {
                    rotation_degrees: rotation,
                    ..base
                })
            );
        }
        let mut tile = PdfeditorTile::default();
        for request in [
            PdfeditorTileRequest { scale: 0.0, ..base },
            PdfeditorTileRequest {
                scale: f64::NAN,
                ..base
            },
            PdfeditorTileRequest {
                device_pixel_ratio: -1.0,
                ..base
            },
            PdfeditorTileRequest {
                rotation_degrees: 45,
                ..base
            },
            PdfeditorTileRequest { width: 513, ..base },
            PdfeditorTileRequest {
                tile_x: i32::MAX,
                ..base
            },
        ] {
            assert_eq!(
                unsafe { pdfeditor_document_render_tile(handle, &request, &mut tile) },
                PDFEDITOR_ERROR_INVALID_TILE_REQUEST
            );
            assert!(tile.data.is_null());
            assert_eq!(tile.len, 0);
        }
        assert_eq!(
            unsafe {
                pdfeditor_document_render_tile(
                    handle,
                    &PdfeditorTileRequest { page_id: 0, ..base },
                    &mut tile,
                )
            },
            PDFEDITOR_ERROR_INVALID_PAGE
        );
        let mut second = ptr::null_mut();
        assert_eq!(
            unsafe { pdfeditor_document_open_utf8(path.as_ptr(), &mut second) },
            PDFEDITOR_OK
        );
        assert_eq!(
            unsafe { pdfeditor_document_render_tile(second, &base, &mut tile) },
            PDFEDITOR_ERROR_INVALID_PAGE
        );
        assert_eq!(pdfeditor_document_close(second), PDFEDITOR_OK);
        assert_eq!(
            unsafe { pdfeditor_document_render_tile(handle, &base, &mut tile) },
            PDFEDITOR_OK
        );
        // Pixel ownership remains valid even after its document closes.
        assert_eq!(pdfeditor_document_close(handle), PDFEDITOR_OK);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(tile.data, tile.len) },
            normal
        );
        unsafe {
            pdfeditor_tile_free(&mut tile);
        }
        assert_eq!(
            unsafe { pdfeditor_document_render_tile(handle, &base, &mut tile) },
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
    }

    #[test]
    fn ffi_tile_round_trip() {
        let mut tile = PdfeditorTile::default();
        assert_eq!(
            unsafe { pdfeditor_render_test_tile(&mut tile) },
            PDFEDITOR_OK
        );
        assert_eq!((tile.width, tile.height, tile.stride), (512, 512, 2048));
        assert_eq!(tile.len, 512 * 512 * 4);
        unsafe {
            pdfeditor_tile_free(&mut tile);
        }
        assert!(tile.data.is_null());
    }

    #[test]
    fn invalid_arguments_and_stale_handle() {
        assert_eq!(
            unsafe { pdfeditor_document_open_utf8(ptr::null(), ptr::null_mut()) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        let mut handle = ptr::null_mut();
        assert_eq!(
            unsafe { pdfeditor_document_open_utf8(ptr::null(), &mut handle) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        let invalid_utf8 = [255u8, 0];
        assert_eq!(
            unsafe { pdfeditor_document_open_utf8(invalid_utf8.as_ptr().cast(), &mut handle) },
            PDFEDITOR_ERROR_INVALID_UTF8
        );
        assert_eq!(
            pdfeditor_document_close(ptr::null_mut()),
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        assert_eq!(
            pdfeditor_document_close(123usize as *mut PdfeditorDocument),
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
        let mut count = 1;
        assert_eq!(
            unsafe {
                pdfeditor_document_page_count(123usize as *mut PdfeditorDocument, &mut count)
            },
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
        assert_eq!(count, 0);
        assert_eq!(
            unsafe { pdfeditor_document_page_count(ptr::null_mut(), ptr::null_mut()) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        assert_eq!(
            unsafe { pdfeditor_document_page_geometry(ptr::null_mut(), 0, ptr::null_mut()) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        assert_eq!(
            unsafe { pdfeditor_document_render_page_preview(ptr::null_mut(), 0, ptr::null_mut()) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
    }

    #[test]
    fn opened_document_geometry_render_and_close() {
        // CI may not have PDFium until WinUI restore. Set this variable when
        // running the Rust suite with a deployed PDFium library.
        if std::env::var_os("PDFEDITOR_PDFIUM_PATH").is_none() {
            return;
        }
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/p0-one-page.pdf");
        let path = CString::new(fixture.to_str().unwrap()).unwrap();
        let mut handle = ptr::null_mut();
        assert_eq!(
            unsafe { pdfeditor_document_open_utf8(path.as_ptr(), &mut handle) },
            PDFEDITOR_OK
        );
        assert!(!handle.is_null());
        let mut count = 0;
        assert_eq!(
            unsafe { pdfeditor_document_page_count(handle, &mut count) },
            PDFEDITOR_OK
        );
        assert_eq!(count, 1);
        let mut geometry = PdfeditorPageGeometry::default();
        assert_eq!(
            unsafe { pdfeditor_document_page_geometry(handle, 0, &mut geometry) },
            PDFEDITOR_OK
        );
        assert!(geometry.page_id > 0);
        assert!(geometry.width_points > 0.0 && geometry.height_points > 0.0);
        assert_eq!(geometry.rotation_degrees % 90, 0);
        let id = geometry.page_id;
        assert_eq!(
            unsafe { pdfeditor_document_page_geometry(handle, 0, &mut geometry) },
            PDFEDITOR_OK
        );
        assert_eq!(geometry.page_id, id);
        assert_eq!(
            unsafe { pdfeditor_document_page_geometry(handle, 1, &mut geometry) },
            PDFEDITOR_ERROR_INVALID_PAGE
        );
        let mut second = ptr::null_mut();
        assert_eq!(
            unsafe { pdfeditor_document_open_utf8(path.as_ptr(), &mut second) },
            PDFEDITOR_OK
        );
        let mut tile = PdfeditorTile::default();
        assert_eq!(
            unsafe { pdfeditor_document_render_page_preview(handle, 1, &mut tile) },
            PDFEDITOR_ERROR_INVALID_PAGE
        );
        assert!(tile.data.is_null());
        assert_eq!(
            unsafe { pdfeditor_document_render_page_preview(handle, 0, &mut tile) },
            PDFEDITOR_OK
        );
        assert_eq!((tile.width, tile.height), (512, 512));
        assert!(unsafe { std::slice::from_raw_parts(tile.data, tile.len) }
            .iter()
            .any(|pixel| *pixel != 255));
        unsafe {
            pdfeditor_tile_free(&mut tile);
        }
        assert_eq!(pdfeditor_document_close(handle), PDFEDITOR_OK);
        assert_eq!(
            unsafe { pdfeditor_document_render_page_preview(second, 0, &mut tile) },
            PDFEDITOR_OK
        );
        unsafe {
            pdfeditor_tile_free(&mut tile);
        }
        assert_eq!(pdfeditor_document_close(second), PDFEDITOR_OK);
        assert_eq!(
            pdfeditor_document_close(handle),
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
        assert_eq!(
            unsafe { pdfeditor_document_page_count(handle, &mut count) },
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
    }
}
