use miku_protocol::{
    constant_time_token_eq, DesignTokens, IslandState, MikuLinkEnvelopeV1, MikuLinkMessageV1,
    PermissionDecisionKind, ProtocolError, RigConfig, DEFAULT_DESIGN_TOKENS_JSON, DEFAULT_RIG_TOML,
    MAX_AUTH_FRAME_BYTES, MAX_FRAME_BYTES, MIKU_PROTOCOL_VERSION_V1,
};

struct Rng64(u64);

impl Rng64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        &slice[(self.next_u64() as usize) % slice.len()]
    }
}

fn seed_envelopes() -> Vec<String> {
    let items = [
        MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "fuzz-1".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Hello {
                client: "android".to_string(),
                token: "tok-123".to_string(),
                capabilities: vec!["chat".to_string()],
            },
        },
        MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "fuzz-2".to_string(),
            reply_to: Some("fuzz-1".to_string()),
            msg: MikuLinkMessageV1::HelloAck {
                session_id: "s-1".to_string(),
                island_state: IslandState::Idle,
            },
        },
        MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "fuzz-3".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Ping { ts: 42 },
        },
        MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "fuzz-4".to_string(),
            reply_to: Some("fuzz-3".to_string()),
            msg: MikuLinkMessageV1::Pong { ts: 42 },
        },
        MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "fuzz-5".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::PermissionDecision {
                req_hash: "sha256:abcd".to_string(),
                decision: PermissionDecisionKind::Allow,
                biometric_verified: true,
            },
        },
        MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "fuzz-6".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Command {
                session_id: "s-1".to_string(),
                action: "prompt".to_string(),
                text: "status".to_string(),
            },
        },
    ];
    items
        .into_iter()
        .map(|e| e.encode_json().expect("seed json"))
        .collect()
}

