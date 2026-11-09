use crate::harness::types::HarnessError;
use crate::secrets::types::Zeroizing;

#[cfg(target_os = "windows")]
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
mod win32 {
    use std::ffi::c_void;

    pub type HINTERNET = *mut c_void;
    pub type DWORD = u32;
    pub type BOOL = i32;
    pub type LPCWSTR = *const u16;
    pub type LPVOID = *mut c_void;
    pub type DWORD_PTR = usize;
    pub type INTERNET_PORT = u16;

    pub const WINHTTP_ACCESS_TYPE_NO_PROXY: DWORD = 1;
    pub const WINHTTP_FLAG_SECURE: DWORD = 0x00800000;
    pub const WINHTTP_QUERY_STATUS_CODE: DWORD = 19;
    pub const WINHTTP_QUERY_FLAG_NUMBER: DWORD = 0x20000000;
    pub const ERROR_WINHTTP_SECURE_FAILURE: DWORD = 12175;

    pub const WINHTTP_CALLBACK_STATUS_SECURE_FAILURE: DWORD = 0x00010000;
    pub const WINHTTP_CALLBACK_FLAG_SECURE_FAILURE: DWORD = 0x00010000;
    pub const WINHTTP_CALLBACK_STATUS_FLAG_INVALID_CA: DWORD = 0x00000008;
    pub const WINHTTP_CALLBACK_STATUS_FLAG_CERT_CN_INVALID: DWORD = 0x00000010;
    pub const WINHTTP_CALLBACK_STATUS_FLAG_CERT_DATE_INVALID: DWORD = 0x00000020;

    pub type WINHTTP_STATUS_CALLBACK = Option<
        unsafe extern "system" fn(
            h_internet: HINTERNET,
            dw_context: DWORD_PTR,
            dw_internet_status: DWORD,
            lpv_status_information: LPVOID,
            dw_status_information_length: DWORD,
        ),
    >;

    #[link(name = "winhttp")]
    extern "system" {
        pub fn WinHttpOpen(
            pszAgentW: LPCWSTR,
            dwAccessType: DWORD,
            pszProxyW: LPCWSTR,
            pszProxyBypassW: LPCWSTR,
            dwFlags: DWORD,
        ) -> HINTERNET;

        pub fn WinHttpSetStatusCallback(
            hInternet: HINTERNET,
            lpfnInternetCallback: WINHTTP_STATUS_CALLBACK,
            dwNotificationFlags: DWORD,
            dwReserved: DWORD_PTR,
        ) -> WINHTTP_STATUS_CALLBACK;

        pub fn WinHttpSetTimeouts(
            hInternet: HINTERNET,
            nResolveTimeout: i32,
            nConnectTimeout: i32,
            nSendTimeout: i32,
            nReceiveTimeout: i32,
        ) -> BOOL;

        pub fn WinHttpConnect(
            hSession: HINTERNET,
            pswzServerName: LPCWSTR,
            nServerPort: INTERNET_PORT,
            dwReserved: DWORD,
        ) -> HINTERNET;

        pub fn WinHttpOpenRequest(
            hConnect: HINTERNET,
            pwszVerb: LPCWSTR,
            pwszObjectName: LPCWSTR,
            pwszVersion: LPCWSTR,
            pwszReferrer: LPCWSTR,
            ppwszAcceptTypes: *const LPCWSTR,
            dwFlags: DWORD,
        ) -> HINTERNET;

        pub fn WinHttpSendRequest(
            hRequest: HINTERNET,
            lpszHeaders: LPCWSTR,
            dwHeadersLength: DWORD,
            lpOptional: LPVOID,
            dwOptionalLength: DWORD,
            dwTotalLength: DWORD,
            dwContext: DWORD_PTR,
        ) -> BOOL;

        pub fn WinHttpReceiveResponse(hRequest: HINTERNET, lpReserved: LPVOID) -> BOOL;

        pub fn WinHttpQueryHeaders(
            hRequest: HINTERNET,
            dwInfoLevel: DWORD,
            pwszName: LPCWSTR,
            lpBuffer: LPVOID,
            lpdwBufferLength: *mut DWORD,
            lpdwIndex: *mut DWORD,
        ) -> BOOL;

        pub fn WinHttpQueryDataAvailable(
            hRequest: HINTERNET,
            lpdwNumberOfBytesAvailable: *mut DWORD,
        ) -> BOOL;

        pub fn WinHttpReadData(
            hRequest: HINTERNET,
            lpBuffer: LPVOID,
            dwNumberOfBytesToRead: DWORD,
            lpdwNumberOfBytesRead: *mut DWORD,
        ) -> BOOL;

        pub fn WinHttpCloseHandle(hInternet: HINTERNET) -> BOOL;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetLastError() -> DWORD;
    }

