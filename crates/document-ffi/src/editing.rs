//! ABI v7 editor commands; all indices are logical and all identity is PageId.
use super::viewport::ffi_call;
use super::*;
use document_core::editing::{EditError, Editor, SelectionMode};

pub const PDFEDITOR_ERROR_NO_SELECTION: i32 = 12;
pub const PDFEDITOR_ERROR_EDIT_ARGUMENT: i32 = 13;
pub const PDFEDITOR_ERROR_LAST_PAGE: i32 = 14;
pub const PDFEDITOR_ERROR_NO_HISTORY: i32 = 15;

pub(super) fn edit_error(e: EditError) -> i32 {
    match e {
        EditError::NoSelection => PDFEDITOR_ERROR_NO_SELECTION,
        EditError::StalePage => PDFEDITOR_ERROR_INVALID_PAGE,
        EditError::InvalidDestination => PDFEDITOR_ERROR_EDIT_ARGUMENT,
        EditError::LastPage => PDFEDITOR_ERROR_LAST_PAGE,
        EditError::InvalidRotation => PDFEDITOR_ERROR_INVALID_TILE_REQUEST,
        EditError::InvalidSource => PDFEDITOR_ERROR_EDIT_ARGUMENT,
        EditError::NoHistory => PDFEDITOR_ERROR_NO_HISTORY,
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PdfeditorEditorStatus {
    pub revision: u64,
    pub selection_revision: u64,
    pub current_page_id: u64,
    pub history_bytes: usize,
    pub page_count: u32,
    pub current_index: u32,
    pub selected_count: u32,
    pub undo_depth: u32,
    pub redo_depth: u32,
    pub structural_dirty: u32,
}
pub(super) fn status(e: &Editor) -> PdfeditorEditorStatus {
    PdfeditorEditorStatus {
        revision: e.revision,
        selection_revision: e.selection_revision,
        current_page_id: e.current().0,
        history_bytes: e.history_bytes(),
        page_count: e.page_plan.entries().len() as u32,
        current_index: e.current_index() as u32,
        selected_count: e.selected_count() as u32,
        undo_depth: e.undo_depth() as u32,
        redo_depth: e.redo_depth() as u32,
        structural_dirty: u32::from(e.dirty()),
    }
}
/// # Safety
/// Output must be writable status storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_editor_status(
    h: *mut PdfeditorDocument,
    out: *mut PdfeditorEditorStatus,
) -> i32 {
    if out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    ffi_call(|| {
        with_document(h, |d| {
            let e = d.editor.lock().unwrap_or_else(|p| p.into_inner());
            unsafe {
                ptr::write(out, status(&e));
            }
            Ok(())
        })
    })
}
/// Plain=0, Ctrl toggle=1, Shift range=2. Optional recycle token=0 for
/// non-card callers; a supplied token must match a currently assigned card.
#[no_mangle]
pub extern "C" fn pdfeditor_document_select_page(
    h: *mut PdfeditorDocument,
    id: u64,
    recycle: u64,
    mode: u32,
) -> i32 {
    ffi_call(|| {
        with_document(h, |d| {
            let mode = match mode {
                0 => SelectionMode::Plain,
                1 => SelectionMode::Toggle,
                2 => SelectionMode::Range,
                _ => return Err(PDFEDITOR_ERROR_EDIT_ARGUMENT),
            };
            if recycle != 0
                && d.thumbnails
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .navigator
                    .navigation_index(PageId(id), recycle)
                    .is_none()
            {
                return Err(PDFEDITOR_ERROR_INVALID_PAGE);
            }
            d.editor
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .select(PageId(id), mode)
                .map_err(edit_error)
        })
    })
}
/// Delete=1, move before original zero-based boundary=2 (count means end),
/// rotate by signed quarter-turn degrees=3, undo=4, redo=5, select all=6.
/// # Safety
/// Output must be writable status storage. Failures leave logical state intact.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_edit(
    h: *mut PdfeditorDocument,
    command: u32,
    argument: i32,
    out: *mut PdfeditorEditorStatus,
) -> i32 {
    if out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    ffi_call(|| {
        with_document(h, |d| {
            let mut e = d.editor.lock().unwrap_or_else(|p| p.into_inner());
            let changed = match command {
                1 => e.delete_selected(),
                2 => usize::try_from(argument)
                    .map_err(|_| EditError::InvalidDestination)
                    .and_then(|i| e.move_selected(i)),
                3 => e.rotate_selected(argument),
                4 => e.undo(),
                5 => e.redo(),
                7 => e.duplicate_selected(),
                6 => {
                    e.select_all();
                    Ok(false)
                }
                _ => return Err(PDFEDITOR_ERROR_EDIT_ARGUMENT),
            }
            .map_err(edit_error)?;
            if changed {
                super::continuous::sync_plan(d, &e.page_plan);
                if let Some(Ok(r)) = d.renderer.get() {
                    r.invalidate_placement();
                }
                // Invalidate tokens immediately; the next thumbnail update reassigns
                // only the bounded live range and can reuse pixel caches.
                d.thumbnails
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .navigator
                    .slots
                    .clear();
            }
            unsafe {
                ptr::write(out, status(&e));
            }
            Ok(())
        })
    })
}
#[no_mangle]
pub extern "C" fn pdfeditor_document_configure_history(
    h: *mut PdfeditorDocument,
    count: u32,
    bytes: usize,
) -> i32 {
    ffi_call(|| {
        with_document(h, |d| {
            if count == 0 || count > 1000 || bytes == 0 || bytes > 256 * 1024 * 1024 {
                return Err(PDFEDITOR_ERROR_EDIT_ARGUMENT);
            }
            d.editor
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .configure_history(count as usize, bytes);
            Ok(())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_layout_and_null_stale_outputs() {
        assert_eq!(std::mem::size_of::<PdfeditorEditorStatus>(), 56);
        let mut out = PdfeditorEditorStatus::default();
        assert_eq!(
            unsafe { pdfeditor_document_edit(ptr::null_mut(), 1, 0, &mut out) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        assert_eq!(
            unsafe {
                pdfeditor_document_editor_status(123usize as *mut PdfeditorDocument, &mut out)
            },
            PDFEDITOR_ERROR_INVALID_HANDLE
        );
        assert_eq!(out.revision, 0);
        assert_eq!(
            pdfeditor_document_select_page(ptr::null_mut(), 1, 0, 0),
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
        assert_eq!(
            unsafe { pdfeditor_document_edit(ptr::null_mut(), 1, 0, ptr::null_mut()) },
            PDFEDITOR_ERROR_NULL_ARGUMENT
        );
    }
}
