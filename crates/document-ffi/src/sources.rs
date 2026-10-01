//! Application-owned persistent sources. Backend handles remain Rust-private.
use super::*;
use document_core::{SourceId, SourcePageRef};
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub size: u64,
    pub file_id: Option<(u32, u64)>,
    pub modified: SystemTime,
    pub created: SystemTime,
}
impl Identity {
    pub fn read(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path)
            .map_err(|e| format!("Source unavailable: {}: {e}", path.display()))?;
        let m = file
            .metadata()
            .map_err(|e| format!("Source unavailable: {}: {e}", path.display()))?;
        if !m.is_file() {
            return Err("Source is not a regular file".into());
        }
        Ok(Self {
            size: m.len(),
            #[cfg(windows)]
            file_id: file_id(&file),
            #[cfg(not(windows))]
            file_id: None,
            modified: m.modified().map_err(|e| e.to_string())?,
            created: m.created().unwrap_or(SystemTime::UNIX_EPOCH),
        })
    }
}
#[derive(Clone)]
pub struct Source {
    pub id: SourceId,
    pub path: PathBuf,
    pub identity: Identity,
    pub backend: Arc<PdfiumDocument>,
    pub count: u32,
}
impl Source {
    pub fn validate(&self) -> Result<(), String> {
        if Identity::read(&self.path)? != self.identity {
            return Err(format!(
                "Source changed since registration: {}",
                self.path.display()
            ));
        }
        Ok(())
    }
}
#[derive(Default)]
pub struct SourceRegistry {
    pub records: RwLock<HashMap<SourceId, Source>>,
    // Active routing only. Workers copy the reference/backend before native calls.
    pub pages: RwLock<HashMap<PageId, SourcePageRef>>,
}
impl SourceRegistry {
    pub fn register(&self, path: &Path) -> Result<Source, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("Cannot open source: {e}"))?;
        let identity = Identity::read(&path)?;
        let mut records = self.records.write().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = records.values().find(|s| {
            s.path == path || (identity.file_id.is_some() && identity.file_id == s.identity.file_id)
        }) {
            s.validate()?;
            return Ok(s.clone());
        }
        let backend = Arc::new(
            PdfiumDocument::open(&LocalFileSource::new(&path)).map_err(|e| e.to_string())?,
        );
        if Identity::read(&path)? != identity {
            return Err("Source changed while opening".into());
        }
        let count = backend.page_count();
        if count == 0 {
            return Err("Source contains no pages".into());
        }
        let source = Source {
            id: SourceId(records.len() as u64),
            path,
            identity,
            backend,
            count,
        };
        records.insert(source.id, source.clone());
        Ok(source)
    }
    pub fn sync(&self, plan: &document_core::PagePlan) {
        *self.pages.write().unwrap_or_else(|p| p.into_inner()) = plan
            .entries()
            .iter()
            .map(|e| (e.id, e.source_ref()))
            .collect();
    }
    pub fn resolve(&self, id: PageId) -> Result<(Arc<PdfiumDocument>, u32), i32> {
        let r = *self
            .pages
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?;
        self.resolve_ref(r)
    }
    pub fn resolve_ref(&self, r: SourcePageRef) -> Result<(Arc<PdfiumDocument>, u32), i32> {
        let records = self.records.read().unwrap_or_else(|p| p.into_inner());
        let s = records
            .get(&r.source_id)
            .ok_or(PDFEDITOR_ERROR_INVALID_PAGE)?;
        if r.source_page_index >= s.count {
            return Err(PDFEDITOR_ERROR_INVALID_PAGE);
        }
        Ok((Arc::clone(&s.backend), r.source_page_index))
    }
}

#[cfg(windows)]
pub(super) fn file_id(file: &std::fs::File) -> Option<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct Info {
        attributes: u32,
        created: [u32; 2],
        accessed: [u32; 2],
        written: [u32; 2],
        volume: u32,
        size_hi: u32,
        size_lo: u32,
        links: u32,
        index_hi: u32,
        index_lo: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Info) -> i32;
    }
    let mut info = Info::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        None
    } else {
        Some((
            info.volume,
            (u64::from(info.index_hi) << 32) | u64::from(info.index_lo),
        ))
    }
}
