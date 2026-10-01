//! ABI v10: bounded predictive preparation and nonblocking worker notification.
use super::viewport::{
    ffi_call, renderer, viewport_error, PdfeditorTileKey, PDFEDITOR_ERROR_VIEWPORT,
};
use super::*;
use std::ffi::c_void;
use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorPredictionSnapshot {
    pub velocity: f64,
    pub preparation_us: f64,
    pub lead_distance: f64,
    pub input_sequence: u64,
    pub viewport_requests: u64,
    pub keys_generated: u64,
    pub cpu_completed: u64,
    pub gpu_uploaded: u64,
    pub cpu_used: u64,
    pub gpu_used: u64,
    pub cpu_wasted: u64,
    pub gpu_wasted: u64,
    pub invalidated: u64,
    pub predicted_at: u64,
    pub predicted_cpu_at: u64,
    pub predicted_gpu_at: u64,
    pub requested_at: u64,
    pub direction: i32,
    pub depth: u32,
    pub tile_count: u32,
    pub cpu_ready: u32,
    pub gpu_ready: u32,
    pub entry_required: u32,
    pub entry_cpu: u32,
    pub entry_gpu: u32,
    pub entry_predictive_cpu: u32,
    pub entry_predictive_gpu: u32,
    pub mandatory_rendered: u32,
    pub mandatory_uploaded: u32,
}
/// Exact current predictive keys; never changes requested or displayed state.
/// # Safety
/// Out and capacity keys are writable separate storage. Capacity <=64.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_prediction_snapshot(
    handle: *mut PdfeditorDocument,
    out: *mut PdfeditorPredictionSnapshot,
    keys: *mut PdfeditorTileKey,
    capacity: u32,
) -> i32 {
    if out.is_null() || (capacity != 0 && keys.is_null()) {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    if capacity > 64 {
        return viewport::PDFEDITOR_ERROR_CAPACITY;
    }
    ffi_call(|| {
        with_document(handle, |d| {
            let r = renderer(d, Default::default())?;
            let (mut snapshot, predicted) = r.prediction(|p| {
                let m = p.metrics;
                (
                    PdfeditorPredictionSnapshot {
                        velocity: m.velocity,
                        preparation_us: m.preparation_us,
                        lead_distance: m.lead_distance,
                        input_sequence: m.input_sequence,
                        viewport_requests: m.viewport_requests,
                        keys_generated: m.keys_generated,
                        cpu_completed: m.cpu_completed,
                        gpu_uploaded: m.gpu_uploaded,
                        cpu_used: m.cpu_used,
                        gpu_used: m.gpu_used,
                        cpu_wasted: m.cpu_wasted,
                        gpu_wasted: m.gpu_wasted,
                        invalidated: m.invalidated,
                        predicted_at: m.predicted_at,
                        predicted_cpu_at: m.predicted_cpu_at,
                        predicted_gpu_at: m.predicted_gpu_at,
                        requested_at: m.requested_at,
                        direction: m.direction,
                        depth: m.depth,
                        tile_count: p.keys.len() as u32,
                        cpu_ready: 0,
                        gpu_ready: 0,
                        entry_required: m.entry_required,
                        entry_cpu: m.entry_cpu,
                        entry_gpu: m.entry_gpu,
                        entry_predictive_cpu: m.entry_predictive_cpu,
                        entry_predictive_gpu: m.entry_predictive_gpu,
                        mandatory_rendered: m.mandatory_rendered,
                        mandatory_uploaded: m.mandatory_uploaded,
                    },
                    p.keys.clone(),
                )
            });
            if predicted.len() > capacity as usize {
                return Err(viewport::PDFEDITOR_ERROR_CAPACITY);
            }
            let (cpu, gpu) = r.predictive_readiness();
            snapshot.cpu_ready = cpu;
            snapshot.gpu_ready = gpu;
            for (i, k) in predicted.into_iter().enumerate() {
                unsafe {
                    ptr::write(keys.add(i), k.into());
                }
            }
            unsafe {
                ptr::write(out, snapshot);
            }
            Ok(())
        })
    })
}
#[no_mangle]
pub extern "C" fn pdfeditor_document_navigation_input(
    handle: *mut PdfeditorDocument,
    direction: i32,
    y: f64,
    jump: u32,
) -> i32 {
    if jump > 1 {
        return PDFEDITOR_ERROR_VIEWPORT;
    }
    ffi_call(|| {
        with_document(handle, |d| {
            renderer(d, Default::default())?
                .navigation_input(direction, y, jump != 0)
                .map_err(viewport_error)
        })
    })
}
/// Developer comparison switch and native normal cache capacity. Reserve is never predictive capacity.
#[no_mangle]
pub extern "C" fn pdfeditor_document_prediction_configure(
    handle: *mut PdfeditorDocument,
    enabled: u32,
    bytes: u64,
    entries: u32,
) -> i32 {
    if enabled > 1 || bytes > 128 * 1024 * 1024 {
        return PDFEDITOR_ERROR_VIEWPORT;
    }
    ffi_call(|| {
        with_document(handle, |d| {
            renderer(d, Default::default())?
                .configure_prediction(enabled != 0, bytes as usize, entries as usize)
                .map_err(viewport_error)
        })
    })
}
/// Worker notification only: must return promptly without calling core APIs or waiting for UI.
/// # Safety
/// The callback/context must remain valid until document close has returned (worker joined).
/// Callback must be thread-safe, must not unwind, and may only post nonblocking UI work.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_set_ready_notify(
    handle: *mut PdfeditorDocument,
    callback: Option<extern "C" fn(*mut c_void)>,
    context: *mut c_void,
) -> i32 {
    let context = context as usize;
    ffi_call(|| {
        with_document(handle, |d| {
            renderer(d, Default::default())?.set_ready_callback(callback.map(|f| {
                Arc::new(move || f(context as *mut c_void)) as Arc<dyn Fn() + Send + Sync>
            }));
            Ok(())
        })
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_bounds_and_nulls() {
        assert_eq!(std::mem::size_of::<PdfeditorPredictionSnapshot>(), 184);
        assert_eq!(
            unsafe {
                pdfeditor_document_prediction_snapshot(
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    0,
                )
            },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        assert_eq!(
            pdfeditor_document_prediction_configure(ptr::null_mut(), 2, 1, 1),
            PDFEDITOR_ERROR_VIEWPORT
        );
    }
}
