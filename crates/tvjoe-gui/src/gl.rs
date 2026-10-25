//! OpenGL-Funktionszeiger für libmpv. GTK 4 nutzt unter Linux EGL (Wayland und X11), unter
//! Windows WGL (`opengl32.dll`).

use std::ffi::{CString, c_char, c_void};
use std::sync::OnceLock;

type GlGetIntegerv = unsafe extern "C" fn(u32, *mut i32);

const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

#[cfg(not(windows))]
fn lookup(name: &CString) -> *mut c_void {
    type EglGetProcAddress = unsafe extern "C" fn(*const c_char) -> *mut c_void;
    static EGL: OnceLock<Option<(libloading::Library, EglGetProcAddress)>> = OnceLock::new();
    let egl = EGL.get_or_init(|| unsafe {
        let lib = libloading::Library::new("libEGL.so.1").ok()?;
        let f: EglGetProcAddress = *lib.get::<EglGetProcAddress>(c"eglGetProcAddress").ok()?;
        Some((lib, f))
    });
    match egl {
        Some((_, f)) => unsafe { f(name.as_ptr()) },
        None => std::ptr::null_mut(),
    }
}

/// `wglGetProcAddress` liefert nur Erweiterungen und Funktionen ab OpenGL 1.2 (bei aktivem
/// Kontext); die Grundfunktionen (z. B. `glGetIntegerv`) stehen direkt in `opengl32.dll`.
/// Ungültige Rückgaben laut Microsoft: 0, 1, 2, 3 und -1.
#[cfg(windows)]
fn lookup(name: &CString) -> *mut c_void {
    type WglGetProcAddress = unsafe extern "system" fn(*const c_char) -> *mut c_void;
    static GL: OnceLock<Option<(libloading::Library, WglGetProcAddress)>> = OnceLock::new();
    let Some((lib, wgl)) = GL.get_or_init(|| unsafe {
        let lib = libloading::Library::new("opengl32.dll").ok()?;
        let f: WglGetProcAddress = *lib.get::<WglGetProcAddress>(c"wglGetProcAddress").ok()?;
        Some((lib, f))
    }) else {
        return std::ptr::null_mut();
    };
    let p = unsafe { wgl(name.as_ptr()) };
    if !matches!(p as isize, -1..=3) {
        return p;
    }
    unsafe { lib.get::<*mut c_void>(name.as_bytes_with_nul()) }
        .map(|s| *s)
        .unwrap_or(std::ptr::null_mut())
}

/// Für `OpenGLInitParams::get_proc_address`.
pub fn get_proc_address(_ctx: &(), name: &str) -> *mut c_void {
    match CString::new(name) {
        Ok(name) => lookup(&name),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Das gerade gebundene Framebuffer-Objekt (GtkGLArea rendert in ein eigenes FBO).
pub fn current_framebuffer() -> i32 {
    static GET: OnceLock<Option<GlGetIntegerv>> = OnceLock::new();
    let get = GET.get_or_init(|| {
        let p = get_proc_address(&(), "glGetIntegerv");
        (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, GlGetIntegerv>(p) })
    });
    let mut fbo = 0;
    if let Some(get) = get {
        unsafe { get(GL_FRAMEBUFFER_BINDING, &mut fbo) };
    }
    fbo
}
