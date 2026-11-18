//! Modular customizable UI resolver (`miku/assets/ui/` + `%LOCALAPPDATA%/Miku/ui/`)
//! and strict CSP / decoupling validator (`Step 6.1`).

pub mod resolver;

#[cfg(test)]
mod tests;

pub use self::resolver::{
    resolve_custom_ui_base_dir, resolve_user_ui_override_dir, validate_custom_ui_tree,
    CustomUiResolver, CustomUiTreeReport, ResolvedUiAsset, REQUIRED_CUSTOM_UI_FILES,
};
