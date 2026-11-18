/**
 * scenes/settings.js — Panel Modular de Ajustes y Capacidades Reales de Miku
 * ----------------------------------------------------------------------------
 * Conecta directamente con los endpoints de los crates Rust:
 *  - Muñeco y Tema (archivos locales modificables sin recompilar)
 *  - Proveedores y Bóveda DPAPI (miku-secrets)
 *  - Gatekeeper .miku.toml y Kill Switch Global
 *  - Módulos: Agentes, Skills hot-reload, Servidores MCP, Memoria SQLite FTS5
 *  - Voz, Volumen SmoothSlider e Idioma (locales/*.json)
 *  - Sistema: Gateway 47400, miku doctor, Actualizador SHA-256, Apps RTX
 */
(function (global) {
  'use strict';

  const TABS_DEF = [
    ['mascot', 'Muñeco'],
    ['providers', 'API Keys'],
    ['security', 'Gatekeeper'],
    ['modules', 'Módulos'],
    ['audio', 'Voz / Idioma'],
    ['system', 'Sistema']
  ];

  function renderTabMascot() {
    const { esc, PRESET } = global.MikuUI;
    const phys = global.MikuMascot?.getPhys?.() || { body: '#ffffff', twintails: false, outfit: 'none' };
    const swatches = PRESET.map(hex => `<button type="button" class="sw8${phys.body.toLowerCase() === hex.toLowerCase() ? ' on' : ''}" data-mascot-color="${hex}" style="--c:${hex}" title="${hex}"></button>`).join('');

    return `<div class="mk-set-pane scr">
      <div class="mk-kv">
        <span>Personalización visual rápida</span>
        <div class="st2">
          <button type="button" data-go="wardrobe">Abrir Guardarropa</button>
          <button type="button" data-set-open-mascot>Playground 2D</button>
        </div>
      </div>
      <div class="mk-kv">
        <span>Color base del muñeco</span>
        <div class="crow">
          <button type="button" class="sw8${phys.body.toLowerCase() === '#ffffff' ? ' on' : ''}" data-mascot-color="#ffffff" style="--c:#ffffff" title="#ffffff"></button>
          ${swatches}
          <label class="colorin" title="Color personalizado"><input type="color" id="set-mascot-color" value="${esc(phys.body)}"></label>
        </div>
      </div>
      <div class="mk-kv">
        <span>Accesorios laterales (opcional)</span>
        <label class="sw"><input type="checkbox" id="set-mascot-tails"${phys.twintails ? ' checked' : ''}><i></i></label>
      </div>
      <div class="mk-kv">
        <span>Agentes predefinidos y Hooks</span>
        <button type="button" data-open-settings="agents">Configurar en Ajustes ↗</button>
      </div>
      <div class="mk-note">
        Personalización por archivos: edita <code>theme.css</code>, <code>mascot/mascot.config.json</code>, <code>locales/*.json</code> o <code>audio/sounds/manifest.json</code> en <code>%LOCALAPPDATA%\\Miku\\ui\\</code> sin tocar código Rust.
      </div>
    </div>`;
  }

  function renderTabProviders() {
    const { CFG, ST, MIKU_PROVIDERS, esc } = global.MikuUI;
    const prov = MIKU_PROVIDERS.find(p => p.id === CFG.provider) || MIKU_PROVIDERS[0];
    const provBtns = MIKU_PROVIDERS.map(p => `<button type="button" class="mk-set-tab${p.id === prov.id ? ' on' : ''}" data-set-provider="${p.id}">${esc(p.label)}</button>`).join('');
    const modelOpts = prov.models.map(m => `<option value="${esc(m)}"${CFG.model === m ? ' selected' : ''}>${esc(m)}</option>`).join('');
    const baseUrlVal = ST.settings?.custom_base_url || CFG.custom_base_url || '';

    const secretsList = ST.secrets.length
      ? ST.secrets.map(s => `<div class="mk-kv"><code>${esc(s)}</code><button type="button" class="mk-danger-btn" data-del-secret="${esc(s)}">Eliminar</button></div>`).join('')
      : `<div class="mk-note">Sin alias sincronizados. Si el daemon está offline, agrega alias vía <code>miku secret set &lt;alias&gt;</code> o inicia el Gateway.</div>`;

    return `<div class="mk-set-pane scr">
      <div class="mk-set-tabs">${provBtns}</div>
      <div class="mk-kv">
        <span>Modelo activo</span>
        <select id="set-model-select" class="mk-mini-input" style="max-width:210px">${modelOpts}</select>
      </div>
      <div class="mk-kv">
        <span>Endpoint / Ollama URL</span>
        <input id="set-base-url" class="mk-mini-input" type="text" placeholder="Opcional (ej. 127.0.0.1:11434 o URL local)" value="${esc(baseUrlVal)}">
      </div>
      <div class="st2" style="margin-top:2px">
        <input id="sec-alias" class="mk-mini-input" type="text" placeholder="Alias (ej. deepseek-main)" value="${esc(CFG.key_alias || '')}">
        <input id="sec-val" class="mk-mini-input" type="password" placeholder="API Key (cifrado DPAPI)">
        <button type="button" class="p" id="sec-save-btn">Guardar DPAPI</button>
        <button type="button" id="sec-test-btn">Probar API</button>
      </div>
      ${runtime.secretTestMsg ? `<div class="mk-note">${esc(runtime.secretTestMsg)}</div>` : ''}
      <div class="mk-note">Bóveda segura (solo nombres de alias; nunca en claro en discos o UI):</div>
      ${secretsList}
    </div>`;
  }

  function renderTabSecurity() {
    const { ST, CFG, esc } = global.MikuUI;
    const ksBtnClass = ST.killSwitch ? 'mk-danger-btn armed' : 'mk-danger-btn';
    const ksLabel = ST.killSwitch ? 'KILL SWITCH ARMADO (Reanudar)' : 'ACTIVAR KILL SWITCH GLOBAL';
    const rulesHtml = ST.rules.length
      ? ST.rules.slice(0, 8).map(r => `<div class="mk-kv"><span>${esc(r.name || r.id || 'Regla')}</span><code>${esc(r.tier || r.mode || 'allow')}</code></div>`).join('')
      : `<div class="mk-note">Al conectar con el Gateway 127.0.0.1:47400 se sincronizan aquí las 16 reglas activas de <code>.miku.toml</code>.</div>`;

    return `<div class="mk-set-pane scr">
      <div class="mk-kv">
        <span>Corte inmediato de ejecución</span>
        <button type="button" id="set-kill-btn" class="${ksBtnClass}">${ksLabel}</button>
      </div>
      <div class="mk-kv">
        <span>Aprobación biométrica / huella (High/Critical)</span>
        <label class="sw"><input type="checkbox" data-cfg-toggle="biometric"${CFG.biometric ? ' checked' : ''}><i></i></label>
      </div>
      <div class="mk-kv">
        <span>Enlace seguro LAN / Tailscale (*.ts.net)</span>
        <label class="sw"><input type="checkbox" data-cfg-toggle="local_net"${CFG.local_net ? ' checked' : ''}><i></i></label>
      </div>
      ${rulesHtml}
    </div>`;
  }

  function renderTabModules() {
    const { ST, runtime, esc } = global.MikuUI;
    const skillsHtml = ST.skills.length
      ? ST.skills.map(s => `<span class="badge">${esc(s.name || s)}</span>`).join(' ')
      : '<span class="mk-note">Sin skills hot-reload sincronizadas aún.</span>';
    const mcpHtml = ST.mcp.length
      ? ST.mcp.map(m => `<div class="mk-kv"><code>${esc(m.name || m.id)}</code><span>${esc(m.command || 'stdio')}</span></div>`).join('')
      : '<span class="mk-note">Sin servidores MCP configurados aún.</span>';
    const catHtml = (runtime.mcpCatalog || []).slice(0, 6).map(c =>
      `<div class="mk-kv"><span>${esc(c.name)} <small>(${esc(c.tag || '')})</small></span><button type="button" data-mcp-install="${esc(c.id)}">${c.installed ? 'Instalado ✓' : 'Instalar'}</button></div>`
    ).join('');
    const hitsHtml = ST.memoryHits.length
      ? ST.memoryHits.slice(0, 4).map(h => `<div class="mk-kv"><span>${esc(h.title || h.key || 'Memoria')}</span><code>${esc(String(h.snippet || '').slice(0, 48))}</code></div>`).join('')
      : '';

    return `<div class="mk-set-pane scr">
      <div class="mk-kv">
        <span>Skills locales (${ST.skills.length})</span>
        <button type="button" id="mod-reload-skills">Recargar Skills</button>
      </div>
      <div class="chips" style="margin-top:0">${skillsHtml}</div>
      <div class="mk-kv" style="margin-top:4px">
        <span>Servidores MCP (${ST.mcp.length})</span>
        <button type="button" id="mcp-cat-refresh">Catálogo MCP</button>
      </div>
      ${mcpHtml}
      ${catHtml}
      <div class="st2">
        <input id="mcp-name" class="mk-mini-input" type="text" placeholder="Nombre MCP">
        <input id="mcp-cmd" class="mk-mini-input" type="text" placeholder="Comando (ej. npx ...)">
        <button type="button" id="mcp-add-btn">Agregar MCP</button>
      </div>
      <div class="st2">
        <input id="mem-query" class="mk-mini-input" type="text" placeholder="Buscar memoria SQLite FTS5...">
        <button type="button" id="mem-search-btn">Buscar FTS5</button>
      </div>
      ${hitsHtml}
    </div>`;
  }

  function renderTabAudio() {
    const { CFG, ST, runtime, esc } = global.MikuUI;
    const micList = ST.audioDevices.length ? ST.audioDevices : [{ id: 'default', label: 'Micrófono predeterminado del sistema' }];
    const micOpts = micList.map(d => `<option value="${esc(d.id)}"${CFG.mic === d.id ? ' selected' : ''}>${esc(d.label || d.id)}</option>`).join('');
    const outs = Array.isArray(runtime.audioOutputs) && runtime.audioOutputs.length
      ? runtime.audioOutputs.map(o => o.name).join(', ')
      : 'Pulsa Detectar audio para leer salidas WinMM';
    const soundFilesTxt = Array.isArray(runtime.soundFiles) && runtime.soundFiles.length
      ? runtime.soundFiles.join(', ')
      : 'Sintetizador WebAudio activo (+ .wav/.mp3/.ogg en audio/sounds/)';

    return `<div class="mk-set-pane scr">
      <div class="mk-kv">
        <span>Idioma de la interfaz</span>
        <div class="st2">
          <button type="button" class="mk-set-tab${CFG.lang === 'es' ? ' on' : ''}" data-set-lang="es">Español</button>
          <button type="button" class="mk-set-tab${CFG.lang === 'en' ? ' on' : ''}" data-set-lang="en">English</button>
        </div>
      </div>
      <div class="mk-kv">
        <span>Respuestas de voz y lectura</span>
        <label class="sw"><input type="checkbox" data-cfg-toggle="voice"${CFG.voice ? ' checked' : ''}><i></i></label>
      </div>
      <div class="mk-kv">
        <span>Dictado por voz (STT)</span>
        <label class="sw"><input type="checkbox" data-cfg-toggle="voice_stt_enabled" data-cfg-bool="true"${CFG.voice_stt_enabled ? ' checked' : ''}><i></i></label>
      </div>
      <div class="mk-kv">
        <span>Motor STT</span>
        <select id="set-stt-engine-select" class="mk-mini-input" style="max-width:210px">
          <option value="off"${CFG.voice_stt_engine === 'off' ? ' selected' : ''}>Desactivado (off)</option>
          <option value="browser_webspeech"${CFG.voice_stt_engine === 'browser_webspeech' ? ' selected' : ''}>WebSpeech (Navegador)</option>
          <option value="groq_whisper"${CFG.voice_stt_engine === 'groq_whisper' ? ' selected' : ''}>Groq Whisper (PTT)</option>
          <option value="local_sapi"${CFG.voice_stt_engine === 'local_sapi' ? ' selected' : ''}>Windows SAPI local</option>
        </select>
      </div>
      <div class="mk-kv">
        <span>Auto-enviar tras dictado</span>
        <label class="sw"><input type="checkbox" data-cfg-toggle="voice_auto_send" data-cfg-bool="true"${CFG.voice_auto_send ? ' checked' : ''}><i></i></label>
      </div>
      <div class="mk-kv">
        <span>Wake Word local (&laquo;Oye Miku&raquo;)</span>
        <label class="sw"><input type="checkbox" data-cfg-toggle="voice_wake_word_enabled" data-cfg-bool="true"${CFG.voice_wake_word_enabled ? ' checked' : ''}><i></i></label>
      </div>
      <div class="mk-kv">
        <span>Efectos de sonido UI (SFX)</span>
        <label class="sw"><input type="checkbox" data-cfg-toggle="sfx_enabled"${CFG.sfx_enabled !== 0 ? ' checked' : ''}><i></i></label>
      </div>
      <div class="mk-kv">
        <span>Abrir isla al pasar el cursor</span>
        <label class="sw"><input type="checkbox" data-cfg-toggle="hover_open"${CFG.hover_open ? ' checked' : ''}><i></i></label>
      </div>
      <div class="mk-kv">
        <span>Volumen de Voz</span>
        <div id="set-vol-voice" style="width:170px"></div>
      </div>
      <div class="mk-kv">
        <span>Volumen SFX</span>
        <div id="set-vol-sfx" style="width:170px"></div>
      </div>
      <div class="mk-kv">
        <span>Probar audio en vivo</span>
        <div class="chips">
          <button type="button" data-test-sfx="click">Click</button>
          <button type="button" data-test-sfx="chime">Chime</button>
          <button type="button" data-test-sfx="alert">Alerta</button>
          <button type="button" data-test-sfx="error">Error</button>
          <button type="button" data-test-sfx="voice">Voz</button>
        </div>
      </div>
      <div class="mk-kv">
        <span>Dispositivo de micrófono</span>
        <select id="set-mic-select" class="mk-mini-input" style="max-width:210px">${micOpts}</select>
      </div>
      <div class="mk-kv">
        <span>Salidas WinMM</span>
        <button type="button" id="aud-refresh-btn">Detectar audio</button>
      </div>
      <div class="mk-note">${esc(outs)}</div>
      <div class="mk-note">Audio personalizado: ${esc(soundFilesTxt)}</div>
    </div>`;
  }

  function renderTabSystem() {
    const { ST, esc } = global.MikuUI;
    const doc = ST.doctorReport;
    const docHtml = doc
      ? `<div class="mk-kv"><span>Doctor (${esc(doc.overall_status)})</span><code>${doc.passed_count}/${(doc.probes || []).length} checks OK</code></div>`
      : '<div class="mk-note">Ejecuta miku doctor para comprobar WebView2, puerto 47400, contratos TOML y presupuesto de RAM.</div>';
    const updHtml = ST.updateInfo
      ? `<div class="mk-kv"><span>Versión</span><code>${esc(ST.updateInfo.version || 'al día')}</code></div>`
      : '';
    const appsHtml = ST.appsDetected.length
      ? `<div class="mk-note">Apps locales detectadas: ${ST.appsDetected.map(a => esc(a.name || a)).join(', ')}</div>`
      : '';

    return `<div class="mk-set-pane scr">
      <div class="mk-kv">
        <span><span class="sd ${ST.ws ? 'on' : ''}"></span>Gateway 127.0.0.1:47400</span>
        <div class="st2">
          <button type="button" id="sys-connect-btn">${ST.ws ? 'Resincronizar' : 'Conectar'}</button>
          <button type="button" data-go="link">Miku-Link QR</button>
        </div>
      </div>
      <div class="st2">
        <button type="button" id="sys-doctor-btn">Ejecutar Doctor</button>
        <button type="button" id="sys-update-btn">Buscar Updates SHA-256</button>
        <button type="button" id="sys-apps-btn">Detectar Apps / RTX</button>
      </div>
      ${docHtml}
      ${updHtml}
      ${appsHtml}
    </div>`;
  }

  function renderScene() {
    const { runtime, tr, esc } = global.MikuUI;
    const activeTab = runtime.settingsTab || 'mascot';

    const tabsHtml = TABS_DEF.map(([id, label]) =>
      `<button type="button" class="mk-set-tab${activeTab === id ? ' on' : ''}" data-set-tab="${id}">${esc(label)}</button>`
    ).join('');

    let pane = '';
    if (activeTab === 'mascot') pane = renderTabMascot();
    else if (activeTab === 'providers') pane = renderTabProviders();
    else if (activeTab === 'security') pane = renderTabSecurity();
    else if (activeTab === 'modules') pane = renderTabModules();
    else if (activeTab === 'audio') pane = renderTabAudio();
    else pane = renderTabSystem();

    return `<div class="card mk">
      <div class="cp">
        <div class="wh">
          <b>${esc(tr('quick_settings'))}</b>
          <button type="button" class="wn" data-open-settings="general">Ventana 1080×780 ↗</button>
        </div>
        <div class="mk-set-wrap">
          <div class="mk-set-tabs">${tabsHtml}</div>
          ${pane}
        </div>
      </div>
    </div>`;
  }

  function saveCfg() {
    try { localStorage.setItem('miku_cfg', JSON.stringify(global.MikuUI.CFG)); } catch (_) {}
  }

  let _settingsAbort = null;

  function bindEvents(col, hooks) {
    if (_settingsAbort) _settingsAbort.abort();
    _settingsAbort = new AbortController();
    const { signal } = _settingsAbort;
    const rawHooks = hooks;
    hooks = {
      ...rawHooks,
      rerender: () => {
        if (!signal.aborted) rawHooks?.rerender?.();
      }
    };
    const busyButtons = new Set();
    const onCol = (type, handler) => {
      col.addEventListener(type, async e => {
        if (signal.aborted) return;
        if (type === 'click') {
          const btn = e.target?.closest?.('button[id], button[data-set-tab], button[data-mcp-install], button[data-del-secret]');
          const lockKey = btn?.id || btn?.dataset?.setTab || btn?.dataset?.mcpInstall || btn?.dataset?.delSecret || '';
          if (lockKey && busyButtons.has(lockKey)) return;
          if (lockKey) busyButtons.add(lockKey);
          try {
            await handler(e);
          } finally {
            if (lockKey) busyButtons.delete(lockKey);
          }
          return;
        }
        await handler(e);
      }, { signal });
    };

    const { $, CFG, ST, runtime, loadLocale } = global.MikuUI;
    const { SmoothSlider } = global.MikuComponents;

    if (runtime.settingsTab === 'audio') {
      SmoothSlider($('#set-vol-voice'), {
        value: CFG.voice_volume,
        onChange: v => {
          CFG.voice_volume = Math.round(v);
          saveCfg();
          global.Bridge?.setVolume?.(CFG.voice_volume);
        }
      });
      SmoothSlider($('#set-vol-sfx'), {
        value: CFG.sfx_volume,
        onChange: v => {
          CFG.sfx_volume = Math.round(v);
          saveCfg();
          global.Bridge?.setSetting?.('sfx_volume', CFG.sfx_volume);
        }
      });
    }

    onCol('change', async e => {
      const t = e.target;
      if (t.id === 'set-mascot-color') {
        global.MikuMascot?.setColor?.(t.value);
        global.Bridge?.setSetting?.('mascot_color', t.value);
      } else if (t.id === 'set-mascot-tails') {
        global.MikuMascot?.setTwintails?.(t.checked);
        global.Bridge?.setSetting?.('mascot_twintails', t.checked);
      } else if (t.id === 'set-model-select') {
        CFG.model = t.value;
        saveCfg();
        global.Bridge?.setSetting?.('model', CFG.model);
      } else if (t.id === 'set-base-url') {
        CFG.custom_base_url = t.value.trim();
        saveCfg();
        global.Bridge?.setSetting?.('custom_base_url', CFG.custom_base_url);
      } else if (t.id === 'set-mic-select') {
        CFG.mic = t.value;
        saveCfg();
        global.Bridge?.setSetting?.('mic', CFG.mic);
      } else if (t.id === 'set-stt-engine-select') {
        CFG.voice_stt_engine = t.value;
        saveCfg();
        global.Bridge?.setSetting?.('voice_stt_engine', CFG.voice_stt_engine);
        global.Bridge?.broadcast?.({ k: 'setting_updated', patch: { voice_stt_engine: CFG.voice_stt_engine } });
      } else if (t.dataset.cfgToggle) {
        const key = t.dataset.cfgToggle;
        const isBool = t.dataset.cfgBool === 'true' || typeof CFG[key] === 'boolean';
        CFG[key] = isBool ? Boolean(t.checked) : (t.checked ? 1 : 0);
        saveCfg();
        global.MikuUI.applyThemeVars?.(CFG);
        global.Bridge?.setSetting?.(key, CFG[key]);
        global.Bridge?.broadcast?.({ k: 'setting_updated', patch: { [key]: CFG[key] } });
      }
    });

    onCol('click', async e => {
      const tabBtn = e.target.closest('[data-set-tab]');
      if (tabBtn) {
        runtime.settingsTab = tabBtn.dataset.setTab;
        global.SoundFX?.tab?.();
        hooks?.rerender?.();
        return;
      }

      const colorBtn = e.target.closest('[data-mascot-color]');
      if (colorBtn) {
        const hex = colorBtn.dataset.mascotColor;
        global.MikuMascot?.setColor?.(hex);
        global.SoundFX?.pop?.();
        hooks?.rerender?.();
        return;
      }

      if (e.target.closest('[data-set-open-mascot]')) {
        window.open('./mascot.html', '_blank', 'noopener');
        return;
      }

      const testSfxBtn = e.target.closest('[data-test-sfx]');
      if (testSfxBtn) {
        const slot = testSfxBtn.dataset.testSfx;
        if (slot === 'voice') {
          global.SoundFX?.previewVoice?.();
        } else {
          global.SoundFX?.play?.(slot, { force: true });
        }
        return;
      }

      const provBtn = e.target.closest('[data-set-provider]');
      if (provBtn) {
        const pid = provBtn.dataset.setProvider;
        const p = global.MikuUI.MIKU_PROVIDERS.find(x => x.id === pid);
        CFG.provider = pid;
        if (p?.models?.length) CFG.model = p.models[0];
        saveCfg();
        global.Bridge?.setSetting?.('provider', CFG.provider);
        global.Bridge?.setSetting?.('model', CFG.model);
        global.SoundFX?.click?.();
        hooks?.rerender?.();
        return;
      }

      const langBtn = e.target.closest('[data-set-lang]');
      if (langBtn) {
        CFG.lang = langBtn.dataset.setLang === 'en' ? 'en' : 'es';
        saveCfg();
        await loadLocale(CFG.lang);
        global.Bridge?.setSetting?.('lang', CFG.lang);
        global.SoundFX?.pop?.();
        hooks?.rerender?.();
        return;
      }

      if (e.target.id === 'sec-save-btn') {
        const alias = $('#sec-alias')?.value?.trim();
        const secret = $('#sec-val')?.value?.trim();
        if (!alias || !secret) return;
        CFG.key_alias = alias;
        saveCfg();
        try {
          await global.Bridge.setSecret(alias, secret, CFG.provider, CFG.model);
          if ($('#sec-val')) $('#sec-val').value = '';
          await global.Bridge.syncAll();
          global.SoundFX?.chime?.();
          hooks?.rerender?.();
        } catch (_) {
          global.SoundFX?.error?.();
        }
        return;
      }

      const delSec = e.target.closest('[data-del-secret]');
      if (delSec) {
        try {
          await global.Bridge.deleteSecret(delSec.dataset.delSecret);
          await global.Bridge.syncAll();
          hooks?.rerender?.();
        } catch (_) {}
        return;
      }

      if (e.target.id === 'set-kill-btn') {
        const res = await global.Bridge.toggleKillSwitch(!ST.killSwitch).catch(() => null);
        if (res) {
          ST.killSwitch = !ST.killSwitch;
          global.SoundFX?.hold?.();
          hooks?.rerender?.();
        }
        return;
      }

      if (e.target.id === 'mod-reload-skills') {
        await global.Bridge.reloadSkills().catch(() => null);
        await global.Bridge.syncAll().catch(() => null);
        global.SoundFX?.chime?.();
        hooks?.rerender?.();
        return;
      }

      if (e.target.id === 'mcp-add-btn') {
        const name = $('#mcp-name')?.value?.trim();
        const cmd = $('#mcp-cmd')?.value?.trim();
        if (!name || !cmd) return;
        await global.Bridge.addMcp(name, cmd, []).catch(() => null);
        await global.Bridge.syncAll().catch(() => null);
        hooks?.rerender?.();
        return;
      }

      if (e.target.id === 'mem-search-btn') {
        const q = $('#mem-query')?.value?.trim() || '';
        const hits = await global.Bridge.searchMemory(q).catch(() => []);
        ST.memoryHits = Array.isArray(hits) ? hits : [];
        hooks?.rerender?.();
        return;
      }

      if (e.target.id === 'mcp-cat-refresh') {
        await global.Bridge.fetchMcpCatalog().catch(() => []);
        global.SoundFX?.pop?.();
        hooks?.rerender?.();
        return;
      }

      const mcpInst = e.target.closest('[data-mcp-install]');
      if (mcpInst) {
        await global.Bridge.installMcpCatalog(mcpInst.dataset.mcpInstall).catch(() => null);
        await global.Bridge.fetchMcpCatalog().catch(() => []);
        await global.Bridge.syncAll().catch(() => null);
        global.SoundFX?.chime?.();
        hooks?.rerender?.();
        return;
      }

      if (e.target.id === 'aud-refresh-btn') {
        await Promise.all([
          global.Bridge.fetchAudioDevices().catch(() => []),
          global.Bridge.fetchSoundsList().catch(() => null)
        ]);
        global.SoundFX?.pop?.();
        hooks?.rerender?.();
        return;
      }

      if (e.target.id === 'sys-connect-btn') {
        global.Bridge.connect(true);
        setTimeout(() => hooks?.rerender?.(), 450);
        return;
      }

      if (e.target.id === 'sys-doctor-btn') {
        await global.Bridge.runDoctor().catch(() => null);
        hooks?.rerender?.();
        return;
      }

      if (e.target.id === 'sys-update-btn') {
        await global.Bridge.checkUpdate().catch(() => null);
        hooks?.rerender?.();
        return;
      }

      if (e.target.id === 'sys-apps-btn') {
        await global.Bridge.detectApps().catch(() => []);
        hooks?.rerender?.();
        return;
      }

      if (e.target.id === 'sec-test-btn') {
        runtime.secretTestMsg = 'Probando conexión...';
        hooks?.rerender?.();
        const res = await global.Bridge.testSecret({
          provider: CFG.provider,
          model: CFG.model,
          alias: $('#sec-alias')?.value?.trim() || CFG.key_alias || '',
          base_url: $('#set-base-url')?.value?.trim() || CFG.custom_base_url || ''
        }).catch(err => ({ ok: false, error: err?.message || 'Gateway offline' }));
        runtime.secretTestMsg = res?.ok
          ? `Conexión OK · HTTP ${res.status || 200} (${res.latency_ms || 0} ms)`
          : `Error: ${res?.error || res?.message || 'No se pudo conectar'}`;
        hooks?.rerender?.();
      }
    });
  }

  global.MikuSettings = { renderScene, bindEvents };
})(window);
