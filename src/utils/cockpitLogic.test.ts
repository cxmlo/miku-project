import test from 'node:test';
import assert from 'node:assert/strict';
import {
  calculatePhaseProgress,
  evaluateBudgetCompliance,
  filterTestLedger,
  formatChangeRequest,
  buildGateAReplySummary,
  type TestLedgerEntry,
  type DiscoveryQuestion,
} from './cockpitLogic';

test('calculatePhaseProgress strictly counts only VERIFICADO steps and never invents progress', () => {
  assert.equal(calculatePhaseProgress([]), 0);
  assert.equal(
    calculatePhaseProgress([
      { status: 'PENDIENTE' },
      { status: 'ESCRITO' },
      { status: 'PENDIENTE-LOCAL' },
      { status: 'FALLIDO' },
    ]),
    0
  );
  assert.equal(
    calculatePhaseProgress([
      { status: 'VERIFICADO' },
      { status: 'PENDIENTE-LOCAL' },
      { status: 'ESCRITO' },
      { status: 'VERIFICADO' },
    ]),
    50
  );
  assert.equal(
    calculatePhaseProgress([{ status: 'VERIFICADO' }, { status: 'VERIFICADO' }]),
    100
  );
});

test('evaluateBudgetCompliance returns PENDIENTE when measuredValue is null and validates thresholds accurately', () => {
  assert.equal(
    evaluateBudgetCompliance({ operator: '<=', thresholdValue: 30, measuredValue: null }),
    'PENDIENTE'
  );
  assert.equal(
    evaluateBudgetCompliance({ operator: '<=', thresholdValue: 30, measuredValue: 26.4 }),
    'CUMPLE'
  );
  assert.equal(
    evaluateBudgetCompliance({ operator: '<=', thresholdValue: 30, measuredValue: 30 }),
    'CUMPLE'
  );
  assert.equal(
    evaluateBudgetCompliance({ operator: '<=', thresholdValue: 30, measuredValue: 31.2 }),
    'NO_CUMPLE'
  );
  assert.equal(
    evaluateBudgetCompliance({ operator: '<', thresholdValue: 100, measuredValue: 85 }),
    'CUMPLE'
  );
  assert.equal(
    evaluateBudgetCompliance({ operator: '<', thresholdValue: 100, measuredValue: 100 }),
    'NO_CUMPLE'
  );
  assert.equal(
    evaluateBudgetCompliance({ operator: '==', thresholdValue: 0, measuredValue: 0 }),
    'CUMPLE'
  );
  assert.equal(
    evaluateBudgetCompliance({ operator: '==', thresholdValue: 0, measuredValue: 12 }),
    'NO_CUMPLE'
  );
});

test('filterTestLedger filters by status and search term across stepId, name, kind, and command', () => {
  const sampleLedger: TestLedgerEntry[] = [
    {
      id: 'T-0.1',
      stepId: '0.1',
      phaseId: 0,
      name: 'Medición de árbol de procesos pastilla nativa vs WebView2',
      kind: 'benchmark',
      status: 'PENDIENTE',
      updatedAt: '2026-11-04',
      command: 'cargo test -p miku-bench --test idle_ram',
      evidenceNote: 'Pendiente de pasar Compuertas A, B y C',
    },
    {
      id: 'T-1.1',
      stepId: '1.1',
      phaseId: 1,
      name: 'Handshake miku-link v1 y validación de token en tiempo constante',
      kind: 'contrato',
      status: 'ESCRITO',
      updatedAt: '2026-11-04',
      command: 'cargo test -p miku-protocol',
      evidenceNote: 'En espera de ejecución local',
    },
  ];

  assert.equal(filterTestLedger(sampleLedger, 'TODOS', '').length, 2);
  assert.equal(filterTestLedger(sampleLedger, 'PENDIENTE', '').length, 1);
  assert.equal(filterTestLedger(sampleLedger, 'VERIFICADO', '').length, 0);
  assert.equal(filterTestLedger(sampleLedger, 'TODOS', 'miku-protocol').length, 1);
  assert.equal(filterTestLedger(sampleLedger, 'TODOS', '1.1').length, 1);
});

test('formatChangeRequest produces governance-compliant CR string', () => {
  const cr = formatChangeRequest({
    action: 'MODIFICAR',
    what: 'WebView2 bajo demanda al expandir',
    why: 'Mantener RAM idle <= 30 MB en árbol de procesos',
    cost: 'M',
    affectedSteps: '0.1, 2.1, 2.2',
  });
  assert.equal(
    cr,
    'CR: [MODIFICAR] WebView2 bajo demanda al expandir / Mantener RAM idle <= 30 MB en árbol de procesos (Coste: M · Pasos afectados: 0.1, 2.1, 2.2)'
  );
});

test('buildGateAReplySummary uses default suggestions when custom answers are empty', () => {
  const questions: DiscoveryQuestion[] = [
    {
      id: 'Q-01',
      group: 'Plataforma',
      question: 'Plataformas objetivo v1.0',
      defaultSuggestion: 'Windows 10/11 64-bit + Android 10+',
      source: 'Compuerta A',
    },
  ];
  const output = buildGateAReplySummary(questions, {}, { 'EX-01': true, 'EX-02': false });
  assert.match(output, /ok \(Windows 10\/11 64-bit \+ Android 10\+\)/);
  assert.match(output, /Ideas extra aprobadas: EX-01/);
});
