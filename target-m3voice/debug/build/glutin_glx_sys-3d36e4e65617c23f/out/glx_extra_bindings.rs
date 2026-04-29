
        mod __gl_imports {
            pub use std::mem;
            pub use std::marker::Send;
            pub use std::os::raw;
        }
    

        pub mod types {
            #![allow(non_camel_case_types, non_snake_case, dead_code, missing_copy_implementations)]
    
// Common types from OpenGL 1.1
pub type GLenum = super::__gl_imports::raw::c_uint;
pub type GLboolean = super::__gl_imports::raw::c_uchar;
pub type GLbitfield = super::__gl_imports::raw::c_uint;
pub type GLvoid = super::__gl_imports::raw::c_void;
pub type GLbyte = super::__gl_imports::raw::c_char;
pub type GLshort = super::__gl_imports::raw::c_short;
pub type GLint = super::__gl_imports::raw::c_int;
pub type GLclampx = super::__gl_imports::raw::c_int;
pub type GLubyte = super::__gl_imports::raw::c_uchar;
pub type GLushort = super::__gl_imports::raw::c_ushort;
pub type GLuint = super::__gl_imports::raw::c_uint;
pub type GLsizei = super::__gl_imports::raw::c_int;
pub type GLfloat = super::__gl_imports::raw::c_float;
pub type GLclampf = super::__gl_imports::raw::c_float;
pub type GLdouble = super::__gl_imports::raw::c_double;
pub type GLclampd = super::__gl_imports::raw::c_double;
pub type GLeglImageOES = *const super::__gl_imports::raw::c_void;
pub type GLchar = super::__gl_imports::raw::c_char;
pub type GLcharARB = super::__gl_imports::raw::c_char;

#[cfg(target_os = "macos")]
pub type GLhandleARB = *const super::__gl_imports::raw::c_void;
#[cfg(not(target_os = "macos"))]
pub type GLhandleARB = super::__gl_imports::raw::c_uint;

pub type GLhalfARB = super::__gl_imports::raw::c_ushort;
pub type GLhalf = super::__gl_imports::raw::c_ushort;

// Must be 32 bits
pub type GLfixed = GLint;

pub type GLintptr = isize;
pub type GLsizeiptr = isize;
pub type GLint64 = i64;
pub type GLuint64 = u64;
pub type GLintptrARB = isize;
pub type GLsizeiptrARB = isize;
pub type GLint64EXT = i64;
pub type GLuint64EXT = u64;

pub enum __GLsync {}
pub type GLsync = *const __GLsync;

// compatible with OpenCL cl_context
pub enum _cl_context {}
pub enum _cl_event {}

pub type GLDEBUGPROC = Option<extern "system" fn(source: GLenum,
                                                 gltype: GLenum,
                                                 id: GLuint,
                                                 severity: GLenum,
                                                 length: GLsizei,
                                                 message: *const GLchar,
                                                 userParam: *mut super::__gl_imports::raw::c_void)>;
pub type GLDEBUGPROCARB = Option<extern "system" fn(source: GLenum,
                                                    gltype: GLenum,
                                                    id: GLuint,
                                                    severity: GLenum,
                                                    length: GLsizei,
                                                    message: *const GLchar,
                                                    userParam: *mut super::__gl_imports::raw::c_void)>;
pub type GLDEBUGPROCKHR = Option<extern "system" fn(source: GLenum,
                                                    gltype: GLenum,
                                                    id: GLuint,
                                                    severity: GLenum,
                                                    length: GLsizei,
                                                    message: *const GLchar,
                                                    userParam: *mut super::__gl_imports::raw::c_void)>;

// GLES 1 types
// "pub type GLclampx = i32;",

// GLES 1/2 types (tagged for GLES 1)
// "pub type GLbyte = i8;",
// "pub type GLubyte = u8;",
// "pub type GLfloat = GLfloat;",
// "pub type GLclampf = GLfloat;",
// "pub type GLfixed = i32;",
// "pub type GLint64 = i64;",
// "pub type GLuint64 = u64;",
// "pub type GLintptr = intptr_t;",
// "pub type GLsizeiptr = ssize_t;",

// GLES 1/2 types (tagged for GLES 2 - attribute syntax is limited)
// "pub type GLbyte = i8;",
// "pub type GLubyte = u8;",
// "pub type GLfloat = GLfloat;",
// "pub type GLclampf = GLfloat;",
// "pub type GLfixed = i32;",
// "pub type GLint64 = i64;",
// "pub type GLuint64 = u64;",
// "pub type GLint64EXT = i64;",
// "pub type GLuint64EXT = u64;",
// "pub type GLintptr = intptr_t;",
// "pub type GLsizeiptr = ssize_t;",

// GLES 2 types (none currently)

// Vendor extension types
pub type GLDEBUGPROCAMD = Option<extern "system" fn(id: GLuint,
                                                    category: GLenum,
                                                    severity: GLenum,
                                                    length: GLsizei,
                                                    message: *const GLchar,
                                                    userParam: *mut super::__gl_imports::raw::c_void)>;
pub type GLhalfNV = super::__gl_imports::raw::c_ushort;
pub type GLvdpauSurfaceNV = GLintptr;

pub type XID = super::__gl_imports::raw::c_ulong;
pub type Bool = super::__gl_imports::raw::c_int; // Not sure if this is correct...
pub enum Display {}

pub type Font = XID;
pub type Pixmap = XID;
pub enum Visual {} // TODO: not sure
pub type VisualID = super::__gl_imports::raw::c_ulong; // TODO: not sure
pub type Window = XID;
pub type GLXFBConfigID = XID;
pub type GLXFBConfig = *const super::__gl_imports::raw::c_void;
pub type GLXContextID = XID;
pub type GLXContext = *const super::__gl_imports::raw::c_void;
pub type GLXPixmap = XID;
pub type GLXDrawable = XID;
pub type GLXWindow = XID;
pub type GLXPbuffer = XID;
pub enum __GLXextFuncPtr_fn {}
pub type __GLXextFuncPtr = *mut __GLXextFuncPtr_fn;
pub type GLXVideoCaptureDeviceNV = XID;
pub type GLXVideoDeviceNV = super::__gl_imports::raw::c_int;
pub type GLXVideoSourceSGIX = XID;
pub type GLXFBConfigIDSGIX = XID;
pub type GLXFBConfigSGIX = *const super::__gl_imports::raw::c_void;
pub type GLXPbufferSGIX = XID;

#[repr(C)]
pub struct XVisualInfo {
    pub visual: *mut Visual,
    pub visualid: VisualID,
    pub screen: super::__gl_imports::raw::c_int,
    pub depth: super::__gl_imports::raw::c_int,
    pub class: super::__gl_imports::raw::c_int,
    pub red_mask: super::__gl_imports::raw::c_ulong,
    pub green_mask: super::__gl_imports::raw::c_ulong,
    pub blue_mask: super::__gl_imports::raw::c_ulong,
    pub colormap_size: super::__gl_imports::raw::c_int,
    pub bits_per_rgb: super::__gl_imports::raw::c_int,
}

#[repr(C)]
pub struct GLXPbufferClobberEvent {
    pub event_type: super::__gl_imports::raw::c_int, // GLX_DAMAGED or GLX_SAVED
    pub draw_type: super::__gl_imports::raw::c_int, // GLX_WINDOW or GLX_PBUFFER
    pub serial: super::__gl_imports::raw::c_ulong, // # of last request processed by server
    pub send_event: Bool, // true if this came for SendEvent request
    pub display: *const Display, // display the event was read from
    pub drawable: GLXDrawable, // XID of Drawable
    pub buffer_mask: super::__gl_imports::raw::c_uint, // mask indicating which buffers are affected
    pub aux_buffer: super::__gl_imports::raw::c_uint, // which aux buffer was affected
    pub x: super::__gl_imports::raw::c_int,
    pub y: super::__gl_imports::raw::c_int,
    pub width: super::__gl_imports::raw::c_int,
    pub height: super::__gl_imports::raw::c_int,
    pub count: super::__gl_imports::raw::c_int, // if nonzero, at least this many more
}

#[repr(C)]
pub struct GLXBufferSwapComplete {
    pub type_: super::__gl_imports::raw::c_int,
    pub serial: super::__gl_imports::raw::c_ulong, // # of last request processed by server
    pub send_event: Bool, // true if this came from a SendEvent request
    pub display: *const Display, // Display the event was read from
    pub drawable: GLXDrawable, // drawable on which event was requested in event mask
    pub event_type: super::__gl_imports::raw::c_int,
    pub ust: i64,
    pub msc: i64,
    pub sbc: i64,
}

// typedef union __GLXEvent {
//     GLXPbufferClobberEvent glxpbufferclobber;
//     GLXBufferSwapComplete glxbufferswapcomplete;
//     long pad[24];
// }

#[repr(C)]
pub struct GLXBufferClobberEventSGIX {
    pub type_: super::__gl_imports::raw::c_int,
    pub serial: super::__gl_imports::raw::c_ulong, // # of last request processed by server
    pub send_event: Bool, // true if this came for SendEvent request
    pub display: *const Display, // display the event was read from
    pub drawable: GLXDrawable, // i.d. of Drawable
    pub event_type: super::__gl_imports::raw::c_int, // GLX_DAMAGED_SGIX or GLX_SAVED_SGIX
    pub draw_type: super::__gl_imports::raw::c_int, // GLX_WINDOW_SGIX or GLX_PBUFFER_SGIX
    pub mask: super::__gl_imports::raw::c_uint, // mask indicating which buffers are affected
    pub x: super::__gl_imports::raw::c_int,
    pub y: super::__gl_imports::raw::c_int,
    pub width: super::__gl_imports::raw::c_int,
    pub height: super::__gl_imports::raw::c_int,
    pub count: super::__gl_imports::raw::c_int, // if nonzero, at least this many more
}

#[repr(C)]
pub struct GLXHyperpipeNetworkSGIX {
    pub pipeName: [super::__gl_imports::raw::c_char; 80], // Should be [GLX_HYPERPIPE_PIPE_NAME_LENGTH_SGIX]
    pub networkId: super::__gl_imports::raw::c_int,
}

#[repr(C)]
pub struct GLXHyperpipeConfigSGIX {
    pub pipeName: [super::__gl_imports::raw::c_char; 80], // Should be [GLX_HYPERPIPE_PIPE_NAME_LENGTH_SGIX]
    pub channel: super::__gl_imports::raw::c_int,
    pub participationType: super::__gl_imports::raw::c_uint,
    pub timeSlice: super::__gl_imports::raw::c_int,
}

#[repr(C)]
pub struct GLXPipeRect {
    pub pipeName: [super::__gl_imports::raw::c_char; 80], // Should be [GLX_HYPERPIPE_PIPE_NAME_LENGTH_SGIX]
    pub srcXOrigin: super::__gl_imports::raw::c_int,
    pub srcYOrigin: super::__gl_imports::raw::c_int,
    pub srcWidth: super::__gl_imports::raw::c_int,
    pub srcHeight: super::__gl_imports::raw::c_int,
    pub destXOrigin: super::__gl_imports::raw::c_int,
    pub destYOrigin: super::__gl_imports::raw::c_int,
    pub destWidth: super::__gl_imports::raw::c_int,
    pub destHeight: super::__gl_imports::raw::c_int,
}

#[repr(C)]
pub struct GLXPipeRectLimits {
    pub pipeName: [super::__gl_imports::raw::c_char; 80], // Should be [GLX_HYPERPIPE_PIPE_NAME_LENGTH_SGIX]
    pub XOrigin: super::__gl_imports::raw::c_int,
    pub YOrigin: super::__gl_imports::raw::c_int,
    pub maxHeight: super::__gl_imports::raw::c_int,
    pub maxWidth: super::__gl_imports::raw::c_int,
}

}
#[allow(dead_code, non_upper_case_globals)] pub const ACCUM_ALPHA_SIZE: types::GLenum = 17;
#[allow(dead_code, non_upper_case_globals)] pub const ACCUM_BLUE_SIZE: types::GLenum = 16;
#[allow(dead_code, non_upper_case_globals)] pub const ACCUM_BUFFER_BIT: types::GLenum = 0x00000080;
#[allow(dead_code, non_upper_case_globals)] pub const ACCUM_GREEN_SIZE: types::GLenum = 15;
#[allow(dead_code, non_upper_case_globals)] pub const ACCUM_RED_SIZE: types::GLenum = 14;
#[allow(dead_code, non_upper_case_globals)] pub const ALPHA_SIZE: types::GLenum = 11;
#[allow(dead_code, non_upper_case_globals)] pub const AUX_BUFFERS: types::GLenum = 7;
#[allow(dead_code, non_upper_case_globals)] pub const AUX_BUFFERS_BIT: types::GLenum = 0x00000010;
#[allow(dead_code, non_upper_case_globals)] pub const BACK_BUFFER_AGE_EXT: types::GLenum = 0x20F4;
#[allow(dead_code, non_upper_case_globals)] pub const BACK_LEFT_BUFFER_BIT: types::GLenum = 0x00000004;
#[allow(dead_code, non_upper_case_globals)] pub const BACK_RIGHT_BUFFER_BIT: types::GLenum = 0x00000008;
#[allow(dead_code, non_upper_case_globals)] pub const BAD_ATTRIBUTE: types::GLenum = 2;
#[allow(dead_code, non_upper_case_globals)] pub const BAD_CONTEXT: types::GLenum = 5;
#[allow(dead_code, non_upper_case_globals)] pub const BAD_ENUM: types::GLenum = 7;
#[allow(dead_code, non_upper_case_globals)] pub const BAD_SCREEN: types::GLenum = 1;
#[allow(dead_code, non_upper_case_globals)] pub const BAD_VALUE: types::GLenum = 6;
#[allow(dead_code, non_upper_case_globals)] pub const BAD_VISUAL: types::GLenum = 4;
#[allow(dead_code, non_upper_case_globals)] pub const BLUE_SIZE: types::GLenum = 10;
#[allow(dead_code, non_upper_case_globals)] pub const BUFFER_SIZE: types::GLenum = 2;
#[allow(dead_code, non_upper_case_globals)] pub const BufferSwapComplete: types::GLenum = 1;
#[allow(dead_code, non_upper_case_globals)] pub const COLOR_INDEX_BIT: types::GLenum = 0x00000002;
#[allow(dead_code, non_upper_case_globals)] pub const COLOR_INDEX_TYPE: types::GLenum = 0x8015;
#[allow(dead_code, non_upper_case_globals)] pub const CONFIG_CAVEAT: types::GLenum = 0x20;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_COMPATIBILITY_PROFILE_BIT_ARB: types::GLenum = 0x00000002;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_CORE_PROFILE_BIT_ARB: types::GLenum = 0x00000001;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_DEBUG_BIT_ARB: types::GLenum = 0x00000001;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_ES2_PROFILE_BIT_EXT: types::GLenum = 0x00000004;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_FLAGS_ARB: types::GLenum = 0x2094;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_FORWARD_COMPATIBLE_BIT_ARB: types::GLenum = 0x00000002;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_MAJOR_VERSION_ARB: types::GLenum = 0x2091;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_MINOR_VERSION_ARB: types::GLenum = 0x2092;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_OPENGL_NO_ERROR_ARB: types::GLenum = 0x31B3;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_PROFILE_MASK_ARB: types::GLenum = 0x9126;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_RELEASE_BEHAVIOR_ARB: types::GLenum = 0x2097;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_RELEASE_BEHAVIOR_FLUSH_ARB: types::GLenum = 0x2098;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_RELEASE_BEHAVIOR_NONE_ARB: types::GLenum = 0;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_RESET_NOTIFICATION_STRATEGY_ARB: types::GLenum = 0x8256;
#[allow(dead_code, non_upper_case_globals)] pub const CONTEXT_ROBUST_ACCESS_BIT_ARB: types::GLenum = 0x00000004;
#[allow(dead_code, non_upper_case_globals)] pub const DAMAGED: types::GLenum = 0x8020;
#[allow(dead_code, non_upper_case_globals)] pub const DEPTH_BUFFER_BIT: types::GLenum = 0x00000020;
#[allow(dead_code, non_upper_case_globals)] pub const DEPTH_SIZE: types::GLenum = 12;
#[allow(dead_code, non_upper_case_globals)] pub const DIRECT_COLOR: types::GLenum = 0x8003;
#[allow(dead_code, non_upper_case_globals)] pub const DONT_CARE: types::GLenum = 0xFFFFFFFF;
#[allow(dead_code, non_upper_case_globals)] pub const DOUBLEBUFFER: types::GLenum = 5;
#[allow(dead_code, non_upper_case_globals)] pub const DRAWABLE_TYPE: types::GLenum = 0x8010;
#[allow(dead_code, non_upper_case_globals)] pub const EVENT_MASK: types::GLenum = 0x801F;
#[allow(dead_code, non_upper_case_globals)] pub const EXTENSIONS: types::GLenum = 0x3;
#[allow(dead_code, non_upper_case_globals)] pub const EXTENSION_NAME: &'static str = "GLX";
#[allow(dead_code, non_upper_case_globals)] pub const FBCONFIG_ID: types::GLenum = 0x8013;
#[allow(dead_code, non_upper_case_globals)] pub const FRAMEBUFFER_SRGB_CAPABLE_ARB: types::GLenum = 0x20B2;
#[allow(dead_code, non_upper_case_globals)] pub const FRAMEBUFFER_SRGB_CAPABLE_EXT: types::GLenum = 0x20B2;
#[allow(dead_code, non_upper_case_globals)] pub const FRONT_LEFT_BUFFER_BIT: types::GLenum = 0x00000001;
#[allow(dead_code, non_upper_case_globals)] pub const FRONT_RIGHT_BUFFER_BIT: types::GLenum = 0x00000002;
#[allow(dead_code, non_upper_case_globals)] pub const GRAY_SCALE: types::GLenum = 0x8006;
#[allow(dead_code, non_upper_case_globals)] pub const GREEN_SIZE: types::GLenum = 9;
#[allow(dead_code, non_upper_case_globals)] pub const HEIGHT: types::GLenum = 0x801E;
#[allow(dead_code, non_upper_case_globals)] pub const LARGEST_PBUFFER: types::GLenum = 0x801C;
#[allow(dead_code, non_upper_case_globals)] pub const LEVEL: types::GLenum = 3;
#[allow(dead_code, non_upper_case_globals)] pub const LOSE_CONTEXT_ON_RESET_ARB: types::GLenum = 0x8252;
#[allow(dead_code, non_upper_case_globals)] pub const MAX_PBUFFER_HEIGHT: types::GLenum = 0x8017;
#[allow(dead_code, non_upper_case_globals)] pub const MAX_PBUFFER_PIXELS: types::GLenum = 0x8018;
#[allow(dead_code, non_upper_case_globals)] pub const MAX_PBUFFER_WIDTH: types::GLenum = 0x8016;
#[allow(dead_code, non_upper_case_globals)] pub const MAX_SWAP_INTERVAL_EXT: types::GLenum = 0x20F2;
#[allow(dead_code, non_upper_case_globals)] pub const NONE: types::GLenum = 0x8000;
#[allow(dead_code, non_upper_case_globals)] pub const NON_CONFORMANT_CONFIG: types::GLenum = 0x800D;
#[allow(dead_code, non_upper_case_globals)] pub const NO_EXTENSION: types::GLenum = 3;
#[allow(dead_code, non_upper_case_globals)] pub const NO_RESET_NOTIFICATION_ARB: types::GLenum = 0x8261;
#[allow(dead_code, non_upper_case_globals)] pub const PBUFFER: types::GLenum = 0x8023;
#[allow(dead_code, non_upper_case_globals)] pub const PBUFFER_BIT: types::GLenum = 0x00000004;
#[allow(dead_code, non_upper_case_globals)] pub const PBUFFER_CLOBBER_MASK: types::GLenum = 0x08000000;
#[allow(dead_code, non_upper_case_globals)] pub const PBUFFER_HEIGHT: types::GLenum = 0x8040;
#[allow(dead_code, non_upper_case_globals)] pub const PBUFFER_WIDTH: types::GLenum = 0x8041;
#[allow(dead_code, non_upper_case_globals)] pub const PIXMAP_BIT: types::GLenum = 0x00000002;
#[allow(dead_code, non_upper_case_globals)] pub const PRESERVED_CONTENTS: types::GLenum = 0x801B;
#[allow(dead_code, non_upper_case_globals)] pub const PSEUDO_COLOR: types::GLenum = 0x8004;
#[allow(dead_code, non_upper_case_globals)] pub const PbufferClobber: types::GLenum = 0;
#[allow(dead_code, non_upper_case_globals)] pub const RED_SIZE: types::GLenum = 8;
#[allow(dead_code, non_upper_case_globals)] pub const RENDER_TYPE: types::GLenum = 0x8011;
#[allow(dead_code, non_upper_case_globals)] pub const RGBA: types::GLenum = 4;
#[allow(dead_code, non_upper_case_globals)] pub const RGBA_BIT: types::GLenum = 0x00000001;
#[allow(dead_code, non_upper_case_globals)] pub const RGBA_FLOAT_BIT_ARB: types::GLenum = 0x00000004;
#[allow(dead_code, non_upper_case_globals)] pub const RGBA_FLOAT_TYPE_ARB: types::GLenum = 0x20B9;
#[allow(dead_code, non_upper_case_globals)] pub const RGBA_TYPE: types::GLenum = 0x8014;
#[allow(dead_code, non_upper_case_globals)] pub const SAMPLES: types::GLenum = 100001;
#[allow(dead_code, non_upper_case_globals)] pub const SAMPLES_ARB: types::GLenum = 100001;
#[allow(dead_code, non_upper_case_globals)] pub const SAMPLE_BUFFERS: types::GLenum = 100000;
#[allow(dead_code, non_upper_case_globals)] pub const SAMPLE_BUFFERS_ARB: types::GLenum = 100000;
#[allow(dead_code, non_upper_case_globals)] pub const SAVED: types::GLenum = 0x8021;
#[allow(dead_code, non_upper_case_globals)] pub const SCREEN: types::GLenum = 0x800C;
#[allow(dead_code, non_upper_case_globals)] pub const SLOW_CONFIG: types::GLenum = 0x8001;
#[allow(dead_code, non_upper_case_globals)] pub const STATIC_COLOR: types::GLenum = 0x8005;
#[allow(dead_code, non_upper_case_globals)] pub const STATIC_GRAY: types::GLenum = 0x8007;
#[allow(dead_code, non_upper_case_globals)] pub const STENCIL_BUFFER_BIT: types::GLenum = 0x00000040;
#[allow(dead_code, non_upper_case_globals)] pub const STENCIL_SIZE: types::GLenum = 13;
#[allow(dead_code, non_upper_case_globals)] pub const STEREO: types::GLenum = 6;
#[allow(dead_code, non_upper_case_globals)] pub const SWAP_INTERVAL_EXT: types::GLenum = 0x20F1;
#[allow(dead_code, non_upper_case_globals)] pub const TRANSPARENT_ALPHA_VALUE: types::GLenum = 0x28;
#[allow(dead_code, non_upper_case_globals)] pub const TRANSPARENT_BLUE_VALUE: types::GLenum = 0x27;
#[allow(dead_code, non_upper_case_globals)] pub const TRANSPARENT_GREEN_VALUE: types::GLenum = 0x26;
#[allow(dead_code, non_upper_case_globals)] pub const TRANSPARENT_INDEX: types::GLenum = 0x8009;
#[allow(dead_code, non_upper_case_globals)] pub const TRANSPARENT_INDEX_VALUE: types::GLenum = 0x24;
#[allow(dead_code, non_upper_case_globals)] pub const TRANSPARENT_RED_VALUE: types::GLenum = 0x25;
#[allow(dead_code, non_upper_case_globals)] pub const TRANSPARENT_RGB: types::GLenum = 0x8008;
#[allow(dead_code, non_upper_case_globals)] pub const TRANSPARENT_TYPE: types::GLenum = 0x23;
#[allow(dead_code, non_upper_case_globals)] pub const TRUE_COLOR: types::GLenum = 0x8002;
#[allow(dead_code, non_upper_case_globals)] pub const USE_GL: types::GLenum = 1;
#[allow(dead_code, non_upper_case_globals)] pub const VENDOR: types::GLenum = 0x1;
#[allow(dead_code, non_upper_case_globals)] pub const VERSION: types::GLenum = 0x2;
#[allow(dead_code, non_upper_case_globals)] pub const VISUAL_ID: types::GLenum = 0x800B;
#[allow(dead_code, non_upper_case_globals)] pub const WIDTH: types::GLenum = 0x801D;
#[allow(dead_code, non_upper_case_globals)] pub const WINDOW: types::GLenum = 0x8022;
#[allow(dead_code, non_upper_case_globals)] pub const WINDOW_BIT: types::GLenum = 0x00000001;
#[allow(dead_code, non_upper_case_globals)] pub const X_RENDERABLE: types::GLenum = 0x8012;
#[allow(dead_code, non_upper_case_globals)] pub const X_VISUAL_TYPE: types::GLenum = 0x22;

        #[allow(dead_code, missing_copy_implementations)]
        #[derive(Clone)]
        pub struct FnPtr {
            /// The function pointer that will be used when calling the function.
            f: *const __gl_imports::raw::c_void,
            /// True if the pointer points to a real function, false if points to a `panic!` fn.
            is_loaded: bool,
        }

        impl FnPtr {
            /// Creates a `FnPtr` from a load attempt.
            fn new(ptr: *const __gl_imports::raw::c_void) -> FnPtr {
                if ptr.is_null() {
                    FnPtr {
                        f: missing_fn_panic as *const __gl_imports::raw::c_void,
                        is_loaded: false
                    }
                } else {
                    FnPtr { f: ptr, is_loaded: true }
                }
            }

            /// Returns `true` if the function has been successfully loaded.
            ///
            /// If it returns `false`, calling the corresponding function will fail.
            #[inline]
            #[allow(dead_code)]
            pub fn is_loaded(&self) -> bool {
                self.is_loaded
            }
        }
    
