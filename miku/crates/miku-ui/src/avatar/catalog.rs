#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 5. Avatar Asset Catalog Loader & Validator (rig.toml v7 + design-tokens.json + 22 SVGs)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigMeta {
    pub version: u32,
    pub source: String,
    pub viewbox: [u32; 2],
    pub body_size: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigSpringConfig {
    pub stiffness: f64,
    pub damping: f64,
    pub mass: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigSmoothingConfig {
    pub look_and_glow: f64,
    pub mouth: f64,
    pub island_follow: f64,
    pub lean: f64,
    pub lean_chase: f64,
}

impl Default for V7RigSmoothingConfig {
    fn default() -> Self {
        Self {
            look_and_glow: 9.0,
            mouth: 16.0,
            island_follow: 10.0,
            lean: 9.0,
            lean_chase: 13.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigIdleConfig {
    pub breath_amplitude: f64,
    pub breath_omega: f64,
    pub breath_omega_sleeping: f64,
    pub blink_interval: [f64; 2],
    pub blink_duration: f64,
    pub blink_closed_scale_y: f64,
}

impl Default for V7RigIdleConfig {
    fn default() -> Self {
        Self {
            breath_amplitude: 0.014,
            breath_omega: 2.2,
            breath_omega_sleeping: 1.1,
            blink_interval: [2.5, 6.0],
            blink_duration: 0.14,
            blink_closed_scale_y: 0.08,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigPhysics {
    pub sleep_epsilon: f64,
    pub idle_fps: u32,
    pub active_fps: u32,
    pub max_dt: f64,
    #[serde(default)]
    pub springs: BTreeMap<String, V7RigSpringConfig>,
    #[serde(default)]
    pub smoothing: V7RigSmoothingConfig,
    #[serde(default)]
    pub idle: V7RigIdleConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigLayer {
    pub id: String,
    pub z: u32,
    pub svg: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7AccessoryItem {
    pub label: String,
    pub slot: String,
    pub svg: String,
    pub svg_back: Option<String>,
    pub layer: String,
    pub pivot: [f64; 2],
    #[serde(default)]
    pub shift_y: f64,
    #[serde(default)]
    pub origin_in_avatar: [f64; 2],
    pub size: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7AccessoriesSection {
    pub items: BTreeMap<String, V7AccessoryItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigDocument {
    pub meta: V7RigMeta,
    pub physics: V7RigPhysics,
    pub layers: Vec<V7RigLayer>,
    pub accessories: V7AccessoriesSection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7ScaleEntry {
    pub k: f64,
    pub body_px: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7ScaleSection {
    pub idle: V7ScaleEntry,
    pub hover: V7ScaleEntry,
    pub hover_open_scene: V7ScaleEntry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7IslandCompactTokens {
    pub width: u32,
    pub height: u32,
    pub radius_bottom: u32,
    pub avatar_slot: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7IslandExpandedTokens {
    pub width_default: u32,
    pub radius_bottom: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7IslandSection {
    pub background: String,
    pub compact: V7IslandCompactTokens,
    pub expanded: V7IslandExpandedTokens,
    pub native_window_pad: u32,
    pub autoclose_s: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7AvatarTokensSection {
    #[serde(rename = "viewBox")]
    pub viewbox: [u32; 2],
}

impl Default for V7AvatarTokensSection {
    fn default() -> Self {
        Self {
            viewbox: AVATAR_VIEWBOX_SIZE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7DesignTokensDocument {
    #[serde(default)]
    pub avatar: V7AvatarTokensSection,
    pub scale: V7ScaleSection,
    pub island: V7IslandSection,
}

pub type V7DesignTokens = V7DesignTokensDocument;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarCatalogAuditReport {
    pub rig_version: u32,
    pub viewbox: [u32; 2],
    pub idle_fps: u32,
    pub layer_count: usize,
    pub accessory_item_count: usize,
    pub verified_svg_count: usize,
    pub compact_width_px: u32,
    pub compact_height_px: u32,
    pub catalog_sha256: String,
}

pub fn resolve_avatar_assets_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/miku-ui -> crates -> miku -> assets/avatar
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|miku_root| miku_root.join("assets").join("avatar"))
        .unwrap_or_else(|| PathBuf::from("assets/avatar"))
}

pub fn validate_avatar_asset_catalog(
    avatar_dir: &Path,
) -> Result<AvatarCatalogAuditReport, UiError> {
    let rig_path = avatar_dir.join("rig.toml");
    let tokens_path = avatar_dir.join("design-tokens.json");

    let rig_raw = fs::read_to_string(&rig_path)
        .map_err(|e| UiError::AssetCatalogError(format!("read {}: {}", rig_path.display(), e)))?;
    let tokens_raw = fs::read_to_string(&tokens_path).map_err(|e| {
        UiError::AssetCatalogError(format!("read {}: {}", tokens_path.display(), e))
    })?;

    let rig: V7RigDocument = toml::from_str(&rig_raw)
        .map_err(|e| UiError::AssetCatalogError(format!("parse rig.toml: {}", e)))?;
    let tokens: V7DesignTokensDocument = serde_json::from_str(&tokens_raw)
        .map_err(|e| UiError::AssetCatalogError(format!("parse design-tokens.json: {}", e)))?;

    if rig.meta.version != 1 || rig.meta.viewbox != [128, 128] {
        return Err(UiError::AssetCatalogError(
            "rig.toml meta must have version=1 and viewbox=[128, 128]".to_string(),
        ));
    }
    if rig.physics.idle_fps != 0 || (rig.physics.sleep_epsilon - 0.001).abs() > 1e-6 {
        return Err(UiError::AssetCatalogError(
            "rig.toml physics must enforce idle_fps=0 and sleep_epsilon=0.001".to_string(),
        ));
    }
    if rig.layers.len() != 9 {
        return Err(UiError::AssetCatalogError(format!(
            "expected 9 layers in rig.toml, found {}",
            rig.layers.len()
        )));
    }
    if rig.accessories.items.len() != 14 {
        return Err(UiError::AssetCatalogError(format!(
            "expected 14 accessory items in rig.toml, found {}",
            rig.accessories.items.len()
        )));
    }

    if tokens.island.compact.width != IDLE_COMPACT_WIDTH_PX
        || tokens.island.compact.height != IDLE_COMPACT_HEIGHT_PX
    {
        return Err(UiError::AssetCatalogError(format!(
            "unexpected compact island dimensions {}x{}",
            tokens.island.compact.width, tokens.island.compact.height
        )));
    }

    let mut hasher_input = Vec::with_capacity(65_536);
    hasher_input.extend_from_slice(rig_raw.trim().as_bytes());
    hasher_input.push(b'|');
    hasher_input.extend_from_slice(tokens_raw.trim().as_bytes());

    let mut verified_svg_count = 0usize;
    for rel_path in EXPECTED_AVATAR_SVG_REL_PATHS {
        let svg_path = avatar_dir.join(rel_path);
        let svg_raw = fs::read_to_string(&svg_path).map_err(|e| {
            UiError::AssetCatalogError(format!("missing SVG {}: {}", svg_path.display(), e))
        })?;
        if !svg_raw.contains("<svg") || !svg_raw.contains("viewBox=\"0 0 128 128\"") {
            return Err(UiError::AssetCatalogError(format!(
                "SVG '{}' missing <svg> or viewBox=\"0 0 128 128\"",
                rel_path
            )));
        }
        if svg_raw.to_ascii_lowercase().contains("<script") {
            return Err(UiError::AssetCatalogError(format!(
                "SVG '{}' contains forbidden <script> tag",
                rel_path
            )));
        }
        hasher_input.push(b'|');
        hasher_input.extend_from_slice(rel_path.as_bytes());
        verified_svg_count += 1;
    }

    Ok(AvatarCatalogAuditReport {
        rig_version: rig.meta.version,
        viewbox: rig.meta.viewbox,
        idle_fps: rig.physics.idle_fps,
        layer_count: rig.layers.len(),
        accessory_item_count: rig.accessories.items.len(),
        verified_svg_count,
        compact_width_px: tokens.island.compact.width,
        compact_height_px: tokens.island.compact.height,
        catalog_sha256: sha256_hex(&hasher_input),
    })
}

pub fn sha256_hex_bytes(data: &[u8]) -> String {
    sha256_hex(data)
}

