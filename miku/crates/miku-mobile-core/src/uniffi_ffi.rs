use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use crate::mobile_client::{
    build_island_mobile_parity_matrix, compute_req_hash_v1, validate_mobile_config_no_provider_keys,
    MobileCoreClient, MobileDecisionInput,
};

pub const UNIFFI_UDL_SPEC: &str = include_str!("miku_mobile.udl");

fn alloc_c_string(text: String) -> *mut c_char {
    let sanitized = text.replace('\0', "");
    match CString::new(sanitized) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn c_str_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: Caller passes a valid null-terminated UTF-8 C string pointer across FFI.
    let c_str = unsafe { CStr::from_ptr(ptr) };
    c_str.to_str().ok().map(|s| s.to_string())
}

/// Frees a C string previously returned by `miku_mobile_*` FFI functions.
///
/// # Safety
/// `ptr` must be null or a pointer returned by one of the `miku_mobile_*` functions in this crate.
#[no_mangle]
pub unsafe extern "C" fn miku_mobile_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = unsafe { CString::from_raw(ptr) };
    }
}

/// C-ABI / UniFFI export computing canonical `MIKU_REQ_HASH_V1`.
///
/// # Safety
/// All non-null pointers must reference valid NUL-terminated UTF-8 strings.
#[no_mangle]
pub unsafe extern "C" fn miku_mobile_compute_req_hash(
    agent: *const c_char,
    capability: *const c_char,
    canonical_target: *const c_char,
    canonical_args: *const c_char,
) -> *mut c_char {
    let Some(a) = c_str_to_string(agent) else {
        return alloc_c_string(String::new());
    };
    let Some(cap) = c_str_to_string(capability) else {
        return alloc_c_string(String::new());
    };
    let target = c_str_to_string(canonical_target).unwrap_or_default();
    let args = c_str_to_string(canonical_args).unwrap_or_default();
    alloc_c_string(compute_req_hash_v1(&a, &cap, &target, &args))
}

/// C-ABI / UniFFI export validating a mobile permission decision payload.
///
/// # Safety
/// `input_json` must point to a valid NUL-terminated UTF-8 JSON object matching `MobileDecisionInput`.
#[no_mangle]
pub unsafe extern "C" fn miku_mobile_decide_permission_json(
    input_json: *const c_char,
) -> *mut c_char {
    let Some(raw) = c_str_to_string(input_json) else {
        return alloc_c_string(r#"{"ok":false,"error":"null input_json"}"#.to_string());
    };
    let Ok(parsed) = serde_json::from_str::<MobileDecisionInput>(&raw) else {
        return alloc_c_string(r#"{"ok":false,"error":"invalid MobileDecisionInput json"}"#.to_string());
    };
    let client = MobileCoreClient::new("ffi-session", "android-device");
    match client.prepare_permission_decision(&parsed) {
        Ok(res) => alloc_c_string(format!(
            r#"{{"ok":true,"result":{}}}"#,
            serde_json::to_string(&res).unwrap_or_else(|_| "null".to_string())
        )),
        Err(err) => alloc_c_string(
            serde_json::json!({ "ok": false, "error": err.to_string() }).to_string(),
        ),
    }
}

/// C-ABI / UniFFI export enforcing zero provider API keys in mobile config state.
///
/// # Safety
/// `config_json` must point to a valid NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn miku_mobile_validate_config_json(
    config_json: *const c_char,
) -> *mut c_char {
    let Some(raw) = c_str_to_string(config_json) else {
        return alloc_c_string(r#"{"ok":false,"error":"null config_json"}"#.to_string());
    };
    match validate_mobile_config_no_provider_keys(&raw) {
        Ok(()) => alloc_c_string(r#"{"ok":true}"#.to_string()),
        Err(err) => alloc_c_string(
            serde_json::json!({ "ok": false, "error": err.to_string() }).to_string(),
        ),
    }
}

/// C-ABI / UniFFI export returning the canonical Island <-> Mobile parity matrix as JSON.
#[no_mangle]
pub extern "C" fn miku_mobile_parity_matrix_json() -> *mut c_char {
    let matrix = build_island_mobile_parity_matrix();
    let json = serde_json::to_string(&matrix).unwrap_or_else(|_| "[]".to_string());
    alloc_c_string(json)
}
