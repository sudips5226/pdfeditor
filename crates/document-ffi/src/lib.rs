//! Narrow C ABI for the native Windows application.
//!
//! The ABI exposes only plain C-compatible values and explicit ownership.

use document_core::render_test_tile;
use std::ptr;

pub const PDFEDITOR_ABI_VERSION: u32 = 1;
pub const PDFEDITOR_OK: i32 = 0;
pub const PDFEDITOR_ERROR_NULL_ARGUMENT: i32 = 1;
pub const PDFEDITOR_ERROR_INTERNAL: i32 = 2;

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

#[no_mangle]
pub extern "C" fn pdfeditor_abi_version() -> u32 {
    PDFEDITOR_ABI_VERSION
}

#[no_mangle]
pub unsafe extern "C" fn pdfeditor_render_test_tile(out_tile: *mut PdfeditorTile) -> i32 {
    if out_tile.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }

    match std::panic::catch_unwind(render_test_tile) {
        Ok(tile) => {
            let boxed = tile.pixels.into_boxed_slice();
            let len = boxed.len();
            let data = Box::into_raw(boxed) as *mut u8;

            ptr::write(
                out_tile,
                PdfeditorTile {
                    width: tile.width,
                    height: tile.height,
                    stride: tile.stride,
                    len,
                    data,
                },
            );

            PDFEDITOR_OK
        }
        Err(_) => {
            ptr::write(out_tile, PdfeditorTile::default());
            PDFEDITOR_ERROR_INTERNAL
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pdfeditor_tile_free(tile: *mut PdfeditorTile) {
    if tile.is_null() {
        return;
    }

    let tile_ref = &mut *tile;

    if !tile_ref.data.is_null() && tile_ref.len != 0 {
        let raw_slice = ptr::slice_from_raw_parts_mut(tile_ref.data, tile_ref.len);
        drop(Box::from_raw(raw_slice));
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
