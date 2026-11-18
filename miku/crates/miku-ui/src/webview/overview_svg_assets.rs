use crate::prelude::*;
use std::fs;
use std::path::Path;

/// Strips `<metadata>...</metadata>`, `<defs>...</defs>`, XML comments, and the outer
/// `<svg ...>` wrapper from an on-disk v7 SVG asset so its graphic primitives can be
/// safely mounted directly inside `#miku-avatar-svg`'s ordered Z-layers.
pub(crate) fn extract_clean_svg_body(raw_svg: &str) -> String {
    let mut s = strip_tag_block(raw_svg, "<metadata", "</metadata>");
    s = strip_tag_block(&s, "<filter", "</filter>");
    s = strip_comments(&s);

    if let Some(svg_start) = s.find("<svg") {
        if let Some(rel_end) = s[svg_start..].find('>') {
            s = s[(svg_start + rel_end + 1)..].to_string();
        }
    }
    if let Some(close_idx) = s.rfind("</svg>") {
        s.truncate(close_idx);
    }
    s.trim().to_string()
}

fn strip_tag_block(input: &str, open_tag: &str, close_tag: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(rel_start) = input[cursor..].find(open_tag) {
        let start = cursor + rel_start;
        out.push_str(&input[cursor..start]);
        if let Some(rel_end) = input[start..].find(close_tag) {
            cursor = start + rel_end + close_tag.len();
        } else {
            cursor = input.len();
            break;
        }
    }
    out.push_str(&input[cursor..]);
    out
}

fn strip_comments(input: &str) -> String {
    strip_tag_block(input, "<!--", "-->")
}

fn read_clean_asset_svg(avatar_dir: &Path, rel_path: &str) -> String {
    let trimmed = rel_path.strip_prefix("avatar/").unwrap_or(rel_path);
    let full_path = avatar_dir.join(trimmed);
    match fs::read_to_string(&full_path) {
        Ok(raw) => extract_clean_svg_body(&raw),
        Err(_) => String::new(),
    }
}

