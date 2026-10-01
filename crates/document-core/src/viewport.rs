//! Physical device-space demand, independent of the renderer and UI.
use crate::{DevicePoint, DocumentId, PageId, PageSize, TileRequest, TILE_SIZE};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportState {
    pub page_id: PageId,
    /// Origin in rotated physical page pixels. Extent is the physical viewport.
    pub origin: DevicePoint,
    pub extent: DeviceSize,
    pub scale: f64,
    pub device_pixel_ratio: f64,
    pub rotation_degrees: u16,
    pub generation: u64,
}

/// Identity uses the normalized physical scale: different scale/DPR pairs
/// that produce the same transform share pixels. No generation is included.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TileKey {
    pub document_id: DocumentId,
    pub document_revision: u64,
    pub page_id: PageId,
    pub tile_x: i32,
    pub tile_y: i32,
    pub physical_scale_bits: u64,
    pub rotation_degrees: u16,
    pub width: u32,
    pub height: u32,
    /// P3 renders with the existing backend's fixed flags (zero).
    pub render_flags: u32,
}

impl TileKey {
    pub fn request(self) -> TileRequest {
        TileRequest {
            page_id: self.page_id,
            tile_x: self.tile_x,
            tile_y: self.tile_y,
            scale: f64::from_bits(self.physical_scale_bits),
            device_pixel_ratio: 1.0,
            rotation_degrees: self.rotation_degrees,
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Priority {
    Visible,
    Directional,
    PredictiveFar,
    Prefetch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TileDemand {
    pub key: TileKey,
    pub priority: Priority,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportError {
    InvalidInput,
    Capacity,
    StaleGeneration,
}

/// Row-major visible tiles followed by a single clipped prefetch ring.
/// The capacity limits both iteration and allocation, even on extreme inputs.
pub fn tile_demand(
    document_id: DocumentId,
    revision: u64,
    page_size: PageSize,
    viewport: ViewportState,
    capacity: usize,
) -> Result<Vec<TileDemand>, ViewportError> {
    let base = TileKey {
        document_id,
        document_revision: revision,
        page_id: viewport.page_id,
        tile_x: 0,
        tile_y: 0,
        physical_scale_bits: (viewport.scale * viewport.device_pixel_ratio).to_bits(),
        rotation_degrees: viewport.rotation_degrees,
        width: TILE_SIZE,
        height: TILE_SIZE,
        render_flags: 0,
    };
    let request = TileRequest {
        scale: viewport.scale,
        device_pixel_ratio: viewport.device_pixel_ratio,
        ..base.request()
    };
    request
        .page_to_tile(page_size)
        .map_err(|_| ViewportError::InvalidInput)?;
    if viewport.generation == 0
        || !viewport.origin.x.is_finite()
        || !viewport.origin.y.is_finite()
        || !viewport.extent.width.is_finite()
        || !viewport.extent.height.is_finite()
        || viewport.extent.width <= 0.0
        || viewport.extent.height <= 0.0
        || viewport.origin.x.abs() + viewport.extent.width > 16_777_216.0
        || viewport.origin.y.abs() + viewport.extent.height > 16_777_216.0
    {
        return Err(ViewportError::InvalidInput);
    }
    let s = viewport.scale * viewport.device_pixel_ratio;
    let page_extent = if viewport.rotation_degrees % 180 == 90 {
        DeviceSize {
            width: page_size.height * s,
            height: page_size.width * s,
        }
    } else {
        DeviceSize {
            width: page_size.width * s,
            height: page_size.height * s,
        }
    };
    let left = viewport.origin.x.max(0.0);
    let top = viewport.origin.y.max(0.0);
    let right = (viewport.origin.x + viewport.extent.width).min(page_extent.width);
    let bottom = (viewport.origin.y + viewport.extent.height).min(page_extent.height);
    if right <= left || bottom <= top {
        return Ok(Vec::new());
    }
    let edge = f64::from(TILE_SIZE);
    let x0 = (left / edge).floor() as i32;
    let y0 = (top / edge).floor() as i32;
    let x1 = (right / edge).ceil() as i32 - 1;
    let y1 = (bottom / edge).ceil() as i32 - 1;
    let rx0 = (x0 - 1).max(0);
    let ry0 = (y0 - 1).max(0);
    let rx1 = (x1 + 1).min((page_extent.width / edge).ceil() as i32 - 1);
    let ry1 = (y1 + 1).min((page_extent.height / edge).ceil() as i32 - 1);
    let count = (rx1 - rx0 + 1) as usize * (ry1 - ry0 + 1) as usize;
    if count > capacity {
        return Err(ViewportError::Capacity);
    }
    let mut demand = Vec::with_capacity(count);
    for priority in [Priority::Visible, Priority::Prefetch] {
        for y in ry0..=ry1 {
            for x in rx0..=rx1 {
                let visible = x >= x0 && x <= x1 && y >= y0 && y <= y1;
                if visible == (priority == Priority::Visible) {
                    demand.push(TileDemand {
                        key: TileKey {
                            tile_x: x,
                            tile_y: y,
                            ..base
                        },
                        priority,
                    });
                }
            }
        }
    }
    Ok(demand)
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn viewport() -> ViewportState {
        ViewportState {
            page_id: PageId(1),
            origin: DevicePoint { x: 512.0, y: 512.0 },
            extent: DeviceSize {
                width: 512.0,
                height: 512.0,
            },
            scale: 1.0,
            device_pixel_ratio: 1.0,
            rotation_degrees: 0,
            generation: 1,
        }
    }
    fn demand(v: ViewportState, capacity: usize) -> Result<Vec<TileDemand>, ViewportError> {
        tile_demand(
            DocumentId(1),
            0,
            PageSize {
                width: 2048.0,
                height: 1536.0,
            },
            v,
            capacity,
        )
    }
    #[test]
    fn visible_and_ring_are_deterministic_and_clipped() {
        let d = demand(viewport(), 256).unwrap();
        assert_eq!(d.len(), 9);
        assert_eq!(d[0].priority, Priority::Visible);
        assert_eq!(d[0].key.tile_x, 1);
        assert_eq!(d[0].key.tile_y, 1);
        assert!(d[1..].iter().all(|t| t.priority == Priority::Prefetch));
        assert_eq!(d, demand(viewport(), 256).unwrap());
        let d = demand(
            ViewportState {
                origin: DevicePoint { x: 0.0, y: 0.0 },
                ..viewport()
            },
            256,
        )
        .unwrap();
        assert_eq!(d.len(), 4);
    }
    #[test]
    fn normalized_identity_rotation_and_partial_edges() {
        let a = demand(viewport(), 256).unwrap()[0].key;
        let b = demand(
            ViewportState {
                scale: 0.5,
                device_pixel_ratio: 2.0,
                generation: 2,
                ..viewport()
            },
            256,
        )
        .unwrap()[0]
            .key;
        assert_eq!(a, b);
        assert_ne!(
            a,
            demand(
                ViewportState {
                    scale: 2.0,
                    ..viewport()
                },
                256
            )
            .unwrap()[0]
                .key
        );
        let d = demand(
            ViewportState {
                rotation_degrees: 90,
                origin: DevicePoint {
                    x: 1535.0,
                    y: 2047.0,
                },
                ..viewport()
            },
            256,
        )
        .unwrap();
        assert_eq!(d[0].key.tile_x, 2);
        assert_eq!(d[0].key.tile_y, 3);
    }
    #[test]
    fn invalid_and_excessive_inputs() {
        for v in [
            ViewportState {
                scale: f64::NAN,
                ..viewport()
            },
            ViewportState {
                extent: DeviceSize {
                    width: 0.0,
                    height: 1.0,
                },
                ..viewport()
            },
            ViewportState {
                rotation_degrees: 45,
                ..viewport()
            },
            ViewportState {
                origin: DevicePoint {
                    x: f64::INFINITY,
                    y: 0.0,
                },
                ..viewport()
            },
            ViewportState {
                generation: 0,
                ..viewport()
            },
        ] {
            assert_eq!(demand(v, 256), Err(ViewportError::InvalidInput));
        }
        assert_eq!(demand(viewport(), 1), Err(ViewportError::Capacity));
        assert!(demand(
            ViewportState {
                origin: DevicePoint { x: -2048.0, y: 0.0 },
                ..viewport()
            },
            256
        )
        .unwrap()
        .is_empty());
    }
}
