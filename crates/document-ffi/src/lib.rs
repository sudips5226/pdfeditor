//! Narrow C ABI for the native Windows application.
//!
//! The ABI exposes only plain C-compatible values and explicit ownership.

use document_core::{render_test_tile, TileBuffer};
use pdfium_backend::render_page_preview;
use std::ffi::{c_char, CStr};
use std::path::Path;
use std::ptr;

pub const PDFEDITOR_ABI_VERSION: u32 = 1;
pub const PDFEDITOR_OK: i32 = 0;
pub const PDFEDITOR_ERROR_NULL_ARGUMENT: i32 = 1;
pub const PDFEDITOR_ERROR_INTERNAL: i32 = 2;
pub const PDFEDITOR_ERROR_INVALID_UTF8: i32 = 3;
pub const PDFEDITOR_ERROR_PDFIUM: i32 = 4;

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

/// Produces a synthetic P0 tile and transfers ownership of its pixel buffer.
///
/// # Safety
///
/// If non-null, out_tile must point to valid writable memory for one PdfeditorTile.
/// On success the caller owns the returned data pointer and must release it exactly
/// once with pdfeditor_tile_free.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_render_test_tile(out_tile: *mut PdfeditorTile) -> i32 {
    if out_tile.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }

    match std::panic::catch_unwind(render_test_tile) {
        Ok(tile) => {
            unsafe {
                ptr::write(out_tile, into_ffi_tile(tile));
            }
            PDFEDITOR_OK
        }
        Err(_) => {
            unsafe {
                ptr::write(out_tile, PdfeditorTile::default());
            }
            PDFEDITOR_ERROR_INTERNAL
        }
    }
}

/// Renders a real PDF page through the isolated PDFium backend.
///
/// This is a P0 proof API. P1 replaces it with persistent document handles and
/// renderer-agnostic tile requests.
///
/// # Safety
///
/// pdf_path_utf8 must be null or point to a valid NUL-terminated UTF-8 string.
/// If non-null, out_tile must point to writable memory for one PdfeditorTile.
/// On success the caller owns the returned data pointer and must release it exactly
/// once with pdfeditor_tile_free.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_render_pdf_preview_utf8(
    pdf_path_utf8: *const c_char,
    page_index: u32,
    out_tile: *mut PdfeditorTile,
) -> i32 {
    if pdf_path_utf8.is_null() || out_tile.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }

    let path = match unsafe { CStr::from_ptr(pdf_path_utf8) }.to_str() {
        Ok(path) => path,
        Err(_) => return PDFEDITOR_ERROR_INVALID_UTF8,
    };

    let result = std::panic::catch_unwind(|| render_page_preview(Path::new(path), page_index));

    match result {
        Ok(Ok(tile)) => {
            unsafe {
                ptr::write(out_tile, into_ffi_tile(tile));
            }
            PDFEDITOR_OK
        }
        Ok(Err(_)) => {
            unsafe {
                ptr::write(out_tile, PdfeditorTile::default());
            }
            PDFEDITOR_ERROR_PDFIUM
        }
        Err(_) => {
            unsafe {
                ptr::write(out_tile, PdfeditorTile::default());
            }
            PDFEDITOR_ERROR_INTERNAL
        }
    }
}

/// Releases pixel memory returned by a pdfeditor tile-rendering function.
///
/// # Safety
///
/// tile must be null or point to a valid PdfeditorTile originally initialized by
/// this ABI. A live data pointer inside the structure must have been allocated by
/// this library and must not already have been freed elsewhere.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_tile_free(tile: *mut PdfeditorTile) {
    if tile.is_null() {
        return;
    }

    let tile_ref = unsafe { &mut *tile };

    if !tile_ref.data.is_null() && tile_ref.len != 0 {
        let raw_slice = ptr::slice_from_raw_parts_mut(tile_ref.data, tile_ref.len);
        unsafe {
            drop(Box::from_raw(raw_slice));
        }
    }

    *tile_ref = PdfeditorTile::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_tile_round_trip() {
        let mut tile = PdfeditorTile::default();

        let rc = unsafe { pdfeditor_render_test_tile(&mut tile) };
        assert_eq!(rc, PDFEDITOR_OK);
        assert_eq!(tile.width, 512);
        assert_eq!(tile.height, 512);
        assert_eq!(tile.stride, 2048);
        assert_eq!(tile.len, 512 * 512 * 4);
        assert!(!tile.data.is_null());

        unsafe { pdfeditor_tile_free(&mut tile) };
        assert!(tile.data.is_null());
        assert_eq!(tile.len, 0);
    }

    #[test]
    fn ffi_rejects_null_output() {
        let rc = unsafe { pdfeditor_render_test_tile(ptr::null_mut()) };
        assert_eq!(rc, PDFEDITOR_ERROR_NULL_ARGUMENT);
    }
}