fn mutate_json_string(base: &str, rng: &mut Rng64, step: usize) -> String {
    let mut bytes = base.as_bytes().to_vec();
    match step % 8 {
        0 => {
            // Flip random byte inside existing frame
            if !bytes.is_empty() {
                let idx = (rng.next_u64() as usize) % bytes.len();
                bytes[idx] ^= ((rng.next_u64() % 254) as u8).saturating_add(1);
            }
        }
        1 => {
            // Truncate arbitrarily at byte boundary
            let cut = (rng.next_u64() as usize) % (bytes.len().max(1));
            bytes.truncate(cut);
        }
        2 => {
            // Version tamper or unknown field injection
            let s = if rng.next_u64().is_multiple_of(2) {
                base.replacen("\"v\":1", "\"v\":99", 1)
            } else {
                base.replacen("\"v\":1", "\"v\":1,\"extra_evil\":true", 1)
            };
            return s;
        }
        3 => {
            // Deep JSON nesting (must fail safely cleanly with zero stack overflow)
            let depth = 64 + (rng.next_u64() % 192) as usize;
            let mut s = String::with_capacity(depth * 2 + 16);
            for _ in 0..depth {
                s.push_str("{\"a\":");
            }
            s.push('1');
            for _ in 0..depth {
                s.push('}');
            }
            return s;
        }
        4 => {
            // Numeric extreme mutations / empty id
            let id_swap = rng.pick(&["\"id\":\"\"", "\"id\":\"   \"", "\"v\":-1", "\"v\":null", "\"ts\":1e999"]);
            return format!("{{{},\"msg\":{{\"type\":\"ping\",\"data\":{{\"ts\":1}}}}}}", id_swap);
        }
        5 => {
            // Raw pseudo-random JSON-like fragments
            let tokens = ["{", "}", "[", "]", "\"v\":", "1", "null", "true", "\"hello\"", "\\u0000", ","];
            let mut s = String::new();
            for _ in 0..((rng.next_u64() % 12) + 1) {
                s.push_str(rng.pick(&tokens));
            }
            return s;
        }
        6 => {
            // Inject control bytes / quotes
            let pos = (rng.next_u64() as usize) % (bytes.len() + 1);
            bytes.insert(pos, (rng.next_u64() % 32) as u8);
        }
        _ => {
            // Leave base valid intermittently to verify Ok invariant too
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn test_5_2_fuzz_miku_protocol_ws_json_100k_inputs_zero_panics() {
    let seeds = seed_envelopes();
    let mut rng = Rng64(0x4D49_4B55_5052_4F54);
    let mut ok_count = 0usize;
    let mut err_count = 0usize;

    for i in 0..100_000usize {
        let base = &seeds[i % seeds.len()];
        let payload = mutate_json_string(base, &mut rng, i);
        match MikuLinkEnvelopeV1::decode_and_validate(&payload) {
            Ok(env) => {
                assert_eq!(env.v, MIKU_PROTOCOL_VERSION_V1);
                assert!(!env.id.trim().is_empty());
                assert!(payload.len() <= MAX_FRAME_BYTES);
                if matches!(
                    env.msg,
                    MikuLinkMessageV1::Hello { .. } | MikuLinkMessageV1::PermissionDecision { .. }
                ) {
                    assert!(payload.len() <= MAX_AUTH_FRAME_BYTES);
                }
                ok_count += 1;
            }
            Err(_) => {
                err_count += 1;
            }
        }
    }

    assert_eq!(ok_count + err_count, 100_000);
    assert!(ok_count > 1_000 && err_count > 50_000);

    // Explicit OOM / frame-cap boundary regression checks
    let big_cmd = format!(
        "{{\"v\":1,\"id\":\"cap-1\",\"msg\":{{\"type\":\"command\",\"data\":{{\"session_id\":\"s\",\"action\":\"p\",\"text\":\"{}\"}}}}}}",
        "x".repeat(MAX_FRAME_BYTES)
    );
    assert!(matches!(
        MikuLinkEnvelopeV1::decode_and_validate(&big_cmd),
        Err(ProtocolError::FrameTooLarge { .. })
    ));

    let big_hello = format!(
        "{{\"v\":1,\"id\":\"cap-2\",\"msg\":{{\"type\":\"hello\",\"data\":{{\"client\":\"m\",\"token\":\"{}\",\"capabilities\":[]}}}}}}",
        "t".repeat(MAX_AUTH_FRAME_BYTES)
    );
    assert!(matches!(
        MikuLinkEnvelopeV1::decode_and_validate(&big_hello),
        Err(ProtocolError::FrameTooLarge { .. })
    ));
}

#[test]
fn test_5_2_fuzz_rig_and_design_tokens_contracts_and_constant_time_tokens() {
    let mut rng = Rng64(0x5249_475F_544F_4B4E);
    for i in 0..2_000usize {
        let rig_mut = match i % 5 {
            0 => DEFAULT_RIG_TOML.replacen("stiffness = ", "stiffness = -10.0 # ", 1),
            1 => DEFAULT_RIG_TOML.replacen("fps_when_asleep = 0", "fps_when_asleep = 60", 1),
            2 => DEFAULT_RIG_TOML.replacen("schema_version = 1", "schema_version = 9", 1),
            3 => format!("{}\nunknown_section = true\n", DEFAULT_RIG_TOML),
            _ => DEFAULT_RIG_TOML[..(DEFAULT_RIG_TOML.len() / ((i % 6) + 2)).max(1)].to_string(),
        };
        assert!(RigConfig::parse_and_validate(&rig_mut).is_err());

        let tok_mut = match i % 4 {
            0 => DEFAULT_DESIGN_TOKENS_JSON.replacen("\"pill_width\"", "\"unknown_field\"", 1),
            1 => DEFAULT_DESIGN_TOKENS_JSON.replacen("\"schema_version\": 1", "\"schema_version\": 2", 1),
            2 => format!("{{\"unknown_top\":true,{}", &DEFAULT_DESIGN_TOKENS_JSON[1..]),
            _ => {
                let trim_len = DEFAULT_DESIGN_TOKENS_JSON.trim_end().len();
                let mut cut = 1 + ((rng.next_u64() as usize) % trim_len.saturating_sub(2).max(1));
                while !DEFAULT_DESIGN_TOKENS_JSON.is_char_boundary(cut) {
                    cut -= 1;
                }
                DEFAULT_DESIGN_TOKENS_JSON[..cut].to_string()
            }
        };
        assert!(DesignTokens::parse_and_validate(&tok_mut).is_err());
    }

    assert!(!constant_time_token_eq("", ""));
    assert!(constant_time_token_eq("miku-tok-123", "miku-tok-123"));
    assert!(!constant_time_token_eq("miku-tok-123", "miku-tok-124"));
}
