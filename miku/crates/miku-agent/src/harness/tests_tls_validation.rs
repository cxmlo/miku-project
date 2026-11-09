#[cfg(all(test, target_os = "windows"))]
#[allow(non_snake_case, non_camel_case_types)]
mod tests {
    use std::ffi::c_void;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::harness::providers::send_provider_http_post;
    use crate::harness::types::{HarnessError, LlmProvider};

    #[repr(C)]
    pub struct CERT_CONTEXT {
        pub dwCertEncodingType: u32,
        pub pbCertEncoded: *mut u8,
        pub cbCertEncoded: u32,
        pub pCertInfo: *mut c_void,
        pub hCertStore: *mut c_void,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CRYPT_DATA_BLOB {
        pub cbData: u32,
        pub pbData: *mut u8,
    }

    #[repr(C)]
    pub struct CredHandle { pub dwLower: usize, pub dwUpper: usize }

    #[repr(C)]
    pub struct CtxtHandle { pub dwLower: usize, pub dwUpper: usize }

    #[repr(C)]
    pub struct SCHANNEL_CRED {
        pub dwVersion: u32,
        pub cCreds: u32,
        pub paCred: *const *const CERT_CONTEXT,
        pub hRootStore: *mut c_void,
        pub cMappers: u32,
        pub aphMappers: *mut c_void,
        pub cSupportedAlgs: u32,
        pub palgSupportedAlgs: *mut c_void,
        pub grbitEnabledProtocols: u32,
        pub dwMinimumCipherStrength: u32,
        pub dwMaximumCipherStrength: u32,
        pub dwSessionLifespan: u32,
        pub dwFlags: u32,
        pub dwCredFormat: u32,
    }

    #[repr(C)]
    pub struct SecBuffer { pub cbBuffer: u32, pub BufferType: u32, pub pvBuffer: *mut c_void }

    #[repr(C)]
    pub struct SecBufferDesc { pub ulVersion: u32, pub cBuffers: u32, pub pBuffers: *mut SecBuffer }

    pub const X509_ASN_ENCODING: u32 = 0x00000001;
    pub const CERT_OID_NAME_STR: u32 = 2;
    pub const SCHANNEL_CRED_VERSION: u32 = 4;
    pub const SECPKG_CRED_INBOUND: u32 = 1;
    pub const SP_PROT_TLS1_2_SERVER: u32 = 0x00000400;
    pub const SP_PROT_TLS1_3_SERVER: u32 = 0x00001000;
    pub const SECBUFFER_TOKEN: u32 = 2;
    pub const SECBUFFER_EMPTY: u32 = 0;
    pub const SECBUFFER_VERSION: u32 = 0;
    pub const SEC_I_CONTINUE_NEEDED: i32 = 0x00090312;
    pub const ASC_REQ_STREAM: u32 = 0x00010000;
    pub const ASC_REQ_CONFIDENTIALITY: u32 = 0x00000010;
    pub const ASC_REQ_EXTENDED_ERROR: u32 = 0x00004000;
    pub const SECURITY_NATIVE_DREP: u32 = 0x00000010;

    pub const CERT_ALT_NAME_DNS_NAME: u32 = 3;
    pub const CERT_ALT_NAME_IP_ADDRESS: u32 = 8;
    pub const PROV_RSA_FULL: u32 = 1;
    pub const AT_KEYEXCHANGE: u32 = 1;
    pub const CRYPT_DELETEKEYSET: u32 = 0x00000010;
    pub const CRYPT_SILENT: u32 = 0x00000040;
    pub const NTE_BAD_KEYSET: u32 = 0x80090016;

    #[repr(C)]
    pub union AltNameValue { pub ptr: *mut u16, pub blob: CRYPT_DATA_BLOB }

    #[repr(C)]
    pub struct CERT_ALT_NAME_ENTRY { pub dwAltNameChoice: u32, pub value: AltNameValue }

    #[repr(C)]
    pub struct CERT_ALT_NAME_INFO { pub cAltEntry: u32, pub rgAltEntry: *mut CERT_ALT_NAME_ENTRY }

    #[repr(C)]
    pub struct CERT_EXTENSION { pub pszObjId: *const u8, pub fCritical: i32, pub Value: CRYPT_DATA_BLOB }

    #[repr(C)]
    pub struct CERT_EXTENSIONS { pub cExtension: u32, pub rgExtension: *mut CERT_EXTENSION }

