//! Renderer-independent document-platform primitives.
//!
//! Backend-specific handles from PDFium, qpdf, DirectX, or WinUI never appear here.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Commercial viewer tile edge in physical pixels for the initial architecture.
pub const TILE_SIZE: u32 = 512;

/// Stable logical identity for a source document.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DocumentId(pub u64);

static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_PAGE_ID: AtomicU64 = AtomicU64::new(1);

impl DocumentId {
    pub fn new() -> Self {
        Self(NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for DocumentId {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable logical identity for a page. Page position is deliberately separate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PageId(pub u64);

impl PageId {
    pub fn new() -> Self {
        Self(NEXT_PAGE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for PageId {
    fn default() -> Self {
        Self::new()
    }
}

/// Page dimensions in normalized application page space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
}

/// Page geometry in PDF points. Rotation is clockwise in quarter turns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeometry {
    pub size: PageSize,
    pub rotation_degrees: u16,
}

macro_rules! point_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct $name {
            pub x: f64,
            pub y: f64,
        }
    };
}

macro_rules! rect_type {
    ($name:ident, $point:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct $name {
            pub origin: $point,
            pub size: PageSize,
        }
    };
}

point_type!(PdfPoint);
point_type!(PagePoint);
point_type!(DevicePoint);
rect_type!(PdfRect, PdfPoint);
rect_type!(PageRect, PagePoint);
rect_type!(DeviceRect, DevicePoint);

/// A source describes where document bytes live without prescribing a renderer.
pub trait DocumentSource: Send + Sync {
    fn location(&self) -> SourceLocation<'_>;
}

pub enum SourceLocation<'a> {
    LocalFile(&'a Path),
}

#[derive(Clone, Debug)]
pub struct LocalFileSource {
    path: PathBuf,
}

impl LocalFileSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl DocumentSource for LocalFileSource {
    fn location(&self) -> SourceLocation<'_> {
        SourceLocation::LocalFile(&self.path)
    }
}

/// An original page reference. `id` identifies this logical occurrence; the
/// source index identifies the page in the opened source PDF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PagePlanEntry {
    pub id: PageId,
    pub source_index: u32,
}

#[derive(Clone, Debug)]
pub struct PagePlan {
    entries: Vec<PagePlanEntry>,
}

impl PagePlan {
    pub fn from_original_pages(page_count: u32) -> Self {
        Self {
            entries: (0..page_count)
                .map(|source_index| PagePlanEntry {
                    id: PageId::new(),
                    source_index,
                })
                .collect(),
        }
    }

    pub fn entries(&self) -> &[PagePlanEntry] {
        &self.entries
    }

    pub fn get(&self, position: u32) -> Option<PagePlanEntry> {
        usize::try_from(position)
            .ok()
            .and_then(|position| self.entries.get(position).copied())
    }

    pub fn position_of(&self, id: PageId) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }
}

/// Persistent renderer-independent document state. Geometry is queried lazily
/// from the backend, so opening does not load every page.
#[derive(Debug)]
pub struct DocumentModel {
    pub id: DocumentId,
    pub page_plan: PagePlan,
}

impl DocumentModel {
    pub fn new(page_count: u32) -> Self {
        Self {
            id: DocumentId::new(),
            page_plan: PagePlan::from_original_pages(page_count),
        }
    }
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
            let offset =
                usize::try_from(y * tile.stride + x * 4).expect("tile offset must fit in usize");

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
        assert!(tile
            .pixels
            .iter()
            .skip(3)
            .step_by(4)
            .all(|alpha| *alpha == 255));
    }

    #[test]
    fn page_identity_survives_position_changes_in_a_plan() {
        let mut plan = PagePlan::from_original_pages(3);
        let original = plan.entries().to_vec();
        assert_eq!(
            original
                .iter()
                .map(|entry| entry.source_index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        plan.entries.swap(0, 2);
        assert_eq!(plan.get(0).unwrap().id, original[2].id);
        assert_eq!(plan.position_of(original[0].id), Some(2));
        assert_eq!(plan.get(0).unwrap().source_index, 2);
        assert_ne!(original[0].id, original[1].id);
    }

    #[test]
    fn document_ids_are_distinct() {
        assert_ne!(DocumentModel::new(1).id, DocumentModel::new(1).id);
    }
}
