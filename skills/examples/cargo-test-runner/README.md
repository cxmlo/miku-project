# Skill: Bounded Cargo Test Runner (`cargo-test-runner`)

## Purpose
Runs bounded `cargo test` and `cargo check` invocations under `miku-gatekeeper` verification.

## Security Contract
- **Declared Capabilities**: `["fs_read", "shell"]`
- **Network / Write**: Strictly forbidden (`UndeclaredSkillCapability`).
- **Gatekeeper Verification**: Every `shell` target is evaluated against the active `ProjectProfile` before execution (`Deny > Ask > Allow`).
- **Execution Limits**: 5,000 ms timeout, 64 KiB max output, isolated runner.
