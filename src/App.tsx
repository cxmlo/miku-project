/**
 * @license
 * SPDX-License-Identifier: Apache-2.0
 */

import React, { useState } from 'react';
import { PROJECT_STATE } from './state/projectState';
import {
  calculatePhaseProgress,
  evaluateBudgetCompliance,
  filterTestLedger,
  formatChangeRequest,
  type VerificationStatus,
} from './utils/cockpitLogic';
import { GateADiscoveryPanel } from './components/GateADiscoveryPanel';
import { ArchitectureAndContractsPanel } from './components/ArchitectureAndContractsPanel';
import {
  Check,
  Download,
  FileCode,
  Folder,
  RotateCcw,
  Search,
} from 'lucide-react';

export default function App() {
  // Vista del árbol de fases: por defecto muestra el árbol inicial vacío por completar (Sección 8),
  // con opción de previsualizar las fichas propuestas en MIKU_PLAN.md v1.2 (todo en PENDIENTE, 0% verificado)
  const [phaseViewMode, setPhaseViewMode] = useState<'empty_initial' | 'draft_v1_2'>(
    'empty_initial'
  );
  const [selectedPhaseId, setSelectedPhaseId] = useState<number>(0);

  // Filtros del Test Ledger (Sección c)
  const [ledgerStatusFilter, setLedgerStatusFilter] = useState<
    VerificationStatus | 'TODOS'
  >('TODOS');
  const [ledgerSearch, setLedgerSearch] = useState<string>('');

  // Simulador / verificador local de mediciones de presupuesto (Sección e)
  // Por defecto vacío (null) hasta que el usuario pegue mediciones reales
  const [localMeasurements, setLocalMeasurements] = useState<
    Record<string, number | null>
  >({});

  // Filtro de Árbol de Archivos
  const [showOnlyCurrentTurnFiles, setShowOnlyCurrentTurnFiles] =
    useState<boolean>(false);

  // Generador de Solicitud de Cambio CR: (Sección 11)
  const [crAction, setCrAction] = useState<
    'AÑADIR' | 'MODIFICAR' | 'QUITAR' | 'REPRIORIZAR' | 'CAMBIAR-DECISIÓN'
  >('MODIFICAR');
  const [crWhat, setCrWhat] = useState<string>('');
  const [crWhy, setCrWhy] = useState<string>('');
  const [crCost, setCrCost] = useState<'S' | 'M' | 'L'>('S');
  const [crSteps, setCrSteps] = useState<string>('0.1');
  const [copiedCr, setCopiedCr] = useState<boolean>(false);
  const [exportedStateNotice, setExportedStateNotice] = useState<boolean>(false);

  const activePhases =
    phaseViewMode === 'empty_initial'
      ? PROJECT_STATE.phases
      : PROJECT_STATE.proposedPhasesDraft;

  const selectedPhase =
    activePhases.find((p) => p.id === selectedPhaseId) ?? activePhases[0];

  const filteredLedger = filterTestLedger(
    PROJECT_STATE.testLedger,
    ledgerStatusFilter,
    ledgerSearch
  );

  const visibleRepoFiles = showOnlyCurrentTurnFiles
    ? PROJECT_STATE.repoFileTree.filter((f) => f.deliveredInCurrentTurn)
    : PROJECT_STATE.repoFileTree;

  const formattedCrPreview = formatChangeRequest({
    action: crAction,
    what: crWhat,
    why: crWhy,
    cost: crCost,
    affectedSteps: crSteps,
  });

  const handleExportProjectState = async () => {
    try {
      await navigator.clipboard.writeText(JSON.stringify(PROJECT_STATE, null, 2));
      setExportedStateNotice(true);
      setTimeout(() => setExportedStateNotice(false), 2000);
    } catch {
      setExportedStateNotice(true);
      setTimeout(() => setExportedStateNotice(false), 2000);
    }
  };

  const handleCopyCr = async () => {
    try {
      await navigator.clipboard.writeText(formattedCrPreview);
      setCopiedCr(true);
      setTimeout(() => setCopiedCr(false), 2000);
    } catch {
      setCopiedCr(true);
      setTimeout(() => setCopiedCr(false), 2000);
    }
  };

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100 flex flex-col">
      {/* Top Bar Contract: 3 zones separated by gap-8 */}
      <header className="sticky top-0 z-30 flex items-center justify-between gap-8 px-6 py-3.5 bg-slate-950/95 backdrop-blur border-b border-slate-800">
        {/* Zone 1: Single text element wordmark */}
        <a
          href="#resumen"
          className="text-base font-bold tracking-tight text-slate-100 whitespace-nowrap shrink-0"
        >
          Miku · Plan Cockpit
        </a>

        {/* Zone 2: 5 concise single-line text navigation links */}
        <nav
          aria-label="Navegación principal del Cockpit"
          className="hidden md:flex items-center gap-6 text-xs font-medium text-slate-400"
        >
          <a
            href="#compuerta-a"
            className="hover:text-slate-100 hover:underline underline-offset-4 transition-colors whitespace-nowrap shrink-0"
          >
            Compuerta A
          </a>
          <a
            href="#fases-y-tests"
            className="hover:text-slate-100 hover:underline underline-offset-4 transition-colors whitespace-nowrap shrink-0"
          >
            Fases y Tests
          </a>
          <a
            href="#adrs-y-presupuestos"
            className="hover:text-slate-100 hover:underline underline-offset-4 transition-colors whitespace-nowrap shrink-0"
          >
            ADRs y Presupuestos
          </a>
          <a
            href="#arquitectura"
            className="hover:text-slate-100 hover:underline underline-offset-4 transition-colors whitespace-nowrap shrink-0"
          >
            Arquitectura y Contratos
          </a>
          <a
            href="#riesgos-y-archivos"
            className="hover:text-slate-100 hover:underline underline-offset-4 transition-colors whitespace-nowrap shrink-0"
          >
            Riesgos y Archivos
          </a>
        </nav>

        {/* Zone 3: 1 primary action */}
        <div className="flex items-center shrink-0">
          <button
            type="button"
            onClick={handleExportProjectState}
            className="inline-flex items-center gap-2 px-4 py-2 text-xs font-semibold text-slate-950 bg-cyan-400 hover:bg-cyan-300 rounded-lg transition-colors whitespace-nowrap shrink-0"
          >
            {exportedStateNotice ? (
              <Check className="w-3.5 h-3.5" />
            ) : (
              <Download className="w-3.5 h-3.5" />
            )}
            {exportedStateNotice ? 'Estado JSON copiado' : 'Exportar Estado JSON'}
          </button>
        </div>
      </header>

      <main className="flex-1 max-w-[1440px] w-full mx-auto px-6 py-6 space-y-8">
        {/* LIVE_PROJECT_STATE & 5-Line Product Confirmation */}
        <section
          id="resumen"
          aria-labelledby="heading-live-state"
          className="border border-slate-800 bg-slate-900/60 rounded-xl p-6 space-y-5"
        >
          <div className="space-y-4 border-b border-slate-800 pb-5">
            <div className="space-y-1.5">
              <div className="flex flex-wrap items-center gap-2 text-xs font-mono text-slate-400 tabular-nums">
                <span className="text-cyan-400 font-medium">
                  LIVE_PROJECT_STATE · {PROJECT_STATE.meta.planVersion}
                </span>
                <span aria-hidden="true">·</span>
                <span>SemVer: {PROJECT_STATE.meta.repoVersion}</span>
                <span aria-hidden="true">·</span>
                <span>Actualizado: {PROJECT_STATE.meta.lastUpdated}</span>
                <span aria-hidden="true">·</span>
                <span>Paso activo: {PROJECT_STATE.meta.currentStepId}</span>
              </div>
              <h1
                id="heading-live-state"
                className="text-2xl font-bold text-slate-100 tracking-tight"
              >
                {PROJECT_STATE.meta.productName} · Panel de Control Arquitectónico
              </h1>
              <p className="text-sm text-slate-400 max-w-4xl">
                Estado actual:{' '}
                <span className="text-slate-200 font-medium">
                  {PROJECT_STATE.meta.activeGateTitle}
                </span>
                . Ningún micro-paso de producto se codifica antes de superar las Compuertas A, B
                (Matriz de lenguaje/stack) y C (Plan Maestro con fichas completas).
              </p>
            </div>

            {/* Gate Status Strip (Full-Width 4-Column Row) */}
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3 w-full">
              {PROJECT_STATE.gates.map((gate) => {
                const isCurrent = gate.status === 'EN_CURSO';
                return (
                  <div
                    key={gate.id}
                    className={`p-3.5 rounded-lg border text-xs min-w-0 ${
                      isCurrent
                        ? 'border-cyan-400/60 bg-cyan-950/25 text-slate-100'
                        : 'border-slate-800 bg-slate-950/60 text-slate-400'
                    }`}
                  >
                    <div className="font-mono font-semibold flex items-center justify-between gap-2">
                      <span className="truncate">{gate.name}</span>
                      <span
                        className={`shrink-0 ${
                          isCurrent ? 'text-cyan-300' : 'text-slate-500'
                        }`}
                      >
                        {gate.status}
                      </span>
                    </div>
                    <div className="mt-1 text-[11px] text-slate-400 leading-relaxed">
                      {gate.subtitle}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>

          {/* 5-Line Confirmation of Product Understanding */}
          <div className="space-y-2">
            <h2 className="text-xs font-semibold text-slate-300">
              Confirmación de Entendimiento del Producto en 5 Líneas (Protocolo de Arranque §8)
            </h2>
            <div className="grid grid-cols-1 gap-1.5 bg-slate-950 border border-slate-800 rounded-lg p-4 text-xs text-slate-300 font-mono leading-relaxed">
              {PROJECT_STATE.meta.summaryFiveLines.map((line) => (
                <div key={line}>{line}</div>
              ))}
            </div>
          </div>
        </section>

        {/* Interactive Gate A Discovery Questionnaire */}
        <GateADiscoveryPanel
          questions={PROJECT_STATE.discoveryQuestions}
          extraProposals={PROJECT_STATE.extraProposals}
        />

        {/* Sections (a), (b), (c): Phase Timeline, Current Micro-Step Card, and Test Ledger */}
        <div id="fases-y-tests" className="space-y-6">
          {/* Section (a): Phase Timeline with Strict VERIFICADO Percentage */}
          <section
            aria-labelledby="heading-phases"
            className="border border-slate-800 bg-slate-900/60 rounded-xl p-6 space-y-5"
          >
            <div className="flex flex-col lg:flex-row lg:items-center lg:justify-between gap-4 border-b border-slate-800 pb-4">
              <div>
                <div className="text-xs font-mono text-slate-400">
                  Sección (a) · El porcentaje se calcula exclusivamente con pasos VERIFICADOS (prohibido progreso inventado)
                </div>
                <h2
                  id="heading-phases"
                  className="text-lg font-semibold text-slate-100 tracking-tight"
                >
                  02. Línea de Tiempo de Fases y Árbol de Micro-pasos
                </h2>
              </div>

              <div
                role="group"
                aria-label="Selector de vista de árbol de fases"
                className="flex items-center gap-1 p-1 bg-slate-950 border border-slate-800 rounded-lg"
              >
                <button
                  type="button"
                  onClick={() => setPhaseViewMode('empty_initial')}
                  className={`px-3 py-1.5 text-xs font-medium rounded-md transition-colors whitespace-nowrap shrink-0 ${
                    phaseViewMode === 'empty_initial'
                      ? 'bg-slate-800 text-cyan-300'
                      : 'text-slate-400 hover:text-slate-200'
                  }`}
                >
                  Árbol Oficial Inicial (Vacío hasta Compuerta C)
                </button>
                <button
                  type="button"
                  onClick={() => setPhaseViewMode('draft_v1_2')}
                  className={`px-3 py-1.5 text-xs font-medium rounded-md transition-colors whitespace-nowrap shrink-0 ${
                    phaseViewMode === 'draft_v1_2'
                      ? 'bg-slate-800 text-cyan-300'
                      : 'text-slate-400 hover:text-slate-200'
                  }`}
                >
                  Previsualizar Borrador Plan v1.2 (Pasos en Pendiente)
                </button>
              </div>
            </div>

            {/* Phase Cards */}
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-6 gap-3">
              {activePhases.map((phase) => {
                const progressPct = calculatePhaseProgress(phase.steps);
                const verifiedCount = phase.steps.filter(
                  (s) => s.status === 'VERIFICADO'
                ).length;
                const isSelected = phase.id === selectedPhase.id;

                return (
                  <button
                    key={phase.id}
                    type="button"
                    onClick={() => setSelectedPhaseId(phase.id)}
                    className={`text-left p-4 rounded-lg border transition-colors flex flex-col justify-between gap-3 ${
                      isSelected
                        ? 'border-cyan-400 bg-slate-900'
                        : 'border-slate-800 bg-slate-950/60 hover:border-slate-700'
                    }`}
                  >
                    <div className="space-y-1">
                      <div className="flex items-center justify-between text-xs font-mono">
                        <span className="text-cyan-400 font-semibold">
                          Fase {phase.id}
                        </span>
                        <span className="tabular-nums text-slate-300 font-semibold">
                          {progressPct}%
                        </span>
                      </div>
                      <div className="text-xs font-semibold text-slate-100 line-clamp-2">
                        {phase.name.replace(/^Fase \d+ · /, '')}
                      </div>
                    </div>

                    <div className="space-y-1.5 w-full">
                      <div className="w-full h-1.5 bg-slate-800 rounded-full overflow-hidden">
                        <div
                          className="h-full bg-cyan-400 transition-transform"
                          style={{ width: `${progressPct}%` }}
                        />
                      </div>
                      <div className="flex items-center justify-between text-[11px] font-mono text-slate-400 tabular-nums">
                        <span>{phase.status}</span>
                        <span>
                          {verifiedCount}/{phase.steps.length} verif.
                        </span>
                      </div>
                    </div>
                  </button>
                );
              })}
            </div>

            {/* Selected Phase Detail */}
            <div className="bg-slate-950 border border-slate-800 rounded-lg p-4 space-y-3">
              <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-2">
                <div>
                  <h3 className="text-sm font-semibold text-slate-100">
                    {selectedPhase.name}
                  </h3>
                  <p className="text-xs text-slate-400">{selectedPhase.subtitle}</p>
                </div>
                <span className="text-xs font-mono text-slate-400 tabular-nums">
                  Progreso verificado: {calculatePhaseProgress(selectedPhase.steps)}%
                </span>
              </div>

              {selectedPhase.steps.length === 0 ? (
                <div className="p-4 rounded border border-dashed border-slate-800 text-xs text-slate-400 font-mono">
                  [Árbol de micro-pasos vacío en estado Pendiente] — De acuerdo con el Protocolo
                  de Trabajo (§3 y §8), las fichas detalladas se congelan y desbloquean al
                  aprobar la Compuerta B (Lenguaje/Stack) y la Compuerta C (Plan Maestro). Puedes
                  pulsar &quot;Previsualizar Borrador Plan v1.2&quot; arriba para inspeccionar los
                  7 spikes de la Fase 0.
                </div>
              ) : (
                <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                  {selectedPhase.steps.map((step) => (
                    <div
                      key={step.id}
                      className="p-3.5 rounded-lg bg-slate-900/70 border border-slate-800 space-y-1.5 text-xs"
                    >
                      <div className="flex items-center justify-between font-mono">
                        <span className="text-cyan-400 font-semibold">
                          Paso {step.id} · {step.title}
                        </span>
                        <span className="text-amber-300">{step.status}</span>
                      </div>
                      <p className="text-slate-300">{step.objective}</p>
                      <div className="text-[11px] font-mono text-slate-400">
                        Presupuesto: {step.budgetAffected}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </section>

          {/* Section (b) & Section (c) side by side */}
          <div className="grid grid-cols-1 lg:grid-cols-12 gap-6 items-stretch">
            {/* Section (b): Current Micro-Step Card */}
            <section
              aria-labelledby="heading-current-step"
              className="lg:col-span-5 border border-slate-800 bg-slate-900/60 rounded-xl p-6 flex flex-col justify-between space-y-4"
            >
              <div className="space-y-4">
                <div className="border-b border-slate-800 pb-3">
                  <div className="text-xs font-mono text-slate-400">
                    Sección (b) · Ficha obligatoria del paso activo
                  </div>
                  <h2
                    id="heading-current-step"
                    className="text-lg font-semibold text-slate-100 tracking-tight"
                  >
                    03. Ficha del Paso Actual ({PROJECT_STATE.currentStepCard.id})
                  </h2>
                </div>

                <dl className="space-y-2.5 text-xs">
                  <div>
                    <dt className="font-mono text-slate-400">ID y Estado:</dt>
                    <dd className="font-mono text-cyan-300 font-semibold mt-0.5">
                      {PROJECT_STATE.currentStepCard.id} ·{' '}
                      {PROJECT_STATE.currentStepCard.status}
                    </dd>
                  </div>
                  <div>
                    <dt className="font-mono text-slate-400">Objetivo:</dt>
                    <dd className="text-slate-200 mt-0.5 leading-relaxed">
                      {PROJECT_STATE.currentStepCard.objective}
                    </dd>
                  </div>
                  <div>
                    <dt className="font-mono text-slate-400">Entregable usable:</dt>
                    <dd className="text-slate-300 mt-0.5 leading-relaxed">
                      {PROJECT_STATE.currentStepCard.deliverable}
                    </dd>
                  </div>
                  <div>
                    <dt className="font-mono text-slate-400">Tests a escribir primero:</dt>
                    <dd className="mt-1 space-y-1 text-slate-300 font-mono text-[11px]">
                      {PROJECT_STATE.currentStepCard.testsFirst.map((t) => (
                        <div key={t}>• {t}</div>
                      ))}
                    </dd>
                  </div>
                  <div>
                    <dt className="font-mono text-slate-400">Criterios de aceptación:</dt>
                    <dd className="mt-1 space-y-1 text-slate-300">
                      {PROJECT_STATE.currentStepCard.acceptanceCriteria.map((c) => (
                        <div key={c}>• {c}</div>
                      ))}
                    </dd>
                  </div>
                  <div>
                    <dt className="font-mono text-slate-400">
                      Presupuesto de rendimiento afectado:
                    </dt>
                    <dd className="font-mono text-slate-200 mt-0.5">
                      {PROJECT_STATE.currentStepCard.budgetAffected}
                    </dd>
                  </div>
                  <div>
                    <dt className="font-mono text-slate-400">Riesgos y mitigación:</dt>
                    <dd className="text-slate-300 mt-0.5">
                      {PROJECT_STATE.currentStepCard.risksAndMitigation}
                    </dd>
                  </div>
                  <div>
                    <dt className="font-mono text-slate-400">Definición de terminado:</dt>
                    <dd className="text-slate-200 font-medium mt-0.5">
                      {PROJECT_STATE.currentStepCard.definitionOfDone}
                    </dd>
                  </div>
                </dl>
              </div>
            </section>

            {/* Section (c): Test Ledger */}
            <section
              aria-labelledby="heading-test-ledger"
              className="lg:col-span-7 border border-slate-800 bg-slate-900/60 rounded-xl p-6 flex flex-col justify-between space-y-4"
            >
              <div className="space-y-4">
                <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 border-b border-slate-800 pb-3">
                  <div>
                    <div className="text-xs font-mono text-slate-400">
                      Sección (c) · Estados: ESCRITO / PENDIENTE-LOCAL / VERIFICADO / FALLIDO
                    </div>
                    <h2
                      id="heading-test-ledger"
                      className="text-lg font-semibold text-slate-100 tracking-tight"
                    >
                      04. Test Ledger (Registro Verificable de Pruebas)
                    </h2>
                  </div>

                  <div className="relative">
                    <Search className="w-3.5 h-3.5 text-slate-500 absolute left-3 top-1/2 -translate-y-1/2" />
                    <input
                      type="search"
                      aria-label="Buscar en Test Ledger"
                      value={ledgerSearch}
                      onChange={(e) => setLedgerSearch(e.target.value)}
                      placeholder="Buscar paso o comando..."
                      className="pl-8 pr-3 py-1.5 text-xs bg-slate-950 border border-slate-800 rounded-lg text-slate-100 placeholder:text-slate-500 focus:outline-none focus:border-cyan-400"
                    />
                  </div>
                </div>

                {/* Status Filter Buttons */}
                <div
                  role="group"
                  aria-label="Filtrar tests por estado"
                  className="flex flex-wrap items-center gap-1 p-1 bg-slate-950 border border-slate-800 rounded-lg"
                >
                  {(
                    [
                      'TODOS',
                      'PENDIENTE',
                      'ESCRITO',
                      'PENDIENTE-LOCAL',
                      'VERIFICADO',
                      'FALLIDO',
                    ] as const
                  ).map((st) => (
                    <button
                      key={st}
                      type="button"
                      onClick={() => setLedgerStatusFilter(st)}
                      className={`px-2.5 py-1 text-xs font-mono rounded transition-colors whitespace-nowrap shrink-0 ${
                        ledgerStatusFilter === st
                          ? 'bg-slate-800 text-cyan-300 font-semibold'
                          : 'text-slate-400 hover:text-slate-200'
                      }`}
                    >
                      {st}
                    </button>
                  ))}
                </div>

                <div className="overflow-x-auto">
                  <table className="w-full text-left border-collapse text-xs">
                    <thead>
                      <tr className="border-b border-slate-800 text-slate-400">
                        <th className="py-2 px-2.5 font-medium">Paso</th>
                        <th className="py-2 px-2.5 font-medium">Prueba</th>
                        <th className="py-2 px-2.5 font-medium">Tipo</th>
                        <th className="py-2 px-2.5 font-medium">Estado</th>
                        <th className="py-2 px-2.5 font-medium">Fecha</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-slate-800/60">
                      {filteredLedger.length === 0 ? (
                        <tr>
                          <td
                            colSpan={5}
                            className="py-6 text-center text-slate-400 font-mono"
                          >
                            No hay entradas con el filtro seleccionado ({ledgerStatusFilter}).
                          </td>
                        </tr>
                      ) : (
                        filteredLedger.map((item) => (
                          <tr key={item.id} className="hover:bg-slate-800/30">
                            <td className="py-2.5 px-2.5 font-mono text-cyan-300 whitespace-nowrap">
                              {item.stepId}
                            </td>
                            <td className="py-2.5 px-2.5 text-slate-200">
                              <div>{item.name}</div>
                              <div className="font-mono text-[11px] text-slate-400 mt-0.5">
                                {item.command}
                              </div>
                            </td>
                            <td className="py-2.5 px-2.5 font-mono text-slate-300 whitespace-nowrap">
                              {item.kind}
                            </td>
                            <td className="py-2.5 px-2.5 font-mono text-amber-300 whitespace-nowrap">
                              {item.status}
                            </td>
                            <td className="py-2.5 px-2.5 font-mono tabular-nums text-slate-400 whitespace-nowrap">
                              {item.updatedAt}
                            </td>
                          </tr>
                        ))
                      )}
                    </tbody>
                  </table>
                </div>
              </div>

              <div className="p-3 rounded-lg bg-slate-950 border border-slate-800 text-xs text-slate-400 font-mono">
                Regla TDD: ningún paso pasa a VERIFICADO sin que el usuario pegue en el chat la
                salida real de terminal de su máquina.
              </div>
            </section>
          </div>
        </div>

        {/* Sections (d) & (e): ADRs + Decision Matrix Link & Performance Budgets */}
        <div id="adrs-y-presupuestos" className="grid grid-cols-1 lg:grid-cols-12 gap-6">
          {/* Section (d): ADRs & Compuerta B Decision Matrix Preview */}
          <section
            aria-labelledby="heading-adrs"
            className="lg:col-span-7 border border-slate-800 bg-slate-900/60 rounded-xl p-6 space-y-5"
          >
            <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-2 border-b border-slate-800 pb-4">
              <div>
                <div className="text-xs font-mono text-slate-400">
                  Sección (d) · Decisiones Arquitectónicas (ADR-001 a ADR-010)
                </div>
                <h2
                  id="heading-adrs"
                  className="text-lg font-semibold text-slate-100 tracking-tight"
                >
                  05. Registro de ADRs y Matriz Ponderada (Compuerta B)
                </h2>
              </div>
              <a
                href="#matriz-compuerta-b"
                className="text-xs font-mono text-cyan-400 hover:underline whitespace-nowrap shrink-0"
              >
                Ir a Matriz Ponderada ↓
              </a>
            </div>

            <div className="overflow-x-auto">
              <table className="w-full text-left border-collapse text-xs">
                <thead>
                  <tr className="border-b border-slate-800 text-slate-400">
                    <th className="py-2 px-2.5 font-medium">ADR</th>
                    <th className="py-2 px-2.5 font-medium">Decisión y Justificación</th>
                    <th className="py-2 px-2.5 font-medium">Estado</th>
                    <th className="py-2 px-2.5 font-medium">Vínculo</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-slate-800/60">
                  {PROJECT_STATE.adrs.map((adr) => (
                    <tr key={adr.id} className="hover:bg-slate-800/30">
                      <td className="py-2.5 px-2.5 font-mono font-semibold text-cyan-300 whitespace-nowrap">
                        {adr.id}
                      </td>
                      <td className="py-2.5 px-2.5 text-slate-200">
                        <div className="font-medium">{adr.title}</div>
                        <div className="text-slate-400 text-[11px] mt-0.5">
                          {adr.rationale}
                        </div>
                      </td>
                      <td className="py-2.5 px-2.5 font-mono whitespace-nowrap">
                        <span
                          className={
                            adr.status === 'Fijado'
                              ? 'text-emerald-400'
                              : adr.status === 'Por validar'
                              ? 'text-cyan-300'
                              : 'text-amber-300'
                          }
                        >
                          {adr.status}
                        </span>
                      </td>
                      <td className="py-2.5 px-2.5 font-mono text-[11px] text-slate-400">
                        {adr.matrixRef}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {/* Matrix Preview for Gate B */}
            <div
              id="matriz-compuerta-b"
              className="pt-4 border-t border-slate-800 space-y-2.5"
            >
              <div className="flex items-center justify-between">
                <h3 className="text-xs font-semibold text-slate-200">
                  Pesos de la Matriz de Decisión (Compuerta B · Candidatos: Rust, C++, C# NativeAOT, Go, Zig)
                </h3>
                <span className="text-xs font-mono text-slate-400 tabular-nums">
                  Total: 100%
                </span>
              </div>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
                {PROJECT_STATE.decisionMatrixPreview.map((row) => (
                  <div
                    key={row.criterion}
                    className="p-2.5 rounded bg-slate-950 border border-slate-800 text-xs flex items-start justify-between gap-2"
                  >
                    <div>
                      <div className="font-medium text-slate-200">{row.criterion}</div>
                      <div className="text-[11px] text-slate-400 mt-0.5">{row.notes}</div>
                    </div>
                    <span className="font-mono font-semibold text-cyan-400 tabular-nums shrink-0">
                      {row.weightPct}%
                    </span>
                  </div>
                ))}
              </div>
            </div>
          </section>

          {/* Section (e): Performance Budgets */}
          <section
            aria-labelledby="heading-budgets"
            className="lg:col-span-5 border border-slate-800 bg-slate-900/60 rounded-xl p-6 flex flex-col justify-between space-y-5"
          >
            <div className="space-y-4">
              <div className="flex items-start justify-between gap-3 border-b border-slate-800 pb-4">
                <div>
                  <div className="text-xs font-mono text-slate-400">
                    Sección (e) · Vacío por defecto hasta pegar mediciones reales
                  </div>
                  <h2
                    id="heading-budgets"
                    className="text-lg font-semibold text-slate-100 tracking-tight"
                  >
                    Presupuestos de Rendimiento (Objetivo vs Medido)
                  </h2>
                </div>
                <button
                  type="button"
                  onClick={() => setLocalMeasurements({})}
                  className="inline-flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium text-slate-300 bg-slate-800 hover:bg-slate-700 rounded-lg transition-colors whitespace-nowrap shrink-0"
                >
                  <RotateCcw className="w-3 h-3" />
                  Vaciar mediciones
                </button>
              </div>

              <div className="space-y-3">
                {PROJECT_STATE.performanceBudgets.map((budget) => {
                  const simulatedOrRealVal =
                    localMeasurements[budget.id] !== undefined
                      ? localMeasurements[budget.id]
                      : budget.measuredValue;

                  const compliance = evaluateBudgetCompliance({
                    operator: budget.operator,
                    thresholdValue: budget.thresholdValue,
                    measuredValue: simulatedOrRealVal,
                  });

                  return (
                    <div
                      key={budget.id}
                      className="p-3.5 rounded-lg bg-slate-950 border border-slate-800 space-y-2 text-xs"
                    >
                      <div className="flex items-center justify-between gap-2">
                        <span className="font-medium text-slate-100">
                          {budget.metric}
                        </span>
                        <span
                          className={`font-mono font-semibold whitespace-nowrap ${
                            compliance === 'CUMPLE'
                              ? 'text-emerald-400'
                              : compliance === 'NO_CUMPLE'
                              ? 'text-rose-400'
                              : 'text-slate-400'
                          }`}
                        >
                          {compliance === 'CUMPLE'
                            ? 'CUMPLE'
                            : compliance === 'NO_CUMPLE'
                            ? 'NO CUMPLE'
                            : 'PENDIENTE (Vacío)'}
                        </span>
                      </div>

                      <div className="flex flex-wrap items-center justify-between gap-2 font-mono text-[11px] text-slate-400 tabular-nums">
                        <span>
                          Objetivo: <strong className="text-slate-200">{budget.targetLabel}</strong>
                        </span>
                        <span>
                          Medido:{' '}
                          <strong className="text-cyan-300">
                            {simulatedOrRealVal !== null && simulatedOrRealVal !== undefined
                              ? `${simulatedOrRealVal} ${budget.unit}`
                              : '— (sin evidencia aún)'}
                          </strong>
                        </span>
                      </div>

                      <div className="pt-1 flex items-center gap-2">
                        <label
                          htmlFor={`measure-${budget.id}`}
                          className="text-[11px] text-slate-400 whitespace-nowrap"
                        >
                          Probar medición ({budget.unit}):
                        </label>
                        <input
                          id={`measure-${budget.id}`}
                          type="number"
                          step="any"
                          value={
                            simulatedOrRealVal !== null && simulatedOrRealVal !== undefined
                              ? simulatedOrRealVal
                              : ''
                          }
                          onChange={(e) => {
                            const raw = e.target.value;
                            setLocalMeasurements((prev) => ({
                              ...prev,
                              [budget.id]: raw === '' ? null : Number(raw),
                            }));
                          }}
                          placeholder={`Ej. ${budget.thresholdValue}`}
                          className="w-28 px-2.5 py-1 text-xs font-mono tabular-nums bg-slate-900 border border-slate-800 rounded text-slate-100 focus:outline-none focus:border-cyan-400"
                        />
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          </section>
        </div>

        {/* Sections (f) & (g): Architecture Diagram, Crates & Protocol Contracts */}
        <ArchitectureAndContractsPanel
          crates={PROJECT_STATE.crates}
          featureCatalog={PROJECT_STATE.featureCatalogSummary}
          protocolContracts={PROJECT_STATE.protocolContracts}
          secondaryContracts={PROJECT_STATE.secondaryContracts}
        />

        {/* Sections (h), (i) & File Tree */}
        <div id="riesgos-y-archivos" className="grid grid-cols-1 lg:grid-cols-12 gap-6">
          {/* Section (i): Open Risks & Plan B + Section (h): Changelog & CR Builder */}
          <div className="lg:col-span-7 space-y-6">
            {/* Section (i): Open Risks */}
            <section
              aria-labelledby="heading-risks"
              className="border border-slate-800 bg-slate-900/60 rounded-xl p-6 space-y-4"
            >
              <div className="border-b border-slate-800 pb-3">
                <div className="text-xs font-mono text-slate-400">
                  Sección (i) · Riesgos técnicos identificados y Plan B con números
                </div>
                <h2
                  id="heading-risks"
                  className="text-lg font-semibold text-slate-100 tracking-tight"
                >
                  08. Riesgos Abiertos y Mitigación (Plan B)
                </h2>
              </div>

              <div className="overflow-x-auto">
                <table className="w-full text-left border-collapse text-xs">
                  <thead>
                    <tr className="border-b border-slate-800 text-slate-400">
                      <th className="py-2 px-2.5 font-medium">ID</th>
                      <th className="py-2 px-2.5 font-medium">Riesgo y Área</th>
                      <th className="py-2 px-2.5 font-medium">Severidad</th>
                      <th className="py-2 px-2.5 font-medium">Plan B</th>
                      <th className="py-2 px-2.5 font-medium">Validación</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-800/60">
                    {PROJECT_STATE.openRisks.map((r) => (
                      <tr key={r.id} className="hover:bg-slate-800/30">
                        <td className="py-2.5 px-2.5 font-mono text-cyan-300 whitespace-nowrap">
                          {r.id}
                        </td>
                        <td className="py-2.5 px-2.5 text-slate-200">
                          <div className="font-medium">{r.risk}</div>
                          <div className="text-[11px] text-slate-400">{r.impactArea}</div>
                        </td>
                        <td className="py-2.5 px-2.5 font-mono whitespace-nowrap">
                          <span
                            className={
                              r.severity === 'Alta'
                                ? 'text-rose-400'
                                : r.severity === 'Media'
                                ? 'text-amber-300'
                                : 'text-slate-300'
                            }
                          >
                            {r.severity}
                          </span>
                        </td>
                        <td className="py-2.5 px-2.5 text-slate-300">{r.planB}</td>
                        <td className="py-2.5 px-2.5 font-mono text-slate-400 whitespace-nowrap">
                          {r.validationStep}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </section>

            {/* Section (h): Chronological Changelog & CR Generator */}
            <section
              aria-labelledby="heading-changelog"
              className="border border-slate-800 bg-slate-900/60 rounded-xl p-6 space-y-5"
            >
              <div className="border-b border-slate-800 pb-3">
                <div className="text-xs font-mono text-slate-400">
                  Sección (h) · Historial del Plan Maestro y Control de Cambios (CR)
                </div>
                <h2
                  id="heading-changelog"
                  className="text-lg font-semibold text-slate-100 tracking-tight"
                >
                  09. Changelog Cronológico y Generador de CR (`CR:`)
                </h2>
              </div>

              <div className="space-y-3">
                {PROJECT_STATE.changelog.map((entry) => (
                  <div
                    key={entry.version}
                    className="p-3.5 rounded-lg bg-slate-950 border border-slate-800 space-y-1.5 text-xs"
                  >
                    <div className="flex flex-wrap items-center justify-between gap-2 font-mono">
                      <span className="text-cyan-300 font-semibold">{entry.version}</span>
                      <span className="text-slate-400 tabular-nums">
                        {entry.date} · {entry.gate}
                      </span>
                    </div>
                    <p className="text-slate-200">{entry.summary}</p>
                    <ul className="space-y-1 text-slate-400 pt-1">
                      {entry.highlights.map((h) => (
                        <li key={h}>• {h}</li>
                      ))}
                    </ul>
                  </div>
                ))}
              </div>

              {/* Change Request Form */}
              <div className="pt-3 border-t border-slate-800 space-y-3">
                <h3 className="text-xs font-semibold text-slate-200">
                  Generador de Solicitud de Cambio (Gobernanza §11)
                </h3>
                <div className="grid grid-cols-1 sm:grid-cols-3 gap-2.5 text-xs">
                  <div>
                    <label
                      htmlFor="cr-action"
                      className="block text-[11px] font-mono text-slate-400 mb-1"
                    >
                      Acción
                    </label>
                    <select
                      id="cr-action"
                      value={crAction}
                      onChange={(e) =>
                        setCrAction(
                          e.target.value as
                            | 'AÑADIR'
                            | 'MODIFICAR'
                            | 'QUITAR'
                            | 'REPRIORIZAR'
                            | 'CAMBIAR-DECISIÓN'
                        )
                      }
                      className="w-full px-2.5 py-1.5 bg-slate-950 border border-slate-800 rounded text-slate-100"
                    >
                      <option value="MODIFICAR">MODIFICAR</option>
                      <option value="AÑADIR">AÑADIR</option>
                      <option value="QUITAR">QUITAR</option>
                      <option value="REPRIORIZAR">REPRIORIZAR</option>
                      <option value="CAMBIAR-DECISIÓN">CAMBIAR-DECISIÓN</option>
                    </select>
                  </div>
                  <div>
                    <label
                      htmlFor="cr-cost"
                      className="block text-[11px] font-mono text-slate-400 mb-1"
                    >
                      Coste estimado
                    </label>
                    <select
                      id="cr-cost"
                      value={crCost}
                      onChange={(e) => setCrCost(e.target.value as 'S' | 'M' | 'L')}
                      className="w-full px-2.5 py-1.5 bg-slate-950 border border-slate-800 rounded text-slate-100"
                    >
                      <option value="S">S (Pequeño)</option>
                      <option value="M">M (Medio)</option>
                      <option value="L">L (Grande)</option>
                    </select>
                  </div>
                  <div>
                    <label
                      htmlFor="cr-steps"
                      className="block text-[11px] font-mono text-slate-400 mb-1"
                    >
                      Pasos afectados
                    </label>
                    <input
                      id="cr-steps"
                      type="text"
                      value={crSteps}
                      onChange={(e) => setCrSteps(e.target.value)}
                      className="w-full px-2.5 py-1.5 bg-slate-950 border border-slate-800 rounded text-slate-100 font-mono"
                    />
                  </div>
                </div>
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5 text-xs">
                  <div>
                    <label
                      htmlFor="cr-what"
                      className="block text-[11px] font-mono text-slate-400 mb-1"
                    >
                      Qué cambia
                    </label>
                    <input
                      id="cr-what"
                      type="text"
                      value={crWhat}
                      onChange={(e) => setCrWhat(e.target.value)}
                      placeholder="Ej. Añadir dominio docs.rs a allow por defecto"
                      className="w-full px-2.5 py-1.5 bg-slate-950 border border-slate-800 rounded text-slate-100"
                    />
                  </div>
                  <div>
                    <label
                      htmlFor="cr-why"
                      className="block text-[11px] font-mono text-slate-400 mb-1"
                    >
                      Por qué
                    </label>
                    <input
                      id="cr-why"
                      type="text"
                      value={crWhy}
                      onChange={(e) => setCrWhy(e.target.value)}
                      placeholder="Ej. Consulta frecuente de documentación Rust"
                      className="w-full px-2.5 py-1.5 bg-slate-950 border border-slate-800 rounded text-slate-100"
                    />
                  </div>
                </div>
                <div className="flex items-center justify-between gap-2 p-2.5 rounded bg-slate-950 border border-slate-800 text-xs font-mono text-slate-300">
                  <span className="truncate">{formattedCrPreview}</span>
                  <button
                    type="button"
                    onClick={handleCopyCr}
                    className="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 text-cyan-300 whitespace-nowrap shrink-0"
                  >
                    {copiedCr ? 'Copiado' : 'Copiar CR'}
                  </button>
                </div>
              </div>
            </section>
          </div>

          {/* Repo File Tree Panel */}
          <section
            aria-labelledby="heading-file-tree"
            className="lg:col-span-5 border border-slate-800 bg-slate-900/60 rounded-xl p-6 flex flex-col justify-between space-y-4"
          >
            <div className="space-y-4">
              <div className="flex items-center justify-between gap-2 border-b border-slate-800 pb-3">
                <div>
                  <div className="text-xs font-mono text-slate-400">
                    Estructura del repositorio y entregas del turno actual
                  </div>
                  <h2
                    id="heading-file-tree"
                    className="text-lg font-semibold text-slate-100 tracking-tight"
                  >
                    10. Árbol de Archivos del Proyecto
                  </h2>
                </div>

                <button
                  type="button"
                  onClick={() => setShowOnlyCurrentTurnFiles((v) => !v)}
                  className="px-3 py-1.5 text-xs font-medium rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-200 transition-colors whitespace-nowrap shrink-0"
                >
                  {showOnlyCurrentTurnFiles
                    ? 'Ver árbol completo'
                    : 'Solo entregados este turno'}
                </button>
              </div>

              <div className="space-y-2">
                {visibleRepoFiles.map((node) => (
                  <div
                    key={node.path}
                    className={`p-3 rounded-lg border text-xs flex items-start gap-3 ${
                      node.deliveredInCurrentTurn
                        ? 'border-cyan-500/40 bg-cyan-950/15'
                        : 'border-slate-800 bg-slate-950/50'
                    }`}
                  >
                    {node.kind === 'dir' ? (
                      <Folder className="w-4 h-4 text-slate-400 shrink-0 mt-0.5" />
                    ) : (
                      <FileCode className="w-4 h-4 text-cyan-400 shrink-0 mt-0.5" />
                    )}
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center justify-between gap-2 font-mono">
                        <span className="text-slate-100 font-medium truncate">
                          {node.path}
                        </span>
                        <span
                          className={`text-[11px] shrink-0 ${
                            node.deliveredInCurrentTurn
                              ? 'text-cyan-300 font-semibold'
                              : 'text-slate-500'
                          }`}
                        >
                          {node.deliveredInCurrentTurn
                            ? 'Entregado este turno'
                            : 'Pendiente'}
                        </span>
                      </div>
                      <p className="text-slate-400 mt-0.5 leading-relaxed">
                        {node.description}
                      </p>
                    </div>
                  </div>
                ))}
              </div>
            </div>

            <div className="p-3.5 rounded-lg bg-slate-950 border border-slate-800 text-xs text-slate-400 font-mono">
              Nota: Los crates de <span className="text-slate-200">miku/</span> están en estado
              Pendiente. El primer archivo Rust se generará en el Paso 1.1 (o Spike 0.1) tras tu
              aprobación de las Compuertas A, B y C.
            </div>
          </section>
        </div>
      </main>

      <footer className="border-t border-slate-900 py-5 px-6 text-xs text-slate-500 max-w-[1440px] w-full mx-auto flex flex-col sm:flex-row items-center justify-between gap-2">
        <div>
          Miku Desktop Orchestrator · Plan Cockpit (Fuente de verdad: MIKU_PLAN v1.2)
        </div>
        <div className="font-mono">
          Compuerta activa: A (Descubrimiento) · Progreso de fases verificado: 0%
        </div>
      </footer>
    </div>
  );
}
