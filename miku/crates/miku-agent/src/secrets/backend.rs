use std::collections::HashMap;

use crate::secrets::types::{SecretError, Zeroize, Zeroizing};

/// Storage backend abstraction for persisting and retrieving encrypted secrets.
pub trait SecretStoreBackend: Send + Sync {
    fn save_secret(&mut self, alias: &str, secret: &[u8]) -> Result<(), SecretError>;
    fn load_secret(&self, alias: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError>;
    fn delete_secret(&mut self, alias: &str) -> Result<bool, SecretError>;
}

/// In-memory encrypted store backend for non-Windows platforms and isolated unit testing.
#[derive(Debug, Default)]
pub struct EncryptedMemoryBackend {
    store: HashMap<String, Zeroizing<Vec<u8>>>,
}

impl EncryptedMemoryBackend {
    pub fn new() -> Self {
        Self {
            store: HashMap::new(),
        }
    }
}

impl SecretStoreBackend for EncryptedMemoryBackend {
    fn save_secret(&mut self, alias: &str, secret: &[u8]) -> Result<(), SecretError> {
        let mut zeroizing_buf = Zeroizing::new(Vec::with_capacity(secret.len()));
        zeroizing_buf.extend_from_slice(secret);
        self.store.insert(alias.to_string(), zeroizing_buf);
        Ok(())
    }

    fn load_secret(&self, alias: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        if let Some(val) = self.store.get(alias) {
            let mut copy = Zeroizing::new(Vec::with_capacity(val.len()));
            copy.extend_from_slice(val.as_slice());
            Ok(Some(copy))
        } else {
            Ok(None)
        }
    }

    fn delete_secret(&mut self, alias: &str) -> Result<bool, SecretError> {
        if let Some(mut val) = self.store.remove(alias) {
            val.zeroize();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

// ============================================================================
// Windows Credential Manager Backend (Advapi32.dll)
// ============================================================================

#[cfg(target_os = "windows")]
#[allow(clippy::upper_case_acronyms)]
#[repr(C)]
struct CREDENTIALW {
    flags: u32,
    cred_type: u32,
    target_name: *mut u16,
    comment: *mut u16,
    last_written: [u32; 2],
    credential_blob_size: u32,
    credential_blob: *mut u8,
    persist: u32,
    attribute_count: u32,
    attributes: *mut std::ffi::c_void,
    target_alias: *mut u16,
    user_name: *mut u16,
}

#[cfg(target_os = "windows")]
const CRED_TYPE_GENERIC: u32 = 1;
#[cfg(target_os = "windows")]
const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
#[cfg(target_os = "windows")]
const ERROR_NOT_FOUND: u32 = 1168;

#[cfg(target_os = "windows")]
#[link(name = "advapi32")]
extern "system" {
    fn CredWriteW(credential: *const CREDENTIALW, flags: u32) -> i32;
    fn CredReadW(
        target_name: *const u16,
        cred_type: u32,
        flags: u32,
        credential: *mut *mut CREDENTIALW,
    ) -> i32;
    fn CredDeleteW(target_name: *const u16, cred_type: u32, flags: u32) -> i32;
    fn CredFree(buffer: *mut std::ffi::c_void);
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
}

#[cfg(target_os = "windows")]
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Production Windows backend utilizing Windows Credential Manager (`Advapi32.dll`).
/// Stores generic credentials indexed by target prefix `miku/secrets/{alias}`.
#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
pub struct WindowsCredentialBackend;

#[cfg(target_os = "windows")]
impl WindowsCredentialBackend {
    pub fn new() -> Self {
        Self
    }

    fn format_target_name(alias: &str) -> String {
        format!("miku/secrets/{}", alias)
    }
}

#[cfg(target_os = "windows")]
impl SecretStoreBackend for WindowsCredentialBackend {
    fn save_secret(&mut self, alias: &str, secret: &[u8]) -> Result<(), SecretError> {
        let target = Self::format_target_name(alias);
        let mut wide_target = to_wide(&target);

        let cred = CREDENTIALW {
            flags: 0,
            cred_type: CRED_TYPE_GENERIC,
            target_name: wide_target.as_mut_ptr(),
            comment: std::ptr::null_mut(),
            last_written: [0, 0],
            credential_blob_size: secret.len() as u32,
            credential_blob: secret.as_ptr() as *mut u8,
            persist: CRED_PERSIST_LOCAL_MACHINE,
            attribute_count: 0,
            attributes: std::ptr::null_mut(),
            target_alias: std::ptr::null_mut(),
            user_name: std::ptr::null_mut(),
        };

        let res = unsafe { CredWriteW(&cred, 0) };
        if res == 0 {
            let err = unsafe { GetLastError() };
            return Err(SecretError::BackendError(format!(
                "CredWriteW failed with system error code {}",
                err
            )));
        }
        Ok(())
    }

    fn load_secret(&self, alias: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        let target = Self::format_target_name(alias);
        let wide_target = to_wide(&target);
        let mut cred_ptr: *mut CREDENTIALW = std::ptr::null_mut();

        let res = unsafe {
            CredReadW(
                wide_target.as_ptr(),
                CRED_TYPE_GENERIC,
                0,
                &mut cred_ptr,
            )
        };

        if res == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(SecretError::BackendError(format!(
                "CredReadW failed with system error code {}",
                err
            )));
        }

        if cred_ptr.is_null() {
            return Ok(None);
        }

        let (mut secret_vec, cred_size) = unsafe {
            let cred = &*cred_ptr;
            let size = cred.credential_blob_size as usize;
            let slice = std::slice::from_raw_parts(cred.credential_blob, size);
            let mut vec = Zeroizing::new(Vec::with_capacity(size));
            vec.extend_from_slice(slice);
            (vec, size)
        };

        unsafe {
            CredFree(cred_ptr as *mut std::ffi::c_void);
        }

        if cred_size == 0 {
            secret_vec.zeroize();
            Ok(None)
        } else {
            Ok(Some(secret_vec))
        }
    }

    fn delete_secret(&mut self, alias: &str) -> Result<bool, SecretError> {
        let target = Self::format_target_name(alias);
        let wide_target = to_wide(&target);

        let res = unsafe { CredDeleteW(wide_target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if res == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                return Ok(false);
            }
            return Err(SecretError::BackendError(format!(
                "CredDeleteW failed with system error code {}",
                err
            )));
        }
        Ok(true)
    }
}