/// Builds the full `<defs>` + 9 Z-ordered `<g id="miku-layer-...">` SVG markup backed by
/// the real v7 SVG files on disk (`body.svg`, `ears.svg`, expressions, and mounted accessories),
/// keeping all 15 mood states switchable cleanly via attributes/JS without `innerHTML`.
pub(crate) fn build_avatar_layers_markup(
    avatar_dir: &Path,
    layers: &[AvatarLayerComposeNode],
) -> String {
    let defs = concat!(
        "<defs>",
        "<filter id=\"glow\" x=\"-30%\" y=\"-30%\" width=\"160%\" height=\"160%\" color-interpolation-filters=\"sRGB\">",
        "<feGaussianBlur in=\"SourceAlpha\" stdDeviation=\"5.5\" result=\"blur\"/>",
        "<feFlood id=\"miku-glow-flood\" flood-color=\"#BDDEFF\" flood-opacity=\"0.6\"/>",
        "<feComposite in2=\"blur\" operator=\"in\" result=\"halo\"/>",
        "<feMerge><feMergeNode in=\"halo\"/><feMergeNode in=\"halo\"/><feMergeNode in=\"SourceGraphic\"/></feMerge>",
        "</filter>",
        "</defs>"
    );

    let body_inner = read_clean_asset_svg(avatar_dir, "avatar/body.svg");
    let ears_inner = read_clean_asset_svg(avatar_dir, "avatar/ears.svg");
    let eyes_open_inner = read_clean_asset_svg(avatar_dir, "avatar/expressions/eyes_open.svg");
    let eyes_blink_inner = read_clean_asset_svg(avatar_dir, "avatar/expressions/eyes_blink.svg");
    let mouth_idle_inner = read_clean_asset_svg(avatar_dir, "avatar/expressions/mouth_idle.svg");
    let mouth_talk_inner = read_clean_asset_svg(avatar_dir, "avatar/expressions/mouth_talk.svg");
    let crown_back_inner = read_clean_asset_svg(avatar_dir, "avatar/accessories/crown_back.svg");
    let crown_front_inner = read_clean_asset_svg(avatar_dir, "avatar/accessories/crown.svg");
    let scarf_front_inner = read_clean_asset_svg(avatar_dir, "avatar/accessories/scarf.svg");

    let mut out = String::with_capacity(8192);
    out.push_str(defs);

    for layer in layers {
        let inner = match layer.id.as_str() {
            "accessory_back" => crown_back_inner.clone(),
            "ears" => ears_inner.clone(),
            "body" => body_inner.clone(),
            "blush" => concat!(
                "<ellipse cx=\"35\" cy=\"84\" rx=\"6.5\" ry=\"3.8\" fill=\"#FF9DB8\" opacity=\"0.55\"/>",
                "<ellipse cx=\"93\" cy=\"84\" rx=\"6.5\" ry=\"3.8\" fill=\"#FF9DB8\" opacity=\"0.55\"/>"
            )
            .to_string(),
            "eyes" => format!(
                concat!(
                    "<g id=\"miku-eyes-open\">{}</g>",
                    "<g id=\"miku-eyes-blink\" style=\"display:none\">{}</g>",
                    "<g id=\"miku-eyes-arc\" style=\"display:none\">",
                    "<path d=\"M38 75 Q44 67 50 75\" fill=\"none\" stroke=\"#172B40\" stroke-width=\"2.8\" stroke-linecap=\"round\"/>",
                    "<path d=\"M78 75 Q84 67 90 75\" fill=\"none\" stroke=\"#172B40\" stroke-width=\"2.8\" stroke-linecap=\"round\"/>",
                    "</g>",
                    "<g id=\"miku-eyes-x\" style=\"display:none\">",
                    "<path d=\"M39 70 L49 78 M49 70 L39 78\" fill=\"none\" stroke=\"#172B40\" stroke-width=\"2.6\" stroke-linecap=\"round\"/>",
                    "<path d=\"M79 70 L89 78 M89 70 L79 78\" fill=\"none\" stroke=\"#172B40\" stroke-width=\"2.6\" stroke-linecap=\"round\"/>",
                    "</g>",
                    "<g id=\"miku-eyes-wink\" style=\"display:none\">",
                    "<ellipse cx=\"43.7\" cy=\"73.8\" rx=\"4.8\" ry=\"7.2\" fill=\"#172B40\"/>",
                    "<path d=\"M78 75 Q84 67 90 75\" fill=\"none\" stroke=\"#172B40\" stroke-width=\"2.8\" stroke-linecap=\"round\"/>",
                    "</g>"
                ),
                eyes_open_inner, eyes_blink_inner
            ),
            "brows" => concat!(
                "<path d=\"M37 63 L50 66\" fill=\"none\" stroke=\"#172B40\" stroke-width=\"3\" stroke-linecap=\"round\"/>",
                "<path d=\"M91 63 L78 66\" fill=\"none\" stroke=\"#172B40\" stroke-width=\"3\" stroke-linecap=\"round\"/>"
            )
            .to_string(),
            "mouth" => format!(
                concat!(
                    "<g id=\"miku-mouth-idle\">{}</g>",
                    "<g id=\"miku-mouth-talk\" style=\"display:none\">{}</g>",
                    "<g id=\"miku-mouth-smile\" style=\"display:none\">",
                    "<path d=\"M57 81 Q64 88 71 81\" fill=\"none\" stroke=\"#172B40\" stroke-width=\"2.6\" stroke-linecap=\"round\"/>",
                    "</g>",
                    "<g id=\"miku-mouth-frown\" style=\"display:none\">",
                    "<path d=\"M58 85 Q64 79 70 85\" fill=\"none\" stroke=\"#172B40\" stroke-width=\"2.6\" stroke-linecap=\"round\"/>",
                    "</g>"
                ),
                mouth_idle_inner, mouth_talk_inner
            ),
            "accessory_front" => format!(
                "<g id=\"miku-acc-scarf\">{}</g><g id=\"miku-acc-crown\">{}</g>",
                scarf_front_inner, crown_front_inner
            ),
            "fx" => "<circle id=\"miku-fx-ring\" cx=\"64\" cy=\"64\" r=\"56\" fill=\"none\" stroke=\"#3B9EFF\" stroke-width=\"1.2\" stroke-dasharray=\"4 5\" opacity=\"0.35\"/>".to_string(),
            _ => String::new(),
        };

        let style_vis = if layer.visible {
            ""
        } else {
            " style=\"display:none\""
        };
        out.push_str(&format!(
            "<g id=\"miku-layer-{}\" data-z=\"{}\" data-svg=\"{}\" data-visible=\"{}\"{}>{}</g>",
            layer.id, layer.z, layer.svg_source, layer.visible, style_vis, inner
        ));
    }

    out
}
