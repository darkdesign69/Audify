//! JavaScriptCore through its public C API (`JavaScriptCore.framework`, present on macOS and iOS).
//!
//! In-process, no WebContent process to be suspended, no extra binary size (system framework).
//! Only ~10 functions are needed, so they are declared here rather than pulling a bindings crate.
//! From the S3 spike (`ios/spike-js/src/jsc.rs`, docs/streams/ios-js.md).

use std::ffi::c_void;
use std::ptr;

use super::inproc::SyncJs;

type JSContextRef = *const c_void;
type JSGlobalContextRef = *mut c_void;
type JSStringRef = *mut c_void;
type JSValueRef = *const c_void;
type JSObjectRef = *mut c_void;
type JSClassRef = *mut c_void;

#[link(name = "JavaScriptCore", kind = "framework")]
extern "C" {
    fn JSGlobalContextCreate(global_object_class: JSClassRef) -> JSGlobalContextRef;
    fn JSGlobalContextRelease(ctx: JSGlobalContextRef);
    fn JSStringCreateWithCharacters(chars: *const u16, num_chars: usize) -> JSStringRef;
    fn JSStringRelease(s: JSStringRef);
    fn JSStringGetLength(s: JSStringRef) -> usize;
    fn JSStringGetCharactersPtr(s: JSStringRef) -> *const u16;
    fn JSEvaluateScript(
        ctx: JSContextRef,
        script: JSStringRef,
        this_object: JSObjectRef,
        source_url: JSStringRef,
        starting_line_number: i32,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    fn JSValueToStringCopy(ctx: JSContextRef, value: JSValueRef, exception: *mut JSValueRef) -> JSStringRef;
    fn JSGarbageCollect(ctx: JSContextRef);
}

pub struct Jsc {
    ctx: JSGlobalContextRef,
}

// A JSGlobalContext may be used from any one thread at a time (JSC locks its VM internally).
unsafe impl Send for Jsc {}

impl Jsc {
    pub fn new() -> Self {
        Jsc { ctx: unsafe { JSGlobalContextCreate(ptr::null_mut()) } }
    }

    fn js_string(s: &str) -> JSStringRef {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe { JSStringCreateWithCharacters(w.as_ptr(), w.len()) }
    }

    fn to_rust(&self, v: JSValueRef) -> String {
        unsafe {
            let s = JSValueToStringCopy(self.ctx, v, ptr::null_mut());
            if s.is_null() {
                return String::new();
            }
            let len = JSStringGetLength(s);
            let chars = std::slice::from_raw_parts(JSStringGetCharactersPtr(s), len);
            let out = String::from_utf16_lossy(chars);
            JSStringRelease(s);
            out
        }
    }
}

impl Default for Jsc {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncJs for Jsc {
    fn eval(&mut self, src: &str) -> Result<String, String> {
        unsafe {
            let script = Self::js_string(src);
            let mut exc: JSValueRef = ptr::null();
            let v = JSEvaluateScript(self.ctx, script, ptr::null_mut(), ptr::null_mut(), 1, &mut exc);
            JSStringRelease(script);
            if !exc.is_null() {
                return Err(self.to_rust(exc));
            }
            Ok(self.to_rust(v))
        }
    }
}

impl Drop for Jsc {
    fn drop(&mut self) {
        unsafe {
            JSGarbageCollect(self.ctx);
            JSGlobalContextRelease(self.ctx);
        }
    }
}
