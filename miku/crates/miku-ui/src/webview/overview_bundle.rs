#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 9. Step 2.4: Expanded Overview Scene / Embedded Local Shell at
//    https://miku.local/index.html with Reactive Procedural Avatar (F-01 / F-02 / ADR-001)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarMood {
    Idle,
    Listening,
    Thinking,
    Talking,
    Working,
    Happy,
    Focused,
    Sleepy,
    Annoyed,
    Love,
    Wink,
    Surprised,
    Sleeping,
    Error,
    Alert,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarExpressionPose {
    pub mood: AvatarMood,
    pub eyes_variant: String,
    pub mouth_variant: String,
    pub glow_hex: String,
    pub glow_gain: f64,
    pub blush_visible: bool,
    pub brows_visible: bool,
}

impl AvatarExpressionPose {
    pub fn for_mood(mood: AvatarMood) -> Self {
        match mood {
            AvatarMood::Idle => Self {
                mood,
                eyes_variant: "o".to_string(),
                mouth_variant: "dot".to_string(),
                glow_hex: "#BDDEFF".to_string(),
                glow_gain: 1.0,
                blush_visible: false,
                brows_visible: false,
            },
            AvatarMood::Listening => Self {
                mood,
                eyes_variant: "o".to_string(),
                mouth_variant: "O".to_string(),
                glow_hex: "#BDDEFF".to_string(),
                glow_gain: 1.5,
                blush_visible: false,
                brows_visible: false,
            },
            AvatarMood::Thinking => Self {
                mood,
                eyes_variant: "o".to_string(),
                mouth_variant: "dot".to_string(),
                glow_hex: "#E2D8FF".to_string(),
                glow_gain: 1.3,
                blush_visible: false,
                brows_visible: false,
            },
            AvatarMood::Talking => Self {
                mood,
                eyes_variant: "o".to_string(),
                mouth_variant: "talk".to_string(),
                glow_hex: "#BDDEFF".to_string(),
                glow_gain: 1.0,
                blush_visible: false,
                brows_visible: false,
            },
            AvatarMood::Working | AvatarMood::Focused => Self {
                mood,
                eyes_variant: "o".to_string(),
                mouth_variant: "frown".to_string(),
                glow_hex: "#BDDEFF".to_string(),
                glow_gain: 1.0,
                blush_visible: false,
                brows_visible: true,
            },
            AvatarMood::Happy => Self {
                mood,
                eyes_variant: "arc".to_string(),
                mouth_variant: "smile".to_string(),
                glow_hex: "#BDDEFF".to_string(),
                glow_gain: 1.0,
                blush_visible: true,
                brows_visible: false,
            },
            AvatarMood::Love => Self {
                mood,
                eyes_variant: "arc".to_string(),
                mouth_variant: "smile".to_string(),
                glow_hex: "#FFD2E8".to_string(),
                glow_gain: 1.35,
                blush_visible: true,
                brows_visible: false,
            },
            AvatarMood::Wink => Self {
                mood,
                eyes_variant: "wink".to_string(),
                mouth_variant: "smile".to_string(),
                glow_hex: "#BDDEFF".to_string(),
                glow_gain: 1.15,
                blush_visible: true,
                brows_visible: false,
            },
            AvatarMood::Surprised => Self {
                mood,
                eyes_variant: "o".to_string(),
                mouth_variant: "O".to_string(),
                glow_hex: "#E2D8FF".to_string(),
                glow_gain: 1.45,
                blush_visible: true,
                brows_visible: true,
            },
            AvatarMood::Annoyed => Self {
                mood,
                eyes_variant: "ang".to_string(),
                mouth_variant: "grr".to_string(),
                glow_hex: "#FFC9C6".to_string(),
                glow_gain: 1.4,
                blush_visible: true,
                brows_visible: true,
            },
            AvatarMood::Sleeping | AvatarMood::Sleepy => Self {
                mood,
                eyes_variant: "line".to_string(),
                mouth_variant: "none".to_string(),
                glow_hex: "#BDDEFF".to_string(),
                glow_gain: 0.5,
                blush_visible: false,
                brows_visible: false,
            },
            AvatarMood::Error => Self {
                mood,
                eyes_variant: "x".to_string(),
                mouth_variant: "frown".to_string(),
                glow_hex: "#FFD2CF".to_string(),
                glow_gain: 1.0,
                blush_visible: false,
                brows_visible: true,
            },
            AvatarMood::Alert => Self {
                mood,
                eyes_variant: "x".to_string(),
                mouth_variant: "frown".to_string(),
                glow_hex: KILL_SWITCH_ALERT_COLOR_HEX.to_string(),
                glow_gain: 1.5,
                blush_visible: false,
                brows_visible: true,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessorySlotTransform {
    pub item_id: String,
    pub label: String,
    pub slot: String,
    pub slot_anchor: [f64; 2],
    pub pivot: [f64; 2],
    pub shift_y: f64,
    pub origin_in_avatar: [f64; 2],
    pub size: [f64; 2],
    pub follow_spring: bool,
    pub parallax_look: bool,
    pub mounts_back_layer_z10: bool,
    pub mounts_front_layer_z70: bool,
    pub back_svg: Option<String>,
    pub front_svg: Option<String>,
}

/// Computes the exact coordinate transform and Z-layer mounting for an accessory in the
/// `128x128` avatar `viewBox` according to `miku/assets/avatar/rig.toml`.
pub fn compute_accessory_slot_transform(
    item_id: &str,
    item: &V7AccessoryItem,
) -> Result<AccessorySlotTransform, UiError> {
    let (slot_anchor, follow_spring, parallax_look) = match item.slot.as_str() {
        "head" => (AVATAR_HEAD_TOP_ANCHOR, true, false),
        "face" => (AVATAR_EYES_CENTER_ANCHOR, false, true),
        "neck" => (AVATAR_NECK_ANCHOR, true, false),
        other => {
            return Err(UiError::AssetCatalogError(format!(
                "unknown accessory slot '{}' for item '{}'",
                other, item_id
            )))
        }
    };

    let (mounts_back_layer_z10, mounts_front_layer_z70, back_svg, front_svg) =
        match item.layer.as_str() {
            "front" => (false, true, None, Some(item.svg.clone())),
            "back" => (true, false, Some(item.svg.clone()), None),
            "front+back" => (
                true,
                true,
                item.svg_back.clone(),
                Some(item.svg.clone()),
            ),
            other => {
                return Err(UiError::AssetCatalogError(format!(
                    "unknown accessory layer mode '{}' for item '{}'",
                    other, item_id
                )))
            }
        };

    let computed_origin = [0.0, -item.shift_y];
    if (item.origin_in_avatar[0] - computed_origin[0]).abs() > 1e-4
        || (item.origin_in_avatar[1] - computed_origin[1]).abs() > 1e-4
    {
        return Err(UiError::AssetCatalogError(format!(
            "accessory '{}' origin_in_avatar {:?} does not match [0.0, -shift_y] ({:?})",
            item_id, item.origin_in_avatar, computed_origin
        )));
    }

    Ok(AccessorySlotTransform {
        item_id: item_id.to_string(),
        label: item.label.clone(),
        slot: item.slot.clone(),
        slot_anchor,
        pivot: item.pivot,
        shift_y: item.shift_y,
        origin_in_avatar: computed_origin,
        size: item.size,
        follow_spring,
        parallax_look,
        mounts_back_layer_z10,
        mounts_front_layer_z70,
        back_svg,
        front_svg,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarLayerComposeNode {
    pub id: String,
    pub z: u32,
    pub svg_source: String,
    pub visible: bool,
    pub mounted_accessories: Vec<AccessorySlotTransform>,
}

pub type AvatarLayerNode = AvatarLayerComposeNode;

/// Composes the 9 Z-index ordered layers of `rig.toml` (`z = 10..100`) in the `128x128` viewBox,
/// mounting any selected accessories onto `z=10 (accessory_back)` and `z=70 (accessory_front)`.
pub fn compose_avatar_layer_stack(
    avatar_dir: &Path,
    mood: AvatarMood,
    active_accessories: &[&str],
) -> Result<(Vec<AvatarLayerComposeNode>, BTreeMap<String, AccessorySlotTransform>), UiError> {
    let rig_path = avatar_dir.join("rig.toml");
    let rig_raw = fs::read_to_string(&rig_path)
        .map_err(|e| UiError::AssetCatalogError(format!("read {}: {}", rig_path.display(), e)))?;
    let rig: V7RigDocument = toml::from_str(&rig_raw)
        .map_err(|e| UiError::AssetCatalogError(format!("parse rig.toml: {}", e)))?;

    compose_avatar_layer_stack_from_rig(&rig, mood, active_accessories)
}

/// Composes the 9 Z-index ordered layers directly from an already-validated in-memory `V7RigDocument`
/// with zero filesystem I/O or TOML re-parsing.
pub fn compose_avatar_layer_stack_from_rig(
    rig: &V7RigDocument,
    mood: AvatarMood,
    active_accessories: &[&str],
) -> Result<(Vec<AvatarLayerComposeNode>, BTreeMap<String, AccessorySlotTransform>), UiError> {
    let mut all_transforms = BTreeMap::new();
    for (id, item) in &rig.accessories.items {
        let tr = compute_accessory_slot_transform(id, item)?;
        all_transforms.insert(id.clone(), tr);
    }

    let pose = AvatarExpressionPose::for_mood(mood);
    let mut back_mounted = Vec::new();
    let mut front_mounted = Vec::new();

    for acc_id in active_accessories {
        let tr = all_transforms.get(*acc_id).ok_or_else(|| {
            UiError::AssetCatalogError(format!("unknown accessory '{}' in active list", acc_id))
        })?;
        if tr.mounts_back_layer_z10 {
            back_mounted.push(tr.clone());
        }
        if tr.mounts_front_layer_z70 {
            front_mounted.push(tr.clone());
        }
    }

    let mut raw_layers = rig.layers.clone();
    raw_layers.sort_by_key(|l| l.z);

    let mut nodes = Vec::with_capacity(raw_layers.len());
    for layer in raw_layers {
        let (visible, mounted_accessories) = match layer.id.as_str() {
            "accessory_back" => (!back_mounted.is_empty(), back_mounted.clone()),
            "blush" => (pose.blush_visible, Vec::new()),
            "brows" => (pose.brows_visible, Vec::new()),
            "accessory_front" => (!front_mounted.is_empty(), front_mounted.clone()),
            _ => (true, Vec::new()),
        };
        nodes.push(AvatarLayerComposeNode {
            id: layer.id,
            z: layer.z,
            svg_source: layer.svg,
            visible,
            mounted_accessories,
        });
    }

    Ok((nodes, all_transforms))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverviewSceneBundle {
    pub virtual_url: String,
    pub html_content: String,
    pub byte_len: usize,
    pub csp_header: String,
    pub expanded_width_px: u32,
    pub expanded_height_px: u32,
    pub corner_radius_px: u32,
    pub avatar_scale_k: f64,
    pub avatar_body_px: f64,
    pub viewbox: [u32; 2],
    pub layer_count: usize,
    pub accessory_count: usize,
    pub uses_only_text_content: bool,
    pub has_external_cdn_links: bool,
    pub bundle_sha256: String,
}

impl OverviewSceneBundle {
    pub fn validate_security_and_budget(&self) -> Result<(), UiError> {
        if self.virtual_url != OVERVIEW_VIRTUAL_INDEX_URL {
            return Err(UiError::UnsafeNavigationBlocked(format!(
                "overview scene must be served at '{}', got '{}'",
                OVERVIEW_VIRTUAL_INDEX_URL, self.virtual_url
            )));
        }
        if self.byte_len == 0 || self.byte_len >= MAX_OVERVIEW_BUNDLE_BYTES {
            return Err(UiError::AssetCatalogError(format!(
                "overview bundle size {} bytes exceeds max {} bytes (< 150 KB)",
                self.byte_len, MAX_OVERVIEW_BUNDLE_BYTES
            )));
        }
        validate_webview_csp_policy(&self.csp_header)?;
        let lower = self.html_content.to_ascii_lowercase();
        for forbidden in ["innerhtml", "outerhtml", "document.write", "eval(", "http://"] {
            if lower.contains(forbidden) {
                return Err(UiError::CspViolation(format!(
                    "overview bundle contains forbidden pattern '{}'",
                    forbidden
                )));
            }
        }
        if !self.html_content.contains("textContent") {
            return Err(UiError::CspViolation(
                "overview bundle must render dynamic text strictly via textContent".to_string(),
            ));
        }
        if self.has_external_cdn_links {
            return Err(UiError::CspViolation(
                "overview bundle must not reference external CDNs".to_string(),
            ));
        }
        Ok(())
    }
}

/// Builds the self-contained HTML/CSS/JS shell for the expanded `overview` scene served
/// exclusively under `https://miku.local/index.html` (< 150 KB, zero external dependencies,
/// strict `MIKU_WEBVIEW_CSP_HEADER`, and `textContent`-only DOM bindings).
pub fn build_overview_scene_bundle(avatar_dir: &Path) -> Result<OverviewSceneBundle, UiError> {
    let catalog = validate_avatar_asset_catalog(avatar_dir)?;
    let tokens_path = avatar_dir.join("design-tokens.json");
    let tokens_raw = fs::read_to_string(&tokens_path).map_err(|e| {
        UiError::AssetCatalogError(format!("read {}: {}", tokens_path.display(), e))
    })?;
    let tokens: V7DesignTokensDocument = serde_json::from_str(&tokens_raw)
        .map_err(|e| UiError::AssetCatalogError(format!("parse design-tokens.json: {}", e)))?;

    let (layers, accessories) =
        compose_avatar_layer_stack(avatar_dir, AvatarMood::Idle, &["crown", "scarf"])?;

    let mut layers_markup = String::with_capacity(1024);
    for layer in &layers {
        layers_markup.push_str(&format!(
            "<g id=\"miku-layer-{}\" data-z=\"{}\" data-svg=\"{}\" data-visible=\"{}\"></g>",
            layer.id, layer.z, layer.svg_source, layer.visible
        ));
    }

    let html_content = format!(
        r#"<!DOCTYPE html>
<html lang="es">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="{csp}">
<title>Miku Island Overview</title>
<style>
:root {{
  --s0: {bg};
  --s1: #0E0F11;
  --card: #17181C;
  --line: #24262C;
  --text-primary: #F5F5F7;
  --text-muted: #8E919A;
  --accent-blue: #3B9EFF;
  --accent-ok: #30D158;
  --accent-err: #FF453A;
  --island-w: {w}px;
  --island-h: {h}px;
  --island-radius: {r}px;
  --avatar-k: {k:.2};
  --avatar-body-px: {body_px:.1}px;
}}
* {{ box-sizing: border-box; margin: 0; padding: 0; user-select: none; }}
body {{
  width: var(--island-w);
  height: var(--island-h);
  background: var(--s0);
  color: var(--text-primary);
  font-family: 'Segoe UI Variable', 'Segoe UI', system-ui, sans-serif;
  border-bottom-left-radius: var(--island-radius);
  border-bottom-right-radius: var(--island-radius);
  overflow: hidden;
  display: grid;
  grid-template-columns: 168px 1fr;
  gap: 16px;
  padding: 18px 22px;
}}
#miku-avatar-stage {{
  background: var(--s1);
  border: 1px solid var(--line);
  border-radius: 24px;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
}}
#miku-overview-panel {{
  background: var(--card);
  border: 1px solid var(--line);
  border-radius: 24px;
  padding: 16px;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
}}
</style>
</head>
<body data-scene="overview" data-origin="https://miku.local/index.html">
  <section id="miku-avatar-stage">
    <svg id="miku-avatar-svg" viewBox="0 0 128 128" width="128" height="128">{layers}</svg>
    <span id="miku-avatar-mood-label">idle</span>
  </section>
  <section id="miku-overview-panel">
    <header>
      <h1 id="miku-overview-title">Miku Orchestrator</h1>
      <p id="miku-overview-status">Ready on https://miku.local</p>
    </header>
    <div id="miku-overview-metrics">
      <span id="miku-overview-ram-badge">RAM <= 30 MB</span>
      <span id="miku-overview-ipc-badge">IPC RTT < 2 ms</span>
    </div>
  </section>
<script>
(function() {{
  "use strict";
  function applySafeText(id, text) {{
    var el = document.getElementById(id);
    if (el) {{ el.textContent = String(text); }}
  }}
  if (window.chrome && window.chrome.webview) {{
    window.chrome.webview.addEventListener("message", function(ev) {{
      try {{
        var msg = typeof ev.data === "string" ? JSON.parse(ev.data) : ev.data;
        if (msg && msg.payload && msg.payload.kind === "dom_safe_text_render") {{
          applySafeText(msg.payload.data.element_id, msg.payload.data.safe_text);
        }}
      }} catch (_err) {{}}
    }});
  }}
}})();
</script>
</body>
</html>"#,
        csp = MIKU_WEBVIEW_CSP_HEADER,
        bg = tokens.island.background,
        w = tokens.island.expanded.width_default,
        h = OVERVIEW_EXPANDED_HEIGHT_PX,
        r = tokens.island.expanded.radius_bottom,
        k = tokens.scale.hover_open_scene.k,
        body_px = tokens.scale.hover_open_scene.body_px,
        layers = layers_markup,
    );

    let byte_len = html_content.len();
    let has_external_cdn_links = html_content.contains("http://")
        || html_content
            .split("https://")
            .skip(1)
            .any(|seg| !seg.starts_with("miku.local"));

    let bundle = OverviewSceneBundle {
        virtual_url: OVERVIEW_VIRTUAL_INDEX_URL.to_string(),
        bundle_sha256: sha256_hex(html_content.as_bytes()),
        html_content,
        byte_len,
        csp_header: MIKU_WEBVIEW_CSP_HEADER.to_string(),
        expanded_width_px: tokens.island.expanded.width_default,
        expanded_height_px: OVERVIEW_EXPANDED_HEIGHT_PX,
        corner_radius_px: tokens.island.expanded.radius_bottom,
        avatar_scale_k: tokens.scale.hover_open_scene.k,
        avatar_body_px: tokens.scale.hover_open_scene.body_px,
        viewbox: catalog.viewbox,
        layer_count: layers.len(),
        accessory_count: accessories.len(),
        uses_only_text_content: true,
        has_external_cdn_links,
    };
    bundle.validate_security_and_budget()?;
    Ok(bundle)
}

