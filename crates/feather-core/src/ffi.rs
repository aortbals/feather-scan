//! C API used by the native apps. Strings are UTF-8 JSON; events arrive on a
//! background thread through the callback.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::engine::{Engine, Event};

pub type EventCallback = extern "C" fn(json: *const c_char, ctx: *mut c_void);

struct Ctx(*mut c_void);
// The context pointer is owned by the caller, who promises it is usable from any thread.
unsafe impl Send for Ctx {}
unsafe impl Sync for Ctx {}

unsafe fn str_arg<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(p) }.to_str().ok()
    }
}

fn to_c(s: String) -> *mut c_char {
    CString::new(s)
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// Starts the engine. Returns an opaque handle, or null if the config is invalid.
///
/// # Safety
/// `config_json` must be a valid C string. `callback` and `ctx` must stay valid until `feather_stop`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn feather_start(
    config_json: *const c_char,
    callback: EventCallback,
    ctx: *mut c_void,
) -> *mut c_void {
    let Some(cfg) = (unsafe { str_arg(config_json) }).and_then(|s| Config::from_json(s).ok())
    else {
        return std::ptr::null_mut();
    };
    let ctx = Arc::new(Ctx(ctx));
    let sink = Arc::new(move |e: Event| {
        if let Ok(s) = CString::new(serde_json::to_string(&e).unwrap_or_default()) {
            callback(s.as_ptr(), ctx.0);
        }
    });
    Box::into_raw(Box::new(Engine::start(cfg, sink))) as *mut c_void
}

/// Replaces the configuration. Returns false if the JSON is invalid.
///
/// # Safety
/// `handle` must come from `feather_start`; `config_json` must be a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn feather_set_config(
    handle: *mut c_void,
    config_json: *const c_char,
) -> bool {
    let (Some(engine), Some(cfg)) = (
        unsafe { (handle as *const Engine).as_ref() },
        (unsafe { str_arg(config_json) }).and_then(|s| Config::from_json(s).ok()),
    ) else {
        return false;
    };
    engine.set_config(cfg);
    true
}

/// Starts a scan of whatever is in the feeder.
///
/// # Safety
/// `handle` must come from `feather_start`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn feather_scan_now(handle: *mut c_void) {
    if let Some(engine) = unsafe { (handle as *const Engine).as_ref() } {
        engine.scan_now();
    }
}

/// Pairs this computer with the scanner (no ScanSnap Home needed). The running
/// session is ended first. Results arrive as events: `identity` and `paired` on
/// success (then the person selects this computer on the scanner's screen), or `error`.
///
/// # Safety
/// `handle` must come from `feather_start`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn feather_pair(handle: *mut c_void) {
    if let Some(engine) = unsafe { (handle as *const Engine).as_ref() } {
        engine.pair();
    }
}

/// Stops the engine, waits for running jobs, and frees the handle.
///
/// # Safety
/// `handle` must come from `feather_start` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn feather_stop(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe { Box::from_raw(handle as *mut Engine) }.stop();
    }
}

/// Looks for scanners for `timeout_ms`. `ip` may be null (broadcast). Returns a JSON array;
/// free it with `feather_free_string`.
///
/// # Safety
/// `ip` must be null or a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn feather_discover(ip: *const c_char, timeout_ms: u32) -> *mut c_char {
    let target = unsafe { str_arg(ip) }.and_then(|s| s.parse().ok());
    let found = crate::proto::discovery::discover(target, Duration::from_millis(timeout_ms as u64))
        .unwrap_or_default();
    to_c(serde_json::to_string(&found).unwrap_or_else(|_| "[]".into()))
}

/// Default configuration as JSON; free it with `feather_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn feather_default_config() -> *mut c_char {
    to_c(serde_json::to_string(&Config::default()).unwrap_or_default())
}

/// # Safety
/// `s` must come from this library, or be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn feather_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}
