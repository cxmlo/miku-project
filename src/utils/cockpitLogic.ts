export type VerificationStatus =
  | 'PENDIENTE'
  | 'ESCRITO'
  | 'PENDIENTE-LOCAL'
  | 'VERIFICADO'
  | 'FALLIDO';

export type AdrStatus =
  | 'Fijado'
  | 'Por validar'
  | 'Pendiente de spike'
  | 'Pendiente de confirmación';

export type GateStatus = 'EN_CURSO' | 'PENDIENTE' | 'APROBADA';

export interface MicroStepCard {
  id: string;
  phaseId: number;
  title: string;
  objective: string;
  deliverable: string;
  testsFirst: string[];
  acceptanceCriteria: string[];
  budgetAffected: string;
  risksAndMitigation: string;
  dependencies: string[];
  definitionOfDone: string;
  status: VerificationStatus;
  verifiedAt: string | null;
}

export interface PhaseItem {
  id: number;
  name: string;
  subtitle: string;
  status: 'Pendiente' | 'En curso' | 'Verificada';
  steps: MicroStepCard[];
}

export interface TestLedgerEntry {
  id: string;
  stepId: string;
  phaseId: number;
  name: string;
  kind: 'unitario' | 'integración' | 'contrato' | 'propiedad' | 'e2e' | 'benchmark';
  status: VerificationStatus;
  updatedAt: string;
  command: string;
  evidenceNote: string;
}

export interface AdrEntry {
  id: string;
  title: string;
  decision: string;
  status: AdrStatus;
  rationale: string;
  matrixRef: string;
}

export interface PerformanceBudget {
  id: string;
  metric: string;
  category: 'PC / Recursos' | 'Latencia / Arranque' | 'Binario' | 'Tokens / Coste';
  targetLabel: string;
  operator: '<=' | '==' | '<';
  thresholdValue: number;
  unit: string;
  measuredValue: number | null;
  measuredNote: string | null;
}

export interface CrateNode {
  name: string;
  layer: 'Protocolo (Sin I/O)' | 'Núcleo y Seguridad' | 'Servicios y Harness' | 'Plataforma y UI';
  responsibility: string;
  dependsOn: string[];
  status: 'Pendiente' | 'En diseño' | 'Verificado';
}

export interface ProtocolMessageContract {
  msgType: string;
  direction: 'Cliente → Daemon' | 'Daemon → Cliente' | 'Bidireccional';
  version: number;
  authRequired: boolean;
  maxFrameBytes: number;
  status: 'Propuesto (v1)' | 'Congelado' | 'Verificado';
  summary: string;
  exampleJson: string;
}

export interface RiskEntry {
  id: string;
  risk: string;
  severity: 'Alta' | 'Media' | 'Baja';
  impactArea: string;
  planB: string;
  validationStep: string;
}

export interface ChangelogEntry {
  version: string;
  date: string;
  gate: string;
  summary: string;
  highlights: string[];
}

export interface RepoFileNode {
  path: string;
  kind: 'file' | 'dir';
  description: string;
  deliveredInCurrentTurn: boolean;
  status: 'Entregado (Plan/Cockpit)' | 'Pendiente (Fase 1+)';
}

export interface DiscoveryQuestion {
  id: string;
  group: string;
  question: string;
  defaultSuggestion: string;
  source: 'Compuerta A' | 'Plan Maestro v1.2 (§15)';
}

export interface ExtraProposalItem {
  id: string;
  title: string;
  description: string;
  targetStep: string;
  defaultIncluded: boolean;
}

/**
 * Calculates the completion percentage of a phase strictly from VERIFICADO steps.
 * Rule: Prohibido mostrar progreso inventado; el porcentaje se calcula solo con pasos VERIFICADOS.
 */
export function calculatePhaseProgress(steps: Pick<MicroStepCard, 'status'>[]): number {
  if (!steps || steps.length === 0) {
    return 0;
  }
  const verifiedCount = steps.filter((step) => step.status === 'VERIFICADO').length;
  return Math.round((verifiedCount / steps.length) * 100);
}

/**
 * Evaluates a performance budget against an optional real user measurement.
 */
export function evaluateBudgetCompliance(
  budget: Pick<PerformanceBudget, 'operator' | 'thresholdValue' | 'measuredValue'>
): 'PENDIENTE' | 'CUMPLE' | 'NO_CUMPLE' {
  if (budget.measuredValue === null || Number.isNaN(budget.measuredValue)) {
    return 'PENDIENTE';
  }
  const val = budget.measuredValue;
  switch (budget.operator) {
    case '<=':
      return val <= budget.thresholdValue ? 'CUMPLE' : 'NO_CUMPLE';
    case '<':
      return val < budget.thresholdValue ? 'CUMPLE' : 'NO_CUMPLE';
    case '==':
      return val === budget.thresholdValue ? 'CUMPLE' : 'NO_CUMPLE';
    default:
      return 'PENDIENTE';
  }
}

/**
 * Filters Test Ledger entries by status and free-text query.
 */
export function filterTestLedger(
  entries: TestLedgerEntry[],
  statusFilter: VerificationStatus | 'TODOS',
  searchQuery: string
): TestLedgerEntry[] {
  const normalizedQuery = searchQuery.trim().toLowerCase();
  return entries.filter((entry) => {
    const matchesStatus = statusFilter === 'TODOS' || entry.status === statusFilter;
    if (!matchesStatus) return false;
    if (!normalizedQuery) return true;
    return (
      entry.stepId.toLowerCase().includes(normalizedQuery) ||
      entry.name.toLowerCase().includes(normalizedQuery) ||
      entry.kind.toLowerCase().includes(normalizedQuery) ||
      entry.command.toLowerCase().includes(normalizedQuery)
    );
  });
}

/**
 * Formats a Change Request (CR) string according to Section 11 governance rules.
 */
export function formatChangeRequest(params: {
  action: 'AÑADIR' | 'MODIFICAR' | 'QUITAR' | 'REPRIORIZAR' | 'CAMBIAR-DECISIÓN';
  what: string;
  why: string;
  cost: 'S' | 'M' | 'L';
  affectedSteps: string;
}): string {
  const cleanWhat = params.what.trim() || 'Por definir';
  const cleanWhy = params.why.trim() || 'Por definir';
  const cleanSteps = params.affectedSteps.trim() || 'Ninguno';
  return `CR: [${params.action}] ${cleanWhat} / ${cleanWhy} (Coste: ${params.cost} · Pasos afectados: ${cleanSteps})`;
}

/**
 * Formats the user's Compuerta A discovery answers into a clean reply block.
 */
export function buildGateAReplySummary(
  questions: DiscoveryQuestion[],
  customAnswers: Record<string, string>,
  selectedExtras: Record<string, boolean>
): string {
  const lines: string[] = [
    'APROBACIÓN COMPUERTA A (Descubrimiento) -> Avanza a Compuerta B:',
    '',
  ];

  for (const q of questions) {
    const custom = customAnswers[q.id]?.trim();
    const answerText = custom && custom.length > 0 ? custom : `ok (${q.defaultSuggestion})`;
    lines.push(`- [${q.id}] ${q.question}: ${answerText}`);
  }

  const chosenExtras = Object.entries(selectedExtras)
    .filter(([, enabled]) => enabled)
    .map(([id]) => id);

  lines.push('');
  lines.push(
    `Ideas extra aprobadas: ${chosenExtras.length > 0 ? chosenExtras.join(', ') : 'Ninguna'}`
  );
  return lines.join('\n');
}
