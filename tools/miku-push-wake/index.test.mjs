import test from 'node:test';
import assert from 'node:assert/strict';
import { validateWakePayload } from './index.mjs';

test('validateWakePayload accepts strict {agent_name, state, event_ts}', () => {
  const res = validateWakePayload({
    agent_name: 'claude-code',
    state: 'needs_permission',
    event_ts: 1763000100,
  });
  assert.deepEqual(res, {
    agent_name: 'claude-code',
    state: 'needs_permission',
    event_ts: 1763000100,
  });
});

test('validateWakePayload rejects extra PII fields, file paths, or secrets', () => {
  assert.throws(() =>
    validateWakePayload({
      agent_name: 'claude-code',
      state: 'completed',
      event_ts: 1,
      prompt: 'leak private prompt',
    })
  );
  assert.throws(() =>
    validateWakePayload({
      agent_name: 'C:/secret/file.rs',
      state: 'completed',
      event_ts: 1,
    })
  );
  assert.throws(() =>
    validateWakePayload({
      agent_name: 'gsk_secret_leak',
      state: 'error',
      event_ts: 1,
    })
  );
});
