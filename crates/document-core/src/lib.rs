//! Renderer-independent document-platform primitives.
//!
//! Backend-specific handles from PDFium, qpdf, DirectX, or WinUI never appear here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub mod editing;
pub mod layout;
#[cfg(test)]
mod p5c_tests;
pub mod prediction;
pub mod presentation;
pub mod scheduler;
pub mod thumbnails;
pub mod viewport;

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

/// Immutable backing PDF identity, separate from session and logical occurrence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct SourceId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourcePageRef {
    pub source_id: SourceId,
    pub source_page_index: u32,
}

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
    pub source_id: SourceId,
    pub source_index: u32,
    /// Persistent application rotation, in clockwise quarter turns.
    pub rotation: u16,
}

impl PagePlanEntry {
    pub fn source_ref(self) -> SourcePageRef {
        SourcePageRef {
            source_id: self.source_id,
            source_page_index: self.source_index,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PagePlan {
    entries: Vec<PagePlanEntry>,
    sources: HashMap<PageId, SourcePageRef>,
    positions: HashMap<PageId, usize>,
}

impl PagePlan {
    pub fn from_original_pages(page_count: u32) -> Self {
        let entries: Vec<_> = (0..page_count)
            .map(|source_index| PagePlanEntry {
                id: PageId::new(),
                source_id: SourceId(0),
                source_index,
                rotation: 0,
            })
            .collect();
        let sources = entries
            .iter()
            .map(|entry| (entry.id, entry.source_ref()))
            .collect();
        let positions = entries.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
        Self {
            entries,
            sources,
            positions,
        }
    }

    /// Resolves a stable identity without scanning the page plan per tile.
    pub fn source_index_of(&self, id: PageId) -> Option<u32> {
        self.source_ref_of(id).map(|r| r.source_page_index)
    }

    pub fn source_ref_of(&self, id: PageId) -> Option<SourcePageRef> {
        self.sources.get(&id).copied()
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
        self.positions.get(&id).copied()
    }

    pub(crate) fn replace(&mut self, entries: Vec<PagePlanEntry>) {
        debug_assert_eq!(
            entries
                .iter()
                .map(|e| e.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            entries.len()
        );
        debug_assert!(entries
            .iter()
            .all(|e| e.rotation < 360 && e.rotation.is_multiple_of(90)));
        self.sources = entries.iter().map(|e| (e.id, e.source_ref())).collect();
        self.positions = entries.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
        self.entries = entries;
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

/// Renderer-agnostic request. Scale is logical pixels per page point (1/72 in),
/// multiplied by DPR to obtain physical pixels per point. Grid coordinates are
/// signed physical-pixel tile indices, after additional clockwise rotation.
/// Page space is top-left, y-down, with source crop and intrinsic rotation applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRequest {
    pub page_id: PageId,
    pub tile_x: i32,
    pub tile_y: i32,
    pub scale: f64,
    pub device_pixel_ratio: f64,
    pub rotation_degrees: u16,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TileRequestError {
    InvalidScale,
    InvalidRotation,
    InvalidDimensions,
    InvalidGeometry,
    CoordinateRange,
}

/// Affine page-to-tile transform: x' = a*x + c*y + e, y' = b*x + d*y + f.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageToTile {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
    pub page_region: PageRect,
}

impl PageToTile {
    pub fn map(&self, point: PagePoint) -> DevicePoint {
        DevicePoint {
            x: self.a * point.x + self.c * point.y + self.e,
            y: self.b * point.x + self.d * point.y + self.f,
        }
    }
}

impl TileRequest {
    pub fn validate(&self) -> Result<(), TileRequestError> {
        let physical_scale = self.scale * self.device_pixel_ratio;
        if !self.scale.is_finite()
            || self.scale <= 0.0
            || !self.device_pixel_ratio.is_finite()
            || self.device_pixel_ratio <= 0.0
            || !physical_scale.is_finite()
            || physical_scale < f64::from(f32::MIN_POSITIVE).sqrt()
            || physical_scale > f64::from(f32::MAX).sqrt()
        {
            return Err(TileRequestError::InvalidScale);
        }
        if ![0, 90, 180, 270].contains(&self.rotation_degrees) {
            return Err(TileRequestError::InvalidRotation);
        }
        if self.width != TILE_SIZE || self.height != TILE_SIZE {
            return Err(TileRequestError::InvalidDimensions);
        }
        Ok(())
    }

    pub fn page_to_tile(&self, size: PageSize) -> Result<PageToTile, TileRequestError> {
        self.validate()?;
        if !size.width.is_finite()
            || !size.height.is_finite()
            || size.width <= 0.0
            || size.height <= 0.0
        {
            return Err(TileRequestError::InvalidGeometry);
        }
        let s = self.scale * self.device_pixel_ratio;
        let tx = f64::from(self.tile_x) * f64::from(self.width);
        let ty = f64::from(self.tile_y) * f64::from(self.height);
        // Bound float backend coordinates before any allocation. Beyond 2^24,
        // float32 can no longer distinguish adjacent physical pixels.
        if [s * size.width, s * size.height, tx.abs(), ty.abs()]
            .iter()
            .any(|v| !v.is_finite() || *v + f64::from(TILE_SIZE) > 16_777_216.0)
        {
            return Err(TileRequestError::CoordinateRange);
        }
        let (a, b, c, d, e, f) = match self.rotation_degrees {
            0 => (s, 0.0, 0.0, s, -tx, -ty),
            90 => (0.0, s, -s, 0.0, s * size.height - tx, -ty),
            180 => (-s, 0.0, 0.0, -s, s * size.width - tx, s * size.height - ty),
            270 => (0.0, -s, s, 0.0, -tx, s * size.width - ty),
            _ => unreachable!("validated rotation"),
        };
        if [e, f]
            .iter()
            .any(|v| v.abs() + f64::from(TILE_SIZE) > 16_777_216.0)
        {
            return Err(TileRequestError::CoordinateRange);
        }
        let inverse = |x: f64, y: f64| PagePoint {
            x: (a * (x - e) + b * (y - f)) / (s * s),
            y: (c * (x - e) + d * (y - f)) / (s * s),
        };
        let p = inverse(0.0, 0.0);
        let q = inverse(f64::from(self.width), f64::from(self.height));
        Ok(PageToTile {
            a,
            b,
            c,
            d,
            e,
            f,
            page_region: PageRect {
                origin: PagePoint {
                    x: p.x.min(q.x),
                    y: p.y.min(q.y),
                },
                size: PageSize {
                    width: (q.x - p.x).abs(),
                    height: (q.y - p.y).abs(),
                },
            },
        })
    }
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

    fn request() -> TileRequest {
        TileRequest {
            page_id: PageId(1),
            tile_x: 0,
            tile_y: 0,
            scale: 2.0,
            device_pixel_ratio: 1.5,
            rotation_degrees: 0,
            width: 512,
            height: 512,
        }
    }

    #[test]
    fn tile_regions_and_adjacent_boundaries() {
        let size = PageSize {
            width: 800.0,
            height: 700.0,
        };
        let first = request().page_to_tile(size).unwrap();
        let next = TileRequest {
            tile_x: 1,
            tile_y: 1,
            ..request()
        }
        .page_to_tile(size)
        .unwrap();
        assert_eq!(first.page_region.origin, PagePoint { x: 0.0, y: 0.0 });
        assert_eq!(first.page_region.size.width, 512.0 / 3.0);
        assert_eq!(
            first.page_region.origin.x + first.page_region.size.width,
            next.page_region.origin.x
        );
        assert_eq!(
            first.page_region.origin.y + first.page_region.size.height,
            next.page_region.origin.y
        );
        let boundary = PagePoint {
            x: 512.0 / 3.0,
            y: 512.0 / 3.0,
        };
        assert_eq!(first.map(boundary), DevicePoint { x: 512.0, y: 512.0 });
        assert_eq!(next.map(boundary), DevicePoint { x: 0.0, y: 0.0 });
    }

    #[test]
    fn edge_and_negative_tiles_preserve_unclipped_region() {
        let size = PageSize {
            width: 800.0,
            height: 700.0,
        };
        let edge = TileRequest {
            tile_x: 1,
            tile_y: 1,
            scale: 1.0,
            device_pixel_ratio: 1.0,
            ..request()
        }
        .page_to_tile(size)
        .unwrap();
        assert_eq!(edge.page_region.origin, PagePoint { x: 512.0, y: 512.0 });
        assert_eq!(
            edge.page_region.size,
            PageSize {
                width: 512.0,
                height: 512.0
            }
        );
        let negative = TileRequest {
            tile_x: -1,
            ..request()
        }
        .page_to_tile(size)
        .unwrap();
        assert_eq!(negative.page_region.origin.x, -512.0 / 3.0);
    }

    #[test]
    fn quarter_turns_map_corners_and_adjacent_tiles() {
        let size = PageSize {
            width: 800.0,
            height: 700.0,
        };
        for (rotation_degrees, expected) in [
            (0, DevicePoint { x: 0.0, y: 0.0 }),
            (90, DevicePoint { x: 2100.0, y: 0.0 }),
            (
                180,
                DevicePoint {
                    x: 2400.0,
                    y: 2100.0,
                },
            ),
            (270, DevicePoint { x: 0.0, y: 2400.0 }),
        ] {
            let r = TileRequest {
                rotation_degrees,
                ..request()
            };
            let t = r.page_to_tile(size).unwrap();
            assert_eq!(t.map(PagePoint { x: 0.0, y: 0.0 }), expected);
            let next = TileRequest { tile_x: 1, ..r }.page_to_tile(size).unwrap();
            let p = PagePoint { x: 200.0, y: 300.0 };
            assert_eq!(t.map(p).x - next.map(p).x, 512.0);
            assert_eq!(t.map(p).y, next.map(p).y);
            assert!((t.page_region.size.width - 512.0 / 3.0).abs() < 1e-10);
        }
    }

    #[test]
    fn rejects_invalid_or_unrepresentable_requests() {
        let size = PageSize {
            width: 800.0,
            height: 700.0,
        };
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MIN_POSITIVE] {
            assert_eq!(
                TileRequest { scale, ..request() }.validate(),
                Err(TileRequestError::InvalidScale)
            );
        }
        for device_pixel_ratio in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                TileRequest {
                    device_pixel_ratio,
                    ..request()
                }
                .validate(),
                Err(TileRequestError::InvalidScale)
            );
        }
        assert_eq!(
            TileRequest {
                rotation_degrees: 45,
                ..request()
            }
            .validate(),
            Err(TileRequestError::InvalidRotation)
        );
        assert_eq!(
            TileRequest {
                width: u32::MAX,
                ..request()
            }
            .validate(),
            Err(TileRequestError::InvalidDimensions)
        );
        assert_eq!(
            TileRequest {
                tile_x: i32::MAX,
                ..request()
            }
            .page_to_tile(size),
            Err(TileRequestError::CoordinateRange)
        );
        assert_eq!(
            TileRequest {
                scale: 100_000.0,
                device_pixel_ratio: 1.0,
                ..request()
            }
            .page_to_tile(size),
            Err(TileRequestError::CoordinateRange)
        );
        assert_eq!(
            request().page_to_tile(PageSize {
                width: 0.0,
                height: 1.0
            }),
            Err(TileRequestError::InvalidGeometry)
        );
    }

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
        plan.replace(plan.entries.clone());
        assert_eq!(plan.get(0).unwrap().id, original[2].id);
        assert_eq!(plan.position_of(original[0].id), Some(2));
        assert_eq!(plan.get(0).unwrap().source_index, 2);
        let mut thumbnails = thumbnails::ThumbnailNavigator::default();
        thumbnails
            .update(
                &plan,
                DocumentId(1),
                Default::default(),
                0.0,
                600.0,
                1.0,
                0,
                0,
            )
            .unwrap();
        assert_eq!(thumbnails.slots[0].key.page_id, original[2].id);
        assert_eq!(thumbnails.slots[0].index, 0);
        assert_eq!(
            thumbnails.navigation_index(original[2].id, thumbnails.slots[0].recycle),
            Some(0)
        );
        assert_eq!(plan.source_index_of(original[0].id), Some(0));
        assert_eq!(plan.source_index_of(PageId(0)), None);
        assert_ne!(original[0].id, original[1].id);
    }

    #[test]
    fn document_ids_are_distinct() {
        assert_ne!(DocumentModel::new(1).id, DocumentModel::new(1).id);
    }
}
