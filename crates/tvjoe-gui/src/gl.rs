//! OpenGL-Funktionszeiger für libmpv. GTK 4 nutzt unter Linux EGL (Wayland und X11).

use std::ffi::{CString, c_char, c_void};
use std::sync::OnceLock;

type EglGetProcAddress = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type GlGetIntegerv = unsafe extern "C" fn(u32, *mut i32);

const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

fn egl_get_proc_address() -> Option<EglGetProcAddress> {
    static EGL: OnceLock<Option<(libloading::Library, EglGetProcAddress)>> = OnceLock::new();
    EGL.get_or_init(|| unsafe {
        let lib = libloading::Library::new("libEGL.so.1").ok()?;
        let f: EglGetProcAddress = *lib.get::<EglGetProcAddress>(c"eglGetProcAddress").ok()?;
        Some((lib, f))
    })
    .as_ref()
    .map(|(_, f)| *f)
}

/// Für `OpenGLInitParams::get_proc_address`.
pub fn get_proc_address(_ctx: &(), name: &str) -> *mut c_void {
    let (Some(f), Ok(name)) = (egl_get_proc_address(), CString::new(name)) else {
        return std::ptr::null_mut();
    };
    unsafe { f(name.as_ptr()) }
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
