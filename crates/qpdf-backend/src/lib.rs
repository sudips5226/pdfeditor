//! Only this crate knows the private qpdf adapter ABI. Streaming file output;
//! no PDFium, UI, or live editor locks. Adapter uses pinned libqpdf 12.3.2.
use libloading::Library;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OutputPage {
    pub source: u32,
    pub index: u32,
    pub rotation: u16,
}
type Progress = unsafe extern "C" fn(*mut c_void, u32, u32);
type Write = unsafe extern "C" fn(
    *const *const c_char,
    usize,
    *const OutputPage,
    usize,
    *const c_char,
    Progress,
    *mut c_void,
    *mut c_char,
    usize,
) -> i32;
type Verify = unsafe extern "C" fn(*const c_char, usize, *mut c_char, usize) -> i32;
pub struct Backend {
    _library: Library,
    write: Write,
    verify: Verify,
}
impl Backend {
    pub fn load() -> Result<Self, String> {
        let path = std::env::var_os("PDFEDITOR_QPDF_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .unwrap_or_default()
                    .with_file_name("pdfeditor_qpdf.dll")
            });
        #[cfg(windows)]
        let library: Library =
            unsafe { libloading::os::windows::Library::load_with_flags(&path, 0x100 | 0x1000) }
                .map_err(|e| format!("Cannot load structural backend: {e}"))?
                .into();
        #[cfg(not(windows))]
        let library = unsafe { Library::new(&path) }.map_err(|e| e.to_string())?;
        unsafe {
            let version = library
                .get::<unsafe extern "C" fn() -> u32>(b"pdfeditor_qpdf_adapter_version\0")
                .map_err(|e| e.to_string())?;
            if version() != 1 {
                return Err("Unsupported structural adapter ABI".into());
            }
            let write = *library
                .get::<Write>(b"pdfeditor_qpdf_write\0")
                .map_err(|e| e.to_string())?;
            let verify = *library
                .get::<Verify>(b"pdfeditor_qpdf_verify\0")
                .map_err(|e| e.to_string())?;
            Ok(Self {
                _library: library,
                write,
                verify,
            })
        }
    }
    pub fn write(
        &self,
        sources: &[std::path::PathBuf],
        pages: &[OutputPage],
        temp: &Path,
        progress: &mut dyn FnMut(u32, u32),
    ) -> Result<(), String> {
        let paths = sources
            .iter()
            .map(|p| path_string(p))
            .collect::<Result<Vec<_>, _>>()?;
        let pointers: Vec<_> = paths.iter().map(|p| p.as_ptr()).collect();
        let temp = path_string(temp)?;
        let mut error = [0i8; 2048];
        unsafe extern "C" fn callback(context: *mut c_void, phase: u32, percent: u32) {
            // Synchronous, same native worker; callbacks cannot unwind through C++.
            let callback = unsafe { &mut *(context as *mut &mut dyn FnMut(u32, u32)) };
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(phase, percent)));
        }
        let mut callback_ref = progress;
        let code = unsafe {
            (self.write)(
                pointers.as_ptr(),
                pointers.len(),
                pages.as_ptr(),
                pages.len(),
                temp.as_ptr(),
                callback,
                &mut callback_ref as *mut _ as *mut c_void,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        result(code, &error)
    }
    pub fn verify(&self, temp: &Path, expected: usize) -> Result<(), String> {
        let temp = path_string(temp)?;
        let mut error = [0i8; 2048];
        let code =
            unsafe { (self.verify)(temp.as_ptr(), expected, error.as_mut_ptr(), error.len()) };
        result(code, &error)
    }
}
fn result(code: i32, error: &[i8]) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_string_lossy()
            .into_owned())
    }
}
fn path_string(path: &Path) -> Result<CString, String> {
    let path = path.to_str().ok_or("Path is not UTF-8")?;
    CString::new(path).map_err(|_| "Path contains NUL".into())
}
