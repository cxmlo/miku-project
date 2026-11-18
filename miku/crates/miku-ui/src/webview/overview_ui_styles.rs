/// Builds the self-contained scoped CSS for the Dynamic Island Overview UI (`https://miku.local`).
/// Adapts fluidly across `100 %`, `125 %`, and `150 %` DPI scaling with zero horizontal/vertical
/// document overflow and obeys `@media (prefers-reduced-motion: reduce)`.
pub(crate) fn build_overview_styles(bg: &str, w: u32, h: u32, r: u32, k: f64, body_px: f64) -> String {
    format!(
        r#"<style>
:root {{
  --s0: {bg};
  --s1: #0E0F11;
  --card: #17181C;
  --card-2: #1E2028;
  --line: #272A33;
  --text-primary: #F5F5F7;
  --text-muted: #9398A6;
  --accent-blue: #3B9EFF;
  --accent-ok: #30D158;
  --accent-warn: #FFD60A;
  --accent-err: #FF453A;
  --island-w: {w}px;
  --island-h: {h}px;
  --island-radius: {r}px;
  --avatar-k: {k:.2};
  --avatar-body-px: {body_px:.1}px;
}}
* {{ box-sizing: border-box; margin: 0; padding: 0; user-select: none; }}
html, body {{ width: 100%; height: 100%; overflow: hidden; background: transparent; }}
body {{
  width: 100%;
  max-width: var(--island-w);
  height: 100%;
  max-height: var(--island-h);
  margin: 0 auto;
  background: radial-gradient(circle at 18% 12%, rgba(59,158,255,0.12), transparent 46%), var(--s0);
  color: var(--text-primary);
  font-family: 'Segoe UI Variable', 'Segoe UI', system-ui, sans-serif;
  font-size: clamp(11px, 1.55vw, 12.5px);
  border-bottom-left-radius: var(--island-radius);
  border-bottom-right-radius: var(--island-radius);
  border: 1px solid rgba(255,255,255,0.08);
  display: grid;
  grid-template-columns: minmax(136px, 162px) minmax(0, 1fr);
  gap: 10px;
  padding: 10px 12px;
}}
#miku-avatar-stage {{
  background: var(--s1);
  border: 1px solid var(--line);
  border-radius: 22px;
  padding: 10px 8px;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  min-width: 0;
  overflow: hidden;
}}
#miku-avatar-svg {{
  width: 108px;
  height: 108px;
  flex-shrink: 0;
  transition: transform 220ms cubic-bezier(.22,1,.36,1);
}}
.miku-stage-meta {{
  width: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 5px;
}}
.miku-pill-badge {{
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 10px;
  font-weight: 600;
  background: var(--card-2);
  border: 1px solid var(--line);
  color: var(--text-primary);
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}}
#miku-mood-select {{
  width: 100%;
  background: var(--card);
  color: var(--text-primary);
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 4px 6px;
  font-size: 10.5px;
}}
#miku-overview-metrics {{
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 4px;
}}
#miku-overview-panel {{
  background: var(--card);
  border: 1px solid var(--line);
  border-radius: 22px;
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}}
.miku-topbar {{
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
}}
.miku-title-wrap {{ min-width: 0; flex: 1; }}
#miku-overview-title {{
  font-size: 13px;
  font-weight: 700;
  letter-spacing: -0.01em;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}}
#miku-overview-status {{
  font-size: 10.5px;
  color: var(--text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}}
#miku-kill-switch-btn {{
  background: rgba(255,69,58,0.18);
  color: #FF8E88;
  border: 1px solid rgba(255,69,58,0.45);
  border-radius: 999px;
  padding: 4px 9px;
  font-size: 10px;
  font-weight: 700;
  cursor: pointer;
  flex-shrink: 0;
}}
#miku-tab-bar {{
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}}
.miku-tab {{
  background: var(--s1);
  color: var(--text-muted);
  border: 1px solid var(--line);
  border-radius: 999px;
  padding: 3px 8px;
  font-size: 10px;
  font-weight: 600;
  cursor: pointer;
}}
.miku-tab[aria-selected="true"] {{
  background: rgba(59,158,255,0.2);
  color: var(--text-primary);
  border-color: var(--accent-blue);
}}
.miku-tab-stage {{
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding-right: 2px;
}}
.miku-tabpanel {{ display: none; flex-direction: column; gap: 7px; }}
.miku-tabpanel.is-active {{ display: flex; }}
.miku-card {{
  background: var(--s1);
  border: 1px solid var(--line);
  border-radius: 14px;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}}
.miku-row {{
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  min-width: 0;
  flex-wrap: wrap;
}}
.miku-mono {{
  font-family: 'Cascadia Mono', Consolas, monospace;
  font-size: 10px;
  color: #B7D8FF;
  word-break: break-word;
}}
.miku-diff-box {{
  background: #090A0D;
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 6px;
  font-family: 'Cascadia Mono', Consolas, monospace;
  font-size: 10px;
  color: #D2FFE0;
  white-space: pre-wrap;
}}
.miku-btn {{
  background: var(--card-2);
  color: var(--text-primary);
  border: 1px solid var(--line);
  border-radius: 9px;
  padding: 4px 9px;
  font-size: 10.5px;
  font-weight: 600;
  cursor: pointer;
}}
.miku-btn-allow {{ background: rgba(48,209,88,0.18); border-color: rgba(48,209,88,0.5); color: #8AF5A6; }}
.miku-btn-deny {{ background: rgba(255,69,58,0.18); border-color: rgba(255,69,58,0.5); color: #FF9E98; }}
.miku-btn-accent {{ background: rgba(59,158,255,0.2); border-color: var(--accent-blue); color: #DCEEFF; }}
.miku-input {{
  flex: 1;
  min-width: 0;
  background: #0B0C10;
  color: var(--text-primary);
  border: 1px solid var(--line);
  border-radius: 9px;
  padding: 5px 8px;
  font-size: 11px;
  user-select: text;
}}
.miku-dropzone {{
  border: 1px dashed rgba(59,158,255,0.5);
  border-radius: 10px;
  padding: 6px 8px;
  font-size: 10.5px;
  color: var(--text-muted);
  text-align: center;
}}
.miku-feed-list {{ list-style: none; display: flex; flex-direction: column; gap: 4px; }}
.miku-feed-item {{
  background: #0E1015;
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 4px 7px;
  font-size: 10.5px;
  word-break: break-word;
}}
.miku-progress {{
  width: 100%;
  height: 5px;
  background: var(--card-2);
  border-radius: 999px;
  overflow: hidden;
}}
.miku-progress > span {{ display: block; height: 100%; width: 42%; background: var(--accent-blue); }}
@media (max-height: 330px), (max-width: 560px) {{
  body {{ grid-template-columns: 132px minmax(0, 1fr); gap: 8px; padding: 8px; }}
  #miku-avatar-svg {{ width: 88px; height: 88px; }}
}}
@media (prefers-reduced-motion: reduce) {{
  *, *::before, *::after {{ transition: none !important; animation: none !important; }}
}}
</style>"#,
        bg = bg,
        w = w,
        h = h,
        r = r,
        k = k,
        body_px = body_px,
    )
}
