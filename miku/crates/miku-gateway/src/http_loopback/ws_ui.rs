use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

use crate::{compute_websocket_accept_key, GatewayError, WsFrame, MAX_FRAME_BYTES};
use super::LoopbackContext;

async fn read_frame_from_half(
    rd: &mut OwnedReadHalf,
    max_payload_bytes: usize,
) -> Result<WsFrame, GatewayError> {
    let mut hdr = [0u8; 2];
    match rd.read_exact(&mut hdr).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(GatewayError::ConnectionClosed);
        }
        Err(e) => return Err(GatewayError::IoError(e.to_string())),
    }

    let opcode = hdr[0] & 0x0F;
    let masked = (hdr[1] & 0x80) != 0;
    if !masked {
        return Err(GatewayError::WebSocketProtocol(
            "RFC 6455 client frames must be masked".into(),
        ));
    }

    let len_code = (hdr[1] & 0x7F) as usize;
    let payload_len = if len_code <= 125 {
        len_code
    } else if len_code == 126 {
        let mut ext = [0u8; 2];
        rd.read_exact(&mut ext)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        u16::from_be_bytes(ext) as usize
    } else {
        let mut ext = [0u8; 8];
        rd.read_exact(&mut ext)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        usize::try_from(u64::from_be_bytes(ext)).unwrap_or(usize::MAX)
    };

    let mut mask_key = [0u8; 4];
    rd.read_exact(&mut mask_key)
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))?;

    if payload_len > max_payload_bytes {
        return Err(GatewayError::FrameTooLarge {
            declared_bytes: payload_len,
            max_bytes: max_payload_bytes,
        });
    }

    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        rd.read_exact(&mut payload)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        for i in 0..payload_len {
            payload[i] ^= mask_key[i & 3];
        }
    }

    Ok(WsFrame { opcode, payload })
}

async fn write_frame_to_half(
    wr: &mut OwnedWriteHalf,
    opcode: u8,
    payload: &[u8],
) -> Result<(), GatewayError> {
    let mut header = Vec::with_capacity(10);
    header.push(0x80 | (opcode & 0x0F));
    let len = payload.len();
    if len <= 125 {
        header.push(len as u8);
    } else if len <= 65_535 {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    wr.write_all(&header)
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))?;
    if !payload.is_empty() {
        wr.write_all(payload)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
    }
    wr.flush()
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))
}

pub async fn handle_ui_ws_connection(
    mut stream: TcpStream,
    ws_key: &str,
    ctx: Arc<LoopbackContext>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), GatewayError> {
    let accept_val = compute_websocket_accept_key(ws_key);
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_val
    );
    stream
        .write_all(resp.as_bytes())
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))?;
    stream
        .flush()
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))?;

    let (mut rd, mut wr) = stream.into_split();

    let init_ws = serde_json::to_string(&json!({ "type": "ws", "ok": true })).unwrap_or_default();
    write_frame_to_half(&mut wr, 0x1, init_ws.as_bytes()).await?;
    let sync_ev = serde_json::to_string(&json!({ "type": "state_sync" })).unwrap_or_default();
    write_frame_to_half(&mut wr, 0x1, sync_ev.as_bytes()).await?;

    let (in_tx, mut in_rx) = mpsc::channel::<WsFrame>(32);
    let reader_task = tokio::task::spawn(async move {
        while let Ok(frame) = read_frame_from_half(&mut rd, MAX_FRAME_BYTES).await {
            let is_close = frame.opcode == 0x8;
            if in_tx.send(frame).await.is_err() || is_close {
                break;
            }
        }
    });

    let mut events_rx = ctx.handler.subscribe_events();
    let (bg_reply_tx, mut bg_reply_rx) = tokio::sync::mpsc::channel::<Value>(32);

    loop {
        if *shutdown_rx.borrow() {
            let _ = write_frame_to_half(&mut wr, 0x8, &1001u16.to_be_bytes()).await;
            break;
        }

        tokio::select! {
            biased;

            changed = shutdown_rx.changed() => {
                if let Ok(()) = changed {
                    if *shutdown_rx.borrow() {
                        let _ = write_frame_to_half(&mut wr, 0x8, &1001u16.to_be_bytes()).await;
                        break;
                    }
                }
            }

            ev = async {
                match events_rx.as_mut() {
                    Some(rx) => rx.recv().await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                if let Some(val) = ev {
                    if let Ok(txt) = serde_json::to_string(&val) {
                        if write_frame_to_half(&mut wr, 0x1, txt.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
            }

            bg_reply = bg_reply_rx.recv() => {
                if let Some(reply) = bg_reply {
                    if let Ok(out) = serde_json::to_string(&reply) {
                        if write_frame_to_half(&mut wr, 0x1, out.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
            }

            maybe_frame = in_rx.recv() => {
                let Some(frame) = maybe_frame else { break; };
                match frame.opcode {
                    0x1 => {
                        if let Ok(text) = String::from_utf8(frame.payload) {
                            if let Ok(cmd) = serde_json::from_str::<Value>(&text) {
                                let handler = Arc::clone(&ctx.handler);
                                let is_chat = cmd.get("type").and_then(|v| v.as_str()) == Some("chat");
                                if is_chat {
                                    let reply_tx = bg_reply_tx.clone();
                                    tokio::task::spawn_blocking(move || {
                                        if let Some(reply) = handler.handle_ws_command(&cmd) {
                                            let _ = reply_tx.blocking_send(reply);
                                        }
                                    });
                                } else {
                                    let reply_opt = tokio::task::spawn_blocking(move || {
                                        handler.handle_ws_command(&cmd)
                                    })
                                    .await
                                    .ok()
                                    .flatten();
                                    if let Some(reply) = reply_opt {
                                        if let Ok(out) = serde_json::to_string(&reply) {
                                            if write_frame_to_half(&mut wr, 0x1, out.as_bytes()).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    0x9 => {
                        if write_frame_to_half(&mut wr, 0xA, &frame.payload).await.is_err() {
                            break;
                        }
                    }
                    0x8 => {
                        let _ = write_frame_to_half(&mut wr, 0x8, &1000u16.to_be_bytes()).await;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    reader_task.abort();
    Ok(())
}
