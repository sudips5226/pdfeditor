//! Renderer-independent document-platform primitives.
//!
//! P0 intentionally keeps this crate dependency-free. Backend-specific handles
//! from PDFium, qpdf, DirectX, or WinUI must never appear in this public core.

/// Commercial viewer tile edge in physical pixels for the initial architecture.
pub const TILE_SIZE: u32 = 512;

/// Stable logical identity for a source document.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(pub u64);

/// Stable logical identity for a page. Page position is deliberately separate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PageId(pub u64);

/// Page dimensions in normalized application page space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
}

/// Renderer-agnostic request for a tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRequest {
    pub page_id: PageId,
    pub tile_x: u32,
    pub tile_y: u32,
    pub scale: f64,
    pub device_pixel_ratio: f64,
    pub rotation_degrees: u16,
}

/// Packed 32-bit BGRA tile pixels.
#[derive(Debug, Eq, PartialEq)]
pub struct TileBuffer {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

impl TileBuffer {
    pub fn new_bgra(width: u32, height: u32) -> Self {
        let stride = width.checked_mul(4).expect("tile stride overflow");
        let len = usize::try_from(stride)
            .expect("stride must fit in usize")
            .checked_mul(usize::try_from(height).expect("height must fit in usize"))
            .expect("tile allocation length overflow");

        Self {
            width,
            height,
            stride,
            pixels: vec![0; len],
        }
    }
}

/// Synthetic P0 tile used only to prove the Rust-to-native ABI path.
///
/// This is not a PDF renderer. PDFium will later supply the same TileBuffer contract.
pub fn render_test_tile() -> TileBuffer {
    let mut tile = TileBuffer::new_bgra(TILE_SIZE, TILE_SIZE);

    for y in 0..tile.height {
        for x in 0..tile.width {
            let offset = usize::try_from(y * tile.stride + x * 4)
                .expect("tile offset must fit in usize");

            tile.pixels[offset] = (x & 0xff) as u8;
            tile.pixels[offset + 1] = (y & 0xff) as u8;
            tile.pixels[offset + 2] = ((x ^ y) & 0xff) as u8;
            tile.pixels[offset + 3] = 255;
        }
    }

    tile
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p0_tile_has_expected_shape() {
        let tile = render_test_tile();

        assert_eq!(tile.width, TILE_SIZE);
        assert_eq!(tile.height, TILE_SIZE);
        assert_eq!(tile.stride, TILE_SIZE * 4);
        assert_eq!(
            tile.pixels.len(),
            usize::try_from(TILE_SIZE * TILE_SIZE * 4).unwrap()
        );
        assert!(tile.pixels.chunks_exact(4).all(|px| px[3] == 255));
    }
}
