import http from 'node:http';

const ALLOWED_STATES = new Set([
  'needs_permission',
  'completed',
  'error',
  'paused',
  'ready',
]);

const FORBIDDEN_MARKERS = ['gsk_', 'sk-', 'bearer', 'c:', 'http', '.rs', '.ts', '\\', '/'];

/**
 * Validates strictly `{ agent_name, state, event_ts }` with zero PII, paths, prompts, or secrets.
 */
export function validateWakePayload(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) {
    throw new Error('Payload must be a JSON object');
  }
  const keys = Object.keys(raw);
  if (
    keys.length !== 3 ||
    !keys.includes('agent_name') ||
    !keys.includes('state') ||
    !keys.includes('event_ts')
  ) {
    throw new Error('Zero-PII policy requires strictly {agent_name, state, event_ts}');
  }

  const { agent_name, state, event_ts } = raw;
  if (typeof agent_name !== 'string' || agent_name.length === 0 || agent_name.length > 32) {
    throw new Error('agent_name must be 1..=32 ASCII chars');
  }
  if (!/^[A-Za-z0-9_-]+$/.test(agent_name)) {
    throw new Error('agent_name contains forbidden characters');
  }
  const lower = agent_name.toLowerCase();
  for (const m of FORBIDDEN_MARKERS) {
    if (lower.includes(m)) {
      throw new Error(`Zero-PII marker detected: ${m}`);
    }
  }
  if (!ALLOWED_STATES.has(state)) {
    throw new Error(`Unsupported wake state: ${String(state)}`);
  }
  if (!Number.isInteger(event_ts) || event_ts < 0) {
    throw new Error('event_ts must be non-negative integer');
  }

  return { agent_name, state, event_ts };
}

export function createStatelessWakeServer(onWake) {
  return http.createServer((req, res) => {
    if (req.method !== 'POST' || req.url !== '/wake') {
      res.writeHead(404).end();
      return;
    }
    let body = '';
    let bytes = 0;
    req.on('data', (chunk) => {
      bytes += chunk.length;
      if (bytes > 512) {
        res.writeHead(413).end();
        req.destroy();
        return;
      }
      body += chunk.toString('utf8');
    });
    req.on('end', () => {
      try {
        const sanitized = validateWakePayload(JSON.parse(body));
        if (typeof onWake === 'function') {
          onWake(sanitized);
        }
        res.writeHead(202, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ accepted: true, ...sanitized }));
      } catch (err) {
        res.writeHead(400, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ accepted: false, error: err.message }));
      }
    });
  });
}

if (import.meta.url === `file://${process.argv[1]?.replace(/\\/g, '/')}`) {
  const port = Number(process.env.MIKU_WAKE_PORT || 47411);
  const host = process.env.MIKU_WAKE_HOST || '127.0.0.1';
  const server = createStatelessWakeServer((evt) => {
    console.log(`[miku-push-wake] wake signal: ${evt.agent_name} -> ${evt.state}`);
  });
  server.listen(port, host, () => {
    console.log(`[miku-push-wake] listening statelessly on http://${host}:${port}/wake`);
  });
}
