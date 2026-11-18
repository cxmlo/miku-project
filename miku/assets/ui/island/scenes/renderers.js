/**
 * scenes/renderers.js — Catálogo Modular de Escenas de la Dynamic Island
 * ------------------------------------------------------------------------
 * Define todas las vistas (`compact`, `boot`, `intro`, `prompt`, `overview`,
 * `wardrobe`, `gallery`, `settings`, `link`, `approve`, `drop`, etc.) sin
 * lógica rígida ni agentes inventados.
 */
(function (global) {
  'use strict';

  function cW(content, width = 340) {
    return `<div style="width:${width}px;max-width:100%">${content}</div>`;
  }

  function renderAgentSelectorPill() {
    const { ST, PRIN, curAg, bodyOf, getAgChat, tr, esc } = global.MikuUI;
    const { ic } = global.MikuComponents;
    const ag = curAg();
    const acCol = bodyOf(ag);
    const subs = Array.isArray(ST.agents) ? ST.agents.filter(a => a && a.id !== PRIN.id && a.active !== false) : [];
    const list = [PRIN, ...subs];

    const ddItems = list.map(item => {
      const selaVal = item.id === PRIN.id ? -1 : ST.agents.indexOf(item);
      const isCur = item.id === ag.id;
      const chat = getAgChat(item.id);
      const col = bodyOf(item);
      return `<button type="button" class="ag-dd-item${isCur ? ' active' : ''}" data-select-agent="${selaVal}">
        <span class="ag-chat-dot" style="color:${col};background:${col}"></span>
        <span>${esc(item.name)}</span>
        ${chat.unread ? '<span class="ag-dd-dot-unread"></span>' : `<span class="ag-dd-badge">${esc(item.id === PRIN.id ? tr('orchestrator') : tr('subagent'))}</span>`}
      </button>`;
    }).join('');

    const killBadge = ST.killSwitch ? ' danger' : '';
    const connTitle = ST.ws ? tr('core_synced') : tr('local_mode');

    return `<div class="ag-chat-pill-wrap">
      <button type="button" id="ag-chat-pill" class="ag-chat-pill${killBadge}" aria-expanded="false" title="${esc(connTitle)}">
        <span class="ag-chat-dot" style="color:${acCol};background:${acCol}"></span>
        <span class="ag-chat-name">${esc(ag.name)}</span>
        <span aria-hidden="true" style="opacity:.65;font-size:10px;line-height:1">▾</span>
      </button>
      <div id="ag-chat-dd" class="ag-chat-dropdown">
        <div class="ag-dd-title">${esc(tr('agents_avail_prefix'))} (${list.length})</div>
        ${ddItems}
        <div class="ag-dd-sep"></div>
        <button type="button" class="ag-dd-item manage" data-go="wardrobe">${ic('shirt')}<span>Estilo y Guardarropa</span></button>
        <button type="button" class="ag-dd-item manage" data-open-settings="agents">${ic('cog')}<span>${esc(tr('quick_settings'))}</span></button>
      </div>
    </div>`;
  }

  function HD(_title = '', backScene = 'overview', extraHtml = '') {
    const { TABS, ST, runtime, tr, esc } = global.MikuUI;
    const { ic } = global.MikuComponents;

    const tabsHtml = TABS.map(([id, icon, label]) =>
      `<button type="button" class="ib tb${runtime.scene === id ? ' on' : ''}" data-go="${id}" title="${esc(label)}" aria-label="${esc(label)}">${ic(icon)}</button>`
    ).join('');

    const showBack = backScene && backScene !== runtime.scene && !['overview', 'prompt'].includes(runtime.scene);
    const backBtn = showBack
      ? `<button type="button" class="ib" data-go="${backScene}" title="Volver" aria-label="Volver">${ic('prev')}</button>`
      : '';

    return `<div class="hd">
      <span>
        ${backBtn}
        <div class="tabs">${tabsHtml}</div>
        ${renderAgentSelectorPill()}
      </span>
      <span class="r">
        ${extraHtml}
        <button type="button" class="ib ph${runtime.scene === 'link' ? ' on' : ''}${ST.link ? ' live' : ''}" data-go="link" title="Miku-Link" aria-label="Miku-Link">${ic('phone')}</button>
        <button type="button" class="ib${runtime.scene === 'settings' ? ' on' : ''}" data-go="settings" title="${esc(tr('quick_settings'))}" aria-label="${esc(tr('quick_settings'))}">${ic('gear')}</button>
        <button type="button" class="ib" data-a="island_hide" title="Minimizar isla (Esc)" aria-label="Minimizar isla">${ic('minus')}</button>
      </span>
    </div>`;
  }

  function MINIH() {
    const { ST, MUS, runtime, getUnreadAgent, esc } = global.MikuUI;
    const { curAg } = global.MikuUI;
    const { ic } = global.MikuComponents;
    const ag = curAg();
    const unread = getUnreadAgent();
    const phys = global.MikuMascot?.getPhys?.() || { cur: 'idle' };

    if (runtime.computerLiveStatus) {
      return `<div class="mini"><div class="mini-live-status"><span class="mls-radar"><i></i></span><span class="mls-text">${esc(runtime.computerLiveStatus)}</span></div></div>`;
    }

    if (phys.cur === 'listening' || ST.mic) {
      return `<div class="mini"><span class="gl" style="--c:var(--sky)"><i></i><i></i><i></i><i></i></span><span class="tl live"><span class="tl-t">Escuchando...</span></span></div>`;
    }

    if (phys.cur === 'thinking' || runtime.thk.on) {
      return `<div class="mini"><span class="gd" style="--c:#a88cff"><i></i></span><span class="tl live"><span class="tl-t">Pensando</span></span></div>`;
    }

    if (runtime.hasUpdate) {
      return `<div class="mini"><button type="button" class="mini-upd-badge" data-go="settings"><span class="mub-spark">${ic('spark')}</span><span>Update</span></button></div>`;
    }

    if (MUS.playing) {
      return `<div class="mini"><canvas class="star" width="68" height="68" title="${esc(MUS.title)}"></canvas></div>`;
    }

    const unreadHint = unread ? `<span class="badge">${esc(unread.name)}</span>` : '';
    const eqWave = (ST.ws || ST.mic || phys.cur === 'listening' || phys.cur === 'talking')
      ? `<span class="mn" style="--c:#38bdf8"><span class="gl"><i></i><i></i><i></i><i></i></span></span>`
      : '';
    return `<div class="mini">
      ${unreadHint}
      <span class="tl"><span class="sd ${ST.ws ? 'on' : ''}"></span><span>${esc(ag.name)}</span></span>
      ${eqWave}
    </div>`;
  }

  function renderOverviewPager() {
    const { ST, PRIN, curAg, getAgChat, bodyOf, esc, runtime } = global.MikuUI;
    const { ic } = global.MikuComponents;
    const avtHtml = global.MikuMascot?.avtHtml || (() => '<span></span>');
    const pg = ((runtime.pg || 0) % 2 + 2) % 2;
    runtime.pg = pg;

    const ag = curAg();
    const pool = [PRIN, ...ST.agents.filter(a => a && a.active !== false && Number(a.isl ?? 1) > 0)];
    const gridAgents = pool.filter(x => x.id !== ag.id).slice(0, 4);
    const rightColors = gridAgents.map(g => (g.id === PRIN.id ? '#ffffff' : (g.c || '#ffffff')));
    const c1 = rightColors[0] || '#ffffff';
    const c2 = rightColors[1] || rightColors[0] || '#a3a3a3';
    const c3 = rightColors[2] || c1;

    let body = '';
    if (pg === 0) {
      if (gridAgents.length > 0) {
        const agentPills = gridAgents.map(g => {
          const agIdx = g.id === PRIN.id ? -1 : ST.agents.findIndex(x => x.id === g.id);
          const ch = getAgChat(g.id);
          const isWorking = !!ch.busy;
          const col = bodyOf(g);
          const pillC = g.id === PRIN.id ? '#ffffff' : (g.c || '#ffffff');
          return `<button type="button" class="miku-ag-pill${isWorking ? ' is-working' : ''}" data-ov-agent="${agIdx}" title="${esc(g.name)}" style="border-color:color-mix(in srgb,${pillC} 35%,rgba(255,255,255,0.12));--pc:${pillC}">
            ${avtHtml(col, g.outfit || 'none', agIdx < 0 ? 0 : agIdx, 0, 'head', isWorking ? 'working' : 'lite')}
            <span class="miku-ag-pill-name marquee-wrap"><span class="marquee-text">${esc(g.name)}</span></span>
          </button>`;
        });
        body = `<div class="miku-grid-2x2">${agentPills.join('')}</div>`;
      } else {
        body = `<div class="miku-grid-2x2">
          <button type="button" class="miku-ag-pill" data-go="prompt">${ic('chat')}<span class="miku-ag-pill-name">Chat con ${esc(ag.name)}</span></button>
          <button type="button" class="miku-ag-pill" data-go="wardrobe">${ic('shirt')}<span class="miku-ag-pill-name">Guardarropa</span></button>
          <button type="button" class="miku-ag-pill" data-open-settings="agents">${ic('cog')}<span class="miku-ag-pill-name">Configurar agentes</span></button>
        </div>`;
      }
    } else {
      body = `<div class="plc ovpl">
        <div id="ov-sound-host"></div>
        <div class="vlh thin">
          ${ic('speaker-off')}
          <span id="ov-slosh-host"></span>
          ${ic('speaker')}
        </div>
      </div>`;
    }

    const dots = [0, 1].map(i => `<span class="pd${pg === i ? ' on' : ''}" data-pg="${i}"></span>`).join('');
    return `<div class="card hubc ovp" style="--ag-c1:${c1};--ag-c2:${c2};--ag-c3:${c3}">
      <div class="ovp-body">${body}</div>
      <div class="pgr">
        <button type="button" data-pg-step="-1" title="Anterior" aria-label="Anterior">${ic('prev')}</button>
        <div class="pds">${dots}<i class="pth" style="transform:translateX(${pg * 15}px)"></i></div>
        <button type="button" data-pg-step="1" title="Siguiente" aria-label="Siguiente">${ic('next')}</button>
      </div>
    </div>`;
  }

  const SC = {
    compact: () => `<span class="cs" data-w="34" data-h="34" style="width:34px;height:34px"></span>${MINIH()}`,

    boot: () => {
      const { ST, tr, esc } = global.MikuUI;
      return `<div class="boot-scene">
        <span class="cs" data-w="72" data-h="72" style="width:72px;height:72px"></span>
        <div class="boot-copy">
          <div class="boot-header-row">
            <span class="boot-eyebrow">${esc(tr('boot_eyebrow'))}</span>
            <span class="boot-beta-badge">${esc(tr('beta_tag'))}</span>
          </div>
          <div class="boot-title">${esc(ST.ws ? tr('boot_connected') : tr('boot_title'))}</div>
          <div class="boot-steps">
            <div class="boot-step is-done"><span class="boot-step-dot"></span><span>${esc(tr('boot_step_0'))}</span></div>
            <div class="boot-step is-done"><span class="boot-step-dot"></span><span>${esc(tr('boot_step_1'))}</span></div>
            <div class="boot-step ${ST.ws ? 'is-done' : 'is-active'}"><span class="boot-step-dot"></span><span>${esc(tr('boot_step_2'))}</span></div>
          </div>
          <div class="boot-beta-footer">
            <span class="boot-beta-note">${esc(tr('boot_note'))}</span>
            <button type="button" class="boot-issue-btn" data-go="overview">Abrir Isla</button>
          </div>
        </div>
      </div>`;
    },

    intro: () => `${HD('Miku')}<div class="card mk">
      <span class="cs" data-w="104" data-h="104" style="width:104px;height:104px"></span>
      <div class="cp">
        <b>¡Hola! Miku Core Modular</b>
        <span class="sub2">Personaliza su aspecto, tema, sonidos o idioma desde tus archivos locales.</span>
        <div class="chips">
          <button type="button" class="p" data-go="prompt">Abrir Chat</button>
          <button type="button" data-go="wardrobe">Guardarropa</button>
          <button type="button" data-go="settings">Ajustes</button>
        </div>
      </div>
    </div>`,

    overview: () => {
      const { ST, CFG, PRIN, curAg, bodyOf, getAgChat, tr, esc } = global.MikuUI;
      const ag = curAg();
      const ch = getAgChat(ag.id);
      const col = bodyOf(ag);
      const isMain = ag.id === PRIN.id || ag.id === 'miku';
      const statusText = ch.busy
        ? tr('thinking')
        : (isMain ? tr('active_status') : (ag.model || CFG.model || tr('ready_status')));

      return `${HD('', 'overview')}<div class="two h2">
        <div class="card hubc live-ag-card" data-go="prompt" style="--ag-card-c:${col}">
          <span class="cs" data-w="76" data-h="76" style="width:76px;height:76px"></span>
          <div class="ov-agent-info">
            <div class="ov-agent-head">
              <span class="ov-agent-dot" style="color:${col};background:${col}"></span>
              <span class="marquee-wrap">${esc(ag.name)}</span>
            </div>
            <div class="ov-agent-metric marquee-wrap">${esc(statusText)}</div>
          </div>
        </div>
        ${renderOverviewPager()}
      </div>`;
    },

    prompt: () => `${global.MikuChat.renderChatHeader()}${global.MikuChat.renderChatCard()}`,

    result: () => {
      const { runtime, curAg, esc } = global.MikuUI;
      const { ic } = global.MikuComponents;
      const ag = curAg();
      return `${HD('Respuesta', 'overview')}<div class="card mk">
        <span class="cs" data-w="84" data-h="84" style="width:84px;height:84px"></span>
        <div class="cp">
          <div class="msg-agent-meta">
            <span class="msg-agent-badge">${esc(ag.name)}</span>
            <span>
              <button type="button" class="ib" data-a="copy-result" title="Copiar">${ic('copy')}</button>
              <button type="button" class="ib" data-go="prompt" title="Continuar chat">${ic('chat')}</button>
            </span>
          </div>
          <div class="msg-agent-inbox"><div class="msg-agent-text">${global.MikuChat.formatAgentMarkdown(runtime.resText || 'Sin respuesta aún.')}</div></div>
        </div>
      </div>`;
    },

    wardrobe: () => {
      const { PRIN, WARDROBE, OL, PRESET, curAg, bodyOf, usedByColor, esc } = global.MikuUI;
      const { ic } = global.MikuComponents;
      const avtHtml = global.MikuMascot?.avtHtml || (() => '');
      const ag = curAg();
      const isPrin = !ag || ag === PRIN || ag.id === 'miku';
      const cu = ag?.outfit || global.MikuMascot?.getPhys?.()?.outfit || 'none';
      const bc = bodyOf(ag || PRIN);
      const agColor = (ag?.c && ag.c !== '#ffffff' ? ag.c : '#3b9eff').toLowerCase();

      const outBtns = WARDROBE.map(([id, label], idx) =>
        `<button type="button" class="wb${cu === id ? ' on' : ''}" data-outfit="${esc(id)}" title="${esc(label)}" aria-label="${esc(label)}">${id === 'none' ? ic('none') : avtHtml(bc, id, idx, 0, 'head', 'lite')}</button>`
      ).join('');

      const colorRow = isPrin ? '' : `<div class="crow">
        <span class="wn">Color</span>
        ${PRESET.map(h => {
          const on = h.toLowerCase() === agColor;
          const dis = usedByColor(h, ag);
          return `<button type="button" class="sw8${on ? ' on' : ''}" data-body-color="${h}" style="--c:${h}" aria-label="Color ${h}"${dis ? ' disabled title="En uso por otro agente"' : ` title="${h}"`}></button>`;
        }).join('')}
        <label class="colorin" title="Color personalizado"><input id="wdr-color-pick" type="color" value="${esc(agColor)}"></label>
      </div>`;

      return `${HD('', 'overview')}<div class="wdr"><div class="card mk">
        <span class="cs" data-w="96" data-h="88" style="width:96px;height:88px"></span>
        <div class="cp">
          <div class="wh">
            <b>Guardarropa${isPrin ? '' : ` <em>${esc(ag.name)}</em>`}</b>
            <span class="wh-meta"><span class="wn" id="wn">${esc(OL[cu] || cu)}</span><span class="wh-sep">·</span><button type="button" class="lnk" data-go="gallery">Galería (25)</button></span>
          </div>
          ${colorRow}
          <div class="wrow">${outBtns}</div>
        </div>
      </div></div>`;
    },

    gallery: () => {
      const { GAL, esc } = global.MikuUI;
      const phys = global.MikuMascot?.getPhys?.() || { base: 'idle' };
      const btns = GAL.map(s => `<button type="button" class="${phys.base === s ? 'on' : ''}" data-gal-state="${s}">${esc(s)}</button>`).join('');
      return `${HD('Galería de Expresiones', 'wardrobe')}<div class="card mk">
        <span class="cs" data-w="96" data-h="96" style="width:96px;height:96px"></span>
        <div class="cp"><div class="gal">${btns}</div></div>
      </div>`;
    },

    settings: () => {
      const { ST, CFG, esc } = global.MikuUI;
      const voiceOn = CFG.voice !== false && CFG.voice !== 0 && CFG.sounds !== false;
      const hoverOn = !!CFG.hover_open;
      const dt = (label, on) => `<span class="dd"><i class="sd${on ? ' on' : ''}"></i>${esc(label)}</span>`;
      return cW(`${HD('', 'overview')}<div class="card mk">
        <span class="cs" data-w="80" data-h="60" style="width:80px;height:60px"></span>
        <div class="cp qs">
          <div class="qs-vol-block">
            <div class="sr">
              <label class="sw"><input type="checkbox" data-qs-toggle="voice"${voiceOn ? ' checked' : ''}><i></i></label>
              <span class="qs-title">Sonido y voz</span>
            </div>
            <div id="qs-vol-host" style="width:100%"></div>
          </div>
          <div class="sr">
            <label class="sw"><input type="checkbox" data-qs-toggle="hover_open"${hoverOn ? ' checked' : ''}><i></i></label>
            <span>Abrir al pasar el cursor</span>
          </div>
          <div class="sr st2">
            ${dt('Core 47400', ST.ws)}
            ${dt('API', ST.api)}
            <button type="button" class="lk2" data-go="link">${dt('Miku-Link', ST.link)}</button>
            <button type="button" class="lnk" data-open-settings="general">Ajustes…</button>
          </div>
        </div>
      </div>`, 460);
    },

    link: () => {
      const { ST, runtime, tr, esc } = global.MikuUI;
      const { ic } = global.MikuComponents;
      return `${HD('Miku-Link', 'overview')}<div class="lkg">
        <div class="card lkd"><span class="cs" data-w="96" data-h="96" style="width:96px;height:96px"></span></div>
        <div class="card lkr">
          <div class="lkt">
            <b>${esc(tr('link_title'))}</b>
            <span class="sub2" style="margin-top:0;font-size:12px">${esc(tr('link_subtitle'))}</span>
            <span class="st ${ST.link ? 'on' : ''}">${esc(ST.link ? `Vinculado: ${ST.name || 'Móvil'}` : (ST.ws ? 'Listo para escanear' : 'Daemon offline (QR local)'))}</span>
            <div class="chips" style="margin-top:4px">
              <button type="button" class="p" data-a="pair-refresh">${ic('retry')}<span>Nuevo QR</span></button>
              <button type="button" data-a="copy-pair">${ic('copy')}<span>Copiar URL</span></button>
              ${ST.link ? '<button type="button" class="mk-danger-btn" data-a="unpair">Desvincular</button>' : ''}
            </div>
          </div>
          <canvas id="qr" width="288" height="288"></canvas>
        </div>
      </div>`;
    },

    approve: () => {
      const { runtime, esc } = global.MikuUI;
      const cmd = runtime.pendingCmd || 'git status && cargo check --workspace';
      const id = runtime.pendingApproveId || 'preview-approve-1';
      const diffHtml = runtime.pendingDiff
        ? `<pre class="ap-diff scr">${esc(runtime.pendingDiff)}</pre>`
        : '';
      return cW(`${HD('Gatekeeper', 'overview')}<div class="card mk">
        <span class="cs" data-w="88" data-h="88" style="width:88px;height:88px"></span>
        <div class="cp">
          <b>Confirmación de acción sensible</b>
          <span class="sub2">La política Gatekeeper solicita autorización explícita antes de ejecutar:</span>
          <div class="cmd-box" style="margin:8px 0">${esc(cmd)}</div>
          ${diffHtml}
          <div class="ap-actions">
            <button type="button" class="btn-deny" data-approve-id="${esc(id)}" data-verdict="deny">Denegar <kbd>N</kbd></button>
            <button type="button" class="btn-allow" data-approve-id="${esc(id)}" data-verdict="allow">Permitir <kbd>Y</kbd></button>
            <button type="button" class="btn-always" data-approve-id="${esc(id)}" data-verdict="always">Siempre en sesión <kbd>A</kbd></button>
          </div>
        </div>
      </div>`, 500);
    },

    tool_use: () => {
      const { runtime, esc } = global.MikuUI;
      return `${HD('Herramienta Activa', 'prompt')}<div class="card mk">
        <span class="cs" data-w="84" data-h="84" style="width:84px;height:84px"></span>
        <div class="cp">
          <b>${esc(runtime.toolName || 'Ejecución local')}</b>
          <div class="cmd-box" style="margin-top:8px">${esc(runtime.toolCmd || 'Procesando contrato...')}</div>
        </div>
      </div>`;
    },

    notify: () => {
      const { runtime, esc } = global.MikuUI;
      return `${HD('Notificación', 'overview')}<div class="card mk">
        <span class="cs" data-w="80" data-h="80" style="width:80px;height:80px"></span>
        <div class="cp">
          <b>${esc(runtime.noteTitle || 'Aviso de Miku')}</b>
          <span class="sub2">${esc(runtime.noteBody || '')}</span>
          <div class="chips"><button type="button" class="p" data-go="compact">Aceptar</button></div>
        </div>
      </div>`;
    },

    alert: () => {
      const { runtime, esc } = global.MikuUI;
      return `${HD('Alerta Gatekeeper', 'overview')}<div class="card mk">
        <span class="cs" data-w="84" data-h="84" style="width:84px;height:84px"></span>
        <div class="cp">
          <b style="color:var(--danger)">${esc(runtime.alertTitle || 'Bloqueo preventivo')}</b>
          <span class="sub2">${esc(runtime.alertBody || 'Se bloqueó una operación fuera del perímetro.')}</span>
          <div class="chips">
            <button type="button" data-go-settings="security">Ver reglas</button>
            <button type="button" class="p" data-go="compact">Entendido</button>
          </div>
        </div>
      </div>`;
    },

    player: () => `${HD('Multimedia', 'overview')}<div class="card mk">
      <span class="cs" data-w="84" data-h="84" style="width:84px;height:84px"></span>
      <div class="cp">
        <div id="pl-sound-host"></div>
      </div>
    </div>`,

    quicklook: () => {
      const { runtime, esc } = global.MikuUI;
      return `${HD('Vista Rápida', 'overview')}<div class="card mk">
        <span class="cs" data-w="76" data-h="76" style="width:76px;height:76px"></span>
        <div class="cp">
          <b>${esc(runtime.ql.title || 'Documento')}</b>
          <div class="cmd-box scr" style="max-height:160px;overflow-y:auto;margin-top:6px">${esc(runtime.ql.text || '')}</div>
        </div>
      </div>`;
    },

    drop: () => `${HD('Subir archivos', 'overview')}<div class="card mk dz" id="dropzone-card">
      <span class="cs" data-w="86" data-h="86" style="width:86px;height:86px"></span>
      <div class="cp">
        <b>Arrastra un archivo aquí o haz clic</b>
        <span class="sub2">Se adjunta directo al agente activo o se envía al Gateway local.</span>
      </div>
    </div>`,

    uploading: () => {
      const { runtime, esc } = global.MikuUI;
      const pct = Math.max(0, Math.min(100, Number(runtime.uploadPct || 0)));
      return `${HD('Procesando archivo', 'overview')}<div class="card mk">
        <span class="cs" data-w="82" data-h="82" style="width:82px;height:82px"></span>
        <div class="cp">
          <b>${esc(runtime.fn || 'archivo')}</b>
          <div class="pg"><i style="width:${pct}%"></i></div>
          <span class="sub2">${pct}% completado</span>
        </div>
      </div>`;
    },

    choose: () => {
      const { runtime, esc } = global.MikuUI;
      const opts = (runtime.choices || ['Continuar', 'Cancelar']).map(c =>
        `<button type="button" data-choose-val="${esc(c)}">${esc(c)}</button>`
      ).join('');
      return `${HD('Selección', 'overview')}<div class="card mk">
        <span class="cs" data-w="84" data-h="84" style="width:84px;height:84px"></span>
        <div class="cp"><b>${esc(runtime.chooseTitle || 'Elige una opción')}</b><div class="chips">${opts}</div></div>
      </div>`;
    },

    question: () => {
      const { runtime, esc } = global.MikuUI;
      const q = runtime.question || {};
      const opts = Array.isArray(q.options) && q.options.length ? q.options : ['Continuar', 'Cancelar'];
      const list = opts.slice(0, 9).map((opt, idx) =>
        `<button type="button" class="q-opt" data-q-idx="${idx}" data-q-val="${esc(opt)}"><kbd>${idx + 1}</kbd><span>${esc(opt)}</span></button>`
      ).join('');
      return cW(`${HD('Decisión', 'overview')}<div class="card mk">
        <span class="cs" data-w="88" data-h="88" style="width:88px;height:88px"></span>
        <div class="cp">
          <b>${esc(q.title || 'Pregunta activa')}</b>
          <span class="sub2">${esc(q.text || 'Elige una opción (teclas 1-9):')}</span>
          <div class="q-opts">${list}</div>
        </div>
      </div>`, 500);
    },

    error_card: () => {
      const { runtime, esc } = global.MikuUI;
      const er = runtime.errorCard || {};
      return cW(`${HD('Diagnóstico de Error', 'overview')}<div class="card mk">
        <span class="cs" data-w="86" data-h="86" style="width:86px;height:86px"></span>
        <div class="cp">
          <div class="err-head">
            <b style="color:var(--danger)">${esc(er.title || 'Error de ejecución')}</b>
            <span class="err-badge">EXIT ${esc(String(er.exitCode ?? 1))}</span>
          </div>
          <pre class="err-pre scr">${esc(er.stderr || 'Sin detalles registrados.')}</pre>
          <div class="chips">
            <button type="button" class="p" id="err-retry-btn">Reintentar</button>
            <button type="button" data-go="prompt">Abrir Chat</button>
            <button type="button" data-go="compact">Cerrar</button>
          </div>
        </div>
      </div>`, 510);
    },

    agent_delegate: () => {
      const { ST, PRIN, runtime, esc } = global.MikuUI;
      const roster = [PRIN, ...(Array.isArray(ST.agents) ? ST.agents : [])];
      const uniq = roster.filter((a, i) => roster.findIndex(x => x.id === a.id) === i);
      const selId = runtime.delegateAgentId || 'miku';
      const mode = runtime.delegateMode || 'code';
      const agChips = uniq.map(a =>
        `<button type="button" class="${a.id === selId ? 'p' : ''}" data-delegate-agent="${esc(a.id)}">${esc(a.name)}</button>`
      ).join('');
      const modeChips = ['code', 'plan', 'research'].map(m =>
        `<button type="button" class="${m === mode ? 'p' : ''}" data-delegate-mode="${m}">${esc(m)}</button>`
      ).join('');
      return cW(`${HD('Delegar Tarea', 'overview')}<div class="card mk">
        <span class="cs" data-w="84" data-h="84" style="width:84px;height:84px"></span>
        <div class="cp">
          <b>Orquestación multi-agente real</b>
          <div class="chips" style="margin-top:6px">${agChips}</div>
          <div class="chips" style="margin-top:4px">${modeChips}</div>
          <textarea id="delegate-task-input" class="del-ta" placeholder="Describe la tarea para ejecutar y guardar en SQLite FTS5..."></textarea>
          <div class="chips">
            <button type="button" class="p" id="delegate-send-btn">Delegar y Ejecutar</button>
            <button type="button" data-go="agent_work">Ver última ejecución</button>
          </div>
        </div>
      </div>`, 520);
    },

    agent_work: () => {
      const { runtime, esc } = global.MikuUI;
      const w = runtime.agentWork || {};
      const steps = Array.isArray(w.phases) ? w.phases : [];
      const stepsHtml = steps.map(s => {
        const cls = s.state === 'done' ? 'is-done' : (s.state === 'active' ? 'is-active' : '');
        return `<div class="aw-step ${cls}"><i></i><span>${esc(s.label || '')}</span></div>`;
      }).join('');
      const bodyHtml = w.result
        ? global.MikuChat.formatAgentMarkdown(w.result)
        : `<span class="sub2">Esperando salida de ${esc(w.agentName || 'Miku')} (${esc(w.mode || 'code')})...</span>`;
      return cW(`${HD('Trabajo de Agente', 'overview')}<div class="aw-split">
        <div class="aw-side">
          <span class="cs" data-w="72" data-h="72" style="width:72px;height:72px"></span>
          <b style="font-size:12.5px">${esc(w.agentName || 'Miku')}</b>
          <span class="dd">${esc(w.status || 'idle')}</span>
          <div class="aw-steps">${stepsHtml}</div>
        </div>
        <div class="aw-main">
          <b>${esc(w.title || 'Sin tarea activa')}</b>
          <div class="aw-body scr">${bodyHtml}</div>
          <div class="chips">
            <button type="button" class="p" data-go="agent_delegate">Nueva delegación</button>
            <button type="button" data-go="prompt">Abrir Chat</button>
            <button type="button" data-go="compact">Cerrar</button>
          </div>
        </div>
      </div>`, 620);
    },

    update_card: () => {
      const { ST, runtime, esc } = global.MikuUI;
      const upd = ST.updateInfo || runtime.updateCard || {};
      return cW(`${HD('Actualizador SHA-256', 'overview')}<div class="card mk">
        <span class="cs" data-w="84" data-h="84" style="width:84px;height:84px"></span>
        <div class="cp">
          <b>Miku Core · ${esc(upd.version || 'al día')}</b>
          <span class="sub2">${esc(upd.notes || 'Integridad verificada por firma SHA-256.')}</span>
          <div class="chips">
            <button type="button" class="p" id="upd-check-btn">Comprobar ahora</button>
            <button type="button" data-go-settings="system">Diagnóstico</button>
          </div>
        </div>
      </div>`, 520);
    },

    mcp_download: () => {
      const { runtime, esc } = global.MikuUI;
      const dl = runtime.mcpDl || {};
      const pct = Math.max(0, Math.min(100, Number(dl.progress || 0)));
      const cat = Array.isArray(runtime.mcpCatalog) ? runtime.mcpCatalog.slice(0, 3) : [];
      const quickCat = cat.map(c =>
        `<button type="button" data-mcp-quick-install="${esc(c.id)}">${esc(c.installed ? `${c.id} ✓` : `+ ${c.id}`)}</button>`
      ).join('');
      return cW(`${HD('Instalación MCP', 'overview')}<div class="card mk">
        <span class="cs" data-w="82" data-h="82" style="width:82px;height:82px"></span>
        <div class="cp">
          <b>${esc(dl.name || 'Servidor MCP')} · ${esc(dl.status || 'listo')}</b>
          <div class="pg"><i style="width:${pct}%"></i></div>
          <span class="sub2">${pct}% completado</span>
          <div class="chips">${quickCat}<button type="button" data-go-settings="modules">Catálogo MCP</button></div>
        </div>
      </div>`, 480);
    },

    ai_inference: () => {
      const { CFG, runtime, esc } = global.MikuUI;
      const ai = runtime.aiInf || {};
      const prov = ai.provider || CFG.provider || 'openai';
      const model = ai.model || CFG.model || 'gpt-4o-mini';
      return cW(`${HD('Telemetría IA', 'overview')}<div class="card mk">
        <span class="cs" data-w="84" data-h="84" style="width:84px;height:84px"></span>
        <div class="cp">
          <b>${esc(prov)} / ${esc(model)}</b>
          <span class="sub2">Estado: ${esc(ai.status || 'idle')}${ai.host ? ` · ${esc(ai.host)}` : ''}</span>
          <div class="ai-stats">
            <div class="ai-stat"><small>Latencia</small><b>${Number(ai.latencyMs || 0)} ms</b></div>
            <div class="ai-stat"><small>Prompt tok</small><b>${ai.inputTokens ?? '—'}</b></div>
            <div class="ai-stat"><small>Salida tok</small><b>${ai.outputTokens ?? '—'}</b></div>
          </div>
          <div class="chips">
            <button type="button" class="p" data-go="prompt">Probar en Chat</button>
            <button type="button" data-go-settings="providers">Configurar API</button>
          </div>
        </div>
      </div>`, 490);
    },

    web_grounding: () => {
      const { runtime, esc } = global.MikuUI;
      const wg = runtime.webGround || {};
      const srcs = (Array.isArray(wg.sources) ? wg.sources : []).map(s =>
        `<span class="wg-src">${esc(s)}</span>`
      ).join('');
      return cW(`${HD('Fuentes Verificadas', 'overview')}<div class="card mk">
        <span class="cs" data-w="80" data-h="80" style="width:80px;height:80px"></span>
        <div class="cp">
          <b>${esc(wg.query || 'Contexto web / memoria')}</b>
          <span class="sub2">${esc(wg.summary || 'Sin citas activas en esta consulta.')}</span>
          <div class="wg-srcs">${srcs}</div>
        </div>
      </div>`, 500);
    }
  };

  function postMount(name, col, hooks) {
    const { $, CFG, MUS, runtime, ST, saveCfg } = global.MikuUI;
    const { mkSound, Slosh, SmoothSlider } = global.MikuComponents;

    runtime.ovPlayer = null;
    runtime.ovSlosh = null;
    runtime.plPlayer = null;

    if (name === 'prompt') {
      global.MikuChat.bindChatEvents(col, hooks);
    } else if (name === 'settings') {
      let volSaveTimer = 0;
      const curVol = Number(CFG.voice_volume ?? MUS.vol ?? 80);
      SmoothSlider?.($('#qs-vol-host'), {
        value: curVol,
        unit: '%',
        onChange: v => {
          CFG.voice_volume = v;
          MUS.vol = v;
          global.SoundFX?.setVolume?.(v);
          saveCfg?.();
          clearTimeout(volSaveTimer);
          volSaveTimer = setTimeout(() => {
            global.Bridge?.setSetting?.('voice_volume', v);
            global.Bridge?.media?.('vol', v);
            global.Bridge?.broadcast?.({ k: 'setting_updated', patch: { voice_volume: v } });
          }, 90);
        }
      });
    } else if (name === 'link') {
      const cv = $('#qr');
      if (cv) global.MikuQR.renderPairingQr(cv, runtime.pairUrl, runtime.pairQrSvg);
      if (ST.ws && (!runtime.pairUrl || runtime.pairUrl.includes('awaiting-gateway'))) {
        global.Bridge.generatePairingUrl().then(() => {
          const cv2 = $('#qr');
          if (cv2) global.MikuQR.renderPairingQr(cv2, runtime.pairUrl, runtime.pairQrSvg);
        }).catch(() => {});
      }
    } else if (name === 'overview' && ((runtime.pg || 0) % 2 + 2) % 2 === 1) {
      let volSaveTimer = 0;
      runtime.ovPlayer = mkSound($('#ov-sound-host'), {
        get: () => MUS,
        act: r => global.Bridge.media(r)
      });
      runtime.ovSlosh = Slosh($('#ov-slosh-host'), {
        value: Number(MUS.vol ?? CFG.voice_volume ?? 80),
        label: 'Volumen',
        onChange: v => {
          MUS.vol = v;
          CFG.voice_volume = v;
          saveCfg?.();
          clearTimeout(volSaveTimer);
          volSaveTimer = setTimeout(() => {
            global.Bridge?.media?.('vol', v);
          }, 90);
        }
      });
      if (ST.ws) global.Bridge?.fetchNowPlaying?.().catch(() => null);
    } else if (name === 'player') {
      runtime.plPlayer = mkSound($('#pl-sound-host'), {
        get: () => MUS,
        act: r => global.Bridge.media(r)
      });
      if (ST.ws) global.Bridge?.fetchNowPlaying?.().catch(() => null);
    } else if (name === 'drop') {
      $('#dropzone-card')?.addEventListener('click', () => document.getElementById('miku-file-input')?.click());
    }

    global.MikuMascot?.avatars?.refresh?.(col);
  }

  global.MikuScenes = { SC, cW, HD, MINIH, postMount };
})(window);
