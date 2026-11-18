(() => {
  'use strict';

  const UI = window.MikuUI || {};
  const BRIDGE = window.Bridge;
  const PH = window.MikuParticlesHeader;
  const esc = UI.esc || (s => String(s ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])));
  const safeColor = UI.safeCssColor || (c => (/^#[0-9a-f]{3,8}$/i.test(String(c || '').trim()) ? String(c).trim() : '#3B9EFF'));

  const NAV_GROUPS = [
    {
      title: 'Base',
      items: [
        { id: 'general', label: 'General', icon: 'ic-gear', desc: 'Idioma, personalidad del asistente y sincronización con la Isla.' },
        { id: 'island', label: 'Isla Dinámica', icon: 'ic-island', desc: 'Comportamiento, vestuario de Miku y apariencia del avatar.' }
      ]
    },
    {
      title: 'Inteligencia',
      items: [
        { id: 'agents', label: 'Agentes', icon: 'ic-bot', desc: 'Crea y edita subagentes con color, rol y modelo asignado.' },
        { id: 'providers', label: 'Modelos de IA', icon: 'ic-sparkles', desc: 'Proveedor activo, modelo, límite de tokens y endpoint.' },
        { id: 'keys', label: 'API Keys (DPAPI)', icon: 'ic-key', desc: 'Bóveda cifrada local Windows DPAPI sin texto plano.' },
        { id: 'mcp', label: 'Servidores MCP', icon: 'ic-server', desc: 'Herramientas conectadas y catálogo online instalable.' },
        { id: 'skills', label: 'Skills y Memoria', icon: 'ic-puzzle', desc: 'Capacidades locales activas y búsqueda SQLite FTS5.' },
        { id: 'telemetry', label: 'Telemetría API', icon: 'ic-activity', desc: 'Consumo de tokens, latencia, costes USD y circuit-breaker.' }
      ]
    },
    {
      title: 'Asistencia',
      items: [
        { id: 'notifications', label: 'Notificaciones', icon: 'ic-bell', desc: 'Alertas proactivas, barge-in y límites de uso en aplicaciones.' },
        { id: 'voice', label: 'Sonido y voz', icon: 'ic-mic', desc: 'Volúmenes SmoothSlider, Web Audio SFX y dispositivos Windows.' },
        { id: 'link', label: 'Miku-Link', icon: 'ic-link', desc: 'Emparejamiento LAN seguro por código QR con teléfono.' }
      ]
    },
    {
      title: 'Sistema',
      items: [
        { id: 'security', label: 'Permisos y Automatización', icon: 'ic-shield', desc: 'Gatekeeper, captura de pantalla, control PC, voz Opt-In y Kill Switch.' },
        { id: 'system', label: 'Sistema y Peligro', icon: 'ic-monitor', desc: 'Diagnóstico Doctor, apps detectadas y zona de restablecimiento.' }
      ]
    }
  ];

  const ALIAS_CAT = {
    mascot: 'island',
    modules: 'mcp',
    permisos: 'security',
    agentes: 'agents',
    modelos: 'providers',
    alertas: 'notifications',
    voz: 'voice',
    peligro: 'system'
  };

  function initialCategory() {
    const hash = decodeURIComponent((location.hash || '').replace(/^#/, '').trim());
    if (hash) return ALIAS_CAT[hash] || hash;
    try {
      const q = (new URLSearchParams(location.search).get('cat') || '').trim();
      if (q) return ALIAS_CAT[q] || q;
    } catch (_) {}
    try {
      const saved = JSON.parse(localStorage.getItem('miku_cat') || 'null');
      if (saved) return ALIAS_CAT[saved] || saved;
    } catch (_) {}
    return 'general';
  }

  let activeCat = initialCategory();
  let selectedAgentId = 'miku';
  let paneAbort = null;
  let heroState = null;
  let gwDoctorReport = null;
  let gwApps = null;
  let updateInfo = null;
  let qrData = null;
  let memoryQuery = '';
  let memoryHits = [];
  let permNotice = '';
  let dangerArmed = '';
  let hookPreview = null;
  let hookBusy = false;
  let hookNotice = '';
  let secretTestStatus = '';

  function currentMeta() {
    for (const g of NAV_GROUPS) {
      const found = g.items.find(i => i.id === activeCat);
      if (found) return found;
    }
    return NAV_GROUPS[0].items[0];
  }

  function allAgents() {
    const subs = Array.isArray(UI.ST?.agents) ? UI.ST.agents.filter(a => a && a.id !== 'miku') : [];
    return [UI.PRIN, ...subs].filter(Boolean);
  }

  function updateGwBadge(ok, portText) {
    const dot = document.getElementById('settings-gw-dot');
    const lbl = document.getElementById('settings-gw-label');
    if (dot) {
      dot.classList.toggle('ok', Boolean(ok));
      dot.classList.toggle('err', !ok);
    }
    if (lbl) lbl.textContent = ok ? `Gateway OK · ${portText || '47400'}` : 'Vista local (offline)';
  }

  function renderNav() {
    const rail = document.getElementById('rail');
    if (!rail) return;
    rail.innerHTML = NAV_GROUPS.map(g => `
      <div class="nav-group">
        <div class="nav-group-title">${esc(g.title)}</div>
        <div class="nav-group-items">
          ${g.items.map(it => `
            <button type="button" class="nv${activeCat === it.id ? ' on' : ''}" data-cat="${esc(it.id)}">
              <svg class="ic"><use href="#${esc(it.icon)}"></use></svg>
              <span>${esc(it.label)}</span>
            </button>`).join('')}
        </div>
      </div>`).join('');
  }

  function renderGeneralSection(ST) {
    const cfg = UI.CFG || {};
    const langs = ['es', 'en'];
    const pers = [
      { id: 'calm', label: 'Calmado' },
      { id: 'direct', label: 'Directo' },
      { id: 'playful', label: 'Expresivo' }
    ];
    const presets = UI.THEME_PRESETS || [
      { id: 'obsidian', label: 'Obsidian', accent: '#f5f5f7' },
      { id: 'cyan', label: 'Miku Cyan', accent: '#39c5bb' },
      { id: 'emerald', label: 'Emerald', accent: '#30d158' },
      { id: 'amber', label: 'Amber Gold', accent: '#fbbf24' },
      { id: 'amethyst', label: 'Amethyst', accent: '#a78bfa' }
    ];
    const activeAccent = safeColor(cfg.ui_accent || '#f5f5f7');
    const modeLabel = { full: 'Toda mi PC', sandbox: 'Sandbox', restricted: 'Restringido' }[ST.permissionMode || 'sandbox'] || 'Sandbox';
    return `
      <section class="card">
        <div class="card-h"><h2>Preferencias generales</h2></div>
        <div class="item">
          <div><div class="item-t">Idioma de la interfaz</div><div class="item-d">Sincronizado instantáneamente con la Dynamic Island.</div></div>
          <div class="pill-row">${langs.map(l => `<button type="button" class="set-pill${(cfg.lang || 'es') === l ? ' on' : ''}" data-setting-lang="${l}">${l.toUpperCase()}</button>`).join('')}</div>
        </div>
        <div class="item">
          <div><div class="item-t">Personalidad de respuesta</div><div class="item-d">Tono empleado por el orquestador en la píldora.</div></div>
          <div class="pill-row">${pers.map(p => `<button type="button" class="set-pill${(cfg.personality || 'calm') === p.id ? ' on' : ''}" data-setting-kv="personality:${p.id}">${esc(p.label)}</button>`).join('')}</div>
        </div>
        <div class="item">
          <div><div class="item-t">Modo de permisos activo</div><div class="item-d">Gatekeeper en vivo sobre archivos, consola y red.</div></div>
          <div class="pill-row">
            <span class="set-pill status-ok">${esc(modeLabel)}</span>
            <button type="button" class="set-btn" data-jump-cat="security">Cambiar modo</button>
          </div>
        </div>
        <div class="item">
          <div><div class="item-t">Estado del vínculo</div><div class="item-d">WebSocket en 127.0.0.1:${esc(String(BRIDGE?.GATEWAY_PORT || 47400))}</div></div>
          <span class="set-pill ${ST.ws ? 'status-ok' : 'status-er'}">${ST.ws ? 'Conectado' : 'Offline'}</span>
        </div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Armonía visual y paleta sincronizada</h2></div>
        <div class="item">
          <div><div class="item-t">Tema de la suite</div><div class="item-d">Aplica colores y contrastes armonizados a Isla y Ajustes.</div></div>
          <div class="preset-row">
            ${presets.map(p => `<button type="button" class="preset-chip${(cfg.theme_preset || 'obsidian') === p.id ? ' on' : ''}" data-setting-kv="theme_preset:${esc(p.id)}"><span class="preset-swatch" style="--sw:${esc(p.accent)}"></span><span>${esc(p.label)}</span></button>`).join('')}
          </div>
        </div>
        <div class="item">
          <div><div class="item-t">Color de acento personalizado</div><div class="item-d">Se adapta automáticamente a botones, focos y bordes.</div></div>
          <div class="pill-row">
            <span class="badge-sm">${esc(activeAccent)}</span>
            <label class="colorin" title="Acento global"><input type="color" id="set-ui-accent" value="${esc(activeAccent)}"></label>
          </div>
        </div>
      </section>`;
  }

  function renderIslandSection() {
    const cfg = UI.CFG || {};
    const prin = UI.PRIN || {};
    const mColor = safeColor(prin.c || '#ffffff');
    const outfit = prin.outfit || 'none';
    const tailsOn = Boolean(window.MikuMascot?.getPhys?.()?.twintails);
    const vals = [0, 8, 15, 30];
    const opacities = [75, 85, 96, 100];
    const radii = [20, 28, 32, 38];
    const preset = UI.PRESET || ['#F2A765', '#F06CF0', '#6C63FF', '#FF5C66', '#4FD3B1', '#3B9EFF', '#FFD84D', '#9BE15D'];
    const wardrobe = UI.WARDROBE || Object.entries(UI.OL || { none: 'Ninguno' });

    return `
      <section class="card">
        <div class="card-h"><h2>Comportamiento y geometría de la Isla</h2></div>
        <div class="item">
          <div><div class="item-t">Auto-ocultar al reposo</div><div class="item-d">Segundos antes de volver a píldora compacta.</div></div>
          <div class="pill-row">${vals.map(v => `<button type="button" class="set-pill${Number(cfg.autoclose ?? 15) === v ? ' on' : ''}" data-setting-autoclose="${v}">${v === 0 ? 'Nunca' : `${v}s`}</button>`).join('')}</div>
        </div>
        <div class="item">
          <div><div class="item-t">Abrir al pasar el cursor</div><div class="item-d">Despliega la isla con debounce suave sin rebote al minimizar.</div></div>
          <button type="button" class="set-pill${cfg.hover_open ? ' on' : ''}" data-toggle-setting="hover_open">${cfg.hover_open ? 'Activo' : 'Manual'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Opacidad del chasis</div><div class="item-d">Transparencia armonizada con el fondo y orejas notch.</div></div>
          <div class="pill-row">${opacities.map(v => `<button type="button" class="set-pill${Number(cfg.island_opacity ?? 96) === v ? ' on' : ''}" data-setting-num="island_opacity:${v}">${v}%</button>`).join('')}</div>
        </div>
        <div class="item">
          <div><div class="item-t">Radio de esquinas expandida</div><div class="item-d">Curvatura suave de la isla abierta.</div></div>
          <div class="pill-row">${radii.map(v => `<button type="button" class="set-pill${Number(cfg.island_radius ?? 32) === v ? ' on' : ''}" data-setting-num="island_radius:${v}">${v}px</button>`).join('')}</div>
        </div>
        <div class="item">
          <div><div class="item-t">Coletas de Miku</div><div class="item-d">Activa u oculta coletas sobre el avatar minimalista.</div></div>
          <button type="button" class="set-pill${tailsOn ? ' on' : ''}" id="set-toggle-tails">${tailsOn ? 'Visibles' : 'Minimalista'}</button>
        </div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Personalizar apariencia y vestuario</h2></div>
        <div class="item">
          <div style="display:flex;align-items:center;gap:14px">
            <span class="apw lg" data-av="${esc(mColor)}|${esc(outfit)}|full|0|"><canvas class="apimg" width="84" height="84"></canvas></span>
            <div>
              <div class="item-t">${esc(prin.name || 'Miku')} · ${esc(UI.OL?.[outfit] || outfit)}</div>
              <div class="item-d">Los cambios de color y accesorios se reflejan al instante en la Isla.</div>
            </div>
          </div>
          <div class="chips">
            <button type="button" class="sw8${mColor.toLowerCase() === '#ffffff' ? ' on' : ''}" data-miku-color="#ffffff" style="--c:#ffffff" title="Blanco clásico"></button>
            ${preset.map(hex => `<button type="button" class="sw8${mColor.toLowerCase() === hex.toLowerCase() ? ' on' : ''}" data-miku-color="${esc(hex)}" style="--c:${esc(hex)}" title="${esc(hex)}"></button>`).join('')}
            <label class="colorin" title="Color personalizado"><input type="color" id="set-miku-colin" value="${esc(mColor)}"></label>
          </div>
        </div>
        <div class="pill-row">
          ${wardrobe.map(([id, label]) => `<button type="button" class="wb${outfit === id ? ' on' : ''}" data-miku-outfit="${esc(id)}">${esc(label)}</button>`).join('')}
        </div>
      </section>`;
  }

  function formatUnifiedDiff(diffText) {
    return String(diffText || '').split(/\r?\n/).map(line => {
      const cls = line.startsWith('+') && !line.startsWith('+++')
        ? 'add'
        : (line.startsWith('-') && !line.startsWith('---') ? 'del' : 'ctx');
      return `<div class="diff-line ${cls}">${esc(line || ' ')}</div>`;
    }).join('');
  }

  function renderAgentsSection() {
    const list = allAgents();
    const sel = list.find(a => a.id === selectedAgentId) || list[0];
    const preset = UI.PRESET || ['#F2A765', '#F06CF0', '#6C63FF', '#FF5C66', '#4FD3B1', '#3B9EFF', '#FFD84D', '#9BE15D'];
    const wardrobe = UI.WARDROBE || Object.entries(UI.OL || { none: 'Ninguno' });
    const providers = UI.MIKU_PROVIDERS || [];
    const harnesses = Array.isArray(UI.ST?.harnesses) ? UI.ST.harnesses : [];
    const promptVal = sel?.prompt || '';
    const estTokens = Math.ceil(promptVal.length / 4);

    const harnessesHtml = harnesses.length
      ? harnesses.map(h => {
        const showingDiff = hookPreview && hookPreview.agent === h.id;
        const canHook = Boolean(h.supports_hook_installer && (h.dir_exists || h.installed));
        const hasBackup = Number(h.backup_count || 0) > 0;
        const sesNote = h.passive_sessions ? ` · ${h.passive_sessions} sesiones` : '';
        return `
          <div class="harness-card">
            <div class="harness-head">
              <div>
                <div class="item-t">${esc(h.name || h.id)}${sesNote}</div>
                <div class="item-d">${esc(h.status_note || h.integration_mode || '')} · <code>${esc(h.config_path || '')}</code></div>
              </div>
              <div class="pill-row">
                <span class="badge-sm ${h.installed ? 'ok' : ''}">${h.installed ? 'Detectado en PC' : 'No detectado'}</span>
                <span class="badge-sm ${h.hook_installed ? 'ok' : 'warn'}">${h.hook_installed ? 'Hook Activo' : (canHook ? 'Listo p/ Hook' : 'Solo detección')}</span>
                ${canHook ? `<button type="button" class="set-btn" data-hook-preview="${esc(h.id)}"${hookBusy ? ' disabled' : ''}>${showingDiff ? 'Actualizar Diff' : 'Ver Diff Hook'}</button>` : ''}
                ${hasBackup ? `<button type="button" class="set-btn" data-hook-restore="${esc(h.id)}"${hookBusy ? ' disabled' : ''}>Restaurar .bak (${h.backup_count})</button>` : ''}
              </div>
            </div>
            ${showingDiff ? `
              <div class="diff-box">${formatUnifiedDiff(hookPreview.unified_diff || '(Sin cambios requeridos; configuración ya sincronizada)')}</div>
              <div class="pill-row" style="justify-content:space-between">
                <span class="item-d">Destino: <code>${esc(hookPreview.settings_path || h.config_path || '')}</code></span>
                <div class="pill-row">
                  <button type="button" class="set-btn" id="set-hook-cancel">Cerrar</button>
                  ${!hookPreview.idempotent_noop ? `<button type="button" class="set-btn prim" data-hook-install="${esc(h.id)}"${hookBusy ? ' disabled' : ''}>Confirmar e Instalar (con .bak)</button>` : '<span class="badge-sm ok">Ya actualizado ✓</span>'}
                </div>
              </div>` : ''}
          </div>`;
      }).join('')
      : '<div class="item-d">Pulsa Escanear PC para detectar Antigravity IDE y Git/GitHub instalados.</div>';

    return `
      <section class="card">
        <div class="card-h">
          <h2>Harnesses instalados &amp; Hooks (estilo Coucou)</h2>
          <div class="pill-row">
            ${hookNotice ? `<span class="set-pill status-ok">${esc(hookNotice)}</span>` : ''}
            <button type="button" class="set-btn prim" id="set-scan-harnesses"${hookBusy ? ' disabled' : ''}>Escanear PC</button>
          </div>
        </div>
        <div class="item-d">Cómo funciona en 3 pasos: <b>1)</b> Pulsa <b>Escanear PC</b> &nbsp;→&nbsp; <b>2)</b> Abre <b>Ver Diff Hook</b> e instala el hook en <code>~/.gemini/config/hooks.json</code> (crea respaldo <code>.bak</code>) &nbsp;→&nbsp; <b>3)</b> En la Isla selecciona <b>Antigravity</b> o <b>GitHub</b> para revisar proyectos, diffs y sesiones.</div>
        <div class="item" style="margin-top:8px">
          <div><div class="item-t">Computer Use automático (clics y UIA)</div><div class="item-d">Permite a los agentes hacer clic en pantalla sin pedir confirmación en cada paso.</div></div>
          <button type="button" class="set-pill${UI.CFG?.auto_pc_control ? ' on' : ''}" data-toggle-setting="auto_pc_control">${UI.CFG?.auto_pc_control ? 'Auto-Click ON' : 'Pedir aprobación'}</button>
        </div>
        ${harnessesHtml}
      </section>
      <section class="card">
        <div class="card-h">
          <h2>Agentes predefinidos personalizables (${list.length})</h2>
          <button type="button" class="set-btn" id="set-reset-all-agents">Restaurar curados</button>
        </div>
        <div class="ag-grid">
          ${list.map((ag, i) => `
            <button type="button" class="ag-pill${ag.id === sel?.id ? ' on' : ''}${ag.active === false ? ' inactive' : ''}" data-select-ag="${esc(ag.id)}">
              <span class="apw" data-av="${esc(safeColor(ag.c || '#ffffff'))}|${esc(ag.outfit || 'none')}|full|${i}|"><canvas class="apimg" width="84" height="84"></canvas></span>
              <span style="text-align:left">
                <div class="item-t">${esc(ag.name || ag.id)}</div>
                <div class="item-d">${esc(ag.role || 'Especialista')} · ${ag.active === false ? 'Pausado' : (Number(ag.isl ?? 1) > 0 ? 'Dock' : 'Selector')}</div>
              </span>
            </button>`).join('')}
        </div>
      </section>
      ${sel ? `
      <section class="card">
        <div class="card-h">
          <h2>Personalizar: ${esc(sel.name || sel.id)}</h2>
          <div class="pill-row">
            <span class="badge-sm">${sel.predefined ? 'Curado predefinido' : 'agent.toml local'}</span>
            ${sel.predefined ? `<button type="button" class="set-btn" id="set-ag-reset-one" data-ag-id="${esc(sel.id)}">Restaurar defecto</button>` : ''}
          </div>
        </div>
        <div class="grid-2">
          <input type="text" class="set-inp" id="set-ag-name" value="${esc(sel.name || '')}" placeholder="Nombre visible" aria-label="Nombre agente">
          <input type="text" class="set-inp" id="set-ag-role" value="${esc(sel.role || '')}" placeholder="Rol o especialidad" aria-label="Rol agente">
          <select class="set-inp" id="set-ag-provider" aria-label="Proveedor agente">
            <option value=""${!sel.provider ? ' selected' : ''}>Proveedor: Global (${esc(UI.CFG?.provider || 'deepseek')})</option>
            ${providers.map(p => `<option value="${esc(p.id)}"${sel.provider === p.id ? ' selected' : ''}>${esc(p.label)}</option>`).join('')}
          </select>
          <input type="text" class="set-inp" id="set-ag-model" value="${esc(sel.model || '')}" placeholder="Modelo (vacío = global)" aria-label="Modelo agente">
          <select class="set-inp" id="set-ag-outfit" aria-label="Vestuario agente">
            ${wardrobe.map(([id, lbl]) => `<option value="${esc(id)}"${(sel.outfit || 'none') === id ? ' selected' : ''}>${esc(lbl)}</option>`).join('')}
          </select>
          <select class="set-inp" id="set-ag-tools" aria-label="Modo herramientas">
            <option value="auto"${(sel.tools_mode || 'auto') === 'auto' ? ' selected' : ''}>Tools: Auto (ahorra ~480 tokens)</option>
            <option value="always"${sel.tools_mode === 'always' ? ' selected' : ''}>Tools: Siempre activas</option>
            <option value="never"${sel.tools_mode === 'never' ? ' selected' : ''}>Tools: Solo texto (0 schemas)</option>
          </select>
        </div>
        ${sel.id !== 'miku' ? `
        <div class="item">
          <div><div class="item-t">Visibilidad del agente</div><div class="item-d">Pausa agentes que no uses sin perder su configuración ni historia.</div></div>
          <div class="pill-row">
            <label class="badge-sm"><input type="checkbox" id="set-ag-active"${sel.active !== false ? ' checked' : ''}> Activo en chat</label>
            <label class="badge-sm"><input type="checkbox" id="set-ag-isl"${Number(sel.isl ?? 1) > 0 ? ' checked' : ''}> Mostrar en Dock 2×2</label>
          </div>
        </div>` : ''}
        <div style="display:flex;flex-direction:column;gap:6px">
          <div style="display:flex;justify-content:space-between;align-items:center">
            <span class="item-t">Directiva / SOUL del agente (prefijo cacheable estático)</span>
            <span class="badge-sm" id="set-ag-prompt-tok">~${estTokens} tokens</span>
          </div>
          <textarea class="set-textarea" id="set-ag-prompt" placeholder="Instrucciones concisas de comportamiento...">${esc(promptVal)}</textarea>
        </div>
        <div class="item">
          <div class="chips">
            ${preset.map(hex => `<button type="button" class="sw8${safeColor(sel.c).toLowerCase() === hex.toLowerCase() ? ' on' : ''}" data-edit-ag-color="${esc(hex)}" style="--c:${esc(hex)}"></button>`).join('')}
            <label class="colorin"><input type="color" id="set-ag-colin" value="${esc(safeColor(sel.c))}"></label>
          </div>
          <button type="button" class="set-btn prim" id="set-ag-save" data-ag-id="${esc(sel.id)}">Guardar cambios</button>
        </div>
      </section>` : ''}`;
  }

  function renderProvidersSection() {
    const cfg = UI.CFG || {};
    const synced = cfg.synced_models || {};
    const baseProviders = UI.MIKU_PROVIDERS || [
      { id: 'deepseek', label: 'DeepSeek', models: ['deepseek-chat', 'deepseek-reasoner'] },
      { id: 'groq', label: 'Groq', models: ['llama-3.3-70b-versatile', 'qwen/qwen3-32b'] },
      { id: 'gemini', label: 'Google Gemini', models: ['gemini-2.5-flash', 'gemini-2.5-pro'] },
      { id: 'anthropic', label: 'Claude (Anthropic)', models: ['claude-sonnet-4-5', 'claude-haiku-4-5'] },
      { id: 'openai', label: 'OpenAI', models: ['gpt-4o-mini', 'gpt-4o'] },
      { id: 'ollama', label: 'Ollama (Local)', models: ['qwen2.5-coder:7b', 'llama3.2:latest'] }
    ];
    const providers = baseProviders.map(p => {
      const live = Array.isArray(synced[p.id]) && synced[p.id].length ? synced[p.id] : p.models;
      return { ...p, models: live, isSynced: Array.isArray(synced[p.id]) && synced[p.id].length > 0 };
    });
    const prov = cfg.provider || 'deepseek';
    const activeProv = providers.find(p => p.id === prov) || providers[0];
    const mod = cfg.model || activeProv.models[0];
    const maxTok = Number(cfg.max_tokens || 384);
    return `
      <section class="card">
        <div class="card-h">
          <h2>Proveedor activo</h2>
          <button type="button" class="set-btn" id="set-sync-active-models" data-sync-provider="${esc(prov)}">Sincronizar modelos (${esc(activeProv.label)})</button>
        </div>
        <div class="pill-row">
          ${providers.map(p => `<button type="button" class="set-pill${prov === p.id ? ' on' : ''}" data-pick-provider="${esc(p.id)}" data-pick-model="${esc(p.models[0])}">${esc(p.label)}${p.isSynced ? ' ✓' : ''}</button>`).join('')}
        </div>
        <div class="pill-row" style="max-height:140px;overflow:auto">
          ${(activeProv.models || []).slice(0, 24).map(m => `<button type="button" class="set-pill${mod === m ? ' on' : ''}" data-pick-model-only="${esc(m)}">${esc(m)}</button>`).join('')}
        </div>
        <div class="item">
          <div><div class="item-t">Modelo por defecto</div><div class="item-d">Se enviará en el próximo turno del agente.</div></div>
          <div class="pill-row">
            <input type="text" class="set-inp" id="set-model-inp" value="${esc(mod)}" aria-label="Modelo">
            <button type="button" class="set-btn prim" id="set-model-save">Aplicar</button>
          </div>
        </div>
        <div class="item">
          <div><div class="item-t">Máximo de tokens por turno</div><div class="item-d">Controla gasto y longitud máxima de respuesta.</div></div>
          <div class="pill-row">
            ${[256, 384, 768, 1536].map(n => `<button type="button" class="set-pill${maxTok === n ? ' on' : ''}" data-setting-maxtok="${n}">${n}</button>`).join('')}
          </div>
        </div>
      </section>`;
  }

  function renderKeysSection(ST) {
    const cfg = UI.CFG || {};
    const secrets = Array.isArray(ST.secrets) ? ST.secrets : [];
    const synced = cfg.synced_models || {};
    const prov = cfg.provider || 'deepseek';
    const categories = [
      {
        title: 'Modelos de IA (sincronizan catálogo oficial al guardar)',
        items: [
          { id: 'deepseek', label: 'DeepSeek', hint: 'deepseek-chat, deepseek-reasoner', syncable: true },
          { id: 'groq', label: 'Groq', hint: 'LLM rápido + Whisper STT', syncable: true },
          { id: 'gemini', label: 'Google Gemini', hint: 'Gemini Flash/Pro + Gemini TTS', syncable: true },
          { id: 'anthropic', label: 'Claude (Anthropic)', hint: 'Claude Sonnet / Haiku / Opus', syncable: true },
          { id: 'openai', label: 'OpenAI', hint: 'GPT-4o / o3 / o4-mini', syncable: true }
        ]
      },
      {
        title: 'Voz Neural & Integraciones',
        items: [
          { id: 'elevenlabs', label: 'ElevenLabs (TTS)', hint: 'Voces femeninas neurales configurables', syncable: false },
          { id: 'github', label: 'GitHub (PAT)', hint: 'Repos, PRs e Issues privados', syncable: false }
        ]
      }
    ];
    return `
      <section class="card">
        <div class="card-h">
          <h2>API Keys por categoría (DPAPI · ${secrets.length} guardadas)</h2>
          ${secretTestStatus ? `<span class="set-pill ${secretTestStatus.startsWith('OK') || secretTestStatus.startsWith('Guardado') ? 'status-ok' : 'status-er'}">${esc(secretTestStatus)}</span>` : ''}
        </div>
        ${categories.map(cat => `
          <div style="margin-top:10px">
            <div class="item-d" style="font-weight:600;margin-bottom:6px">${esc(cat.title)}</div>
            ${cat.items.map(it => {
              const hasKey = secrets.includes(it.id);
              const count = Array.isArray(synced[it.id]) ? synced[it.id].length : 0;
              return `
                <div class="item" style="gap:8px;flex-wrap:wrap">
                  <div style="min-width:170px">
                    <div class="item-t">${esc(it.label)} <span class="badge-sm ${hasKey ? 'ok' : ''}">${hasKey ? 'DPAPI ✓' : 'Sin clave'}</span>${count ? ` <span class="badge-sm ok">${count} modelos</span>` : ''}</div>
                    <div class="item-d">${esc(it.hint)}</div>
                  </div>
                  <input type="password" class="set-inp" data-key-input="${esc(it.id)}" style="flex:1;min-width:180px" placeholder="${hasKey ? 'Actualizar API key...' : `Pegar API key de ${it.label}...`}" aria-label="API Key ${esc(it.label)}">
                  <div class="pill-row">
                    <button type="button" class="set-btn prim" data-save-sync-key="${esc(it.id)}" data-syncable="${it.syncable ? '1' : '0'}">${it.syncable ? 'Guardar + Sync' : 'Guardar'}</button>
                    ${hasKey && it.syncable ? `<button type="button" class="set-btn" data-test-key="${esc(it.id)}">Probar</button>` : ''}
                    ${hasKey ? `<button type="button" class="set-btn" data-del-secret="${esc(it.id)}" title="Eliminar">×</button>` : ''}
                  </div>
                </div>`;
            }).join('')}
          </div>`).join('')}
        <div class="item" style="margin-top:12px;border-top:1px solid rgba(255,255,255,.08);padding-top:10px">
          <input type="text" class="set-inp" id="set-sec-alias" value="${esc(prov)}" placeholder="alias personalizado" aria-label="Alias secreto">
          <input type="password" class="set-inp" id="set-sec-val" style="flex:1" placeholder="Otro secreto DPAPI (nunca se reenvía al UI)" aria-label="API Key">
          <button type="button" class="set-btn" id="set-sec-save">Guardar alias</button>
          <button type="button" class="set-btn" id="set-sec-test">Probar activo</button>
        </div>
      </section>`;
  }

  function renderMcpSection(ST) {
    const mcps = Array.isArray(ST.mcp) ? ST.mcp : [];
    const cat = Array.isArray(UI.runtime?.mcpCatalog) ? UI.runtime.mcpCatalog : [];
    return `
      <section class="card">
        <div class="card-h"><h2>Servidores MCP instalados (${mcps.length})</h2></div>
        ${mcps.map(m => `
          <div class="item">
            <div><div class="item-t">${esc(m.name)}</div><div class="item-d">${esc(m.command)} ${esc((m.args || []).join(' '))}</div></div>
            <span class="set-pill status-ok">Configurado</span>
          </div>`).join('') || '<div class="item-d">Sin servidores MCP registrados todavía.</div>'}
        <div class="item">
          <input type="text" class="set-inp" id="set-mcp-name" placeholder="Nombre (ej. docs)" aria-label="MCP nombre">
          <input type="text" class="set-inp" id="set-mcp-cmd" placeholder="Comando (npx / uvx)" aria-label="MCP comando">
          <input type="text" class="set-inp" id="set-mcp-args" style="flex:1" placeholder="Argumentos separados por espacio" aria-label="MCP args">
          <button type="button" class="set-btn prim" id="set-mcp-add">Añadir</button>
        </div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Catálogo Online MCP (${cat.length})</h2><button type="button" class="set-btn" id="set-mcp-cat-refresh">Actualizar catálogo</button></div>
        <div class="grid-2">
          ${cat.map(c => `
            <div class="item" style="flex-direction:column;align-items:stretch;gap:8px">
              <div style="display:flex;justify-content:space-between;align-items:center">
                <span class="item-t">${esc(c.name)}</span>
                <span class="set-pill">${esc(c.tag || 'mcp')}</span>
              </div>
              <div class="item-d">${esc(c.description)}</div>
              <button type="button" class="set-btn ${c.installed ? 'status-ok' : 'prim'}" data-install-mcp="${esc(c.id)}">${c.installed ? 'Reinstalar / Activo' : 'Instalar MCP'}</button>
            </div>`).join('') || '<div class="item-d">Pulsa Actualizar catálogo para explorar servidores MCP.</div>'}
        </div>
      </section>`;
  }

  function renderSkillsSection(ST) {
    const skills = Array.isArray(ST.skills) ? ST.skills : [];
    return `
      <section class="card">
        <div class="card-h"><h2>Skills locales (${skills.length})</h2><button type="button" class="set-btn prim" id="set-skills-reload">Recargar Skills</button></div>
        <div class="pill-row">${skills.map(s => `<span class="set-pill">${esc(typeof s === 'string' ? s : s.name || s.id)}</span>`).join('') || '<span class="item-d">Ningún skill cargado aún.</span>'}</div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Memoria SQLite FTS5</h2></div>
        <div class="item">
          <input type="text" class="set-inp" id="set-mem-q" style="flex:1" value="${esc(memoryQuery)}" placeholder="Buscar recuerdos indexados por BM25..." aria-label="Buscar memoria">
          <button type="button" class="set-btn prim" id="set-mem-search">Buscar</button>
        </div>
        ${memoryHits.length ? memoryHits.map(h => `
          <div class="item">
            <div><div class="item-t">${esc(h.title || h.doc_id)}</div><div class="item-d">${esc(h.snippet || '')}</div></div>
            <span class="set-pill">${Number(h.bm25_score || 0).toFixed(2)}</span>
          </div>`).join('') : '<div class="item-d">Ingresa una palabra clave para consultar el índice FTS5.</div>'}
      </section>`;
  }

  function renderTelemetrySection(ST) {
    const cfg = UI.CFG || {};
    const tel = ST.telemetry || { total_requests: 0, total_tokens: 0, estimated_usd_cost: 0, avg_latency_ms: 0, by_model: [] };
    const rows = Array.isArray(tel.by_model) ? tel.by_model : [];
    const cb = tel.circuit_breaker || {};
    const localRouterOn = cfg.token_saver_local_router !== 0 && cfg.token_saver_local_router !== false;
    const prefixCacheOn = cfg.token_saver_prefix_cache !== 0 && cfg.token_saver_prefix_cache !== false;
    const pruneToolsOn = cfg.token_saver_prune_tools !== 0 && cfg.token_saver_prune_tools !== false;
    const histTurns = Number(cfg.history_turns ?? 6);
    const budgetUsd = Number(cfg.daily_budget_usd ?? 5);
    return `
      <section class="card">
        <div class="card-h">
          <h2>Resumen de Consumo y Ahorro de Tokens</h2>
          <div class="pill-row">
            <button type="button" class="set-btn" id="set-tel-refresh">Actualizar</button>
            <button type="button" class="set-btn" id="set-tel-reset">Reiniciar Métricas</button>
          </div>
        </div>
        <div class="telemetry-summary-cards">
          <div class="tsc-card"><div class="tsc-label">Peticiones API</div><div class="tsc-val">${Number(tel.total_requests || 0).toLocaleString()}</div></div>
          <div class="tsc-card"><div class="tsc-label">Tokens API</div><div class="tsc-val">${Number(tel.total_tokens || 0).toLocaleString()}</div></div>
          <div class="tsc-card"><div class="tsc-label">Coste USD</div><div class="tsc-val">$${Number(tel.estimated_usd_cost || 0).toFixed(5)}</div></div>
          <div class="tsc-card"><div class="tsc-label">Latencia Prom.</div><div class="tsc-val">${Math.round(tel.avg_latency_ms || 0)} ms</div></div>
          <div class="tsc-card"><div class="tsc-label">0-Token Local</div><div class="tsc-val">${Number(tel.local_routed_requests || 0).toLocaleString()}</div></div>
          <div class="tsc-card"><div class="tsc-label">KV Cache Hits</div><div class="tsc-val">${Number(tel.cached_prompt_tokens || 0).toLocaleString()}</div></div>
          <div class="tsc-card"><div class="tsc-label">Tokens Ahorrados</div><div class="tsc-val">${Number(tel.estimated_tokens_saved || 0).toLocaleString()}</div></div>
          <div class="tsc-card"><div class="tsc-label">Gasto Hoy</div><div class="tsc-val">$${Number(tel.daily_usd_spent || 0).toFixed(4)} / $${budgetUsd}</div></div>
        </div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Pipeline de Ahorro de Tokens</h2></div>
        <div class="item">
          <div><div class="item-t">LocalIntentRouter (0 tokens)</div><div class="item-d">Responde saludos, hora, ayuda y estado local sin invocar API externa.</div></div>
          <button type="button" class="set-pill${localRouterOn ? ' on' : ''}" data-toggle-num="token_saver_local_router">${localRouterOn ? 'Activo' : 'Off'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">KV Prefix Cache estático ([MIKU-SYS-V1])</div><div class="item-d">Mantiene el System Prompt determinista y mueve FTS5 + archivos al turno de usuario.</div></div>
          <button type="button" class="set-pill${prefixCacheOn ? ' on' : ''}" data-toggle-num="token_saver_prefix_cache">${prefixCacheOn ? 'Activo' : 'Off'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Tool Schema Pruning</div><div class="item-d">Omite esquemas JSON de herramientas (~480 tokens) en consultas conversacionales.</div></div>
          <button type="button" class="set-pill${pruneToolsOn ? ' on' : ''}" data-toggle-num="token_saver_prune_tools">${pruneToolsOn ? 'Activo' : 'Off'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Ventana de historial enviado a API</div><div class="item-d">Cantidad máxima de turnos recientes adjuntados por petición.</div></div>
          <div class="pill-row">${[4, 6, 10, 16].map(n => `<button type="button" class="set-pill${histTurns === n ? ' on' : ''}" data-setting-num="history_turns:${n}">${n} turnos</button>`).join('')}</div>
        </div>
        <div class="item">
          <div><div class="item-t">Presupuesto diario de gasto USD</div><div class="item-d">Frena nuevas llamadas externas si se supera el límite diario configurado.</div></div>
          <div class="pill-row">${[1, 5, 10, 25].map(v => `<button type="button" class="set-pill${budgetUsd === v ? ' on' : ''}" data-setting-num="daily_budget_usd:${v}">$${v}</button>`).join('')}</div>
        </div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Circuit-Breaker</h2></div>
        <div class="circuit-breaker-grid">
          <div class="circuit-node">
            <div style="display:flex;justify-content:space-between;align-items:center">
              <span class="item-t">1. Proveedor Principal</span>
              <span class="set-pill ${cb.primary?.healthy !== false ? 'status-ok' : 'status-er'}">${esc(cb.primary?.status || 'CLOSED')}</span>
            </div>
            <div class="item-d" style="margin-top:4px">${esc(cb.primary?.name || `${UI.CFG?.provider || 'deepseek'} / ${UI.CFG?.model || 'deepseek-chat'}`)} · Fallos: ${cb.primary?.failures || 0}</div>
          </div>
          <div class="circuit-node">
            <div style="display:flex;justify-content:space-between;align-items:center">
              <span class="item-t">2. Respaldo Local</span>
              <span class="set-pill status-ok">${esc(cb.secondary?.status || 'CLOSED')}</span>
            </div>
            <div class="item-d" style="margin-top:4px">${esc(cb.secondary?.name || 'Ollama Local (Failover)')}</div>
          </div>
        </div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Desglose por modelo</h2></div>
        ${rows.length ? `
          <table class="telemetry-table">
            <thead><tr><th>Modelo</th><th>Peticiones</th><th>Tokens</th><th>Coste USD</th><th>Latencia</th></tr></thead>
            <tbody>${rows.map(r => `<tr><td>${esc(r.model)}</td><td>${r.requests}</td><td>${r.tokens}</td><td>$${Number(r.cost_usd || 0).toFixed(5)}</td><td>${Math.round(r.latency_ms || 0)} ms</td></tr>`).join('')}</tbody>
          </table>` : '<div class="item-d">Sin llamadas registradas aún en esta sesión.</div>'}
      </section>`;
  }

  function renderNotificationsSection() {
    const cfg = UI.CFG || {};
    const notifOn = cfg.notifications !== 0 && cfg.notifications !== false;
    const bargeOn = cfg.barge_in !== 0 && cfg.barge_in !== false;
    const limit = Number(cfg.app_time_limit_min || 0);
    return `
      <section class="card">
        <div class="card-h"><h2>Alertas y monitoreo</h2></div>
        <div class="item">
          <div><div class="item-t">Notificaciones proactivas</div><div class="item-d">Muestra avisos de progreso y confirmaciones en la Isla.</div></div>
          <button type="button" class="set-pill${notifOn ? ' on' : ''}" data-toggle-num="notifications">${notifOn ? 'Activas' : 'Pausadas'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Barge-in por voz</div><div class="item-d">Interrumpe la síntesis TTS si empiezas a hablar o escribir.</div></div>
          <button type="button" class="set-pill${bargeOn ? ' on' : ''}" data-toggle-num="barge_in">${bargeOn ? 'Activo' : 'Desactivado'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Recordatorio de tiempo en pantalla</div><div class="item-d">Avisa cuando excedas minutos continuos en una app.</div></div>
          <div class="pill-row">
            ${[0, 30, 60, 120].map(m => `<button type="button" class="set-pill${limit === m ? ' on' : ''}" data-setting-limit="${m}">${m === 0 ? 'Off' : `${m}m`}</button>`).join('')}
          </div>
        </div>
      </section>`;
  }

  function renderVoiceSection() {
    const cfg = UI.CFG || {};
    const voiceOn = cfg.voice !== 0 && cfg.voice !== false;
    const sfxOn = cfg.sfx_enabled !== 0 && cfg.sfx_enabled !== false;
    const sttEnabled = Boolean(cfg.voice_stt_enabled);
    const sttEngine = cfg.voice_stt_engine || 'off';
    const autoSend = Boolean(cfg.voice_auto_send);
    const devs = Array.isArray(UI.runtime?.audioOutputs) ? UI.runtime.audioOutputs : [];
    const sounds = Array.isArray(UI.runtime?.soundFiles) ? UI.runtime.soundFiles : [];
    const ttsEngine = cfg.voice_tts_engine || 'auto';
    const ttsVoice = cfg.voice_tts_voice || 'Kore';
    const wakeWordOn = Boolean(cfg.voice_wake_word_enabled);
    const geminiVoices = [
      { id: 'Kore', label: 'Kore (Femenina · Natural)' },
      { id: 'Aoede', label: 'Aoede (Femenina · Cálida)' },
      { id: 'Leda', label: 'Leda (Femenina · Juvenil)' },
      { id: 'Zephyr', label: 'Zephyr (Femenina · Brillante)' }
    ];
    const elevenVoices = [
      { id: '21m00Tcm4TlvDq8ikWAM', label: 'Rachel (Femenina)' },
      { id: 'EXAVITQu4vr4xnSDxMaL', label: 'Bella (Femenina)' },
      { id: 'MF3mGyEYCl7XYWbV9V6O', label: 'Elli (Femenina)' },
      { id: 'AZnzlk1XvdvUeBnXmlld', label: 'Domi (Femenina)' }
    ];
    const voiceChoices = ttsEngine === 'elevenlabs' ? elevenVoices : geminiVoices;
    return `
      <section class="card">
        <div class="card-h">
          <h2>Voz Femenina Neural (TTS) &amp; Niveles</h2>
          <div class="pill-row">
            <button type="button" class="set-btn" id="set-test-voice">Probar voz</button>
            <button type="button" class="set-btn" id="set-test-sfx">Probar chime</button>
          </div>
        </div>
        <div class="item">
          <div><div class="item-t">Motor de voz TTS</div><div class="item-d">Usa Gemini TTS (<code>gemini-2.5-flash-preview-tts</code>) o ElevenLabs si tienes clave DPAPI, con fallback local.</div></div>
          <div class="pill-row">
            ${[
              { id: 'auto', label: 'Auto' },
              { id: 'gemini', label: 'Gemini TTS' },
              { id: 'elevenlabs', label: 'ElevenLabs' },
              { id: 'browser', label: 'Sistema' }
            ].map(e => `<button type="button" class="set-pill${ttsEngine === e.id ? ' on' : ''}" data-setting-kv="voice_tts_engine:${e.id}">${e.label}</button>`).join('')}
          </div>
        </div>
        ${ttsEngine !== 'browser' ? `
        <div class="item">
          <div><div class="item-t">Voz femenina seleccionada</div><div class="item-d">Configura qué voz sale en las respuestas habladas.</div></div>
          <div class="pill-row">
            ${voiceChoices.map(v => `<button type="button" class="set-pill${ttsVoice === v.id ? ' on' : ''}" data-setting-kv="voice_tts_voice:${v.id}">${v.label}</button>`).join('')}
          </div>
        </div>` : ''}
        <div class="item">
          <div><div class="item-t">Volumen de voz (TTS)</div><div class="item-d">Ajuste de síntesis de respuestas habladas.</div></div>
          <div id="set-slider-voice"></div>
        </div>
        <div class="item">
          <div><div class="item-t">Volumen de efectos (SFX)</div><div class="item-d">Sonidos Web Audio al hacer clic o recibir eventos.</div></div>
          <div id="set-slider-sfx"></div>
        </div>
        <div class="item">
          <div><div class="item-t">Canales habilitados</div><div class="item-d">Desacoplamiento entre voz hablada y efectos de interfaz.</div></div>
          <div class="pill-row">
            <button type="button" class="set-pill${voiceOn ? ' on' : ''}" data-toggle-num="voice">${voiceOn ? 'Voz ON' : 'Voz OFF'}</button>
            <button type="button" class="set-pill${sfxOn ? ' on' : ''}" data-toggle-num="sfx_enabled">${sfxOn ? 'SFX ON' : 'SFX OFF'}</button>
          </div>
        </div>
      </section>
      <section class="card">
        <div class="card-h">
          <h2>Reconocimiento de voz (STT)</h2>
          <button type="button" class="set-pill${sttEnabled ? ' on' : ''}" data-toggle-setting="voice_stt_enabled">${sttEnabled ? 'STT Activo' : 'STT Desactivado'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Motor de transcripción</div><div class="item-d">Selecciona el motor de reconocimiento para dictado y PTT.</div></div>
          <div class="pill-row">
            ${[
              { id: 'off', label: 'Off' },
              { id: 'browser_webspeech', label: 'WebSpeech' },
              { id: 'groq_whisper', label: 'Groq Whisper' },
              { id: 'local_sapi', label: 'Windows SAPI' }
            ].map(e => `<button type="button" class="set-pill${sttEngine === e.id ? ' on' : ''}" data-setting-kv="voice_stt_engine:${e.id}">${e.label}</button>`).join('')}
          </div>
        </div>
        <div class="item">
          <div><div class="item-t">Envío automático</div><div class="item-d">Envía la consulta al chat automáticamente al finalizar la transcripción.</div></div>
          <button type="button" class="set-pill${autoSend ? ' on' : ''}" data-toggle-setting="voice_auto_send">${autoSend ? 'Auto-enviar ON' : 'Auto-enviar OFF'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Palabra de activación ("Oye Miku")</div><div class="item-d">Escucha pasiva en la Isla para abrir el micrófono al decir "Oye Miku".</div></div>
          <button type="button" class="set-pill${wakeWordOn ? ' on' : ''}" data-toggle-setting="voice_wake_word_enabled">${wakeWordOn ? 'Oye Miku ON' : 'Oye Miku OFF'}</button>
        </div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Dispositivos de audio Windows (${devs.length})</h2><button type="button" class="set-btn" id="set-audio-refresh">Escanear WinMM</button></div>
        ${devs.map(d => `<div class="item"><div class="item-t">${esc(d.name)}</div><span class="set-pill ${d.is_default ? 'status-ok' : ''}">${d.is_default ? 'Predeterminado' : 'Disponible'}</span></div>`).join('') || '<div class="item-d">Pulsa Escanear WinMM para listar dispositivos activos.</div>'}
        <div class="item-d">Audios personalizados detectados: ${sounds.length ? esc(sounds.join(', ')) : 'sintetizador Web Audio integrado'}</div>
      </section>`;
  }

  function renderLinkSection(ST) {
    if (window.MikuSettingsLink?.renderSection) {
      return window.MikuSettingsLink.renderSection(ST, qrData);
    }
    const cfg = UI.CFG || {};
    const lanOn = cfg.local_net !== 0 && cfg.local_net !== false;
    const paired = Boolean(ST.link && ST.name && ST.name !== 'pairing-pending');
    return `
      <section class="card">
        <div class="card-h">
          <h2>Miku-Link móvil (LAN / USB)</h2>
          <div class="pill-row">
            <button type="button" class="set-btn prim" id="set-qr-refresh">Generar QR</button>
            <button type="button" class="set-btn" id="set-link-unpair">Desvincular</button>
          </div>
        </div>
        <div class="item">
          <div><div class="item-t">Permitir emparejamiento en red local</div><div class="item-d">Activa escucha segura con QR temporal de un solo uso.</div></div>
          <button type="button" class="set-pill${lanOn ? ' on' : ''}" data-toggle-num="local_net">${lanOn ? 'Habilitado' : 'Bloqueado'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">${paired ? `Emparejado: ${esc(ST.name)}` : 'Listo para vincular dispositivo'}</div><div class="item-d">${qrData?.url ? esc(qrData.url) : 'Pulsa Generar QR para emitir un token temporal.'}</div></div>
        </div>
        ${qrData?.dataUrl && qrData.dataUrl.startsWith('data:image/') ? `<div class="qr-box"><img src="${esc(qrData.dataUrl)}" alt="Miku-Link QR" width="144" height="144"></div>` : ''}
      </section>`;
  }

  function renderSecuritySection(ST) {
    const ks = Boolean(ST.killSwitch);
    const mode = ST.permissionMode || 'sandbox';
    const cfg = UI.CFG || {};
    const autoCapture = Boolean(cfg.auto_screen_capture);
    const autoPcControl = Boolean(cfg.auto_pc_control);
    const sttEnabled = Boolean(cfg.voice_stt_enabled);
    const sttEngine = cfg.voice_stt_engine || 'off';
    const rules = Array.isArray(ST.rules) ? ST.rules : [];
    const modes = [
      { id: 'full', title: 'Toda mi PC', desc: 'Acceso amplio a escritura, red y consola excepto comandos destructivos inmutables.' },
      { id: 'sandbox', title: 'Sandbox', desc: 'Recomendado: escritura libre en workspace/, confirmación humana para notas/shell y red segura.' },
      { id: 'restricted', title: 'Restringido', desc: 'Solo lectura y controles multimedia seguros; escritura, portapapeles y consola bloqueados.' }
    ];
    return `
      <section class="card">
        <div class="card-h">
          <h2>Nivel de Permisos Gatekeeper</h2>
          ${permNotice ? `<span class="set-pill status-ok">${esc(permNotice)}</span>` : `<span class="set-pill">${esc(mode)}</span>`}
        </div>
        <div class="exec-mode-grid">
          ${modes.map(m => `
            <button type="button" class="mode-card${mode === m.id ? ' active on' : ''}" data-setmode="${esc(m.id)}">
              <div class="mode-card-head"><span>${esc(m.title)}</span></div>
              <div class="mode-card-desc">${esc(m.desc)}</div>
            </button>`).join('')}
        </div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Automatización, Visión y Voz Opt-In</h2></div>
        <div class="item">
          <div><div class="item-t">Captura de pantalla bajo demanda</div><div class="item-d">Excluye la ventana de Miku (WDA_EXCLUDEFROMCAPTURE) y bloquea ventanas de credenciales/UAC.</div></div>
          <button type="button" class="set-pill${autoCapture ? ' on' : ''}" data-toggle-setting="auto_screen_capture">${autoCapture ? 'Permitida sin prompt' : 'Pedir confirmación'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Control de PC (UIA + SendInput)</div><div class="item-d">Permite clics/teclas normalizados Multi-DPI con guardián RAII y bloqueo en ventanas sensibles.</div></div>
          <button type="button" class="set-pill${autoPcControl ? ' on' : ''}" data-toggle-setting="auto_pc_control">${autoPcControl ? 'Habilitado' : 'Solo bajo aprobación'}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Micrófono y STT Opt-In</div><div class="item-d">Motor actual: ${esc(sttEnabled ? sttEngine : 'desactivado (por defecto)')}. Ningún audio se abre sin tu permiso explícito.</div></div>
          <button type="button" class="set-pill${sttEnabled ? ' on' : ''}" data-toggle-setting="voice_stt_enabled">${sttEnabled ? 'Opt-In Activo' : 'Bloqueado (Opt-In)'}</button>
        </div>
      </section>
      <section class="card">
        <div class="card-h"><h2>Kill Switch &amp; Reglas activas (${rules.length})</h2></div>
        <div class="item">
          <div><div class="item-t">Interruptor de emergencia (Ctrl+Alt+Shift+K)</div><div class="item-d">Detiene al instante todas las herramientas y llamadas externas.</div></div>
          <button type="button" class="set-btn ${ks ? 'status-er' : 'status-ok'}" id="set-ks-toggle">${ks ? 'Pausado (Kill Switch ON)' : 'Activo (Normal)'}</button>
        </div>
        ${rules.map(r => `
          <div class="item">
            <div class="item-t">${esc(r.name)}</div>
            <span class="set-pill ${r.tier === 'allow' ? 'status-ok' : (r.tier === 'deny' ? 'status-er' : '')}">${esc(String(r.tier).toUpperCase())}</span>
          </div>`).join('') || '<div class="item-d">Reglas cargándose desde Gatekeeper...</div>'}
      </section>`;
  }

  function renderSystemSection(ST) {
    const apps = gwApps || ST.appsDetected || [];
    const doc = gwDoctorReport;
    const upd = updateInfo;
    const resetLabel = dangerArmed === 'settings' ? '¿Seguro? Confirmar restablecer ajustes' : 'Restablecer ajustes';
    const agentsLabel = dangerArmed === 'agents' ? '¿Seguro? Confirmar restablecer agentes' : 'Restablecer agentes';
    return `
      <section class="card">
        <div class="card-h">
          <h2>Diagnóstico Doctor y Sistema</h2>
          <div class="pill-row">
            <button type="button" class="set-btn prim" id="set-run-doctor">Ejecutar Doctor</button>
            <button type="button" class="set-btn" id="set-scan-apps">Detectar Apps</button>
            <button type="button" class="set-btn" id="set-check-update">Estado / RAM</button>
          </div>
        </div>
        <div class="item">
          <div><div class="item-t">Resultado Doctor</div><div class="item-d">${doc ? `${doc.passed_count}/${doc.total_checks || 6} checks OK · ${Math.round(doc.total_duration_ms || 0)} ms` : 'Pulsa Ejecutar Doctor para auditar Gateway y WebView2.'}</div></div>
          <span class="set-pill ${doc && doc.failed_count === 0 ? 'status-ok' : ''}">${doc ? (doc.failed_count === 0 ? '6/6 OK' : 'Revisar') : 'Pendiente'}</span>
        </div>
        ${Array.isArray(doc?.probes) ? doc.probes.map(p => `
          <div class="item">
            <div><div class="item-t">${esc(p.title || p.probe_id || p.name || '')}</div><div class="item-d">${esc(p.summary || p.detail || '')}</div></div>
            <span class="set-pill ${String(p.status).toUpperCase() === 'PASS' ? 'status-ok' : 'status-er'}">${esc(p.status)}</span>
          </div>`).join('') : ''}
        <div class="item">
          <div><div class="item-t">Versión y memoria del proceso</div><div class="item-d">${upd ? `v${esc(upd.version)} (${esc(upd.status)}) · RAM privada: ${Number(upd.process_private_mb || 0).toFixed(1)} MB` : 'v0.1.0 · Pulsa Estado / RAM para comprobar'}</div></div>
        </div>
        <div class="item">
          <div><div class="item-t">Apps / Navegadores detectados</div><div class="item-d">${apps.length ? esc(apps.map(a => a.name || a.id).join(', ')) : 'Pulsa Detectar Apps'}</div></div>
        </div>
      </section>
      <section class="card danger-card">
        <div class="card-h"><h2>Zona de peligro</h2></div>
        <div class="item">
          <div><div class="item-t">Restablecer ajustes de interfaz</div><div class="item-d">Devuelve idioma, voz y comportamiento al estado inicial (doble clic para confirmar).</div></div>
          <button type="button" class="set-btn ${dangerArmed === 'settings' ? 'armed' : 'danger'}" id="set-danger-reset-cfg">${esc(resetLabel)}</button>
        </div>
        <div class="item">
          <div><div class="item-t">Restablecer lista de agentes</div><div class="item-d">Elimina subagentes personalizados y conserva solo a Miku (doble clic para confirmar).</div></div>
          <button type="button" class="set-btn ${dangerArmed === 'agents' ? 'armed' : 'danger'}" id="set-danger-reset-agents">${esc(agentsLabel)}</button>
        </div>
      </section>`;
  }

  function buildBodyHtml(cat, ST) {
    switch (cat) {
      case 'general': return renderGeneralSection(ST);
      case 'island': return renderIslandSection();
      case 'agents': return renderAgentsSection();
      case 'providers': return renderProvidersSection();
      case 'keys': return renderKeysSection(ST);
      case 'mcp': return renderMcpSection(ST);
      case 'skills': return renderSkillsSection(ST);
      case 'telemetry': return renderTelemetrySection(ST);
      case 'notifications': return renderNotificationsSection();
      case 'voice': return renderVoiceSection();
      case 'link': return renderLinkSection(ST);
      case 'security': return renderSecuritySection(ST);
      case 'system': return renderSystemSection(ST);
      default: return renderGeneralSection(ST);
    }
  }

  function mountVoiceSliders() {
    const comp = UI.components || window.MikuComponents;
    if (!comp?.SmoothSlider) return;
    const vHost = document.getElementById('set-slider-voice');
    const sHost = document.getElementById('set-slider-sfx');
    if (vHost) {
      comp.SmoothSlider(vHost, {
        value: Number(UI.CFG?.voice_volume ?? 80),
        onChange: val => {
          UI.CFG.voice_volume = val;
          BRIDGE?.setSetting?.('voice_volume', val);
          UI.saveCfg?.();
          BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { voice_volume: val } });
        }
      });
    }
    if (sHost) {
      comp.SmoothSlider(sHost, {
        value: Number(UI.CFG?.sfx_volume ?? 60),
        onChange: val => {
          UI.CFG.sfx_volume = val;
          BRIDGE?.setSetting?.('sfx_volume', val);
          UI.saveCfg?.();
          BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { sfx_volume: val } });
        }
      });
    }
  }

  function ensureHeroShell(pane, meta, ST) {
    let body = document.getElementById('paneBody');
    if (!body) {
      const heroMarkup = PH?.renderParticlesHeader
        ? PH.renderParticlesHeader({
          canvasId: 'heroGalaxyCv',
          sectionId: activeCat,
          suiteTag: 'MIKU DESKTOP',
          title: meta.label,
          desc: meta.desc,
          statusPillHtml: `<span class="set-pill ${ST.ws ? 'status-ok' : ''}" id="hero-sync-pill">${ST.ws ? 'Sincronizado' : 'Local'}</span>`
        })
        : `<div class="pane-hero"><div><h1 id="hh">${esc(meta.label)}</h1><p id="hp">${esc(meta.desc)}</p></div></div>`;
      pane.innerHTML = `<div id="heroHost">${heroMarkup}</div><div id="paneBody" style="display:flex;flex-direction:column;gap:14px"></div>`;
      body = document.getElementById('paneBody');
      const cv = document.getElementById('heroGalaxyCv');
      if (cv && PH?.createParticlesEngine) {
        const hue = PH.getSectionHue(activeCat);
        heroState = PH.createParticlesEngine(cv, { c: { ...hue }, t: { ...hue } });
      }
    } else {
      const hh = document.getElementById('hh');
      const hp = document.getElementById('hp');
      const pill = document.getElementById('hero-sync-pill');
      if (hh) hh.textContent = meta.label;
      if (hp) hp.textContent = meta.desc;
      if (pill) {
        pill.textContent = ST.ws ? 'Sincronizado' : 'Local';
        pill.className = `set-pill ${ST.ws ? 'status-ok' : ''}`;
      }
      heroState?.setSection?.(activeCat);
    }
    return body;
  }

  function renderPane() {
    const pane = document.getElementById('pane');
    if (!pane) return;
    if (paneAbort) paneAbort.abort();
    paneAbort = new AbortController();
    const { signal } = paneAbort;

    const ST = UI.ST || {};
    UI.applyThemeVars?.(UI.CFG);
    const meta = currentMeta();
    updateGwBadge(Boolean(ST.ws), BRIDGE?.GATEWAY_PORT || '47400');
    syncSidebarAvatar();

    const body = ensureHeroShell(pane, meta, ST);
    window.MikuSettingsLink?.stopCountdown?.();
    body.innerHTML = buildBodyHtml(activeCat, ST);

    if (activeCat === 'voice') mountVoiceSliders();
    if (activeCat === 'link') {
      window.MikuSettingsLink?.bindEvents?.(body, signal, { renderPane, getQrData: () => qrData });
    }
    window.MikuMascot?.avatars?.start?.();
    bindPaneEvents(body, signal);
  }

  async function persistAgentsAndRender(broadcastMsg = null) {
    const subs = Array.isArray(UI.ST?.agents) ? UI.ST.agents.filter(a => a && a.id !== 'miku') : [];
    UI.ST.agents = subs;
    try { localStorage.setItem('miku_agents', JSON.stringify(subs)); } catch (_) {}
    await BRIDGE?.saveAgentsBackend?.(subs);
    if (broadcastMsg) BRIDGE?.broadcast?.(broadcastMsg);
    renderPane();
  }

  function bindPaneEvents(container, signal) {
    container.addEventListener('click', async e => {
      const jump = e.target.closest('[data-jump-cat]');
      if (jump) {
        selectCategory(jump.dataset.jumpCat);
        return;
      }

      const modeBtn = e.target.closest('[data-setmode]');
      if (modeBtn) {
        const nextMode = modeBtn.dataset.setmode;
        permNotice = 'Aplicando política...';
        renderPane();
        try {
          await BRIDGE?.setPermissionMode?.(nextMode);
          permNotice = `Modo ${nextMode} activo`;
        } catch (_) {
          permNotice = 'Error al cambiar modo';
        }
        renderPane();
        return;
      }

      const langBtn = e.target.closest('[data-setting-lang]');
      if (langBtn) {
        const lang = langBtn.dataset.settingLang;
        UI.CFG.lang = lang;
        await BRIDGE?.saveSettings?.({ lang });
        BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { lang } });
        renderPane();
        return;
      }

        const kvBtn = e.target.closest('[data-setting-kv]');
      if (kvBtn) {
        const [k, v] = kvBtn.dataset.settingKv.split(':');
        UI.CFG[k] = v;
        if (k === 'theme_preset') {
          const pr = (UI.THEME_PRESETS || []).find(p => p.id === v);
          if (pr) {
            UI.CFG.ui_accent = pr.accent;
            await BRIDGE?.saveSettings?.({ theme_preset: v, ui_accent: pr.accent });
            BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { theme_preset: v, ui_accent: pr.accent } });
            UI.applyThemeVars?.(UI.CFG);
            renderPane();
            return;
          }
        }
        await BRIDGE?.saveSettings?.({ [k]: v });
        BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { [k]: v } });
        UI.applyThemeVars?.(UI.CFG);
        renderPane();
        return;
      }

      const toggleBtn = e.target.closest('[data-toggle-setting]');
      if (toggleBtn) {
        const key = toggleBtn.dataset.toggleSetting;
        const nextVal = !UI.CFG[key];
        UI.CFG[key] = nextVal;
        await BRIDGE?.saveSettings?.({ [key]: nextVal });
        BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { [key]: nextVal } });
        renderPane();
        return;
      }

      const toggleNum = e.target.closest('[data-toggle-num]');
      if (toggleNum) {
        const key = toggleNum.dataset.toggleNum;
        const nextVal = (UI.CFG[key] === 0 || UI.CFG[key] === false) ? 1 : 0;
        UI.CFG[key] = nextVal;
        await BRIDGE?.saveSettings?.({ [key]: nextVal });
        BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { [key]: nextVal } });
        UI.applyThemeVars?.(UI.CFG);
        renderPane();
        return;
      }

      const numBtn = e.target.closest('[data-setting-num]');
      if (numBtn) {
        const [k, rawV] = numBtn.dataset.settingNum.split(':');
        const val = Number(rawV);
        if (k && Number.isFinite(val)) {
          UI.CFG[k] = val;
          await BRIDGE?.saveSettings?.({ [k]: val });
          BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { [k]: val } });
          UI.applyThemeVars?.(UI.CFG);
          renderPane();
        }
        return;
      }

      const autoBtn = e.target.closest('[data-setting-autoclose]');
      if (autoBtn) {
        const autoclose = Number(autoBtn.dataset.settingAutoclose || 15);
        UI.CFG.autoclose = autoclose;
        await BRIDGE?.saveSettings?.({ autoclose });
        BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { autoclose } });
        renderPane();
        return;
      }

      const limBtn = e.target.closest('[data-setting-limit]');
      if (limBtn) {
        const val = Number(limBtn.dataset.settingLimit || 0);
        UI.CFG.app_time_limit_min = val;
        await BRIDGE?.saveSettings?.({ app_time_limit_min: val });
        BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { app_time_limit_min: val } });
        renderPane();
        return;
      }

      if (e.target.closest('#set-toggle-tails')) {
        const next = !Boolean(window.MikuMascot?.getPhys?.()?.twintails);
        window.MikuMascot?.setTwintails?.(next);
        BRIDGE?.broadcast?.({ k: 'mascot', agentId: 'miku', twintails: next });
        renderPane();
        return;
      }

      const mColorBtn = e.target.closest('[data-miku-color]');
      if (mColorBtn) {
        const col = mColorBtn.dataset.mikuColor;
        UI.PRIN.c = col;
        window.MikuMascot?.setColor?.(col);
        await BRIDGE?.saveAgentsBackend?.();
        BRIDGE?.broadcast?.({ k: 'mascot', agentId: 'miku', color: col });
        renderPane();
        return;
      }

      const mOutBtn = e.target.closest('[data-miku-outfit]');
      if (mOutBtn) {
        const outfit = mOutBtn.dataset.mikuOutfit || 'none';
        UI.PRIN.outfit = outfit;
        window.MikuMascot?.setOutfit?.(outfit, true);
        await BRIDGE?.saveAgentsBackend?.();
        BRIDGE?.broadcast?.({ k: 'mascot', agentId: 'miku', outfit });
        renderPane();
        return;
      }

      const selAgBtn = e.target.closest('[data-select-ag]');
      if (selAgBtn) {
        selectedAgentId = selAgBtn.dataset.selectAg;
        renderPane();
        return;
      }

      const editColBtn = e.target.closest('[data-edit-ag-color]');
      if (editColBtn) {
        const target = allAgents().find(a => a.id === selectedAgentId);
        if (target) {
          target.c = editColBtn.dataset.editAgColor;
          if (target.id === 'miku') window.MikuMascot?.setColor?.(target.c);
          await BRIDGE?.saveAgentsBackend?.();
          renderPane();
        }
        return;
      }

      if (e.target.closest('#set-ag-save')) {
        const target = allAgents().find(a => a.id === selectedAgentId);
        if (target) {
          target.name = document.getElementById('set-ag-name')?.value.trim() || target.name;
          target.role = document.getElementById('set-ag-role')?.value.trim() || target.role;
          target.provider = document.getElementById('set-ag-provider')?.value ?? target.provider ?? '';
          target.model = document.getElementById('set-ag-model')?.value.trim() ?? '';
          target.outfit = document.getElementById('set-ag-outfit')?.value || target.outfit || 'none';
          target.tools_mode = document.getElementById('set-ag-tools')?.value || target.tools_mode || 'auto';
          const promptBox = document.getElementById('set-ag-prompt');
          if (promptBox) target.prompt = promptBox.value;
          if (target.id !== 'miku') {
            const actEl = document.getElementById('set-ag-active');
            const islEl = document.getElementById('set-ag-isl');
            if (actEl) target.active = Boolean(actEl.checked);
            if (islEl) target.isl = islEl.checked ? (Number(target.isl) > 0 ? Number(target.isl) : 2) : 0;
          }
          const pickC = document.getElementById('set-ag-colin')?.value;
          if (pickC) target.c = pickC;
          if (target.id === 'miku') {
            window.MikuMascot?.setColor?.(target.c);
            window.MikuMascot?.setOutfit?.(target.outfit, true);
          }
          await persistAgentsAndRender({ k: 'mascot', agentId: target.id, outfit: target.outfit, color: target.c });
        }
        return;
      }

      if (e.target.closest('#set-ag-reset-one')) {
        const id = document.getElementById('set-ag-reset-one')?.dataset.agId || selectedAgentId;
        hookNotice = 'Restaurando agente...';
        renderPane();
        await BRIDGE?.resetAgents?.(id).catch(() => null);
        hookNotice = 'Agente restaurado';
        renderPane();
        return;
      }

      if (e.target.closest('#set-reset-all-agents')) {
        hookNotice = 'Restaurando agentes curados...';
        renderPane();
        await BRIDGE?.resetAgents?.('').catch(() => null);
        selectedAgentId = 'miku';
        hookNotice = 'Roster predefinido restaurado';
        renderPane();
        return;
      }

      if (e.target.closest('#set-scan-harnesses')) {
        hookBusy = true;
        hookNotice = 'Escaneando harnesses locales...';
        renderPane();
        await BRIDGE?.fetchHarnesses?.().catch(() => []);
        hookBusy = false;
        hookNotice = `Escaneo completo (${(UI.ST?.harnesses || []).filter(h => h.installed).length} detectados)`;
        renderPane();
        return;
      }

      if (e.target.closest('#set-hook-cancel')) {
        hookPreview = null;
        renderPane();
        return;
      }

      const hookPrevBtn = e.target.closest('[data-hook-preview]');
      if (hookPrevBtn) {
        const agent = hookPrevBtn.dataset.hookPreview;
        hookBusy = true;
        hookNotice = `Calculando diff para ${agent}...`;
        renderPane();
        const res = await BRIDGE?.previewHarnessHook?.(agent).catch(err => ({ ok: false, error: err?.message }));
        hookBusy = false;
        if (res && res.ok !== false) {
          hookPreview = { agent, ...res };
          hookNotice = '';
        } else {
          hookNotice = res?.error || 'No se pudo previsualizar diff';
        }
        renderPane();
        return;
      }

      const hookInstBtn = e.target.closest('[data-hook-install]');
      if (hookInstBtn) {
        const agent = hookInstBtn.dataset.hookInstall;
        hookBusy = true;
        hookNotice = `Instalando hook en ${agent}...`;
        renderPane();
        const res = await BRIDGE?.installHarnessHook?.(agent, '', true).catch(err => ({ ok: false, error: err?.message }));
        await BRIDGE?.fetchHarnesses?.().catch(() => []);
        hookBusy = false;
        hookPreview = null;
        hookNotice = res?.ok !== false ? `Hook instalado en ${agent} ✓` : (res?.error || 'Error instalando hook');
        renderPane();
        return;
      }

      const hookRestBtn = e.target.closest('[data-hook-restore]');
      if (hookRestBtn) {
        const agent = hookRestBtn.dataset.hookRestore;
        hookBusy = true;
        hookNotice = `Restaurando .bak de ${agent}...`;
        renderPane();
        const res = await BRIDGE?.restoreHarnessHook?.(agent).catch(err => ({ ok: false, error: err?.message }));
        await BRIDGE?.fetchHarnesses?.().catch(() => []);
        hookBusy = false;
        hookPreview = null;
        hookNotice = res?.ok !== false ? `Backup restaurado en ${agent}` : (res?.error || 'Sin respaldo disponible');
        renderPane();
        return;
      }

      const provBtn = e.target.closest('[data-pick-provider]');
      if (provBtn) {
        const provider = provBtn.dataset.pickProvider;
        const model = provBtn.dataset.pickModel;
        UI.CFG.provider = provider;
        UI.CFG.model = model;
        await BRIDGE?.saveSettings?.({ provider, model });
        BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { provider, model } });
        renderPane();
        return;
      }

      const modOnly = e.target.closest('[data-pick-model-only]');
      if (modOnly) {
        const model = modOnly.dataset.pickModelOnly;
        UI.CFG.model = model;
        await BRIDGE?.saveSettings?.({ model });
        BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { model } });
        renderPane();
        return;
      }

      const maxTokBtn = e.target.closest('[data-setting-maxtok]');
      if (maxTokBtn) {
        const max_tokens = Number(maxTokBtn.dataset.settingMaxtok || 384);
        UI.CFG.max_tokens = max_tokens;
        await BRIDGE?.saveSettings?.({ max_tokens });
        renderPane();
        return;
      }

      if (e.target.closest('#set-model-save')) {
        const model = document.getElementById('set-model-inp')?.value.trim();
        if (model) {
          UI.CFG.model = model;
          await BRIDGE?.saveSettings?.({ model });
          BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { model } });
          renderPane();
        }
        return;
      }

      const saveSyncBtn = e.target.closest('[data-save-sync-key]');
      if (saveSyncBtn) {
        const provId = saveSyncBtn.dataset.saveSyncKey;
        const syncable = saveSyncBtn.dataset.syncable === '1';
        const inp = document.querySelector(`[data-key-input="${provId}"]`);
        const val = inp?.value.trim();
        if (!val) {
          secretTestStatus = `Pega la API key de ${provId} primero`;
          renderPane();
          return;
        }
        secretTestStatus = `Guardando ${provId}...`;
        renderPane();
        await BRIDGE?.setSecret?.(provId, val, provId, UI.CFG?.model);
        if (inp) inp.value = '';
        if (syncable && BRIDGE?.syncProviderModels) {
          const syncRes = await BRIDGE.syncProviderModels(provId, provId).catch(err => ({ ok: false, error: err?.message }));
          const n = Array.isArray(syncRes?.models) ? syncRes.models.length : 0;
          secretTestStatus = syncRes?.ok
            ? `Guardado DPAPI: ${provId} · ${n} modelos oficiales descargados`
            : `Guardado DPAPI: ${provId} (sync: ${syncRes?.error || 'sin conexión'})`;
        } else {
          secretTestStatus = `Guardado DPAPI: ${provId}`;
        }
        BRIDGE?.broadcast?.({ k: 'secrets_updated' });
        await BRIDGE?.syncAll?.();
        renderPane();
        return;
      }

      const testKeyBtn = e.target.closest('[data-test-key]');
      if (testKeyBtn) {
        const provId = testKeyBtn.dataset.testKey;
        secretTestStatus = `Probando ${provId}...`;
        renderPane();
        const res = await BRIDGE?.testSecret?.({ provider: provId, alias: provId }).catch(err => ({ ok: false, error: err?.message || 'Error de red' }));
        secretTestStatus = res?.ok
          ? `OK ${provId} · HTTP ${res.status || 200} (${res.latency_ms || 0} ms)`
          : `Error ${provId}: ${res?.error || res?.message || 'Fallo de autenticación'}`;
        renderPane();
        return;
      }

      if (e.target.closest('#set-sync-active-models')) {
        const provId = document.getElementById('set-sync-active-models')?.dataset.syncProvider || UI.CFG?.provider || 'deepseek';
        const syncRes = await BRIDGE?.syncProviderModels?.(provId).catch(err => ({ ok: false, error: err?.message }));
        if (syncRes?.ok) {
          await BRIDGE?.syncAll?.();
        }
        renderPane();
        return;
      }

      if (e.target.closest('#set-sec-save')) {
        const alias = document.getElementById('set-sec-alias')?.value.trim() || UI.CFG?.provider || 'deepseek';
        const val = document.getElementById('set-sec-val')?.value.trim();
        if (val) {
          await BRIDGE?.setSecret?.(alias, val, UI.CFG?.provider, UI.CFG?.model);
          const secEl = document.getElementById('set-sec-val');
          if (secEl) secEl.value = '';
          secretTestStatus = `Guardado DPAPI: ${alias}`;
          BRIDGE?.broadcast?.({ k: 'secrets_updated' });
          await BRIDGE?.syncAll?.();
          renderPane();
        }
        return;
      }

      if (e.target.closest('#set-sec-test')) {
        secretTestStatus = 'Probando conexión API...';
        renderPane();
        const alias = document.getElementById('set-sec-alias')?.value.trim() || UI.CFG?.provider || 'deepseek';
        const res = await BRIDGE?.testSecret?.({
          provider: UI.CFG?.provider,
          model: UI.CFG?.model,
          alias,
          base_url: UI.CFG?.custom_base_url || ''
        }).catch(err => ({ ok: false, error: err?.message || 'Error de red' }));
        secretTestStatus = res?.ok
          ? `OK · HTTP ${res.status || 200} (${res.latency_ms || 0} ms)`
          : `Error: ${res?.error || res?.message || 'Fallo de autenticación o red'}`;
        renderPane();
        return;
      }

      const delBtn = e.target.closest('[data-del-secret]');
      if (delBtn) {
        await BRIDGE?.deleteSecret?.(delBtn.dataset.delSecret);
        BRIDGE?.broadcast?.({ k: 'secrets_updated' });
        await BRIDGE?.syncAll?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-mcp-add')) {
        const name = document.getElementById('set-mcp-name')?.value.trim();
        const command = document.getElementById('set-mcp-cmd')?.value.trim();
        const rawArgs = document.getElementById('set-mcp-args')?.value.trim() || '';
        if (name && command) {
          await BRIDGE?.addMcp?.(name, command, rawArgs ? rawArgs.split(/\s+/) : []);
          await BRIDGE?.syncAll?.();
          renderPane();
        }
        return;
      }

      if (e.target.closest('#set-mcp-cat-refresh')) {
        await BRIDGE?.fetchMcpCatalog?.();
        renderPane();
        return;
      }

      const instMcp = e.target.closest('[data-install-mcp]');
      if (instMcp) {
        await BRIDGE?.installMcpCatalog?.(instMcp.dataset.installMcp);
        await BRIDGE?.fetchMcpCatalog?.();
        await BRIDGE?.syncAll?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-skills-reload')) {
        await BRIDGE?.reloadSkills?.();
        await BRIDGE?.syncAll?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-mem-search')) {
        memoryQuery = document.getElementById('set-mem-q')?.value.trim() || '';
        memoryHits = memoryQuery ? (await BRIDGE?.searchMemory?.(memoryQuery)) || [] : [];
        renderPane();
        return;
      }

      if (e.target.closest('#set-tel-refresh')) {
        await BRIDGE?.fetchTelemetry?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-tel-reset')) {
        await BRIDGE?.resetTelemetry?.();
        await BRIDGE?.fetchTelemetry?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-test-voice')) {
        window.SoundFX?.previewVoice?.();
        return;
      }

      if (e.target.closest('#set-test-sfx')) {
        window.SoundFX?.chime?.();
        return;
      }

      if (e.target.closest('#set-audio-refresh')) {
        await BRIDGE?.fetchAudioDevices?.();
        await BRIDGE?.fetchSoundsList?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-qr-refresh')) {
        qrData = await BRIDGE?.getLinkQr?.();
        await BRIDGE?.fetchLinkStatus?.().catch(() => null);
        window.MikuSettingsLink?.setNotice?.(qrData ? 'Nuevo código QR generado (válido 60s)' : 'Conecta el Gateway para generar QR', !qrData);
        renderPane();
        return;
      }

      if (e.target.closest('#set-link-unpair')) {
        await BRIDGE?.unpairDevices?.();
        qrData = null;
        await Promise.all([
          BRIDGE?.syncAll?.().catch(() => null),
          BRIDGE?.fetchLinkStatus?.().catch(() => null)
        ]);
        window.MikuSettingsLink?.setNotice?.('Dispositivos móviles desvinculados');
        renderPane();
        return;
      }

      if (e.target.closest('#set-ks-toggle')) {
        await BRIDGE?.toggleKillSwitch?.(!UI.ST?.killSwitch);
        await BRIDGE?.syncAll?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-run-doctor')) {
        gwDoctorReport = await BRIDGE?.runDoctor?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-scan-apps')) {
        gwApps = await BRIDGE?.detectApps?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-check-update')) {
        updateInfo = await BRIDGE?.checkUpdate?.();
        renderPane();
        return;
      }

      if (e.target.closest('#set-danger-reset-cfg')) {
        if (dangerArmed !== 'settings') {
          dangerArmed = 'settings';
          renderPane();
          return;
        }
        dangerArmed = '';
        await BRIDGE?.resetSettings?.();
        await BRIDGE?.syncAll?.();
        BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { lang: 'es', autoclose: 15 } });
        renderPane();
        return;
      }

      if (e.target.closest('#set-danger-reset-agents')) {
        if (dangerArmed !== 'agents') {
          dangerArmed = 'agents';
          renderPane();
          return;
        }
        dangerArmed = '';
        selectedAgentId = 'miku';
        await BRIDGE?.resetAgents?.('');
        renderPane();
      }
    }, { signal });

    document.getElementById('set-ui-accent')?.addEventListener('input', e => {
      UI.CFG.ui_accent = e.target.value;
      UI.applyThemeVars?.(UI.CFG);
    }, { signal });

    document.getElementById('set-ui-accent')?.addEventListener('change', async e => {
      const hex = e.target.value;
      UI.CFG.ui_accent = hex;
      UI.applyThemeVars?.(UI.CFG);
      await BRIDGE?.saveSettings?.({ ui_accent: hex });
      BRIDGE?.broadcast?.({ k: 'setting_updated', patch: { ui_accent: hex } });
      renderPane();
    }, { signal });

    document.getElementById('set-ag-prompt')?.addEventListener('input', e => {
      const badge = document.getElementById('set-ag-prompt-tok');
      if (badge) badge.textContent = `~${Math.ceil(e.target.value.length / 4)} tokens`;
    }, { signal });

    document.getElementById('set-miku-colin')?.addEventListener('change', async e => {
      const col = e.target.value;
      UI.PRIN.c = col;
      window.MikuMascot?.setColor?.(col);
      await BRIDGE?.saveAgentsBackend?.();
      BRIDGE?.broadcast?.({ k: 'mascot', agentId: 'miku', color: col });
      renderPane();
    }, { signal });
  }

  function selectCategory(cat) {
    const resolved = ALIAS_CAT[cat] || cat;
    activeCat = resolved;
    dangerArmed = '';
    permNotice = '';
    try { localStorage.setItem('miku_cat', JSON.stringify(resolved)); } catch (_) {}
    renderNav();
    renderPane();
    if (resolved === 'agents' && !(UI.ST?.harnesses?.length)) {
      BRIDGE?.fetchHarnesses?.().then(() => { if (activeCat === 'agents') renderPane(); }).catch(() => {});
    } else if (resolved === 'mcp' && !(UI.runtime?.mcpCatalog?.length)) {
      BRIDGE?.fetchMcpCatalog?.().then(() => { if (activeCat === 'mcp') renderPane(); }).catch(() => {});
    } else if (resolved === 'voice' && !(UI.runtime?.audioOutputs?.length)) {
      BRIDGE?.fetchAudioDevices?.().then(() => { if (activeCat === 'voice') renderPane(); }).catch(() => {});
    } else if (resolved === 'telemetry' && !UI.ST?.telemetry) {
      BRIDGE?.fetchTelemetry?.().then(() => { if (activeCat === 'telemetry') renderPane(); }).catch(() => {});
    } else if (resolved === 'link') {
      Promise.all([
        BRIDGE?.fetchLinkStatus?.().catch(() => null),
        BRIDGE?.fetchLinkNotifications?.().catch(() => null)
      ]).then(() => {
        const ae = document.activeElement?.tagName;
        if (activeCat === 'link' && ae !== 'INPUT' && ae !== 'TEXTAREA') renderPane();
      }).catch(() => {});
    }
  }

  function syncSidebarAvatar() {
    const slot = document.getElementById('sidebar-av');
    if (!slot) return;
    const col = safeColor(UI.PRIN?.c || '#ffffff');
    const out = UI.PRIN?.outfit || 'none';
    const nextAv = `${col}|${out}|head|0|`;
    if (slot.dataset.av !== nextAv || !slot.dataset.rendered) {
      slot.dataset.av = nextAv;
      const freshSlot = slot.cloneNode(true);
      freshSlot.dataset.rendered = '1';
      slot.replaceWith(freshSlot);
      const curCv = document.getElementById('sidebar-hl');
      const frame = window.MikuMascot?.avFrame?.(col, out, 'head', 0, 0, 0, 'idle', false);
      if (curCv && frame) {
        const ctx = curCv.getContext('2d');
        ctx?.clearRect(0, 0, 84, 84);
        ctx?.drawImage(frame, 0, 0);
      }
    }
    window.MikuMascot?.avatars?.start?.();
  }

  function boot() {
    UI.applyThemeVars?.(UI.CFG);
    renderNav();
    renderPane();
    if (activeCat === 'agents') {
      BRIDGE?.fetchHarnesses?.().then(() => { if (activeCat === 'agents') renderPane(); }).catch(() => {});
    } else if (activeCat === 'link') {
      Promise.all([
        BRIDGE?.fetchLinkStatus?.().catch(() => null),
        BRIDGE?.fetchLinkNotifications?.().catch(() => null)
      ]).then(() => { if (activeCat === 'link') renderPane(); }).catch(() => {});
    }

    document.addEventListener('visibilitychange', () => {
      if (document.hidden) window.MikuMascot?.freeze?.();
      else window.MikuMascot?.thaw?.();
    });

    document.getElementById('top-chrome')?.addEventListener('pointerdown', e => {
      if (e.target.closest('.win-ctrls') || e.target.closest('button')) return;
      BRIDGE?.startDrag?.();
    });

    document.getElementById('win-min')?.addEventListener('click', () => {
      window.SoundFX?.pop?.();
      BRIDGE?.windowControl?.('minimize');
    });

    document.getElementById('win-cls')?.addEventListener('click', () => {
      window.SoundFX?.pop?.();
      BRIDGE?.windowControl?.('hide');
    });

    document.getElementById('rail')?.addEventListener('click', e => {
      const btn = e.target.closest('[data-cat]');
      if (!btn) return;
      window.SoundFX?.click?.();
      selectCategory(btn.dataset.cat);
    });

    BRIDGE?.onBroadcast?.(msg => {
      if (!msg) return;
      if (msg.k === 'go_cat' && msg.cat) {
        selectCategory(msg.cat);
      } else if (msg.k === 'setting_updated' && msg.patch) {
        Object.assign(UI.CFG, msg.patch);
        UI.applyThemeVars?.(UI.CFG);
        if (document.activeElement?.tagName !== 'TEXTAREA' && document.activeElement?.tagName !== 'INPUT') {
          renderPane();
        }
      } else if (msg.k === 'mascot') {
        if ((!msg.agentId || msg.agentId === 'miku')) {
          if (msg.outfit) {
            UI.PRIN.outfit = msg.outfit;
            window.MikuMascot?.setOutfit?.(msg.outfit, true);
          }
          if (msg.color) {
            UI.PRIN.c = msg.color;
            window.MikuMascot?.setColor?.(msg.color);
          }
          if (msg.twintails !== undefined) {
            window.MikuMascot?.setTwintails?.(Boolean(msg.twintails));
          }
        }
        renderPane();
      } else if (msg.k === 'secrets_updated') {
        BRIDGE?.syncAll?.();
      }
    });

    BRIDGE?.onFeed?.(msg => {
      if (!msg) return;
      if (msg.type === 'ws') {
        UI.ST.ws = msg.ok ? 1 : 0;
        updateGwBadge(Boolean(UI.ST.ws), BRIDGE?.GATEWAY_PORT || '47400');
        if (UI.ST.ws) {
          BRIDGE?.syncAll?.().then(() => renderPane()).catch(() => {});
        }
        return;
      }
      if (msg.type === 'go_cat' && msg.cat) {
        selectCategory(msg.cat);
        return;
      }
      if (msg.type === 'link_status' || msg.type === 'link_notification' || msg.type === 'link_paired' || msg.type === 'link_unpaired') {
        if (activeCat === 'link') {
          Promise.all([
            BRIDGE?.fetchLinkStatus?.().catch(() => null),
            BRIDGE?.fetchLinkNotifications?.().catch(() => null)
          ]).then(() => {
            const ae = document.activeElement?.tagName;
            if (activeCat === 'link' && ae !== 'INPUT' && ae !== 'TEXTAREA') renderPane();
          });
        } else {
          BRIDGE?.fetchLinkStatus?.().catch(() => null);
        }
        return;
      }
      if (msg.type === 'state_sync') {
        const snap = msg.state || msg;
        const hasInline = Array.isArray(snap?.secrets) || Array.isArray(snap?.agents);
        const p = hasInline
          ? Promise.resolve(BRIDGE?.applySnapshot?.(snap))
          : BRIDGE?.syncAll?.().catch(() => null);
        p.then(() => {
          const ae = document.activeElement?.tagName;
          if (ae !== 'INPUT' && ae !== 'TEXTAREA' && ae !== 'SELECT') {
            renderPane();
          }
        });
      }
    });

    BRIDGE?.connect?.();
    BRIDGE?.syncAll?.().then(() => renderPane()).catch(() => {});
  }

  document.addEventListener('DOMContentLoaded', boot);
})();
