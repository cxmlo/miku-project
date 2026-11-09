# Skill: Repository Status Inspector (`repo-status-inspector`)

## Purpose
Inspects `Cargo.toml` and workspace crate manifests in read-only mode (`fs_read`) confined within the project `CanonicalPathJail`.

## Security Contract
- **Declared Capabilities**: `["fs_read"]`
- **Network / Shell / Write**: Strictly forbidden (`UndeclaredSkillCapability`).
- **Execution Limits**: 2,000 ms timeout, 32 KiB max output, isolated runner.