    pub struct AutoWinHttp(pub HINTERNET);
    impl Drop for AutoWinHttp {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    pub fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub unsafe extern "system" fn winhttp_status_callback(
        _h_internet: HINTERNET,
        dw_context: DWORD_PTR,
        dw_internet_status: DWORD,
        lpv_status_information: LPVOID,
        _dw_status_information_length: DWORD,
    ) {
        if dw_internet_status == WINHTTP_CALLBACK_STATUS_SECURE_FAILURE
            && dw_context != 0
            && !lpv_status_information.is_null()
        {
            let flags = *(lpv_status_information as *const DWORD);
            *(dw_context as *mut DWORD) |= flags;
        }
    }
}

/// Dispatches an HTTPS POST request with full TLS 1.2/1.3 and certificate validation
/// using Windows native WinHTTP (`winhttp.dll`). Zero added external dependencies.
/// Sensitive header buffers holding credentials are strictly zeroized.
#[cfg(target_os = "windows")]
pub async fn send_winhttp_https_request(
    host: &str,
    port: u16,
    path: &str,
    headers_w: Zeroizing<Vec<u16>>,
    body: &[u8],
    timeout_ms: u64,
) -> Result<(u16, Vec<u8>), HarnessError> {
    let host_s = host.to_string();
    let path_s = path.to_string();
    let body_v = body.to_vec();

    tokio::task::spawn_blocking(move || {
        use win32::*;

        let agent = to_wide("Miku-Agent-Harness/1.0");
        let session = AutoWinHttp(unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        });
        if session.0.is_null() {
            let err = unsafe { GetLastError() };
            return Err(HarnessError::NetworkError(format!(
                "WinHttpOpen failed: error code {}",
                err
            )));
        }

        let timeout_i32 = (timeout_ms as i32).max(1000);
        unsafe {
            WinHttpSetTimeouts(session.0, timeout_i32, timeout_i32, timeout_i32, timeout_i32);
        }

        let host_w = to_wide(&host_s);
        let connect = AutoWinHttp(unsafe {
            WinHttpConnect(session.0, host_w.as_ptr(), port, 0)
        });
        if connect.0.is_null() {
            let err = unsafe { GetLastError() };
            return Err(HarnessError::NetworkError(format!(
                "WinHttpConnect to {}:{} failed: error code {}",
                host_s, port, err
            )));
        }

        let verb_w = to_wide("POST");
        let path_w = to_wide(&path_s);
        let request = AutoWinHttp(unsafe {
            WinHttpOpenRequest(
                connect.0,
                verb_w.as_ptr(),
                path_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        });
        if request.0.is_null() {
            let err = unsafe { GetLastError() };
            return Err(HarnessError::NetworkError(format!(
                "WinHttpOpenRequest failed: error code {}",
                err
            )));
        }

        let mut secure_failure_flags: DWORD = 0;
        unsafe {
            WinHttpSetStatusCallback(
                request.0,
                Some(winhttp_status_callback),
                WINHTTP_CALLBACK_FLAG_SECURE_FAILURE,
                0,
            );
        }

        let send_res = unsafe {
            WinHttpSendRequest(
                request.0,
                headers_w.as_ptr(),
                headers_w.len().saturating_sub(1) as DWORD,
                body_v.as_ptr() as LPVOID,
                body_v.len() as DWORD,
                body_v.len() as DWORD,
                &mut secure_failure_flags as *mut DWORD as DWORD_PTR,
            )
        };
        // headers_w is wrapped in Zeroizing RAII guard, ensuring zeroization upon drop on all branches!

        if send_res == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_WINHTTP_SECURE_FAILURE {
                let invalid_ca =
                    (secure_failure_flags & WINHTTP_CALLBACK_STATUS_FLAG_INVALID_CA) != 0;
                let cn_invalid =
                    (secure_failure_flags & WINHTTP_CALLBACK_STATUS_FLAG_CERT_CN_INVALID) != 0;
                let date_invalid =
                    (secure_failure_flags & WINHTTP_CALLBACK_STATUS_FLAG_CERT_DATE_INVALID) != 0;
                return Err(HarnessError::TlsCertificateValidationFailed(format!(
                    "TLS certificate validation failed for https://{}:{} (error code {}, secure_flags=0x{:08X}, invalid_ca={}, cn_invalid={}, date_invalid={})",
                    host_s, port, err, secure_failure_flags, invalid_ca, cn_invalid, date_invalid
                )));
            }
            return Err(HarnessError::NetworkError(format!(
                "WinHttpSendRequest failed with error code {}",
                err
            )));
        }

        let recv_res = unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) };
        if recv_res == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_WINHTTP_SECURE_FAILURE {
                let invalid_ca =
                    (secure_failure_flags & WINHTTP_CALLBACK_STATUS_FLAG_INVALID_CA) != 0;
                let cn_invalid =
                    (secure_failure_flags & WINHTTP_CALLBACK_STATUS_FLAG_CERT_CN_INVALID) != 0;
                let date_invalid =
                    (secure_failure_flags & WINHTTP_CALLBACK_STATUS_FLAG_CERT_DATE_INVALID) != 0;
                return Err(HarnessError::TlsCertificateValidationFailed(format!(
                    "TLS certificate validation failed during response handshake for https://{}:{} (error code {}, secure_flags=0x{:08X}, invalid_ca={}, cn_invalid={}, date_invalid={})",
                    host_s, port, err, secure_failure_flags, invalid_ca, cn_invalid, date_invalid
                )));
            }
            return Err(HarnessError::NetworkError(format!(
                "WinHttpReceiveResponse failed with error code {}",
                err
            )));
        }

        let mut status_code: DWORD = 0;
        let mut status_size: DWORD = std::mem::size_of::<DWORD>() as DWORD;
        let query_res = unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                &mut status_code as *mut DWORD as LPVOID,
                &mut status_size,
                std::ptr::null_mut(),
            )
        };
        if query_res == 0 {
            return Err(HarnessError::NetworkError(
                "failed to query HTTP status code".to_string(),
            ));
        }

        let mut response_body = Vec::new();
        loop {
            let mut bytes_avail: DWORD = 0;
            if unsafe { WinHttpQueryDataAvailable(request.0, &mut bytes_avail) } == 0 {
                break;
            }
            if bytes_avail == 0 {
                break;
            }
            let mut chunk = vec![0u8; bytes_avail as usize];
            let mut bytes_read: DWORD = 0;
            if unsafe {
                WinHttpReadData(
                    request.0,
                    chunk.as_mut_ptr() as LPVOID,
                    bytes_avail,
                    &mut bytes_read,
                )
            } == 0 {
                break;
            }
            chunk.truncate(bytes_read as usize);
            response_body.extend_from_slice(&chunk);
        }

        Ok((status_code as u16, response_body))
    })
    .await
    .map_err(|e| HarnessError::NetworkError(format!("WinHTTP task panicked: {}", e)))?
}

#[cfg(not(target_os = "windows"))]
pub async fn send_winhttp_https_request(
    _host: &str,
    _port: u16,
    _path: &str,
    _headers_w: Zeroizing<Vec<u16>>,
    _body: &[u8],
    _timeout_ms: u64,
) -> Result<(u16, Vec<u8>), HarnessError> {
    Err(HarnessError::InsecureTransport(
        "HTTPS on non-Windows platforms requires external TLS provider".to_string(),
    ))
}