    #[repr(C)]
    pub struct CRYPT_KEY_PROV_INFO {
        pub pwszContainerName: *mut u16,
        pub pwszProvName: *mut u16,
        pub dwProvType: u32,
        pub dwFlags: u32,
        pub cProvParam: u32,
        pub rgProvParam: *mut c_void,
        pub dwKeySpec: u32,
    }

    #[link(name = "crypt32")]
    extern "system" {
        pub fn CertStrToNameW(
            dwCertEncodingType: u32,
            pszX500: *const u16,
            dwStrType: u32,
            pReserved: *mut c_void,
            pbEncoded: *mut u8,
            pcbEncoded: *mut u32,
            ppszError: *mut *const u16,
        ) -> i32;

        pub fn CryptEncodeObjectEx(
            dwCertEncodingType: u32,
            lpszStructType: *const u8,
            pvStructInfo: *const c_void,
            dwFlags: u32,
            pEncodePara: *mut c_void,
            pvEncoded: *mut u8,
            pcbEncoded: *mut u32,
        ) -> i32;

        pub fn CertCreateSelfSignCertificate(
            hCryptProvOrNCryptKey: *mut c_void,
            pSubjectIssuerBlob: *const CRYPT_DATA_BLOB,
            dwFlags: u32,
            pKeyProvInfo: *const c_void,
            pSignatureAlgorithm: *const c_void,
            pStartTime: *const c_void,
            pEndTime: *const c_void,
            pExtensions: *const c_void,
        ) -> *const CERT_CONTEXT;

        pub fn CertFreeCertificateContext(pCertContext: *const CERT_CONTEXT) -> i32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        pub fn CryptAcquireContextW(
            phProv: *mut usize,
            szContainer: *const u16,
            szProvider: *const u16,
            dwProvType: u32,
            dwFlags: u32,
        ) -> i32;

        pub fn CryptReleaseContext(hProv: usize, dwFlags: u32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetLastError() -> u32;
        pub fn GetCurrentProcessId() -> u32;
    }

    #[link(name = "secur32")]
    extern "system" {
        pub fn AcquireCredentialsHandleW(
            pszPrincipal: *const u16,
            pszPackage: *const u16,
            fCredentialUse: u32,
            pvLogonID: *const c_void,
            pAuthData: *const c_void,
            pGetKeyFn: *const c_void,
            pvGetKeyArgument: *const c_void,
            phCredential: *mut CredHandle,
            ptsExpiry: *mut i64,
        ) -> i32;

        pub fn AcceptSecurityContext(
            phCredential: *const CredHandle,
            phContext: *mut CtxtHandle,
            pInput: *const SecBufferDesc,
            fContextReq: u32,
            TargetDataRep: u32,
            phNewContext: *mut CtxtHandle,
            pOutput: *mut SecBufferDesc,
            pfContextAttr: *mut u32,
            ptsExpiry: *mut i64,
        ) -> i32;

        pub fn FreeCredentialsHandle(phCredential: *mut CredHandle) -> i32;
        pub fn DeleteSecurityContext(phContext: *mut CtxtHandle) -> i32;
    }

    struct EphemeralTlsServerCredentials {
        cert: *const CERT_CONTEXT,
        cred_handle: CredHandle,
        container_w: Vec<u16>,
    }

    impl EphemeralTlsServerCredentials {
        fn create_untrusted_self_signed(subject_cn: &str) -> Self {
            let cn: Vec<u16> = format!("CN={}", subject_cn).encode_utf16().chain(std::iter::once(0)).collect();
            let mut encoded_len: u32 = 0;
            let res = unsafe {
                CertStrToNameW(X509_ASN_ENCODING, cn.as_ptr(), CERT_OID_NAME_STR, std::ptr::null_mut(), std::ptr::null_mut(), &mut encoded_len, std::ptr::null_mut())
            };
            assert_ne!(res, 0, "CertStrToNameW length query failed");

            let mut encoded_bytes = vec![0u8; encoded_len as usize];
            let res = unsafe {
                CertStrToNameW(X509_ASN_ENCODING, cn.as_ptr(), CERT_OID_NAME_STR, std::ptr::null_mut(), encoded_bytes.as_mut_ptr(), &mut encoded_len, std::ptr::null_mut())
            };
            assert_ne!(res, 0, "CertStrToNameW encode failed");

            let subject_blob = CRYPT_DATA_BLOB { cbData: encoded_len, pbData: encoded_bytes.as_mut_ptr() };

            // Unique named key container for verifiable lifecycle and RAII deletion
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let pid = unsafe { GetCurrentProcessId() };
            let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let container_str = format!("miku-test-tls-{}-{}", pid, count);
            let mut container_w: Vec<u16> = container_str.encode_utf16().chain(std::iter::once(0)).collect();

            let mut key_prov_info = CRYPT_KEY_PROV_INFO {
                pwszContainerName: container_w.as_mut_ptr(),
                pwszProvName: std::ptr::null_mut(),
                dwProvType: PROV_RSA_FULL,
                dwFlags: 0,
                cProvParam: 0,
                rgProvParam: std::ptr::null_mut(),
                dwKeySpec: AT_KEYEXCHANGE,
            };

            // Subject Alternative Name (SAN) matching endpoint 127.0.0.1 and localhost
            // to ensure certificate name validation succeeds and isolates untrusted CA rejection
            let ip_bytes: [u8; 4] = [127, 0, 0, 1];
            let ip_blob = CRYPT_DATA_BLOB { cbData: 4, pbData: ip_bytes.as_ptr() as *mut u8 };
            let mut dns_localhost: Vec<u16> = "localhost".encode_utf16().chain(std::iter::once(0)).collect();
            let mut dns_ip: Vec<u16> = "127.0.0.1".encode_utf16().chain(std::iter::once(0)).collect();

            let mut alt_entries = [
                CERT_ALT_NAME_ENTRY { dwAltNameChoice: CERT_ALT_NAME_IP_ADDRESS, value: AltNameValue { blob: ip_blob } },
                CERT_ALT_NAME_ENTRY { dwAltNameChoice: CERT_ALT_NAME_DNS_NAME, value: AltNameValue { ptr: dns_localhost.as_mut_ptr() } },
                CERT_ALT_NAME_ENTRY { dwAltNameChoice: CERT_ALT_NAME_DNS_NAME, value: AltNameValue { ptr: dns_ip.as_mut_ptr() } },
            ];

            let mut alt_name_info = CERT_ALT_NAME_INFO { cAltEntry: alt_entries.len() as u32, rgAltEntry: alt_entries.as_mut_ptr() };

            let san_oid = c"2.5.29.17";
            let mut encoded_san_len: u32 = 0;
            let enc_res = unsafe {
                CryptEncodeObjectEx(
                    X509_ASN_ENCODING,
                    san_oid.as_ptr() as *const u8,
                    &mut alt_name_info as *mut _ as *const c_void,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut encoded_san_len,
                )
            };
            assert_ne!(enc_res, 0, "CryptEncodeObjectEx length query failed");

            let mut encoded_san_bytes = vec![0u8; encoded_san_len as usize];
            let enc_res = unsafe {
                CryptEncodeObjectEx(
                    X509_ASN_ENCODING,
                    san_oid.as_ptr() as *const u8,
                    &mut alt_name_info as *mut _ as *const c_void,
                    0,
                    std::ptr::null_mut(),
                    encoded_san_bytes.as_mut_ptr(),
                    &mut encoded_san_len,
                )
            };
            assert_ne!(enc_res, 0, "CryptEncodeObjectEx encoding failed");

            let mut ext = CERT_EXTENSION {
                pszObjId: san_oid.as_ptr() as *const u8,
                fCritical: 0,
                Value: CRYPT_DATA_BLOB { cbData: encoded_san_len, pbData: encoded_san_bytes.as_mut_ptr() },
            };
            let mut extensions = CERT_EXTENSIONS { cExtension: 1, rgExtension: &mut ext };

            // Creates an isolated self-signed certificate with matching SAN and isolated key container
            let cert = unsafe {
                CertCreateSelfSignCertificate(
                    std::ptr::null_mut(),
                    &subject_blob,
                    0,
                    &mut key_prov_info as *mut _ as *const c_void,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    &mut extensions as *mut _ as *const c_void,
                )
            };
            assert!(!cert.is_null(), "CertCreateSelfSignCertificate failed");

            let cert_array = [cert];
            let schannel_cred = SCHANNEL_CRED {
                dwVersion: SCHANNEL_CRED_VERSION,
                cCreds: 1,
                paCred: cert_array.as_ptr(),
                hRootStore: std::ptr::null_mut(),
                cMappers: 0,
                aphMappers: std::ptr::null_mut(),
                cSupportedAlgs: 0,
                palgSupportedAlgs: std::ptr::null_mut(),
                grbitEnabledProtocols: SP_PROT_TLS1_2_SERVER | SP_PROT_TLS1_3_SERVER,
                dwMinimumCipherStrength: 0,
                dwMaximumCipherStrength: 0,
                dwSessionLifespan: 0,
                dwFlags: 0,
                dwCredFormat: 0,
            };

            let package: Vec<u16> = "Microsoft Unified Security Protocol Provider".encode_utf16().chain(std::iter::once(0)).collect();
            let mut cred_handle = CredHandle { dwLower: 0, dwUpper: 0 };
            let mut expiry: i64 = 0;

            let status = unsafe {
                AcquireCredentialsHandleW(
                    std::ptr::null(),
                    package.as_ptr(),
                    SECPKG_CRED_INBOUND,
                    std::ptr::null(),
                    &schannel_cred as *const _ as *const c_void,
                    std::ptr::null(),
                    std::ptr::null(),
                    &mut cred_handle,
                    &mut expiry,
                )
            };
            assert_eq!(status, 0, "AcquireCredentialsHandleW failed with status 0x{:08X}", status);

            Self { cert, cred_handle, container_w }
        }

        pub fn is_keyset_deleted(container_name: &[u16]) -> bool {
            let mut h_prov: usize = 0;
            let res = unsafe {
                CryptAcquireContextW(
                    &mut h_prov,
                    container_name.as_ptr(),
                    std::ptr::null(),
                    PROV_RSA_FULL,
                    CRYPT_SILENT,
                )
            };
            if res != 0 {
                unsafe { CryptReleaseContext(h_prov, 0) };
                return false;
            }
            let err = unsafe { GetLastError() };
            err == NTE_BAD_KEYSET
        }

        pub fn is_keyset_present(container_name: &[u16]) -> bool {
            let mut h_prov: usize = 0;
            let res = unsafe {
                CryptAcquireContextW(
                    &mut h_prov,
                    container_name.as_ptr(),
                    std::ptr::null(),
                    PROV_RSA_FULL,
                    CRYPT_SILENT,
                )
            };
            if res != 0 {
                unsafe { CryptReleaseContext(h_prov, 0) };
                true
            } else {
                false
            }
        }
    }

    impl Drop for EphemeralTlsServerCredentials {
        fn drop(&mut self) {
            unsafe {
                FreeCredentialsHandle(&mut self.cred_handle);
                CertFreeCertificateContext(self.cert);
                let mut h_prov: usize = 0;
                CryptAcquireContextW(
                    &mut h_prov,
                    self.container_w.as_ptr(),
                    std::ptr::null(),
                    PROV_RSA_FULL,
                    CRYPT_DELETEKEYSET | CRYPT_SILENT,
                );
            }
        }
    }

    fn accept_security_token(
        server_cred: &CredHandle,
        context: &mut CtxtHandle,
        in_bytes: &[u8],
    ) -> (i32, Vec<u8>) {
        let mut in_buf = in_bytes.to_vec();
        let mut in_buffers = [
            SecBuffer {
                cbBuffer: in_buf.len() as u32,
                BufferType: SECBUFFER_TOKEN,
                pvBuffer: in_buf.as_mut_ptr() as *mut c_void,
            },
            SecBuffer {
                cbBuffer: 0,
                BufferType: SECBUFFER_EMPTY,
                pvBuffer: std::ptr::null_mut(),
            },
        ];
        let in_desc = SecBufferDesc {
            ulVersion: SECBUFFER_VERSION,
            cBuffers: in_buffers.len() as u32,
            pBuffers: in_buffers.as_mut_ptr(),
        };

        let mut out_token = vec![0u8; 32768];
        let mut out_buffers = [
            SecBuffer {
                cbBuffer: out_token.len() as u32,
                BufferType: SECBUFFER_TOKEN,
                pvBuffer: out_token.as_mut_ptr() as *mut c_void,
            },
            SecBuffer {
                cbBuffer: 0,
                BufferType: SECBUFFER_EMPTY,
                pvBuffer: std::ptr::null_mut(),
            },
        ];
        let mut out_desc = SecBufferDesc {
            ulVersion: SECBUFFER_VERSION,
            cBuffers: out_buffers.len() as u32,
            pBuffers: out_buffers.as_mut_ptr(),
        };

        let mut attr: u32 = 0;
        let mut expiry: i64 = 0;
        let is_init = context.dwLower == 0 && context.dwUpper == 0;
        let ph_ctx = if is_init {
            std::ptr::null_mut()
        } else {
            context as *mut _
        };

        let sc = unsafe {
            AcceptSecurityContext(
                server_cred,
                ph_ctx,
                &in_desc,
                ASC_REQ_CONFIDENTIALITY | ASC_REQ_STREAM | ASC_REQ_EXTENDED_ERROR,
                SECURITY_NATIVE_DREP,
                context,
                &mut out_desc,
                &mut attr,
                &mut expiry,
            )
        };

        let mut out = Vec::new();
        if (sc == SEC_I_CONTINUE_NEEDED || sc == 0) && out_buffers[0].cbBuffer > 0 {
            out.extend_from_slice(&out_token[..out_buffers[0].cbBuffer as usize]);
        }
        (sc, out)
    }

    /// [LEDGER-3.7-7] Local TLS server with in-memory untrusted self-signed certificate.
    /// Demonstrates empirical rejection by WinHTTP TLS certificate validation with ERROR_WINHTTP_SECURE_FAILURE.
    #[tokio::test]
    async fn test_winhttp_rejects_untrusted_self_signed_certificate() {
        let creds = EphemeralTlsServerCredentials::create_untrusted_self_signed("127.0.0.1");
        let container_w = creds.container_w.clone();
        assert!(
            EphemeralTlsServerCredentials::is_keyset_present(&container_w),
            "ephemeral key container must exist before cleanup"
        );
        let cred_lower = creds.cred_handle.dwLower;
        let cred_upper = creds.cred_handle.dwUpper;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test listener");
        let port = listener.local_addr().expect("local addr").port();

        // Spawn mock server task that completes the server-side TLS handshake with bounded timeouts
        let server_task = tokio::spawn(async move {
            let Ok(Ok((mut socket, _peer))) = tokio::time::timeout(
                std::time::Duration::from_millis(2_500),
                listener.accept(),
            )
            .await
            else {
                return;
            };
            let mut client_hello = vec![0u8; 8192];
            let n = tokio::time::timeout(
                std::time::Duration::from_millis(1_500),
                socket.read(&mut client_hello),
            )
            .await
            .unwrap_or(Ok(0))
            .unwrap_or(0);
            if n == 0 {
                return;
            }
            client_hello.truncate(n);

            let server_cred = CredHandle {
                dwLower: cred_lower,
                dwUpper: cred_upper,
            };
            let mut new_context = CtxtHandle {
                dwLower: 0,
                dwUpper: 0,
            };

            let (_sc1, out_bytes1) =
                accept_security_token(&server_cred, &mut new_context, &client_hello);
            if !out_bytes1.is_empty() {
                let _ = socket.write_all(&out_bytes1).await;
                let _ = socket.flush().await;
            }

            let mut client_response = vec![0u8; 4096];
            let n2 = tokio::time::timeout(
                std::time::Duration::from_millis(300),
                socket.read(&mut client_response),
            )
            .await
            .unwrap_or(Ok(0))
            .unwrap_or(0);
            if n2 > 0 {
                client_response.truncate(n2);
                let (_sc2, out_bytes2) =
                    accept_security_token(&server_cred, &mut new_context, &client_response);
                if !out_bytes2.is_empty() {
                    let _ = socket.write_all(&out_bytes2).await;
                    let _ = socket.flush().await;
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

            if new_context.dwLower != 0 || new_context.dwUpper != 0 {
                unsafe {
                    DeleteSecurityContext(&mut new_context);
                }
            }
        });

        let endpoint = format!("https://127.0.0.1:{}/v1/chat/completions", port);
        let result = send_provider_http_post(
            &endpoint,
            LlmProvider::DeepSeek,
            Some(b"sk-test-secret-key-123"),
            b"{}",
            3_000,
        )
        .await;

        let _ = server_task.await;
        drop(creds);

        // Verify key container is completely deleted via RAII
        assert!(
            EphemeralTlsServerCredentials::is_keyset_deleted(&container_w),
            "ephemeral key container must be cleanly deleted via RAII"
        );

        // WinHTTP must strictly reject the untrusted self-signed certificate with TlsCertificateValidationFailed
        assert!(
            result.is_err(),
            "connection to server with untrusted cert must fail"
        );
        let err = result.unwrap_err();
        match &err {
            HarnessError::TlsCertificateValidationFailed(detail) => {
                println!("[LEDGER-3.7-7] TLS validation rejection verified: {}", detail);
                assert!(
                    detail.contains("invalid_ca=true"),
                    "rejection must be explicitly due to untrusted CA: {}",
                    detail
                );
                assert!(
                    detail.contains("cn_invalid=false"),
                    "rejection must NOT be due to CN/SAN mismatch: {}",
                    detail
                );
            }
            other => panic!(
                "expected HarnessError::TlsCertificateValidationFailed, got: {:?}",
                other
            ),
        }
    }
}
