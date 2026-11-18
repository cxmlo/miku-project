/**
 * core/state.js — Estado reactivo local, carga de locales e helpers DOM seguros
 * ----------------------------------------------------------------------------
 * Cero agentes falsos ni texto estático ajeno a Miku. Todo estado se sincroniza
 * con miku-gateway (127.0.0.1:47400) y respeta los overrides de %LOCALAPPDATA%\Miku\ui\.
 */
(function (global) {
  'use strict';

  const DEFAULT_LOCALES = {
    es: {
      tab_home: 'Inicio',
      tab_chat: 'Preguntar',
      tab_files: 'Subir archivos',
      boot_eyebrow: 'MIKU CORE',
      beta_tag: 'BETA',
      boot_title: 'Conectando con Miku Daemon...',
      boot_step_0: 'Inicializando puente local',
      boot_step_1: 'Cargando tema y muñeco modular',
      boot_step_2: 'Sincronizando Gateway 127.0.0.1:47400',
      boot_connected: 'Core 47400 conectado',
      boot_offline: 'Modo vista previa local (sin daemon activo)',
      boot_note: 'Arquitectura modular 100% personalizable en disco',
      orchestrator: 'Orquestador',
      subagent: 'Agente',
      active_status: 'Activo',
      ready_status: 'Listo',
      offline_status: 'Sin daemon',
      memory_active: 'SQLite WAL + FTS5 listo',
      agents_avail_prefix: 'Agentes',
      agents_avail: 'registrados',
      system_status: 'Gateway',
      core_synced: '127.0.0.1:47400 conectado',
      local_mode: 'Vista previa desconectada',
      model_label: 'Modelo',
      role_label: 'Rol',
      status_label: 'Estado',
      thinking: 'Pensando...',
      done_task: 'Última tarea completada',
      idle_task: 'Esperando instrucciones',
      chat_placeholder: 'Escribe un mensaje o comando...',
      quick_settings: 'Ajustes y Módulos',
      sound: 'Sonido y voz',
      mascot_customize: 'Personalizar muñeco y tema',
      gatekeeper_title: 'Gatekeeper (.miku.toml)',
      kill_switch: 'KILL SWITCH GLOBAL',
      providers_title: 'Proveedores y API Keys (DPAPI)',
      mcp_title: 'Servidores MCP',
      skills_title: 'Skills en caliente',
      memory_search_title: 'Memoria SQLite FTS5',
      link_title: 'Vincular con el celular (Miku-Link)',
      link_subtitle: 'Escanea el QR con la app móvil por LAN o Tailscale (*.ts.net) para aprobar permisos y sincronizar.',
      doctor_title: 'Diagnóstico (miku doctor)',
      updater_title: 'Actualizador SHA-256',
      no_agents_yet: 'Conecta el daemon o define agentes en .miku/agents/'
    },
    en: {
      tab_home: 'Home',
      tab_chat: 'Ask',
      tab_files: 'Drop files',
      boot_eyebrow: 'MIKU CORE',
      beta_tag: 'BETA',
      boot_title: 'Connecting to Miku Daemon...',
      boot_step_0: 'Initializing local bridge',
      boot_step_1: 'Loading modular theme & mascot',
      boot_step_2: 'Syncing Gateway 127.0.0.1:47400',
      boot_connected: 'Core 47400 connected',
      boot_offline: 'Local UI preview (daemon offline)',
      boot_note: '100% file-customizable modular UI architecture',
      orchestrator: 'Orchestrator',
      subagent: 'Agent',
      active_status: 'Active',
      ready_status: 'Ready',
      offline_status: 'Offline',
      memory_active: 'SQLite WAL + FTS5 ready',
      agents_avail_prefix: 'Agents',
      agents_avail: 'registered',
      system_status: 'Gateway',
      core_synced: '127.0.0.1:47400 synced',
      local_mode: 'Preview disconnected',
      model_label: 'Model',
      role_label: 'Role',
      status_label: 'Status',
      thinking: 'Thinking...',
      done_task: 'Last task finished',
      idle_task: 'Awaiting orders',
      chat_placeholder: 'Write a message or command...',
      quick_settings: 'Settings & Modules',
      sound: 'Sound & Voice',
      mascot_customize: 'Customize mascot & theme',
      gatekeeper_title: 'Gatekeeper (.miku.toml)',
      kill_switch: 'GLOBAL KILL SWITCH',
      providers_title: 'Providers & API Keys (DPAPI)',
      mcp_title: 'MCP Servers',
      skills_title: 'Hot-Reload Skills',
      memory_search_title: 'SQLite FTS5 Memory',
      link_title: 'Pair phone (Miku-Link)',
      link_subtitle: 'Scan QR with mobile app over LAN or Tailscale (*.ts.net) for live approvals & sync.',
      doctor_title: 'Diagnostics (miku doctor)',
      updater_title: 'SHA-256 Updater',
      no_agents_yet: 'Connect daemon or add agents under .miku/agents/'
    }
  };

  const LOCALES = {
    es: { ...DEFAULT_LOCALES.es },
    en: { ...DEFAULT_LOCALES.en }
  };

  const $ = s => document.querySelector(s);
  const $$ = s => document.querySelectorAll(s);
  const clamp = (v, a = -1, b = 1) => Math.max(a, Math.min(b, v));
  const esc = s => String(s ?? '').replace(/[&<>"']/g, m => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m]));
  const safeCssColor = (c, fb = '#3B9EFF') => /^#[0-9a-f]{3,8}$/i.test(String(c || '').trim()) ? String(c).trim() : fb;

  const lum = h => {
    const clean = safeCssColor(h, '#3B9EFF').slice(1);
    const full = clean.length === 3 ? clean.split('').map(c => c + c).join('') : clean.slice(0, 6);
    const n = parseInt(full, 16);
    return (0.299 * (n >> 16) + 0.587 * ((n >> 8) & 255) + 0.114 * (n & 255)) / 255;
  };
  const mixColor = (h, k = 0.35) => {
    const clean = safeCssColor(h, '#3B9EFF').slice(1);
    const full = clean.length === 3 ? clean.split('').map(c => c + c).join('') : clean.slice(0, 6);
    const n = parseInt(full, 16);
    const ch = s => Math.round(((n >> s) & 255) * (1 - k) + 255 * k);
    return `rgb(${ch(16)},${ch(8)},${ch(0)})`;
  };
  const hexDist = (a, b) => {
    const x = parseInt(safeCssColor(a, '#000000').slice(1, 7), 16);
    const y = parseInt(safeCssColor(b, '#000000').slice(1, 7), 16);
    return Math.hypot((x >> 16) - (y >> 16), ((x >> 8) & 255) - ((y >> 8) & 255), (x & 255) - (y & 255));
  };

  function el(tag, attrs = {}, ...children) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v == null || v === false) continue;
      if (k === 'class') node.className = v;
      else if (k === 'style' && typeof v === 'object') Object.assign(node.style, v);
      else if (k.startsWith('on') && typeof v === 'function') node.addEventListener(k.slice(2).toLowerCase(), v);
      else if (k === 'text') node.textContent = v;
      else if (v === true) node.setAttribute(k, '');
      else node.setAttribute(k, String(v));
    }
    for (const ch of children.flat()) {
      if (ch == null || ch === false) continue;
      node.appendChild(typeof ch === 'string' || typeof ch === 'number' ? document.createTextNode(String(ch)) : ch);
    }
    return node;
  }

  const PRIN = {
    id: 'miku',
    name: 'Miku',
    role: 'Orquestador Local (Rust)',
    model: 'DeepSeek / Multi-Provider',
    c: '#ffffff',
    outfit: 'none'
  };
  try { const o = localStorage.getItem('miku_outfit'); if (o) PRIN.outfit = o; } catch (_) {}

  const ST = {
    ws: 0,
    mic: 0,
    link: 0,
    name: '',
    tasks: 0,
    api: 0,
    killSwitch: false,
    agents: [],
    secrets: [],
    rules: [],
    skills: [],
    mcp: [],
    memoryHits: [],
    audioDevices: [],
    doctorReport: null,
    updateInfo: null,
    appsDetected: []
  };

  const CFG = {
    voice: 1,
    sfx_enabled: 1,
    voice_stt_enabled: false,
    voice_stt_engine: 'off',
    voice_auto_send: false,
    auto_screen_capture: false,
    auto_pc_control: false,
    autoclose: 15,
    voice_volume: 80,
    sfx_volume: 60,
    barge_in: 1,
    wake_paused: 0,
    mic: 'default',
    personality: 'calm',
    hover_open: 0,
    local_net: 1,
    biometric: 1,
    lang: 'es',
    provider: 'deepseek',
    model: 'deepseek-chat',
    key_alias: 'deepseek-main',
    custom_base_url: '',
    theme_preset: 'obsidian',
    ui_accent: '#f5f5f7',
    island_opacity: 96,
    island_radius: 32,
    island_scale: 100,
    token_saver_local_router: 1,
    token_saver_prefix_cache: 1,
    token_saver_prune_tools: 1,
    daily_budget_usd: 5,
    history_turns: 6
  };
  try { Object.assign(CFG, JSON.parse(localStorage.getItem('miku_cfg') || '{}')); } catch (_) {}
  if (!Array.isArray(ST.harnesses)) ST.harnesses = [];
  function saveCfg() {
    try { localStorage.setItem('miku_cfg', JSON.stringify(CFG)); } catch (_) {}
  }

  const THEME_PRESETS = [
    { id: 'obsidian', label: 'Obsidian', accent: '#f5f5f7', bg: '5, 6, 9' },
    { id: 'cyan', label: 'Miku Cyan', accent: '#39c5bb', bg: '5, 10, 12' },
    { id: 'emerald', label: 'Emerald', accent: '#30d158', bg: '5, 10, 7' },
    { id: 'amber', label: 'Amber Gold', accent: '#fbbf24', bg: '10, 8, 5' },
    { id: 'amethyst', label: 'Amethyst', accent: '#a78bfa', bg: '8, 6, 13' }
  ];

  function hexToRgbTuple(hex) {
    const clean = safeCssColor(hex, '#f5f5f7').replace('#', '');
    if (clean.length !== 6) return [245, 245, 247];
    return [
      parseInt(clean.slice(0, 2), 16) || 0,
      parseInt(clean.slice(2, 4), 16) || 0,
      parseInt(clean.slice(4, 6), 16) || 0
    ];
  }

  function applyThemeVars(cfgInput = CFG) {
    const root = global.document?.documentElement;
    if (!root) return;
    const cfg = cfgInput || CFG;
    const preset = THEME_PRESETS.find(p => p.id === cfg.theme_preset) || THEME_PRESETS[0];
    const accent = safeCssColor(cfg.ui_accent || preset.accent, preset.accent);
    const [r, g, b] = hexToRgbTuple(accent);
    const bright = (0.299 * r + 0.587 * g + 0.114 * b) > 152;
    const ink = bright ? '#0d0e12' : '#f5f5f7';
    const sky = mixColor(accent, '#ffffff', 0.28);
    const opacityPct = clamp(Number(cfg.island_opacity ?? 96), 65, 100);
    const alpha = (opacityPct / 100).toFixed(2);
    const radiusOpen = clamp(Number(cfg.island_radius ?? 32), 16, 40);
    const bgTuple = preset.bg || '5, 6, 9';

    root.style.setProperty('--ac', accent);
    root.style.setProperty('--acx', ink);
    root.style.setProperty('--sky', sky);
    root.style.setProperty('--ac-soft', `rgba(${r}, ${g}, ${b}, 0.14)`);
    root.style.setProperty('--ac-border', `rgba(${r}, ${g}, ${b}, 0.34)`);
    root.style.setProperty('--ac-glow', `rgba(${r}, ${g}, ${b}, 0.22)`);
    root.style.setProperty('--island-bg', `rgba(${bgTuple}, ${alpha})`);
    root.style.setProperty('--island-alpha', alpha);
    root.style.setProperty('--island-radius-open', `${radiusOpen}px`);
    root.style.setProperty('--isl-scale', String(clamp(Number(cfg.island_scale ?? 100), 85, 115) / 100));
  }
  applyThemeVars(CFG);

  const MUS = { title: 'Sin reproducción', artist: '—', playing: 0, vol: 50, cover: '', dur: 214, pos: 0 };

  const PRESET = ['#F2A765', '#F06CF0', '#6C63FF', '#FF5C66', '#4FD3B1', '#3B9EFF', '#FFD84D', '#9BE15D'];
  const OL = {
    none: 'Ninguno', beanie: 'Gorro', crown: 'Corona', party: 'Fiesta', santa: 'Santa',
    witch: 'Bruja', shades: 'Gafas sol', round: 'Gafas', bow: 'Moño', scarf: 'Bufanda',
    pumpkin: 'Calabaza', bunny: 'Orejas', halo: 'Aureola', propeller: 'Hélice', phones: 'Audífonos'
  };
  const WARDROBE = Object.entries(OL);
  const GAL = ['ask', 'approve', 'idle', 'listening', 'thinking', 'talking', 'searching', 'working', 'happy', 'proud', 'wink', 'love', 'surprised', 'question', 'annoyed', 'sleeping', 'error', 'dizzy', 'look_up', 'look_down', 'look_left', 'look_right', 'squash', 'swallow', 'wave'];

  const MIKU_PROVIDERS = [
    { id: 'deepseek', label: 'DeepSeek', models: ['deepseek-chat', 'deepseek-reasoner'] },
    { id: 'anthropic', label: 'Anthropic', models: ['claude-sonnet-4-5', 'claude-haiku-4-5'] },
    { id: 'openai', label: 'OpenAI', models: ['gpt-4o', 'o3-mini'] },
    { id: 'gemini', label: 'Google Gemini', models: ['gemini-2.5-flash', 'gemini-2.5-pro'] },
    { id: 'ollama', label: 'Ollama (Local)', models: ['qwen2.5-coder:7b', 'llama3.2:latest'] }
  ];

  const CHATS = {};
  try { Object.assign(CHATS, JSON.parse(localStorage.getItem('miku_chats') || '{}')); } catch (_) {}

  function getAgChat(id = 'miku') {
    if (!CHATS[id]) CHATS[id] = { lastQ: '', lastA: '', lastFile: null, busy: false, unread: false, thinkStart: 0, thinkDur: 0, steps: [], history: [] };
    return CHATS[id];
  }
  function saveAgChats() {
    try { localStorage.setItem('miku_chats', JSON.stringify(CHATS)); } catch (_) {}
  }

  function saveAgents() {
    const subs = Array.isArray(ST.agents) ? ST.agents.filter(a => a && a.id !== 'miku') : [];
    ST.agents = subs;
    try { localStorage.setItem('miku_agents', JSON.stringify(subs)); } catch (_) {}
    global.Bridge?.saveAgentsBackend?.(subs);
  }

  let SELA = -1;
  const curAg = () => (SELA < 0 ? PRIN : (ST.agents[SELA] || PRIN));
  const setSela = idx => { SELA = idx; };
  const getSela = () => SELA;
  const bodyOf = a => (a === PRIN || a.id === 'miku') ? (global.MikuMascot?.getConfig?.().defaultColor || '#ffffff') : safeCssColor(a.c);
  const usedByColor = (h, a) => ST.agents.some(x => x !== a && hexDist(x.c, h) < 44);
  const getUnreadAgent = () => {
    const order = [PRIN, ...ST.agents];
    return order.find(a => getAgChat(a.id).unread && a.id !== curAg().id) || null;
  };

  function tr(k) {
    const lang = CFG.lang || 'es';
    return LOCALES[lang]?.[k] || LOCALES.es[k] || k;
  }

  async function loadLocale(lang) {
    const code = lang === 'en' ? 'en' : 'es';
    try {
      const res = await fetch(`/shared/locales/${code}.json`);
      if (res.ok) {
        const json = await res.json();
        LOCALES[code] = { ...DEFAULT_LOCALES[code], ...json };
      }
    } catch (_) {}
    if (TABS[0]) TABS[0][2] = tr('tab_home');
    if (TABS[1]) TABS[1][2] = tr('tab_chat');
    if (TABS[2]) TABS[2][2] = tr('tab_files');
  }

  const TABS = [
    ['overview', 'home', tr('tab_home')],
    ['prompt', 'chat', tr('tab_chat')]
  ];

  const runtime = {
    scene: 'boot',
    previousScene: 'compact',
    pg: 0,
    pgLock: 0,
    settingsTab: 'mascot',
    pairUrl: 'miku://pair?mode=awaiting-gateway',
    pairQrSvg: '',
    pairError: '',
    resText: '',
    ql: { title: '', text: '' },
    fn: 'archivo',
    lastQ: '',
    hasUpdate: false,
    mascotConfig: null,
    thk: { on: 0, start: 0, end: 0, dur: 0 },
    turnWatchdogTimer: null,
    computerLiveStatus: '',
    pendingDiff: '',
    question: { title: 'Confirmar preferencia', text: '¿Cómo deseas proceder con la ejecución?', options: ['Ejecutar y verificar pruebas', 'Solo mostrar diff sin editar', 'Cancelar operación'] },
    errorCard: { title: 'Fallo de ejecución', exitCode: 1, stderr: 'Sin errores recientes.', retryPrompt: '' },
    agentWork: {
      taskId: '',
      agentId: 'miku',
      agentName: 'Miku',
      mode: 'code',
      title: 'Sin tarea delegada',
      status: 'idle',
      result: '',
      phases: [
        { label: 'Contexto SQLite', state: 'done' },
        { label: 'Ejecución de agente', state: 'active' },
        { label: 'Validación Gatekeeper', state: 'pending' }
      ]
    },
    delegateAgentId: 'miku',
    delegateMode: 'code',
    updateCard: { version: 'al día', channel: 'stable', notes: 'Verificación SHA-256 lista.' },
    mcpDl: { id: 'mcp', name: 'Servidor MCP', progress: 0, status: 'idle' },
    aiInf: { provider: 'openai', model: 'gpt-4o-mini', status: 'idle', latencyMs: 0, inputTokens: 0, outputTokens: 0, host: '' },
    webGround: { query: '', summary: '', sources: [] },
    mcpCatalog: [],
    audioOutputs: [],
    soundFiles: []
  };

  global.MikuUI = {
    $, $$, clamp, esc, safeCssColor, lum, mixColor, hexDist, el,
    PRIN, ST, CFG, MUS, PRESET, OL, WARDROBE, GAL, MIKU_PROVIDERS, TABS, THEME_PRESETS,
    getAgChat, saveAgChats, saveAgents, curAg, setSela, getSela, bodyOf, usedByColor, getUnreadAgent,
    tr, loadLocale, saveCfg, applyThemeVars, runtime
  };
})(window);
