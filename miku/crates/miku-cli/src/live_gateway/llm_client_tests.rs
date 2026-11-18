use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

#[test]
fn test_7_4_llm_client_executes_real_http_request_against_mock_server() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();

    let server = thread::spawn(move || {
        for idx in 0..3 {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
            let mut req_buf = Vec::new();
            let mut chunk = [0u8; 2048];
            let mut hdr_end = None;
            let mut clen = 0usize;
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        req_buf.extend_from_slice(&chunk[..n]);
                        if hdr_end.is_none() {
                            if let Some(pos) = req_buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                hdr_end = Some(pos);
                                for line in String::from_utf8_lossy(&req_buf[..pos]).lines() {
                                    if let Some((k, v)) = line.split_once(':') {
                                        if k.trim().eq_ignore_ascii_case("content-length") {
                                            clen = v.trim().parse().unwrap_or(0);
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(pos) = hdr_end {
                            if req_buf.len() >= pos + 4 + clen {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let req_str = String::from_utf8_lossy(&req_buf).to_string();
            let resp = match idx {
                0 => {
                    assert!(req_str.contains("Authorization: Bearer sk-mock-openai-123"));
                    let body = json!({
                        "choices": [{ "message": { "content": "Respuesta real OpenAI" } }],
                        "usage": { "prompt_tokens": 18, "completion_tokens": 9 }
                    })
                    .to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                }
                1 => {
                    assert!(req_str.contains("x-api-key: sk-ant-mock-999"));
                    let body = json!({
                        "content": [{ "type": "text", "text": "Respuesta real Anthropic" }],
                        "usage": { "input_tokens": 11, "output_tokens": 6 }
                    })
                    .to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                }
                _ => {
                    let body = r#"{"error":{"message":"invalid key sk-secret-leak-check"}}"#;
                    format!(
                        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                }
            };
            stream.write_all(resp.as_bytes()).expect("write resp");
        }
    });

    let r1 = execute_llm_chat(&LlmRequestConfig {
        provider: "openai".to_string(),
        model: "gpt-4o-mini".to_string(),
        api_key: "sk-mock-openai-123".to_string(),
        base_url: Some(format!("http://127.0.0.1:{}", port)),
        prompt: "hola".to_string(),
        system_prompt: "sys".to_string(),
        attachment: None,
    })
    .expect("openai mock call");
    assert_eq!(r1.text, "Respuesta real OpenAI");
    assert_eq!(r1.input_tokens, Some(18));
    assert_eq!(r1.output_tokens, Some(9));

    let r2 = execute_llm_chat(&LlmRequestConfig {
        provider: "anthropic".to_string(),
        model: "claude-3-5-sonnet".to_string(),
        api_key: "sk-ant-mock-999".to_string(),
        base_url: Some(format!("http://127.0.0.1:{}", port)),
        prompt: "hola".to_string(),
        system_prompt: "sys".to_string(),
        attachment: None,
    })
    .expect("anthropic mock call");
    assert_eq!(r2.text, "Respuesta real Anthropic");

    let r3 = execute_llm_chat(&LlmRequestConfig {
        provider: "openai".to_string(),
        model: "gpt-4o-mini".to_string(),
        api_key: "sk-secret-leak-check".to_string(),
        base_url: Some(format!("http://127.0.0.1:{}", port)),
        prompt: "hola".to_string(),
        system_prompt: "sys".to_string(),
        attachment: None,
    })
    .expect_err("must return 401 error");
    assert_eq!(r3.status_code, Some(401));
    assert!(!r3.message.contains("sk-secret-leak-check"));
    assert!(r3.message.contains("[REDACTED]"));

    server.join().expect("server thread");
}

#[test]
fn multimodal_payload_and_deepseek_ocr_fallback_work() {
    use miku_protocol::CaptureTransform;

    let mut att = LlmImageAttachment {
        mime_type: "image/jpeg".to_string(),
        base64_data: "AQIDBA==".to_string(),
        ocr_text: Some("Boton Guardar\nArchivo Config".to_string()),
        uia_summary: Some("[Button] Guardar @(500,320)".to_string()),
        window_title: "Editor".to_string(),
        width: 1024,
        height: 576,
        transform: CaptureTransform {
            origin_virtual_x: 0,
            origin_virtual_y: 0,
            width_px: 1920,
            height_px: 1080,
            dpi_scale: 1.25,
        },
    };

    let vision_cfg = LlmRequestConfig {
        provider: "openai".to_string(),
        model: "gpt-4o-mini".to_string(),
        api_key: "k".to_string(),
        base_url: None,
        prompt: "Describe".to_string(),
        system_prompt: "sys".to_string(),
        attachment: Some(att.clone()),
    };
    let (_url, _hdrs, vision_body) = build_provider_request_with_context(&vision_cfg, &[], 256);
    let user_parts = &vision_body["messages"][1]["content"];
    assert!(user_parts.is_array());
    assert_eq!(user_parts[1]["type"], "image_url");

    let deepseek_cfg = LlmRequestConfig {
        provider: "deepseek".to_string(),
        model: "deepseek-chat".to_string(),
        api_key: "k".to_string(),
        base_url: None,
        prompt: "Describe".to_string(),
        system_prompt: "sys".to_string(),
        attachment: Some(att.clone()),
    };
    let (_u2, _h2, text_body) = build_provider_request_with_context(&deepseek_cfg, &[], 256);
    let text_content = text_body["messages"][1]["content"].as_str().unwrap_or("");
    assert!(text_content.contains("[OCR Windows]"));
    assert!(text_content.contains("Boton Guardar"));
    assert!(text_content.contains("[Árbol UIA Acotado]"));

    let anth_cfg = LlmRequestConfig {
        provider: "anthropic".to_string(),
        model: "claude-3-5-sonnet".to_string(),
        api_key: "k".to_string(),
        base_url: None,
        prompt: "Describe".to_string(),
        system_prompt: "sys".to_string(),
        attachment: Some(att.clone()),
    };
    let (_u3, _h3, anth_body) = build_provider_request_with_context(&anth_cfg, &[], 256);
    assert_eq!(anth_body["messages"][0]["content"][0]["type"], "image");

    att.zeroize();
    assert!(att.base64_data.is_empty());
}
