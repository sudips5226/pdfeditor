//! Narrow C ABI with opaque, checked document handles and explicit tile ownership.

use document_core::{render_test_tile, DocumentModel, LocalFileSource, TileBuffer};
use pdfium_backend::{PdfiumDocument, PdfiumError};
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

pub const PDFEDITOR_ABI_VERSION: u32 = 2;
pub const PDFEDITOR_OK: i32 = 0;
pub const PDFEDITOR_ERROR_NULL_ARGUMENT: i32 = 1;
pub const PDFEDITOR_ERROR_INTERNAL: i32 = 2;
pub const PDFEDITOR_ERROR_INVALID_UTF8: i32 = 3;
pub const PDFEDITOR_ERROR_PDFIUM: i32 = 4;
pub const PDFEDITOR_ERROR_INVALID_HANDLE: i32 = 5;
pub const PDFEDITOR_ERROR_INVALID_PAGE: i32 = 6;

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
    backend: PdfiumDocument,
    model: DocumentModel,
    _source: LocalFileSource,
}

static DOCUMENTS: OnceLock<Mutex<HashMap<usize, OpenDocument>>> = OnceLock::new();
static NEXT_HANDLE: AtomicUsize = AtomicUsize::new(1);

fn documents() -> &'static Mutex<HashMap<usize, OpenDocument>> {
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
        .ok_or(PDFEDITOR_ERROR_INVALID_HANDLE)?;
    operation(document)
}

fn backend_error(error: PdfiumError) -> i32 {
    match error {
        PdfiumError::InvalidPageIndex(_) => PDFEDITOR_ERROR_INVALID_PAGE,
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
                OpenDocument {
                    _source: source,
                    model,
                    backend,
                },
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
            Ok(document.model.page_plan.entries().len() as u32)
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
                .model
                .page_plan
                .get(page_index)
                .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?;
            let geometry = document
                .backend
                .page_geometry(entry.source_index)
                .map_err(backend_error)?;
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

/// Renders from the already-open document into a 512x512 BGRA tile.
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
                .model
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
