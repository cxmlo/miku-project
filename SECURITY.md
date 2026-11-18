# Security Policy · Miku Desktop Orchestrator

Miku Desktop Orchestrator is designed around **deny-by-default isolation**, **minimal attack surface**, **zero hardcoded secrets**, and **verifiable regression defenses** enforced in automated tests and CI.

---

## 1. Threat Model & Implemented Defenses (Section 8)

| Attack Surface | Threat Vector | Implemented Mitigation | Verified In |
| --- | --- | --- | --- |
| **Loopback & Remote WS (`miku-gateway`)** | Cross-process token guessing (timing side-channel) | `miku_protocol::constant_time_token_eq` compares bytes in constant time and rejects empty tokens. | `security_audit_section8` |
| **Loopback Bind Guard (`miku-gateway`)** | Accidental exposure of desktop RPC on LAN/WAN interfaces | `GatewayConfig::loopback(port)` enforces `127.0.0.1` (`NonLoopbackBindForbidden` when `allow_lan_pairing == false`). Remote access uses explicit QR pairing + Tailscale/Cloudflare tunnel with `X25519`/`Ed25519` anti-PII relay sanitization. | `security_audit_section8`, `miku-gateway` |
| **API Keys & Secret Storage (`secrets_cmd` / DPAPI)** | Secrets leaked in argv, shell history, echo terminal, logs, or unencrypted disk files | `miku secret set` rejects CLI value arguments (`--value`), fails closed on non-TTY / failed `SetConsoleMode` echo suppression, scrubs buffers on `Drop` (`Zeroizing`), and uses OS credential protection (Windows DPAPI). | `secrets_cmd::tests`, `security_audit_section8` |
| **Diagnostics & Log Redaction** | Secret keys (`sk-*`, `ghp_*`, `github_pat_*`, `gsk_*`, `Bearer`), voice transcripts, raw PCM audio, or OS paths written to logs/UI | `redact_sensitive_text` and `redact_sensitive_voice_diagnostic_with_secrets` strip tokens, transcripts, PCM arrays, and OS paths before logs or IPC leave local subsystems. | `security_audit_section8` |
| **WebView2 Island & XSS (`miku-ui`)** | Malicious prompt injection (`<img onerror=...>`, `<script>`) or untrusted remote URLs running in the Island | `WebView2SecurityPolicy::release_hardened()` disables DevTools, context menus, status bar, zoom control, and host objects; locks origin to `https://miku.local/*` (`validate_ipc_sender_origin`); CSP enforces `default-src 'self'; script-src 'self'; connect-src 'none'; object-src 'none'`; DOM uses `textContent` (`sanitize_for_text_content`). Isolated user-data directory under `%LOCALAPPDATA%/miku/webview2`. | `tests_overview_ui_v7`, `security_audit_section8` |
| **Gatekeeper & Path Traversal (`miku-gatekeeper`)** | `../`, UNC shares (`\\server\share`), NTFS Alternate Data Streams (`file:stream`), Windows DOS devices (`CON`, `NUL`, `COM1`), or TOCTOU target/arg swap | `CanonicalPathJail` confines all `FsRead`/`FsWrite` paths and rejects UNC, ADS (in both targets and roots), DOS devices, and null/control bytes. Strict `Deny > Ask > Allow` precedence; cryptographic SHA-256 `req_hash` ticket binding (`ToctouHashMismatch` on mutation); TTL expiration; tamper-evident SHA-256 audit log chain. | `fuzz_toml_and_jail_100k`, `security_audit_section8` |
| **Mobile Miku-Link Approvals** | Forging `Allow` on high-risk desktop actions from mobile without biometric verification or bypassing PC-only rules | `GatekeeperEngine::resolve_permission_decision` rejects mobile `Allow` without `biometric_verified = true` (`BiometricRequiredOnMobile`) and blocks `pc_only` capabilities even with biometrics (`PcOnlyFromMobileForbidden`). | `security_audit_section8`, `miku-mobile-core` |
| **Global Kill Switch** | Runaway agent, voice capture, audio ducking, or remote tunnel session | Immediate revocation of active permissions, zeroing of voice capture PCM ring, volume restoration, remote away-mode shutdown, and `DenyReason::KillSwitchActive`. | `tests_unified_e2e`, `security_audit_section8` |
| **IDE Hook Integration (`miku-agent` / `hook_cmd`)** | Slow/dead Miku daemon stalling user's IDE or corrupting `~/.claude/settings.json` / `.cursor/hooks.json` | `execute_hook_with_fail_open` fails open under `CLAUDE_HOOK_FAIL_OPEN_TIMEOUT_MS` (`15 ms`). Installer forbids symlinks/reparse points, validates JSON syntax with exact line/column diagnostics, writes a verified `.bak` backup before atomic updates, and rolls back cleanly on user cancellation. | `hook_cmd::tests`, `security_audit_section8` |
| **Supply Chain & Dependencies** | Vulnerable, yanked, wildcard, or copyleft dependencies | Root + workspace `LICENSE` enforce 100% MIT (`15/15` crates). `miku/deny.toml` enforces `yanked = "deny"`, permissive SPDX allowlist (`copyleft = 0`), `wildcards = "deny"`, and `crates.io-index` only. `.github/workflows/ci.yml` executes `cargo audit`, `cargo deny`, `cargo clippy -- -D warnings`, `miku-license-audit`, and `miku.exe ci-bench`. | `miku-license-audit`, `security_audit_section8` |

---

## 2. Verification Commands (Windows PowerShell)

```powershell
# Run full Section 8 security regression suite + fuzz tests + license audit
cargo test --manifest-path miku/Cargo.toml -p miku-cli --test security_audit_section8
cargo run --manifest-path miku/Cargo.toml -p miku-cli --bin miku-license-audit
.\miku\target\release\miku.exe doctor --json
```

In CI (`.github/workflows/ci.yml` on `windows-latest`), dependency advisories and policy rules are additionally audited via:
```powershell
cargo audit --file miku/Cargo.lock
cargo deny --manifest-path miku/Cargo.toml check advisories bans licenses sources
```

---

## 3. Supported Versions & Reporting a Vulnerability

| Version | Supported |
| --- | --- |
| `0.1.0-pre` / `1.0.x` (`main`) | Supported |
| Pre-Phase-4 spikes (`miku/spikes/`) | Experimental reference only |

If you discover a security vulnerability, privilege bypass, path-jail escape, or secret exposure:

1. **Do not open a public issue with exploit payloads or credentials.**
2. Report privately to the maintainers via GitHub Private Vulnerability Reporting / Security Advisories on the repository with:
   - Affected crate, function, or CLI subcommand.
   - Minimal reproduction steps or input payload.
   - Observed vs. expected security invariant.
3. Any confirmed reproduction is converted into a permanent regression test in `miku/crates/miku-cli/tests/security_audit_section8.rs` before releasing a patched binary with updated SHA-256 release digests.
