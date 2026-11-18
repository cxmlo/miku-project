(() => {
  'use strict';

  const UI = window.MikuUI || {};
  const BRIDGE = window.Bridge;
  const esc = UI.esc || (s => String(s ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])));

  let qrExpiresAt = 0;
  let countdownTimer = null;
  let noticeMsg = '';
  let noticeErr = false;

  function stopCountdown() {
    if (countdownTimer) {
      clearInterval(countdownTimer);
      countdownTimer = null;
    }
  }

  function startCountdown(seconds) {
    stopCountdown();
    const sec = Math.max(0, Number(seconds || 0));
    if (!sec) {
      qrExpiresAt = 0;
      return;
    }
    qrExpiresAt = Date.now() + sec * 1000;
    const tick = () => {
      const el = document.getElementById('set-qr-countdown');
      if (!el) {
        stopCountdown();
        return;
      }
      const left = Math.max(0, Math.ceil((qrExpiresAt - Date.now()) / 1000));
      el.textContent = left > 0 ? `Expira en ${left}s` : 'QR expirado — genera uno nuevo';
      el.classList.toggle('status-er', left === 0);
      el.classList.toggle('status-ok', left > 0);
      if (left <= 0) stopCountdown();
    };
    tick();
    countdownTimer = setInterval(tick, 1000);
  }

  function fmtRel(tsMs) {
    const n = Number(tsMs || 0);
    if (!n) return '—';
    const d = Math.max(0, Math.floor((Date.now() - n) / 1000));
    if (d < 5) return 'ahora mismo';
    if (d < 60) return `hace ${d}s`;
    if (d < 3600) return `hace ${Math.floor(d / 60)}m`;
    return `hace ${Math.floor(d / 3600)}h`;
  }

  function setNotice(msg, isErr = false) {
    noticeMsg = String(msg || '');
    noticeErr = Boolean(isErr);
  }

  async function copyText(txt, label) {
    if (!txt) return;
    try {
      await navigator.clipboard.writeText(txt);
      setNotice(`${label} copiado al portapapeles`);
    } catch (_) {
      setNotice(`No se pudo copiar automáticamente: ${txt}`, true);
    }
  }

  function isDeviceOnline(dev) {
    if (!dev) return false;
    if (dev.online === true) return true;
    const seen = Number(dev.last_seen_ms || dev.paired_at_ms || 0);
    return seen > 0 && (Date.now() - seen) < 120_000;
  }

  function renderPhoneMockup(status, primaryDev, port) {
    const devices = Array.isArray(status?.devices) ? status.devices : (Array.isArray(UI.ST?.linkDevices) ? UI.ST.linkDevices : []);
    const dev = primaryDev || devices.find(isDeviceOnline) || devices[0] || null;
    const online = isDeviceOnline(dev);
    const name = dev?.device_name || (UI.ST?.link && UI.ST?.name ? UI.ST.name : 'Sin teléfono conectado');
    const bat = typeof dev?.battery_pct === 'number' && dev.battery_pct >= 0 ? `${dev.battery_pct}%${dev.battery_charging ? ' ⚡' : ''}` : 'Batería —';
    const transport = dev?.transport ? String(dev.transport).toUpperCase() : 'LAN / USB';
    const lanIp = status?.lan_ip || '127.0.0.1';
    const pcName = status?.desktop_name || 'Miku Desktop';

    return `
      <div class="pl-phone-frame${online ? ' online' : ''}">
        <div class="pl-phone-notch"></div>
        <div class="pl-phone-screen">
          <div class="pl-phone-top">
            <span class="pl-dot${online ? ' on' : ''}"></span>
            <span class="pl-phone-state">${online ? 'Conectado' : (dev ? 'En espera' : 'Listo para vincular')}</span>
            <span class="pl-phone-bat">${esc(bat)}</span>
          </div>
          <div class="pl-phone-name">${esc(name)}</div>
          <div class="pl-phone-meta">${esc(transport)} · Puerto ${esc(String(port))}</div>
          <div class="pl-phone-pc">
            <div><span class="pl-k">PC</span> <strong>${esc(pcName)}</strong></div>
            <div><span class="pl-k">IP</span> <code>${esc(lanIp)}:${esc(String(port))}</code></div>
            <div><span class="pl-k">Pulso</span> <span>${dev ? esc(fmtRel(dev.last_seen_ms)) : 'Sin enlace aún'}</span></div>
          </div>
        </div>
      </div>`;
  }

  function renderSection(ST, qrData) {
    const cfg = UI.CFG || {};
    const lanOn = cfg.local_net !== 0 && cfg.local_net !== false;
    const status = ST?.linkStatus || {};
    const port = Number(qrData?.port || status.port || BRIDGE?.GATEWAY_PORT || 47400);
    const lanIp = qrData?.lan_ip || status.lan_ip || '127.0.0.1';
    const lanBaseUrl = qrData?.lan_base_url || status.lan_base_url || `http://${lanIp}:${port}`;
    const adbCmd = qrData?.adb_reverse_cmd || status.adb_reverse_cmd || `adb reverse tcp:${port} tcp:${port}`;
    const pairUrl = qrData?.url || status.pair_url || '';
    const devices = Array.isArray(status.devices) ? status.devices : (Array.isArray(ST?.linkDevices) ? ST.linkDevices : []);
    const primaryDev = devices.find(isDeviceOnline) || devices[0] || null;
    const notifs = Array.isArray(ST?.linkNotifications) ? ST.linkNotifications : [];
    const unread = Number(ST?.linkUnread ?? status.unread_notifications ?? 0);

    return `
      <section class="card pl-hero-card">
        <div class="card-h">
          <div>
            <h2>Miku Phone Link — Enlace Móvil Android</h2>
            <div class="item-d">Controla la Isla Dinámica, multimedia, Gatekeeper y notificaciones en tiempo real.</div>
          </div>
          <div class="pill-row">
            <button type="button" class="set-btn prim" id="set-qr-refresh">Generar QR</button>
            <button type="button" class="set-btn" id="set-link-refresh">Actualizar</button>
            <button type="button" class="set-btn danger" id="set-link-unpair">Desvincular</button>
          </div>
        </div>

        ${noticeMsg ? `<div class="pl-notice ${noticeErr ? 'err' : 'ok'}">${esc(noticeMsg)}</div>` : ''}

        <div class="pl-hero-grid">
          ${renderPhoneMockup(status, primaryDev, port)}
          <div class="pl-hero-stats">
            <div class="item">
              <div>
                <div class="item-t">Escucha segura en red local (Wi-Fi LAN)</div>
                <div class="item-d">Habilita emparejamiento por código QR temporal con token de dispositivo aislado.</div>
              </div>
              <button type="button" class="set-pill${lanOn ? ' on' : ''}" data-toggle-num="local_net">${lanOn ? 'Habilitado' : 'Bloqueado'}</button>
            </div>
            <div class="pl-stat-row">
              <div class="pl-stat">
                <span class="pl-stat-k">Dispositivos vinculados</span>
                <strong>${esc(String(devices.length || (ST.link ? 1 : 0)))}</strong>
              </div>
              <div class="pl-stat">
                <span class="pl-stat-k">Modo Gatekeeper</span>
                <strong>${esc(String(ST.permissionMode || 'sandbox').toUpperCase())}</strong>
              </div>
              <div class="pl-stat">
                <span class="pl-stat-k">Alertas sin leer</span>
                <strong>${esc(String(unread))}</strong>
              </div>
            </div>
            ${devices.length ? `
              <div class="pl-dev-list">
                ${devices.map(d => `
                  <div class="pl-dev-chip">
                    <span class="pl-dot${isDeviceOnline(d) ? ' on' : ''}"></span>
                    <span>${esc(d.device_name || d.device_id)}</span>
                    <span class="pl-badge">${esc(d.transport || 'wifi')}</span>
                    ${typeof d.battery_pct === 'number' && d.battery_pct >= 0 ? `<span class="pl-badge">${esc(String(d.battery_pct))}%</span>` : ''}
                  </div>`).join('')}
              </div>` : ''}
          </div>
        </div>
      </section>

      <div class="pl-two-col">
        <section class="card">
          <div class="card-h">
            <h2>Emparejamiento Wi-Fi LAN (QR)</h2>
            ${qrData?.dataUrl ? `<span class="set-pill status-ok" id="set-qr-countdown">Expira en ${esc(String(qrData.expires_in_sec || 60))}s</span>` : '<span class="set-pill">Sin QR activo</span>'}
          </div>
          ${qrData?.dataUrl && qrData.dataUrl.startsWith('data:image/') ? `
            <div class="pl-qr-wrap">
              <div class="qr-box"><img src="${esc(qrData.dataUrl)}" alt="Miku-Link QR" width="164" height="164"></div>
              <div class="pl-qr-meta">
                <div class="item-t">Escanea desde la app Miku Android → Vincular</div>
                <div class="item-d">Gateway LAN: <code>${esc(lanBaseUrl)}</code></div>
                <div class="pl-uri-box"><code>${esc(pairUrl)}</code></div>
                <div class="pill-row">
                  <button type="button" class="set-btn" id="set-link-copy-uri">Copiar URI</button>
                  <button type="button" class="set-btn" id="set-link-copy-ip">Copiar IP LAN</button>
                </div>
              </div>
            </div>` : `
            <div class="item">
              <div>
                <div class="item-t">Genera un código QR de un solo uso</div>
                <div class="item-d">Endpoint LAN detectado: <code>${esc(lanBaseUrl)}</code>. Caduca en 60 segundos.</div>
              </div>
            </div>`}
        </section>

        <section class="card">
          <div class="card-h">
            <h2>Cable USB (ADB Reverse) &amp; Permisos</h2>
            <span class="set-pill">Zero-Secrets</span>
          </div>
          <div class="item">
            <div>
              <div class="item-t">Conexión directa por cable USB</div>
              <div class="item-d">En Android sin Wi-Fi compartida, redirige el puerto por ADB y usa <code>http://127.0.0.1:${esc(String(port))}</code> en la app.</div>
            </div>
            <button type="button" class="set-btn" id="set-link-copy-adb">Copiar comando</button>
          </div>
          <div class="pl-uri-box"><code>${esc(adbCmd)}</code></div>
          <div class="pl-boundary">
            <div class="pl-b-col ok">
              <div class="pl-b-t">Permitido desde el móvil</div>
              <ul>
                <li>Isla Dinámica, chat y respuestas de agentes</li>
                <li>Multimedia GSMTC y volumen Windows</li>
                <li>Modo Gatekeeper, Kill Switch y Biometría</li>
                <li>Notificaciones PC ↔ Teléfono</li>
              </ul>
            </div>
            <div class="pl-b-col lock">
              <div class="pl-b-t">Bloqueado en tokens LAN/móvil</div>
              <ul>
                <li><code>/api/secrets*</code> (API Keys DPAPI)</li>
                <li><code>/api/settings*</code> y <code>/api/harnesses*</code></li>
                <li><code>/api/files*</code>, <code>/api/mcp*</code> y UI local</li>
              </ul>
            </div>
          </div>
        </section>
      </div>

      <section class="card">
        <div class="card-h">
          <h2>Notificaciones Bidireccionales &amp; Control Rápido</h2>
          <div class="pill-row">
            <button type="button" class="set-btn" id="set-link-cmd-ping">Ping Isla</button>
            <button type="button" class="set-btn" id="set-link-cmd-play">Play / Pausa</button>
            <button type="button" class="set-btn" id="set-link-notif-ack">Marcar leídas</button>
          </div>
        </div>
        <div class="pl-notif-compose">
          <input type="text" class="set-input" id="set-link-notif-title" placeholder="Título (ej. Compilación lista)">
          <input type="text" class="set-input" id="set-link-notif-body" placeholder="Mensaje para tu teléfono Android...">
          <button type="button" class="set-btn prim" id="set-link-notif-send">Enviar al móvil</button>
        </div>
        <div class="pl-notif-list">
          ${notifs.length ? notifs.slice(0, 12).map(n => `
            <div class="pl-notif-item${n.read === false || n.unread ? ' unread' : ''}">
              <div class="pl-notif-head">
                <strong>${esc(n.title || 'Miku Desktop')}</strong>
                <span class="pl-badge">${esc(n.source || 'desktop')}</span>
                <span class="item-d">${esc(fmtRel(n.ts_ms || n.created_ms))}</span>
              </div>
              <div class="item-d">${esc(n.body || '')}</div>
            </div>`).join('') : '<div class="item-d">No hay notificaciones recientes. Envía una prueba arriba o chatea con un agente.</div>'}
        </div>
      </section>`;
  }

  function bindEvents(container, signal, context) {
    const status = UI.ST?.linkStatus || {};
    const port = Number(context.getQrData?.()?.port || status.port || BRIDGE?.GATEWAY_PORT || 47400);
    const lanIp = context.getQrData?.()?.lan_ip || status.lan_ip || '127.0.0.1';
    const lanBaseUrl = context.getQrData?.()?.lan_base_url || status.lan_base_url || `http://${lanIp}:${port}`;
    const adbCmd = context.getQrData?.()?.adb_reverse_cmd || status.adb_reverse_cmd || `adb reverse tcp:${port} tcp:${port}`;
    const pairUrl = context.getQrData?.()?.url || status.pair_url || '';

    if (context.getQrData?.()?.dataUrl) {
      startCountdown(context.getQrData()?.expires_in_sec || 60);
    } else {
      stopCountdown();
    }

    container.addEventListener('click', async e => {
      if (e.target.closest('#set-link-refresh')) {
        await Promise.all([
          BRIDGE?.fetchLinkStatus?.().catch(() => null),
          BRIDGE?.fetchLinkNotifications?.().catch(() => null)
        ]);
        setNotice('Estado de Miku-Link actualizado');
        context.renderPane?.();
        return;
      }
      if (e.target.closest('#set-link-copy-uri')) {
        await copyText(pairUrl, 'URI de emparejamiento');
        context.renderPane?.();
        return;
      }
      if (e.target.closest('#set-link-copy-ip')) {
        await copyText(lanBaseUrl, 'URL LAN');
        context.renderPane?.();
        return;
      }
      if (e.target.closest('#set-link-copy-adb')) {
        await copyText(adbCmd, 'Comando ADB reverse');
        context.renderPane?.();
        return;
      }
      if (e.target.closest('#set-link-cmd-ping')) {
        await BRIDGE?.sendLinkCommand?.({ action: 'island_ping' });
        setNotice('Ping enviado a la Isla Dinámica');
        context.renderPane?.();
        return;
      }
      if (e.target.closest('#set-link-cmd-play')) {
        await BRIDGE?.sendLinkCommand?.({ action: 'media_play_pause' });
        setNotice('Comando multimedia Play/Pausa ejecutado');
        context.renderPane?.();
        return;
      }
      if (e.target.closest('#set-link-notif-ack')) {
        await BRIDGE?.ackLinkNotifications?.('all');
        await BRIDGE?.fetchLinkNotifications?.().catch(() => null);
        setNotice('Notificaciones marcadas como leídas');
        context.renderPane?.();
        return;
      }
      if (e.target.closest('#set-link-notif-send')) {
        const title = document.getElementById('set-link-notif-title')?.value.trim() || 'Miku Desktop';
        const body = document.getElementById('set-link-notif-body')?.value.trim() || 'Notificación de prueba desde Settings PC.';
        await BRIDGE?.sendLinkNotification?.({ title, body, level: 'info' });
        await BRIDGE?.fetchLinkNotifications?.().catch(() => null);
        setNotice('Notificación enviada al flujo móvil');
        context.renderPane?.();
      }
    }, { signal });
  }

  window.MikuSettingsLink = {
    renderSection,
    bindEvents,
    setNotice,
    stopCountdown
  };
})();
