/// Builds the semantic DOM body of the Dynamic Island expanded UI (`https://miku.local`),
/// preserving the required stage/overview landmarks while surfacing every subsystem panel
/// (Permisos + Visión, Chat + PTT + File Drop, Feed técnico, API Keys/Modelos, Agentes/Skills,
/// Multimedia + Rutinas, y Miku-Link QR + Tailscale).
pub(crate) fn build_overview_html_body(layers_svg_markup: &str) -> String {
    format!(
        r#"<body data-scene="overview" data-origin="https://miku.local/index.html" data-mood="idle" data-dom-bind="textContent">
  <section id="miku-avatar-stage" aria-label="Avatar de Miku">
    <svg id="miku-avatar-svg" viewBox="0 0 128 128" width="128" height="128">{layers}</svg>
    <div class="miku-stage-meta">
      <span id="miku-avatar-mood-label" class="miku-pill-badge">idle</span>
      <select id="miku-mood-select" aria-label="Estado de animo de Miku">
        <option value="idle">idle</option>
        <option value="listening">listening</option>
        <option value="thinking">thinking</option>
        <option value="working">working</option>
        <option value="talking">talking</option>
        <option value="happy">happy</option>
        <option value="focused">focused</option>
        <option value="love">love</option>
        <option value="wink">wink</option>
        <option value="surprised">surprised</option>
        <option value="annoyed">annoyed</option>
        <option value="sleepy">sleepy</option>
        <option value="sleeping">sleeping</option>
        <option value="error">error</option>
        <option value="alert">alert</option>
      </select>
      <div id="miku-overview-metrics">
        <span id="miku-overview-ram-badge" class="miku-pill-badge">RAM <= 30 MB</span>
        <span id="miku-overview-ipc-badge" class="miku-pill-badge">IPC RTT &lt; 2 ms</span>
      </div>
    </div>
  </section>

  <section id="miku-overview-panel" aria-label="Paneles de orquestacion Miku">
    <header class="miku-topbar">
      <div class="miku-title-wrap">
        <h1 id="miku-overview-title">Miku Orchestrator</h1>
        <p id="miku-overview-status">Ready on https://miku.local</p>
      </div>
      <button id="miku-kill-switch-btn" type="button" data-action="kill_switch">KILL SWITCH</button>
    </header>

    <nav id="miku-tab-bar" role="tablist" aria-label="Modulos Miku">
      <button class="miku-tab" type="button" role="tab" aria-selected="true" data-tab="permissions">Permisos</button>
      <button class="miku-tab" type="button" role="tab" aria-selected="false" data-tab="chat">Chat/PTT</button>
      <button class="miku-tab" type="button" role="tab" aria-selected="false" data-tab="feed">Feed</button>
      <button class="miku-tab" type="button" role="tab" aria-selected="false" data-tab="keys">Keys</button>
      <button class="miku-tab" type="button" role="tab" aria-selected="false" data-tab="agents">Agentes</button>
      <button class="miku-tab" type="button" role="tab" aria-selected="false" data-tab="media">Media</button>
      <button class="miku-tab" type="button" role="tab" aria-selected="false" data-tab="link">Miku-Link</button>
    </nav>

    <main class="miku-tab-stage">
      <section id="panel-permissions" class="miku-tabpanel is-active" role="tabpanel">
        <article class="miku-card">
          <div class="miku-row">
            <strong id="miku-perm-agent">cursor · fs_write</strong>
            <span id="miku-perm-ttl" class="miku-pill-badge">TTL 30s</span>
          </div>
          <div id="miku-perm-target" class="miku-mono">src/state/projectState.ts</div>
          <div id="miku-perm-diff" class="miku-diff-box">+ Gatekeeper single-use sha256 verification</div>
          <div class="miku-row">
            <span id="miku-vision-thumb-badge" class="miku-pill-badge">Vision: 0px ROI listo</span>
            <div class="miku-row">
              <button id="miku-perm-allow-btn" class="miku-btn miku-btn-allow" type="button" data-action="perm_allow">Allow Once</button>
              <button id="miku-perm-deny-btn" class="miku-btn miku-btn-deny" type="button" data-action="perm_deny">Deny</button>
            </div>
          </div>
        </article>
      </section>

      <section id="panel-chat" class="miku-tabpanel" role="tabpanel">
        <div class="miku-card">
          <div class="miku-row">
            <input id="miku-prompt-input" class="miku-input" type="text" placeholder="Instruccion para agente activo..." aria-label="Prompt para agente"/>
            <button id="miku-prompt-send-btn" class="miku-btn miku-btn-accent" type="button" data-action="prompt_send">Enviar</button>
            <button id="miku-ptt-toggle-btn" class="miku-btn" type="button" data-action="ptt_toggle">PTT 20%</button>
          </div>
          <div id="miku-file-dropzone" class="miku-dropzone" data-action="file_drop">
            <span id="miku-file-drop-label">Suelta archivo aqui o pulsa para adjuntar a la sesion</span>
          </div>
          <ul id="miku-chat-queue" class="miku-feed-list">
            <li class="miku-feed-item">[Cola] Listo para recibir prompts PTT o texto</li>
          </ul>
        </div>
      </section>

      <section id="panel-feed" class="miku-tabpanel" role="tabpanel">
        <div class="miku-card">
          <div class="miku-row">
            <strong>Feed Tecnico (CI / PR / Radar)</strong>
            <span class="miku-pill-badge">Ring 32</span>
          </div>
          <ul id="miku-overview-feed" class="miku-feed-list">
            <li id="miku-feed-latest" class="miku-feed-item">CI workspace clippy -D warnings: PASS · miku-license-audit 15/15 MIT</li>
          </ul>
        </div>
      </section>

      <section id="panel-keys" class="miku-tabpanel" role="tabpanel">
        <div class="miku-card">
          <div class="miku-row">
            <strong>Vault DPAPI / Env (Solo alias, 0 secretos)</strong>
            <span id="miku-active-model-badge" class="miku-pill-badge">claude-sonnet</span>
          </div>
          <div class="miku-mono">ANTHROPIC_API_KEY [DPAPI OK] · GROQ_API_KEY [STT OK] · OPENAI_API_KEY [Alias]</div>
          <div class="miku-row">
            <button class="miku-btn miku-btn-accent miku-model-btn" type="button" data-model="claude-sonnet" data-action="model_select">Claude Sonnet</button>
            <button class="miku-btn miku-model-btn" type="button" data-model="gpt-4o" data-action="model_select">GPT-4o</button>
            <button class="miku-btn miku-model-btn" type="button" data-model="whisper-large-v3" data-action="model_select">Groq Whisper</button>
          </div>
        </div>
      </section>

      <section id="panel-agents" class="miku-tabpanel" role="tabpanel">
        <div class="miku-card">
          <div class="miku-row">
            <strong>Agentes Conectados + Skills Hot-Reload</strong>
            <button id="miku-skill-reload-btn" class="miku-btn miku-btn-accent" type="button" data-action="skill_reload">Recargar Skills</button>
          </div>
          <div id="miku-agents-summary" class="miku-mono">claude_code · cursor · antigravity · github | MCP Host activo</div>
          <div id="miku-skill-status" class="miku-pill-badge">skill.toml hot-reload listo (0 ms leak)</div>
        </div>
      </section>

      <section id="panel-media" class="miku-tabpanel" role="tabpanel">
        <div class="miku-card">
          <div class="miku-row">
            <strong id="miku-island-media-track">Sin reproduccion multimedia</strong>
            <span id="miku-media-ducking" class="miku-pill-badge">Ducking 20%: Normal</span>
          </div>
          <div id="miku-media-artist" class="miku-mono">WinMM + Media Transport via Gatekeeper</div>
          <div class="miku-progress"><span></span></div>
          <div class="miku-row">
            <div class="miku-row">
              <button class="miku-btn" type="button" data-action="media_prev">Prev</button>
              <button class="miku-btn miku-btn-accent" type="button" data-action="media_play_pause">Play/Pause</button>
              <button class="miku-btn" type="button" data-action="media_next">Next</button>
            </div>
            <button id="miku-routine-toggle" class="miku-btn" type="button" data-action="routine_toggle">Focus 25m</button>
          </div>
        </div>
      </section>

      <section id="panel-link" class="miku-tabpanel" role="tabpanel">
        <div class="miku-card">
          <div class="miku-row">
            <div>
              <strong>Miku-Link · Emparejamiento QR</strong>
              <div id="miku-link-tailscale-status" class="miku-mono">LAN WS + Tailscale Zero-PII Push</div>
              <button id="miku-link-pair-btn" class="miku-btn miku-btn-accent" type="button" data-action="link_pair">Rotar QR Session</button>
            </div>
            <svg id="miku-link-qr" viewBox="0 0 21 21" width="64" height="64" role="img" aria-label="QR Miku-Link"></svg>
          </div>
        </div>
      </section>
    </main>
  </section>
"#,
        layers = layers_svg_markup
    )
}