#[inline(never)]
        fn missing_fn_panic() -> ! {
            panic!("glx function was not loaded")
        }

        #[allow(non_camel_case_types, non_snake_case, dead_code)]
        #[derive(Clone)]
        pub struct Glx {
pub ChooseFBConfig: FnPtr,
pub ChooseVisual: FnPtr,
pub CopyContext: FnPtr,
pub CreateContext: FnPtr,
pub CreateContextAttribsARB: FnPtr,
pub CreateGLXPixmap: FnPtr,
pub CreateNewContext: FnPtr,
pub CreatePbuffer: FnPtr,
pub CreatePixmap: FnPtr,
pub CreateWindow: FnPtr,
pub DestroyContext: FnPtr,
pub DestroyGLXPixmap: FnPtr,
pub DestroyPbuffer: FnPtr,
pub DestroyPixmap: FnPtr,
pub DestroyWindow: FnPtr,
pub GetClientString: FnPtr,
pub GetConfig: FnPtr,
pub GetCurrentContext: FnPtr,
pub GetCurrentDisplay: FnPtr,
pub GetCurrentDrawable: FnPtr,
pub GetCurrentReadDrawable: FnPtr,
pub GetFBConfigAttrib: FnPtr,
pub GetFBConfigs: FnPtr,
pub GetProcAddress: FnPtr,
pub GetSelectedEvent: FnPtr,
pub GetSwapIntervalMESA: FnPtr,
pub GetVisualFromFBConfig: FnPtr,
pub IsDirect: FnPtr,
pub MakeContextCurrent: FnPtr,
pub MakeCurrent: FnPtr,
pub QueryContext: FnPtr,
pub QueryDrawable: FnPtr,
pub QueryExtension: FnPtr,
pub QueryExtensionsString: FnPtr,
pub QueryServerString: FnPtr,
pub QueryVersion: FnPtr,
pub SelectEvent: FnPtr,
pub SwapBuffers: FnPtr,
pub SwapIntervalEXT: FnPtr,
pub SwapIntervalMESA: FnPtr,
pub SwapIntervalSGI: FnPtr,
pub UseXFont: FnPtr,
pub WaitGL: FnPtr,
pub WaitX: FnPtr,
_priv: ()
}
impl Glx {
            /// Load each OpenGL symbol using a custom load function. This allows for the
            /// use of functions like `glfwGetProcAddress` or `SDL_GL_GetProcAddress`.
            ///
            /// ~~~ignore
            /// let gl = Gl::load_with(|s| glfw.get_proc_address(s));
            /// ~~~
            #[allow(dead_code, unused_variables)]
            pub fn load_with<F>(mut loadfn: F) -> Glx where F: FnMut(&'static str) -> *const __gl_imports::raw::c_void {
                #[inline(never)]
                fn do_metaloadfn(loadfn: &mut dyn FnMut(&'static str) -> *const __gl_imports::raw::c_void,
                                 symbol: &'static str,
                                 symbols: &[&'static str])
                                 -> *const __gl_imports::raw::c_void {
                    let mut ptr = loadfn(symbol);
                    if ptr.is_null() {
                        for &sym in symbols {
                            ptr = loadfn(sym);
                            if !ptr.is_null() { break; }
                        }
                    }
                    ptr
                }
                let mut metaloadfn = |symbol: &'static str, symbols: &[&'static str]| {
                    do_metaloadfn(&mut loadfn, symbol, symbols)
                };
                Glx {
ChooseFBConfig: FnPtr::new(metaloadfn("glXChooseFBConfig", &[])),
ChooseVisual: FnPtr::new(metaloadfn("glXChooseVisual", &[])),
CopyContext: FnPtr::new(metaloadfn("glXCopyContext", &[])),
CreateContext: FnPtr::new(metaloadfn("glXCreateContext", &[])),
CreateContextAttribsARB: FnPtr::new(metaloadfn("glXCreateContextAttribsARB", &[])),
CreateGLXPixmap: FnPtr::new(metaloadfn("glXCreateGLXPixmap", &[])),
CreateNewContext: FnPtr::new(metaloadfn("glXCreateNewContext", &[])),
CreatePbuffer: FnPtr::new(metaloadfn("glXCreatePbuffer", &[])),
CreatePixmap: FnPtr::new(metaloadfn("glXCreatePixmap", &[])),
CreateWindow: FnPtr::new(metaloadfn("glXCreateWindow", &[])),
DestroyContext: FnPtr::new(metaloadfn("glXDestroyContext", &[])),
DestroyGLXPixmap: FnPtr::new(metaloadfn("glXDestroyGLXPixmap", &[])),
DestroyPbuffer: FnPtr::new(metaloadfn("glXDestroyPbuffer", &[])),
DestroyPixmap: FnPtr::new(metaloadfn("glXDestroyPixmap", &[])),
DestroyWindow: FnPtr::new(metaloadfn("glXDestroyWindow", &[])),
GetClientString: FnPtr::new(metaloadfn("glXGetClientString", &[])),
GetConfig: FnPtr::new(metaloadfn("glXGetConfig", &[])),
GetCurrentContext: FnPtr::new(metaloadfn("glXGetCurrentContext", &[])),
GetCurrentDisplay: FnPtr::new(metaloadfn("glXGetCurrentDisplay", &[])),
GetCurrentDrawable: FnPtr::new(metaloadfn("glXGetCurrentDrawable", &[])),
GetCurrentReadDrawable: FnPtr::new(metaloadfn("glXGetCurrentReadDrawable", &[])),
GetFBConfigAttrib: FnPtr::new(metaloadfn("glXGetFBConfigAttrib", &[])),
GetFBConfigs: FnPtr::new(metaloadfn("glXGetFBConfigs", &[])),
GetProcAddress: FnPtr::new(metaloadfn("glXGetProcAddress", &[])),
GetSelectedEvent: FnPtr::new(metaloadfn("glXGetSelectedEvent", &[])),
GetSwapIntervalMESA: FnPtr::new(metaloadfn("glXGetSwapIntervalMESA", &[])),
GetVisualFromFBConfig: FnPtr::new(metaloadfn("glXGetVisualFromFBConfig", &[])),
IsDirect: FnPtr::new(metaloadfn("glXIsDirect", &[])),
MakeContextCurrent: FnPtr::new(metaloadfn("glXMakeContextCurrent", &[])),
MakeCurrent: FnPtr::new(metaloadfn("glXMakeCurrent", &[])),
QueryContext: FnPtr::new(metaloadfn("glXQueryContext", &[])),
QueryDrawable: FnPtr::new(metaloadfn("glXQueryDrawable", &[])),
QueryExtension: FnPtr::new(metaloadfn("glXQueryExtension", &[])),
QueryExtensionsString: FnPtr::new(metaloadfn("glXQueryExtensionsString", &[])),
QueryServerString: FnPtr::new(metaloadfn("glXQueryServerString", &[])),
QueryVersion: FnPtr::new(metaloadfn("glXQueryVersion", &[])),
SelectEvent: FnPtr::new(metaloadfn("glXSelectEvent", &[])),
SwapBuffers: FnPtr::new(metaloadfn("glXSwapBuffers", &[])),
SwapIntervalEXT: FnPtr::new(metaloadfn("glXSwapIntervalEXT", &[])),
SwapIntervalMESA: FnPtr::new(metaloadfn("glXSwapIntervalMESA", &[])),
SwapIntervalSGI: FnPtr::new(metaloadfn("glXSwapIntervalSGI", &[])),
UseXFont: FnPtr::new(metaloadfn("glXUseXFont", &[])),
WaitGL: FnPtr::new(metaloadfn("glXWaitGL", &[])),
WaitX: FnPtr::new(metaloadfn("glXWaitX", &[])),
_priv: ()
}
        }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn ChooseFBConfig(&self, dpy: *mut types::Display, screen: __gl_imports::raw::c_int, attrib_list: *const __gl_imports::raw::c_int, nelements: *mut __gl_imports::raw::c_int) -> *mut types::GLXFBConfig { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, __gl_imports::raw::c_int, *const __gl_imports::raw::c_int, *mut __gl_imports::raw::c_int) -> *mut types::GLXFBConfig>(self.ChooseFBConfig.f)(dpy, screen, attrib_list, nelements) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn ChooseVisual(&self, dpy: *mut types::Display, screen: __gl_imports::raw::c_int, attribList: *mut __gl_imports::raw::c_int) -> *mut types::XVisualInfo { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, __gl_imports::raw::c_int, *mut __gl_imports::raw::c_int) -> *mut types::XVisualInfo>(self.ChooseVisual.f)(dpy, screen, attribList) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn CopyContext(&self, dpy: *mut types::Display, src: types::GLXContext, dst: types::GLXContext, mask: __gl_imports::raw::c_ulong) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXContext, types::GLXContext, __gl_imports::raw::c_ulong) -> ()>(self.CopyContext.f)(dpy, src, dst, mask) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn CreateContext(&self, dpy: *mut types::Display, vis: *mut types::XVisualInfo, shareList: types::GLXContext, direct: types::Bool) -> types::GLXContext { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, *mut types::XVisualInfo, types::GLXContext, types::Bool) -> types::GLXContext>(self.CreateContext.f)(dpy, vis, shareList, direct) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn CreateContextAttribsARB(&self, dpy: *mut types::Display, config: types::GLXFBConfig, share_context: types::GLXContext, direct: types::Bool, attrib_list: *const __gl_imports::raw::c_int) -> types::GLXContext { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXFBConfig, types::GLXContext, types::Bool, *const __gl_imports::raw::c_int) -> types::GLXContext>(self.CreateContextAttribsARB.f)(dpy, config, share_context, direct, attrib_list) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn CreateGLXPixmap(&self, dpy: *mut types::Display, visual: *mut types::XVisualInfo, pixmap: types::Pixmap) -> types::GLXPixmap { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, *mut types::XVisualInfo, types::Pixmap) -> types::GLXPixmap>(self.CreateGLXPixmap.f)(dpy, visual, pixmap) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn CreateNewContext(&self, dpy: *mut types::Display, config: types::GLXFBConfig, render_type: __gl_imports::raw::c_int, share_list: types::GLXContext, direct: types::Bool) -> types::GLXContext { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXFBConfig, __gl_imports::raw::c_int, types::GLXContext, types::Bool) -> types::GLXContext>(self.CreateNewContext.f)(dpy, config, render_type, share_list, direct) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn CreatePbuffer(&self, dpy: *mut types::Display, config: types::GLXFBConfig, attrib_list: *const __gl_imports::raw::c_int) -> types::GLXPbuffer { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXFBConfig, *const __gl_imports::raw::c_int) -> types::GLXPbuffer>(self.CreatePbuffer.f)(dpy, config, attrib_list) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn CreatePixmap(&self, dpy: *mut types::Display, config: types::GLXFBConfig, pixmap: types::Pixmap, attrib_list: *const __gl_imports::raw::c_int) -> types::GLXPixmap { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXFBConfig, types::Pixmap, *const __gl_imports::raw::c_int) -> types::GLXPixmap>(self.CreatePixmap.f)(dpy, config, pixmap, attrib_list) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn CreateWindow(&self, dpy: *mut types::Display, config: types::GLXFBConfig, win: types::Window, attrib_list: *const __gl_imports::raw::c_int) -> types::GLXWindow { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXFBConfig, types::Window, *const __gl_imports::raw::c_int) -> types::GLXWindow>(self.CreateWindow.f)(dpy, config, win, attrib_list) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn DestroyContext(&self, dpy: *mut types::Display, ctx: types::GLXContext) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXContext) -> ()>(self.DestroyContext.f)(dpy, ctx) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn DestroyGLXPixmap(&self, dpy: *mut types::Display, pixmap: types::GLXPixmap) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXPixmap) -> ()>(self.DestroyGLXPixmap.f)(dpy, pixmap) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn DestroyPbuffer(&self, dpy: *mut types::Display, pbuf: types::GLXPbuffer) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXPbuffer) -> ()>(self.DestroyPbuffer.f)(dpy, pbuf) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn DestroyPixmap(&self, dpy: *mut types::Display, pixmap: types::GLXPixmap) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXPixmap) -> ()>(self.DestroyPixmap.f)(dpy, pixmap) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn DestroyWindow(&self, dpy: *mut types::Display, win: types::GLXWindow) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXWindow) -> ()>(self.DestroyWindow.f)(dpy, win) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetClientString(&self, dpy: *mut types::Display, name: __gl_imports::raw::c_int) -> *const __gl_imports::raw::c_char { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, __gl_imports::raw::c_int) -> *const __gl_imports::raw::c_char>(self.GetClientString.f)(dpy, name) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetConfig(&self, dpy: *mut types::Display, visual: *mut types::XVisualInfo, attrib: __gl_imports::raw::c_int, value: *mut __gl_imports::raw::c_int) -> __gl_imports::raw::c_int { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, *mut types::XVisualInfo, __gl_imports::raw::c_int, *mut __gl_imports::raw::c_int) -> __gl_imports::raw::c_int>(self.GetConfig.f)(dpy, visual, attrib, value) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetCurrentContext(&self, ) -> types::GLXContext { __gl_imports::mem::transmute::<_, extern "system" fn() -> types::GLXContext>(self.GetCurrentContext.f)() }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetCurrentDisplay(&self, ) -> *mut types::Display { __gl_imports::mem::transmute::<_, extern "system" fn() -> *mut types::Display>(self.GetCurrentDisplay.f)() }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetCurrentDrawable(&self, ) -> types::GLXDrawable { __gl_imports::mem::transmute::<_, extern "system" fn() -> types::GLXDrawable>(self.GetCurrentDrawable.f)() }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetCurrentReadDrawable(&self, ) -> types::GLXDrawable { __gl_imports::mem::transmute::<_, extern "system" fn() -> types::GLXDrawable>(self.GetCurrentReadDrawable.f)() }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetFBConfigAttrib(&self, dpy: *mut types::Display, config: types::GLXFBConfig, attribute: __gl_imports::raw::c_int, value: *mut __gl_imports::raw::c_int) -> __gl_imports::raw::c_int { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXFBConfig, __gl_imports::raw::c_int, *mut __gl_imports::raw::c_int) -> __gl_imports::raw::c_int>(self.GetFBConfigAttrib.f)(dpy, config, attribute, value) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetFBConfigs(&self, dpy: *mut types::Display, screen: __gl_imports::raw::c_int, nelements: *mut __gl_imports::raw::c_int) -> *mut types::GLXFBConfig { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, __gl_imports::raw::c_int, *mut __gl_imports::raw::c_int) -> *mut types::GLXFBConfig>(self.GetFBConfigs.f)(dpy, screen, nelements) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetProcAddress(&self, procName: *const types::GLubyte) -> types::__GLXextFuncPtr { __gl_imports::mem::transmute::<_, extern "system" fn(*const types::GLubyte) -> types::__GLXextFuncPtr>(self.GetProcAddress.f)(procName) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetSelectedEvent(&self, dpy: *mut types::Display, draw: types::GLXDrawable, event_mask: *mut __gl_imports::raw::c_ulong) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXDrawable, *mut __gl_imports::raw::c_ulong) -> ()>(self.GetSelectedEvent.f)(dpy, draw, event_mask) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetSwapIntervalMESA(&self, ) -> __gl_imports::raw::c_int { __gl_imports::mem::transmute::<_, extern "system" fn() -> __gl_imports::raw::c_int>(self.GetSwapIntervalMESA.f)() }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn GetVisualFromFBConfig(&self, dpy: *mut types::Display, config: types::GLXFBConfig) -> *mut types::XVisualInfo { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXFBConfig) -> *mut types::XVisualInfo>(self.GetVisualFromFBConfig.f)(dpy, config) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn IsDirect(&self, dpy: *mut types::Display, ctx: types::GLXContext) -> types::Bool { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXContext) -> types::Bool>(self.IsDirect.f)(dpy, ctx) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn MakeContextCurrent(&self, dpy: *mut types::Display, draw: types::GLXDrawable, read: types::GLXDrawable, ctx: types::GLXContext) -> types::Bool { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXDrawable, types::GLXDrawable, types::GLXContext) -> types::Bool>(self.MakeContextCurrent.f)(dpy, draw, read, ctx) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn MakeCurrent(&self, dpy: *mut types::Display, drawable: types::GLXDrawable, ctx: types::GLXContext) -> types::Bool { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXDrawable, types::GLXContext) -> types::Bool>(self.MakeCurrent.f)(dpy, drawable, ctx) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn QueryContext(&self, dpy: *mut types::Display, ctx: types::GLXContext, attribute: __gl_imports::raw::c_int, value: *mut __gl_imports::raw::c_int) -> __gl_imports::raw::c_int { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXContext, __gl_imports::raw::c_int, *mut __gl_imports::raw::c_int) -> __gl_imports::raw::c_int>(self.QueryContext.f)(dpy, ctx, attribute, value) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn QueryDrawable(&self, dpy: *mut types::Display, draw: types::GLXDrawable, attribute: __gl_imports::raw::c_int, value: *mut __gl_imports::raw::c_uint) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXDrawable, __gl_imports::raw::c_int, *mut __gl_imports::raw::c_uint) -> ()>(self.QueryDrawable.f)(dpy, draw, attribute, value) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn QueryExtension(&self, dpy: *mut types::Display, errorb: *mut __gl_imports::raw::c_int, event: *mut __gl_imports::raw::c_int) -> types::Bool { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, *mut __gl_imports::raw::c_int, *mut __gl_imports::raw::c_int) -> types::Bool>(self.QueryExtension.f)(dpy, errorb, event) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn QueryExtensionsString(&self, dpy: *mut types::Display, screen: __gl_imports::raw::c_int) -> *const __gl_imports::raw::c_char { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, __gl_imports::raw::c_int) -> *const __gl_imports::raw::c_char>(self.QueryExtensionsString.f)(dpy, screen) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn QueryServerString(&self, dpy: *mut types::Display, screen: __gl_imports::raw::c_int, name: __gl_imports::raw::c_int) -> *const __gl_imports::raw::c_char { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, __gl_imports::raw::c_int, __gl_imports::raw::c_int) -> *const __gl_imports::raw::c_char>(self.QueryServerString.f)(dpy, screen, name) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn QueryVersion(&self, dpy: *mut types::Display, maj: *mut __gl_imports::raw::c_int, min: *mut __gl_imports::raw::c_int) -> types::Bool { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, *mut __gl_imports::raw::c_int, *mut __gl_imports::raw::c_int) -> types::Bool>(self.QueryVersion.f)(dpy, maj, min) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn SelectEvent(&self, dpy: *mut types::Display, draw: types::GLXDrawable, event_mask: __gl_imports::raw::c_ulong) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXDrawable, __gl_imports::raw::c_ulong) -> ()>(self.SelectEvent.f)(dpy, draw, event_mask) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn SwapBuffers(&self, dpy: *mut types::Display, drawable: types::GLXDrawable) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXDrawable) -> ()>(self.SwapBuffers.f)(dpy, drawable) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn SwapIntervalEXT(&self, dpy: *mut types::Display, drawable: types::GLXDrawable, interval: __gl_imports::raw::c_int) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(*mut types::Display, types::GLXDrawable, __gl_imports::raw::c_int) -> ()>(self.SwapIntervalEXT.f)(dpy, drawable, interval) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn SwapIntervalMESA(&self, interval: __gl_imports::raw::c_uint) -> __gl_imports::raw::c_int { __gl_imports::mem::transmute::<_, extern "system" fn(__gl_imports::raw::c_uint) -> __gl_imports::raw::c_int>(self.SwapIntervalMESA.f)(interval) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn SwapIntervalSGI(&self, interval: __gl_imports::raw::c_int) -> __gl_imports::raw::c_int { __gl_imports::mem::transmute::<_, extern "system" fn(__gl_imports::raw::c_int) -> __gl_imports::raw::c_int>(self.SwapIntervalSGI.f)(interval) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn UseXFont(&self, font: types::Font, first: __gl_imports::raw::c_int, count: __gl_imports::raw::c_int, list: __gl_imports::raw::c_int) -> () { __gl_imports::mem::transmute::<_, extern "system" fn(types::Font, __gl_imports::raw::c_int, __gl_imports::raw::c_int, __gl_imports::raw::c_int) -> ()>(self.UseXFont.f)(font, first, count, list) }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn WaitGL(&self, ) -> () { __gl_imports::mem::transmute::<_, extern "system" fn() -> ()>(self.WaitGL.f)() }
#[allow(non_snake_case, unused_variables, dead_code)]
            #[inline] pub unsafe fn WaitX(&self, ) -> () { __gl_imports::mem::transmute::<_, extern "system" fn() -> ()>(self.WaitX.f)() }
}

        unsafe impl __gl_imports::Send for Glx {}
