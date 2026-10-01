//! Rust-owned structural output jobs. One bounded worker per document; native
//! writing never holds the editor, operation gate, or render scheduler locks.
use super::*;
use document_core::{PagePlanEntry, SourceId};
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Instant;

pub const ERROR_OUTPUT: i32 = 16;
pub const ERROR_OUTPUT_BUSY: i32 = 17;
pub const ERROR_SOURCE: i32 = 18;
pub const ERROR_SOURCE_TARGET: i32 = 19;
pub const ERROR_OVERWRITE: i32 = 20;
pub const IDLE: u32 = 0;
pub const SNAPSHOTTING: u32 = 1;
pub const OPENING: u32 = 2;
pub const BUILDING: u32 = 3;
pub const WRITING: u32 = 4;
pub const VERIFYING: u32 = 5;
pub const FINALIZING: u32 = 6;
pub const SUCCEEDED: u32 = 7;
pub const FAILED: u32 = 8;
pub const CANCEL_REQUESTED: u32 = 9;
pub const CANCELLED: u32 = 10;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PdfeditorOutputStatus {
    pub document_id: u64,
    pub snapshot_fingerprint: u64,
    pub saved_fingerprint: u64,
    pub current_revision: u64,
    pub snapshot_revision: u64,
    pub snapshot_micros: u64,
    pub build_write_micros: u64,
    pub verification_micros: u64,
    pub elapsed_micros: u64,
    pub output_bytes: u64,
    pub coordination_bytes: usize,
    pub saved_revision: u64,
    pub phase: u32,
    pub kind: u32,
    pub percent: u32,
    pub page_count: u32,
    pub source_count: u32,
    pub registered_sources: u32,
    pub temp_exists: u32,
    pub error_code: i32,
    pub target_utf8: [u8; 1024],
    pub error_utf8: [u8; 2048],
}
impl Default for PdfeditorOutputStatus {
    fn default() -> Self {
        let mut value: Self = unsafe { std::mem::zeroed() };
        value.phase = IDLE;
        value
    }
}
fn copy_text<const N: usize>(out: &mut [u8; N], text: &str) {
    out.fill(0);
    let mut end = text.len().min(N - 1);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    out[..end].copy_from_slice(&text.as_bytes()[..end]);
}
#[derive(Default)]
struct JobState {
    status: PdfeditorOutputStatus,
    cancel: bool,
    finalized: bool,
}
#[derive(Default)]
pub(super) struct SaveCoordinator {
    shared: Arc<Mutex<JobState>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl SaveCoordinator {
    pub fn shutdown(&self) {
        self.cancel();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = worker.join();
        }
    }
    fn cancel(&self) {
        let mut s = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        if !s.finalized
            && ((SNAPSHOTTING..=FINALIZING).contains(&s.status.phase)
                || s.status.phase == CANCEL_REQUESTED)
        {
            s.cancel = true;
            s.status.phase = CANCEL_REQUESTED;
        }
    }
    fn start(
        &self,
        snapshot: ExportSnapshot,
        editor: Arc<Mutex<document_core::editing::Editor>>,
        registry: Arc<sources::SourceRegistry>,
        operation: Arc<Mutex<()>>,
        backend: Arc<dyn StructuralBackend>,
    ) -> Result<(), i32> {
        let state = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        if (SNAPSHOTTING..=FINALIZING).contains(&state.status.phase)
            || state.status.phase == CANCEL_REQUESTED
        {
            return Err(ERROR_OUTPUT_BUSY);
        }
        drop(state);
        let mut worker = self.worker.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(old) = worker.take() {
            let _ = old.join();
        }
        let mut state = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        state.cancel = false;
        state.finalized = false;
        state.status = PdfeditorOutputStatus {
            document_id: snapshot.document_id.0,
            snapshot_fingerprint: document_core::editing::plan_fingerprint(&snapshot.entries),
            snapshot_revision: snapshot.revision,
            snapshot_micros: snapshot.micros,
            coordination_bytes: snapshot.entries.capacity() * std::mem::size_of::<PagePlanEntry>()
                + snapshot.mapping.capacity() * std::mem::size_of::<qpdf_backend::OutputPage>()
                + snapshot.sources.capacity() * std::mem::size_of::<sources::Source>(),
            kind: snapshot.kind,
            phase: SNAPSHOTTING,
            page_count: snapshot.entries.len() as u32,
            source_count: snapshot.sources.len() as u32,
            ..Default::default()
        };
        copy_text(
            &mut state.status.target_utf8,
            &snapshot.target.to_string_lossy(),
        );
        drop(state);
        let shared = Arc::clone(&self.shared);
        let thread = std::thread::Builder::new()
            .name("pdf-structural-output".into())
            .spawn(move || {
                let start = Instant::now();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(
                        &snapshot,
                        backend.as_ref(),
                        &shared,
                        &editor,
                        &registry,
                        &operation,
                    )
                }));
                let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                s.status.elapsed_micros = start.elapsed().as_micros() as u64;
                match result {
                    Ok(Ok(())) => {
                        s.status.phase = SUCCEEDED;
                        s.status.percent = 100;
                    }
                    Ok(Err(message)) if s.cancel => {
                        s.status.phase = CANCELLED;
                        copy_text(&mut s.status.error_utf8, &message);
                    }
                    Ok(Err(message)) => {
                        s.status.phase = FAILED;
                        s.status.error_code = ERROR_OUTPUT;
                        copy_text(&mut s.status.error_utf8, &message);
                    }
                    Err(_) => {
                        s.status.phase = FAILED;
                        s.status.error_code = PDFEDITOR_ERROR_INTERNAL;
                        copy_text(&mut s.status.error_utf8, "Output worker panicked");
                    }
                }
            });
        match thread {
            Ok(t) => {
                *worker = Some(t);
                Ok(())
            }
            Err(_) => {
                self.shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .status
                    .phase = FAILED;
                Err(ERROR_OUTPUT)
            }
        }
    }
}
impl Drop for SaveCoordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ExportSnapshot {
    document_id: document_core::DocumentId,
    revision: u64,
    entries: Vec<PagePlanEntry>,
    sources: Vec<sources::Source>,
    mapping: Vec<qpdf_backend::OutputPage>,
    target: PathBuf,
    overwrite: bool,
    kind: u32, // Save As=1; Extract=2
    micros: u64,
}
trait StructuralBackend: Send + Sync {
    fn write(
        &self,
        snapshot: &ExportSnapshot,
        temp: &Path,
        progress: &mut dyn FnMut(u32, u32),
    ) -> Result<(), String>;
    fn verify(&self, temp: &Path, expected: usize) -> Result<(), String>;
}
struct Qpdf;
impl StructuralBackend for Qpdf {
    fn write(
        &self,
        s: &ExportSnapshot,
        temp: &Path,
        progress: &mut dyn FnMut(u32, u32),
    ) -> Result<(), String> {
        let paths: Vec<_> = s.sources.iter().map(|s| s.path.clone()).collect();
        qpdf_backend::Backend::load()?.write(&paths, &s.mapping, temp, progress)
    }
    fn verify(&self, temp: &Path, expected: usize) -> Result<(), String> {
        qpdf_backend::Backend::load()?.verify(temp, expected)?;
        let pdf = PdfiumDocument::open(&LocalFileSource::new(temp))
            .map_err(|e| format!("PDFium verification failed: {e}"))?;
        if pdf.page_count() as usize != expected {
            return Err("PDFium page-count mismatch".into());
        }
        for index in [0, expected as u32 / 2, expected as u32 - 1] {
            pdf.page_geometry(index)
                .map_err(|e| format!("PDFium geometry verification failed: {e}"))?;
        }
        Ok(())
    }
}
fn target_path(path: &Path) -> Result<PathBuf, i32> {
    if path.file_name().is_none() {
        return Err(ERROR_OUTPUT);
    }
    if path.exists() {
        return path.canonicalize().map_err(|_| ERROR_OUTPUT);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent
        .canonicalize()
        .map_err(|_| ERROR_OUTPUT)?
        .join(path.file_name().unwrap()))
}
fn validate_target(
    target: &Path,
    registry: &sources::SourceRegistry,
    overwrite: bool,
) -> Result<(), i32> {
    let canonical = target_path(target)?;
    let records = registry.records.read().unwrap_or_else(|p| p.into_inner());
    if records
        .values()
        .any(|s| s.path == canonical || same_file(&s.path, &canonical))
    {
        return Err(ERROR_SOURCE_TARGET);
    }
    if canonical.exists() && (!overwrite || !canonical.is_file()) {
        return Err(ERROR_OVERWRITE);
    }
    Ok(())
}
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        if let (Ok(a), Ok(b)) = (File::open(a), File::open(b)) {
            let id = sources::file_id(&a);
            return id.is_some() && id == sources::file_id(&b);
        }
    }
    false
}
fn capture(
    d: &OpenDocument,
    path: &Path,
    kind: u32,
    overwrite: bool,
) -> Result<ExportSnapshot, i32> {
    if ![1, 2].contains(&kind) {
        return Err(PDFEDITOR_ERROR_NULL_ARGUMENT);
    }
    let started = Instant::now();
    let target = target_path(path)?;
    validate_target(&target, &d.sources, overwrite)?;
    let editor = d.editor.lock().unwrap_or_else(|p| p.into_inner());
    let entries = if kind == 1 {
        editor.page_plan.entries().to_vec()
    } else {
        editor.selected_entries().map_err(editing::edit_error)?
    };
    let revision = editor.revision;
    drop(editor);
    let records = d.sources.records.read().unwrap_or_else(|p| p.into_inner());
    let mut source_indices = HashMap::<SourceId, u32>::new();
    let mut sources = Vec::new();
    let mut mapping = Vec::with_capacity(entries.len());
    for entry in &entries {
        let source = records.get(&entry.source_id).ok_or(ERROR_SOURCE)?;
        if entry.source_index >= source.count {
            return Err(PDFEDITOR_ERROR_INVALID_PAGE);
        }
        let index = *source_indices.entry(entry.source_id).or_insert_with(|| {
            sources.push(source.clone());
            (sources.len() - 1) as u32
        });
        mapping.push(qpdf_backend::OutputPage {
            source: index,
            index: entry.source_index,
            rotation: entry.rotation,
        });
    }
    Ok(ExportSnapshot {
        document_id: d.model.id,
        revision,
        entries,
        sources,
        mapping,
        target,
        overwrite,
        kind,
        micros: started.elapsed().as_micros() as u64,
    })
}
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
static NEXT_TEMP: AtomicUsize = AtomicUsize::new(1);
fn phase(shared: &Mutex<JobState>, value: u32) -> Result<(), String> {
    let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
    if state.cancel {
        return Err("Output cancelled at safe boundary".into());
    }
    state.status.phase = value;
    Ok(())
}
fn run(
    s: &ExportSnapshot,
    backend: &dyn StructuralBackend,
    shared: &Mutex<JobState>,
    editor: &Mutex<document_core::editing::Editor>,
    registry: &sources::SourceRegistry,
    operation: &Mutex<()>,
) -> Result<(), String> {
    phase(shared, OPENING)?;
    let mut guards = Vec::with_capacity(s.sources.len());
    for source in &s.sources {
        // Deny write/delete sharing on Windows for the entire output lifetime.
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let file = options
            .open(&source.path)
            .map_err(|e| format!("Cannot lock source {}: {e}", source.path.display()))?;
        source.validate()?;
        guards.push(file);
    }
    let temp = loop {
        let path = s.target.parent().unwrap().join(format!(
            ".pdfeditor-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                drop(file);
                break Temp(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Cannot create temporary output: {e}")),
        }
    };
    shared
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .status
        .temp_exists = 1;
    let result = (|| {
        phase(shared, BUILDING)?;
        let start = Instant::now();
        backend.write(s, &temp.0, &mut |p, percent| {
            let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
            if !state.cancel {
                state.status.phase = match p {
                    OPENING | BUILDING | WRITING => p,
                    _ => WRITING,
                };
            }
            state.status.percent = percent;
        })?;
        shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .status
            .build_write_micros = start.elapsed().as_micros() as u64;
        phase(shared, VERIFYING)?;
        let start = Instant::now();
        backend.verify(&temp.0, s.entries.len())?;
        shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .status
            .verification_micros = start.elapsed().as_micros() as u64;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temp.0)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| format!("Flush failed: {e}"))?;
        let bytes = file.metadata().map_err(|e| e.to_string())?.len();
        drop(file);
        phase(shared, FINALIZING)?;
        // Short gate covers active-source target revalidation + atomic rename +
        // saved baseline, preventing an Insert from registering our destination.
        let _gate = operation.lock().unwrap_or_else(|p| p.into_inner());
        validate_target(&s.target, registry, s.overwrite)
            .map_err(|e| format!("Target became unsafe before finalization (code {e})"))?;
        for source in &s.sources {
            source.validate()?;
        }
        let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
        if state.cancel {
            return Err("Output cancelled before finalization".into());
        }
        finalize(&temp.0, &s.target, s.overwrite)?;
        if s.kind == 1 {
            editor
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .mark_saved(s.entries.clone(), s.revision);
        }
        state.status.output_bytes = bytes;
        // Commit cancellation point: after atomic rename, job is completed.
        state.finalized = true;
        Ok(())
    })();
    let temp_path = temp.0.clone();
    drop(temp);
    shared
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .status
        .temp_exists = u32::from(temp_path.exists());
    result
}
#[cfg(windows)]
fn finalize(temp: &Path, target: &Path, overwrite: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 8 | u32::from(overwrite)) } == 0 {
        return Err(format!(
            "Atomic finalization failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
#[cfg(not(windows))]
fn finalize(temp: &Path, target: &Path, overwrite: bool) -> Result<(), String> {
    if overwrite {
        std::fs::rename(temp, target).map_err(|e| e.to_string())
    } else {
        std::fs::hard_link(temp, target).map_err(|e| e.to_string())
    }
}

/// Insert all pages when count=0; otherwise copy the explicit source indices.
/// # Safety
/// UTF-8 path and count readable indices must remain valid for this call; out writable.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_insert_source(
    h: *mut PdfeditorDocument,
    path: *const c_char,
    boundary: u32,
    indices: *const u32,
    count: u32,
    out: *mut editing::PdfeditorEditorStatus,
) -> i32 {
    if out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    if path.is_null() || (count != 0 && indices.is_null()) {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let path = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return PDFEDITOR_ERROR_INVALID_UTF8,
    };
    viewport::ffi_call(|| {
        with_document(h, |d| {
            let mut e = d.editor.lock().unwrap_or_else(|p| p.into_inner());
            if boundary as usize > e.page_plan.entries().len() {
                return Err(editing::PDFEDITOR_ERROR_EDIT_ARGUMENT);
            }
            let source = d
                .sources
                .register(Path::new(path))
                .map_err(|_| ERROR_SOURCE)?;
            let pages = if count == 0 {
                (0..source.count).collect::<Vec<_>>()
            } else {
                unsafe { std::slice::from_raw_parts(indices, count as usize) }.to_vec()
            };
            e.register_source(source.id, source.count);
            e.insert_pages(source.id, &pages, boundary as usize)
                .map_err(editing::edit_error)?;
            continuous::sync_plan(d, &e.page_plan);
            if let Some(Ok(r)) = d.renderer.get() {
                r.invalidate_placement();
            }
            d.thumbnails
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .navigator
                .slots
                .clear();
            unsafe {
                ptr::write(out, editing::status(&e));
            }
            Ok(())
        })
    })
}
/// Start Save As=1 or Extract=2. overwrite must explicitly be 0 or 1.
/// # Safety
/// Path must be a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_output_start(
    h: *mut PdfeditorDocument,
    path: *const c_char,
    kind: u32,
    overwrite: u32,
) -> i32 {
    if path.is_null() || overwrite > 1 {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    let path = match unsafe { CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return PDFEDITOR_ERROR_INVALID_UTF8,
    };
    viewport::ffi_call(|| {
        with_document(h, |d| {
            let snapshot = capture(d, Path::new(path), kind, overwrite == 1)?;
            d.output.start(
                snapshot,
                Arc::clone(&d.editor),
                Arc::clone(&d.sources),
                Arc::clone(&d.operation),
                Arc::new(Qpdf),
            )
        })
    })
}
/// # Safety
/// Output must point to writable status storage.
#[no_mangle]
pub unsafe extern "C" fn pdfeditor_document_output_status(
    h: *mut PdfeditorDocument,
    out: *mut PdfeditorOutputStatus,
) -> i32 {
    if out.is_null() {
        return PDFEDITOR_ERROR_NULL_ARGUMENT;
    }
    unsafe {
        ptr::write(out, Default::default());
    }
    viewport::ffi_call(|| {
        with_document(h, |d| {
            let mut status = d
                .output
                .shared
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .status;
            status.registered_sources = d
                .sources
                .records
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .len() as u32;
            let editor = d.editor.lock().unwrap_or_else(|p| p.into_inner());
            status.saved_revision = editor.saved_revision;
            status.saved_fingerprint = editor.saved_fingerprint();
            status.current_revision = editor.revision;
            unsafe {
                ptr::write(out, status);
            }
            Ok(())
        })
    })
}
#[no_mangle]
pub extern "C" fn pdfeditor_document_output_cancel(h: *mut PdfeditorDocument) -> i32 {
    viewport::ffi_call(|| {
        with_document(h, |d| {
            d.output.cancel();
            Ok(())
        })
    })
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
