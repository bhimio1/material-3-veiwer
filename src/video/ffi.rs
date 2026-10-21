//! The parts of libmpv's client and render APIs the viewer uses, loaded at
//! run time so the viewer still starts (and shows images) without mpv.

#![allow(non_camel_case_types)]

use libloading::Library;
use std::ffi::{c_char, c_double, c_int, c_void};

pub enum mpv_handle {}
pub enum mpv_render_context {}

pub const FORMAT_FLAG: c_int = 3;
pub const FORMAT_INT64: c_int = 4;
pub const FORMAT_DOUBLE: c_int = 5;

pub const EVENT_NONE: c_int = 0;
pub const EVENT_SHUTDOWN: c_int = 1;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;
pub const EVENT_VIDEO_RECONFIG: c_int = 17;

pub const RENDER_PARAM_INVALID: c_int = 0;
pub const RENDER_PARAM_API_TYPE: c_int = 1;
pub const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub const RENDER_PARAM_OPENGL_FBO: c_int = 3;
pub const RENDER_PARAM_FLIP_Y: c_int = 4;
pub const RENDER_PARAM_BLOCK_FOR_TARGET_TIME: c_int = 12;

#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

#[repr(C)]
pub struct mpv_render_param {
    pub kind: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct mpv_opengl_init_params {
    pub get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    pub get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
pub struct mpv_opengl_fbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

macro_rules! api {
    ($($name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// libmpv's functions, resolved once.
        pub struct Mpv {
            _lib: Library,
            $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Mpv {
            /// Open libmpv (libmpv.so.2, or .so.1 for older builds).
            pub fn load() -> Result<Self, String> {
                let mut last = String::new();
                for name in ["libmpv.so.2", "libmpv.so", "libmpv.so.1"] {
                    // SAFETY: loading a well-known system library; its
                    // initialisers have no preconditions.
                    match unsafe { Library::new(name) } {
                        Ok(lib) => {
                            // SAFETY: the signatures match mpv/client.h and
                            // mpv/render.h (client API 2.x).
                            unsafe {
                                return Ok(Self {
                                    $($name: *lib
                                        .get(concat!(stringify!($name), "\0").as_bytes())
                                        .map_err(|e| format!("libmpv is missing {}: {e}", stringify!($name)))?,)*
                                    _lib: lib,
                                });
                            }
                        }
                        Err(e) => last = e.to_string(),
                    }
                }
                Err(format!("libmpv isn't installed ({last})"))
            }
        }
    };
}

api! {
    mpv_create: fn() -> *mut mpv_handle;
    mpv_initialize: fn(*mut mpv_handle) -> c_int;
    mpv_terminate_destroy: fn(*mut mpv_handle);
    mpv_set_option_string: fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int;
    mpv_command: fn(*mut mpv_handle, *mut *const c_char) -> c_int;
    mpv_set_property: fn(*mut mpv_handle, *const c_char, c_int, *mut c_void) -> c_int;
    mpv_set_property_string: fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int;
    mpv_get_property: fn(*mut mpv_handle, *const c_char, c_int, *mut c_void) -> c_int;
    mpv_get_property_string: fn(*mut mpv_handle, *const c_char) -> *mut c_char;
    mpv_free: fn(*mut c_void);
    mpv_wait_event: fn(*mut mpv_handle, c_double) -> *mut mpv_event;
    mpv_set_wakeup_callback: fn(*mut mpv_handle, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void);
    mpv_error_string: fn(c_int) -> *const c_char;
    mpv_render_context_create: fn(*mut *mut mpv_render_context, *mut mpv_handle, *mut mpv_render_param) -> c_int;
    mpv_render_context_set_update_callback: fn(*mut mpv_render_context, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void);
    mpv_render_context_update: fn(*mut mpv_render_context) -> u64;
    mpv_render_context_render: fn(*mut mpv_render_context, *mut mpv_render_param) -> c_int;
    mpv_render_context_free: fn(*mut mpv_render_context);
}
