#[cfg(windows)]
pub(super) fn post_https(
    host: &str,
    port: u16,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(u16, Vec<u8>), String> {
    request_https("POST", host, port, path, headers, body)
}

#[cfg(windows)]
pub(super) fn get_https(
    host: &str,
    port: u16,
    path: &str,
    headers: &[(String, String)],
) -> Result<(u16, Vec<u8>), String> {
    request_https("GET", host, port, path, headers, &[])
}

#[cfg(windows)]
pub(super) fn request_https(
    verb: &str,
    host: &str,
    port: u16,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(u16, Vec<u8>), String> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
    const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
    const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
    const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

    #[link(name = "winhttp")]
    extern "system" {
        fn WinHttpOpen(
            agent: *const u16,
            access_type: u32,
            proxy: *const u16,
            bypass: *const u16,
            flags: u32,
        ) -> *mut c_void;
        fn WinHttpSetTimeouts(
            handle: *mut c_void,
            resolve_ms: i32,
            connect_ms: i32,
            send_ms: i32,
            receive_ms: i32,
        ) -> i32;
        fn WinHttpConnect(
            session: *mut c_void,
            server: *const u16,
            port: u16,
            reserved: u32,
        ) -> *mut c_void;
        fn WinHttpOpenRequest(
            connect: *mut c_void,
            verb: *const u16,
            object_name: *const u16,
            version: *const u16,
            referrer: *const u16,
            accept_types: *const *const u16,
            flags: u32,
        ) -> *mut c_void;
        fn WinHttpSendRequest(
            request: *mut c_void,
            headers: *const u16,
            headers_len: u32,
            optional: *mut c_void,
            optional_len: u32,
            total_len: u32,
            context: usize,
        ) -> i32;
        fn WinHttpReceiveResponse(request: *mut c_void, reserved: *mut c_void) -> i32;
        fn WinHttpQueryHeaders(
            request: *mut c_void,
            info_level: u32,
            name: *const u16,
            buffer: *mut c_void,
            buffer_len: *mut u32,
            index: *mut u32,
        ) -> i32;
        fn WinHttpQueryDataAvailable(request: *mut c_void, bytes_avail: *mut u32) -> i32;
        fn WinHttpReadData(
            request: *mut c_void,
            buffer: *mut u8,
            bytes_to_read: u32,
            bytes_read: *mut u32,
        ) -> i32;
        fn WinHttpCloseHandle(handle: *mut c_void) -> i32;
    }

    struct HInternet(*mut c_void);
    impl Drop for HInternet {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let w_agent = wide("MikuDesktop/1.0");
    let w_host = wide(host);
    let w_verb = wide(verb);
    let w_path = wide(path);

    let mut hdr_block = String::new();
    for (k, v) in headers {
        hdr_block.push_str(&format!("{}: {}\r\n", k, v));
    }
    let w_hdrs = wide(&hdr_block);

    unsafe {
        let session = HInternet(WinHttpOpen(
            w_agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() {
            return Err("WinHttpOpen falló".to_string());
        }
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 20_000, 120_000);

        let connect = HInternet(WinHttpConnect(session.0, w_host.as_ptr(), port, 0));
        if connect.0.is_null() {
            return Err(format!("WinHttpConnect({}) falló", host));
        }

        let request = HInternet(WinHttpOpenRequest(
            connect.0,
            w_verb.as_ptr(),
            w_path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err("WinHttpOpenRequest falló".to_string());
        }

        let body_ptr = if body.is_empty() {
            null_mut()
        } else {
            body.as_ptr() as *mut c_void
        };
        let body_len = u32::try_from(body.len()).unwrap_or(u32::MAX);
        if WinHttpSendRequest(
            request.0,
            w_hdrs.as_ptr(),
            u32::MAX,
            body_ptr,
            body_len,
            body_len,
            0,
        ) == 0
        {
            return Err(format!(
                "WinHttpSendRequest({}) error OS {}",
                host,
                std::io::Error::last_os_error()
            ));
        }
        if WinHttpReceiveResponse(request.0, null_mut()) == 0 {
            return Err(format!(
                "WinHttpReceiveResponse({}) error OS {}",
                host,
                std::io::Error::last_os_error()
            ));
        }

        let mut status: u32 = 0;
        let mut status_size: u32 = std::mem::size_of::<u32>() as u32;
        if WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32) as *mut c_void,
            &mut status_size,
            null_mut(),
        ) == 0
        {
            return Err("WinHttpQueryHeaders status falló".to_string());
        }

        let mut out = Vec::new();
        loop {
            let mut avail: u32 = 0;
            if WinHttpQueryDataAvailable(request.0, &mut avail) == 0 || avail == 0 {
                break;
            }
            let chunk_cap = avail.min(64 * 1024) as usize;
            let mut buf = vec![0u8; chunk_cap];
            let mut read: u32 = 0;
            if WinHttpReadData(request.0, buf.as_mut_ptr(), chunk_cap as u32, &mut read) == 0 {
                break;
            }
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }

        Ok((status as u16, out))
    }
}

#[cfg(not(windows))]
pub(super) fn post_https(
    host: &str,
    _port: u16,
    _path: &str,
    _headers: &[(String, String)],
    _body: &[u8],
) -> Result<(u16, Vec<u8>), String> {
    Err(format!(
        "HTTPS nativo (WinHTTP) solo disponible en Windows ({})",
        host
    ))
}

#[cfg(not(windows))]
pub(super) fn get_https(
    host: &str,
    _port: u16,
    _path: &str,
    _headers: &[(String, String)],
) -> Result<(u16, Vec<u8>), String> {
    Err(format!(
        "HTTPS nativo (WinHTTP) solo disponible en Windows ({})",
        host
    ))
}

pub(super) fn parse_http_url(raw: &str) -> Result<(bool, String, u16, String), String> {
    let trimmed = raw.trim();
    if trimmed.chars().any(|c| c.is_control() || c == ' ') {
        return Err("URL contiene caracteres inválidos".to_string());
    }
    let (is_https, rest) = if let Some(r) = trimmed.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = trimmed.strip_prefix("http://") {
        (false, r)
    } else {
        return Err("URL debe iniciar con https:// o http://".to_string());
    };
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err("URL sin host".to_string());
    }
    let default_port = if is_https { 443 } else { 80 };
    let (host, port) = if let Some(colon) = authority.rfind(':') {
        let p_str = &authority[colon + 1..];
        let port = p_str
            .parse::<u16>()
            .map_err(|_| format!("puerto inválido en URL: {}", p_str))?;
        (&authority[..colon], port)
    } else {
        (authority, default_port)
    };
    Ok((is_https, host.trim_matches(['[', ']']).to_string(), port, path.to_string()))
}

pub(super) fn post_plain_http(
    host: &str,
    port: u16,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<(u16, Vec<u8>), String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let addr = format!("{}:{}", host, port)
        .to_socket_addrs()
        .map_err(|e| format!("DNS/host {}: {}", host, e))?
        .next()
        .ok_or_else(|| format!("sin dirección IP para {}", host))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|e| format!("conexión TCP {}:{}: {}", host, port, e))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));

    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Length: {}\r\nConnection: close\r\n",
        path,
        host,
        port,
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("\r\n");
    stream
        .write_all(req.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|e| format!("escritura HTTP: {}", e))?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut hdr_end_opt: Option<usize> = None;
    let mut expected_len: Option<usize> = None;
    let mut is_chunked = false;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&chunk[..n]);
                if hdr_end_opt.is_none() {
                    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        hdr_end_opt = Some(pos);
                        let head_s = String::from_utf8_lossy(&raw[..pos]).to_ascii_lowercase();
                        is_chunked = head_s.contains("transfer-encoding: chunked");
                        for line in head_s.lines() {
                            if let Some((k, v)) = line.split_once(':') {
                                if k.trim() == "content-length" {
                                    expected_len = v.trim().parse::<usize>().ok();
                                }
                            }
                        }
                    }
                }
                if let (Some(pos), Some(len)) = (hdr_end_opt, expected_len) {
                    if raw.len() >= pos + 4 + len {
                        break;
                    }
                }
                if let Some(pos) = hdr_end_opt {
                    if is_chunked && raw[pos + 4..].windows(5).any(|w| w == b"0\r\n\r\n") {
                        break;
                    }
                }
            }
            Err(e) => {
                if hdr_end_opt.is_some() && !raw.is_empty() {
                    break;
                }
                return Err(format!("lectura HTTP: {}", e));
            }
        }
    }
    let hdr_end = hdr_end_opt.ok_or_else(|| "respuesta HTTP sin cabecera".to_string())?;
    let head = String::from_utf8_lossy(&raw[..hdr_end]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| "código de estado HTTP inválido".to_string())?;
    let resp_body = if is_chunked {
        decode_chunked(&raw[hdr_end + 4..])
    } else if let Some(len) = expected_len {
        let start = hdr_end + 4;
        raw[start..raw.len().min(start + len)].to_vec()
    } else {
        raw[hdr_end + 4..].to_vec()
    };
    Ok((status, resp_body))
}

fn decode_chunked(mut src: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(pos) = src.windows(2).position(|w| w == b"\r\n") {
        let len_line = String::from_utf8_lossy(&src[..pos]);
        let size_hex = len_line.split(';').next().unwrap_or("").trim();
        let Ok(len) = usize::from_str_radix(size_hex, 16) else {
            break;
        };
        if len == 0 {
            break;
        }
        let rest = &src[pos + 2..];
        if rest.len() < len {
            out.extend_from_slice(rest);
            break;
        }
        out.extend_from_slice(&rest[..len]);
        src = rest.get(len + 2..).unwrap_or(b"");
    }
    out
}
