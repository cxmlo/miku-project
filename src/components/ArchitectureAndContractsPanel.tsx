import React, { useState } from 'react';
import type {
  CrateNode,
  ProtocolMessageContract,
} from '../utils/cockpitLogic';

interface ArchitectureAndContractsPanelProps {
  crates: CrateNode[];
  featureCatalog: {
    id: string;
    name: string;
    summary: string;
    status: 'Pendiente';
  }[];
  protocolContracts: ProtocolMessageContract[];
  secondaryContracts: {
    name: string;
    file: string;
    status: 'Propuesto (Pendiente)';
    snippet: string;
  }[];
}

export const ArchitectureAndContractsPanel: React.FC<ArchitectureAndContractsPanelProps> = ({
  crates,
  featureCatalog,
  protocolContracts,
  secondaryContracts,
}) => {
  const [selectedLayer, setSelectedLayer] = useState<string>('TODOS');
  const [selectedMessage, setSelectedMessage] = useState<ProtocolMessageContract>(
    protocolContracts[0]
  );
  const [featureSearch, setFeatureSearch] = useState<string>('');
  const [showAllFeatures, setShowAllFeatures] = useState<boolean>(false);

  const layers = [
    'TODOS',
    'Protocolo (Sin I/O)',
    'Núcleo y Seguridad',
    'Servicios y Harness',
    'Plataforma y UI',
  ];

  const filteredCrates =
    selectedLayer === 'TODOS'
      ? crates
      : crates.filter((c) => c.layer === selectedLayer);

  const filteredFeatures = featureCatalog.filter((f) => {
    const q = featureSearch.trim().toLowerCase();
    if (!q) return true;
    return (
      f.id.toLowerCase().includes(q) ||
      f.name.toLowerCase().includes(q) ||
      f.summary.toLowerCase().includes(q)
    );
  });

  const visibleFeatures = showAllFeatures
    ? filteredFeatures
    : filteredFeatures.slice(0, 12);

  return (
    <div id="arquitectura" className="space-y-6">
      {/* Section (f): Architecture Diagram & Crates */}
      <section
        aria-labelledby="heading-architecture"
        className="border border-slate-800 bg-slate-900/60 rounded-xl p-6 space-y-6"
      >
        <div className="flex flex-col lg:flex-row lg:items-center lg:justify-between gap-4 border-b border-slate-800 pb-4">
          <div>
            <div className="text-xs font-mono text-slate-400">
              Sección (f) · Fronteras de módulos y dependencias unidireccionales
            </div>
            <h2
              id="heading-architecture"
              className="text-lg font-semibold text-slate-100 tracking-tight"
            >
              06. Diagrama de Arquitectura y Mapa de Crates (15 Crates en Monorepo)
            </h2>
          </div>

          <div
            role="group"
            aria-label="Filtrar crates por capa"
            className="flex flex-wrap items-center gap-1 p-1 bg-slate-950 border border-slate-800 rounded-lg"
          >
            {layers.map((layer) => (
              <button
                key={layer}
                type="button"
                onClick={() => setSelectedLayer(layer)}
                className={`px-2.5 py-1.5 text-xs font-medium rounded-md transition-colors whitespace-nowrap shrink-0 ${
                  selectedLayer === layer
                    ? 'bg-slate-800 text-cyan-300'
                    : 'text-slate-400 hover:text-slate-200'
                }`}
              >
                {layer}
              </button>
            ))}
          </div>
        </div>

        {/* Structural Diagram */}
        <div className="grid grid-cols-1 lg:grid-cols-12 gap-6 items-stretch">
          <div className="lg:col-span-7 bg-slate-950 border border-slate-800 rounded-lg p-4 flex flex-col justify-between">
            <div className="text-xs font-medium text-slate-300 mb-2">
              Topología de Proceso Único (`miku.exe`) y UI Híbrida (ADR-001)
            </div>
            <pre className="text-[11px] leading-relaxed font-mono text-slate-300 overflow-x-auto">
{`Claude Code ─┐         ┌────────────────────── miku.exe (único) ─────────────────────┐
Cursor ──────┤ hooks / │ miku-gateway (axum WS) ──► EventBus ──► miku-core (FSM,cola)│
Antigravity ─┤ MCP     │         ▲                       │              │            │
GitHub ──────┘◄───────►│ miku-agents (harness)           ▼              ▼            │
Móvil ─(LAN/túnel)────►│ miku-mcp (host+cliente)    miku-gatekeeper ◄── perfiles     │
                       │ miku-memory · miku-skills       │   (deny-by-default)       │
                       │ miku-browser (CDP) · miku-platform (Win32, DPAPI, Fullscreen)│
                       │ Pastilla nativa (siempre, <=30 MB) ◄── estado               │
                       │ WebView2 (solo al expandir, destruido tras colapsar)        │
                       └─────────────────────────────────────────────────────────────┘`}
            </pre>
            <div className="mt-3 pt-3 border-t border-slate-800/80 text-xs text-slate-400 flex flex-wrap items-center gap-x-3 gap-y-1">
              <span>Regla dura 1: miku-protocol no tiene I/O</span>
              <span aria-hidden="true">·</span>
              <span>Regla dura 2: Ninguna capacidad elude miku-gatekeeper</span>
              <span aria-hidden="true">·</span>
              <span>Regla dura 3: La UI solo recibe estado tipado</span>
            </div>
          </div>

          <div className="lg:col-span-5 bg-slate-950 border border-slate-800 rounded-lg p-4 space-y-3 flex flex-col justify-between">
            <div className="space-y-2">
              <h3 className="text-xs font-semibold text-slate-200">
                Jerarquía de Control de PC (Coste Creciente)
              </h3>
              <ol className="space-y-2 text-xs text-slate-300">
                <li className="p-2 rounded bg-slate-900/90 border border-slate-800/80">
                  <span className="font-mono text-cyan-400">Nivel 1 (Mínimo coste):</span>{' '}
                  Herramienta directa o servidor MCP tipado.
                </li>
                <li className="p-2 rounded bg-slate-900/90 border border-slate-800/80">
                  <span className="font-mono text-cyan-400">Nivel 2 (Web):</span> Navegador por CDP
                  con perfil propio leyendo texto del DOM y descargas enjauladas.
                </li>
                <li className="p-2 rounded bg-slate-900/90 border border-slate-800/80">
                  <span className="font-mono text-cyan-400">Nivel 3 (Nativo Win32):</span> Windows
                  UI Automation (árbol de accesibilidad sin capturas).
                </li>
                <li className="p-2 rounded bg-slate-900/90 border border-slate-800/80">
                  <span className="font-mono text-amber-400">Nivel 4 (Último recurso):</span>{' '}
                  Visión por píxeles bajo demanda y con permiso explícito.
                </li>
              </ol>
            </div>
            <div className="text-xs text-slate-400 pt-2 border-t border-slate-800">
              Hilos activos en idle: daemon en <span className="font-mono text-slate-200">tokio current_thread</span> + hilo Win32 de pastilla nativa (0 timers de polling).
            </div>
          </div>
        </div>

        {/* Crates Table */}
        <div className="overflow-x-auto">
          <table className="w-full text-left border-collapse text-xs">
            <thead>
              <tr className="border-b border-slate-800 text-slate-400">
                <th className="py-2.5 px-3 font-medium">Crate</th>
                <th className="py-2.5 px-3 font-medium">Capa arquitectónica</th>
                <th className="py-2.5 px-3 font-medium">Responsabilidad única</th>
                <th className="py-2.5 px-3 font-medium">Depende de</th>
                <th className="py-2.5 px-3 font-medium text-right">Estado</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-800/60">
              {filteredCrates.map((crate) => (
                <tr key={crate.name} className="hover:bg-slate-800/30">
                  <td className="py-2.5 px-3 font-mono font-medium text-cyan-300 whitespace-nowrap">
                    {crate.name}
                  </td>
                  <td className="py-2.5 px-3 text-slate-300 whitespace-nowrap">{crate.layer}</td>
                  <td className="py-2.5 px-3 text-slate-300">{crate.responsibility}</td>
                  <td className="py-2.5 px-3 font-mono text-slate-400">
                    {crate.dependsOn.length > 0 ? crate.dependsOn.join(', ') : 'Ninguna (raíz)'}
                  </td>
                  <td className="py-2.5 px-3 text-right font-mono text-slate-400 whitespace-nowrap">
                    {crate.status}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        {/* Feature Catalog F-01..F-30 */}
        <div className="pt-4 border-t border-slate-800 space-y-3">
          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3">
            <div>
              <h3 className="text-sm font-semibold text-slate-200">
                Catálogo Declarativo de Funciones (F-01 a F-30 · docs/design/F-XX.md)
              </h3>
              <p className="text-xs text-slate-400">
                Cada función exige ficha de diseño antes de escribir código. Estado actual: 30 en Pendiente.
              </p>
            </div>
            <div className="flex items-center gap-2">
              <input
                type="search"
                aria-label="Buscar función F-01 a F-30"
                value={featureSearch}
                onChange={(e) => setFeatureSearch(e.target.value)}
                placeholder="Filtrar F-01..F-30 (ej. CDP, Gatekeeper)..."
                className="px-3 py-1.5 text-xs bg-slate-950 border border-slate-800 rounded-lg text-slate-100 placeholder:text-slate-500 focus:outline-none focus:border-cyan-400"
              />
              <button
                type="button"
                onClick={() => setShowAllFeatures((v) => !v)}
                className="px-3 py-1.5 text-xs font-medium text-slate-300 bg-slate-800 hover:bg-slate-700 rounded-lg transition-colors whitespace-nowrap shrink-0"
              >
                {showAllFeatures ? 'Mostrar resumen (12)' : `Ver las ${filteredFeatures.length} funciones`}
              </button>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-2.5">
            {visibleFeatures.map((feat) => (
              <div
                key={feat.id}
                className="p-3 rounded-lg bg-slate-950/70 border border-slate-800/80 flex flex-col justify-between gap-1"
              >
                <div className="flex items-center justify-between text-xs">
                  <span className="font-mono font-semibold text-cyan-400">
                    {feat.id} · {feat.name}
                  </span>
                  <span className="font-mono text-[11px] text-slate-500">{feat.status}</span>
                </div>
                <p className="text-xs text-slate-400 leading-relaxed">{feat.summary}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Section (g): Protocol Contracts */}
      <section
        aria-labelledby="heading-contracts"
        className="border border-slate-800 bg-slate-900/60 rounded-xl p-6 space-y-6"
      >
        <div className="border-b border-slate-800 pb-4">
          <div className="text-xs font-mono text-slate-400">
            Sección (g) · Principio 2: Contrato primero (sin cambios incompatibles sin versión nueva y ADR)
          </div>
          <h2
            id="heading-contracts"
            className="text-lg font-semibold text-slate-100 tracking-tight"
          >
            07. Contratos del Protocolo miku-link v1 (`ws://127.0.0.1:47400/link`) y Manifiestos
          </h2>
        </div>

        <div className="grid grid-cols-1 lg:grid-cols-12 gap-6">
          <div className="lg:col-span-7 overflow-x-auto">
            <table className="w-full text-left border-collapse text-xs">
              <thead>
                <tr className="border-b border-slate-800 text-slate-400">
                  <th className="py-2 px-3 font-medium">Mensaje (`msg.type`)</th>
                  <th className="py-2 px-3 font-medium">Dirección</th>
                  <th className="py-2 px-3 font-medium text-right">Límite Trama</th>
                  <th className="py-2 px-3 font-medium">Estado</th>
                  <th className="py-2 px-3 font-medium text-right">Inspeccionar</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-800/60">
                {protocolContracts.map((contract) => {
                  const isSelected = selectedMessage.msgType === contract.msgType;
                  return (
                    <tr
                      key={contract.msgType}
                      className={isSelected ? 'bg-cyan-950/20' : 'hover:bg-slate-800/30'}
                    >
                      <td className="py-2.5 px-3 font-mono font-medium text-slate-100">
                        {contract.msgType}
                      </td>
                      <td className="py-2.5 px-3 text-slate-300 whitespace-nowrap">
                        {contract.direction}
                      </td>
                      <td className="py-2.5 px-3 text-right font-mono tabular-nums text-slate-300">
                        {(contract.maxFrameBytes / 1024).toFixed(0)} KB
                      </td>
                      <td className="py-2.5 px-3 font-mono text-amber-300 whitespace-nowrap">
                        {contract.status}
                      </td>
                      <td className="py-2.5 px-3 text-right">
                        <button
                          type="button"
                          onClick={() => setSelectedMessage(contract)}
                          className="px-2.5 py-1 text-xs font-medium rounded bg-slate-800 hover:bg-slate-700 text-cyan-300 transition-colors whitespace-nowrap shrink-0"
                        >
                          Ver JSON
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>

          <div className="lg:col-span-5 bg-slate-950 border border-slate-800 rounded-lg p-4 flex flex-col justify-between space-y-3">
            <div className="space-y-1.5">
              <div className="flex items-center justify-between text-xs font-mono text-slate-400">
                <span>Trama seleccionada: {selectedMessage.msgType}</span>
                <span>v = {selectedMessage.version}</span>
              </div>
              <p className="text-xs text-slate-300 leading-relaxed">
                {selectedMessage.summary}
              </p>
            </div>
            <pre className="p-3 rounded bg-slate-900 border border-slate-800 text-[11px] font-mono text-cyan-200 overflow-x-auto whitespace-pre-wrap break-all">
              {selectedMessage.exampleJson}
            </pre>
            <div className="text-[11px] text-slate-400 font-mono">
              Autenticación: {selectedMessage.authRequired ? 'Token obligatorio (comparación en tiempo constante)' : 'Público / Pre-handshake'}
            </div>
          </div>
        </div>

        {/* Secondary TOML Contracts */}
        <div className="grid grid-cols-1 md:grid-cols-3 gap-4 pt-3 border-t border-slate-800">
          {secondaryContracts.map((sec) => (
            <div
              key={sec.name}
              className="bg-slate-950 border border-slate-800 rounded-lg p-4 space-y-2"
            >
              <div className="flex items-center justify-between text-xs">
                <span className="font-semibold text-slate-200">{sec.name}</span>
                <span className="font-mono text-slate-400">{sec.file}</span>
              </div>
              <pre className="text-[11px] font-mono text-slate-300 overflow-x-auto leading-relaxed">
                {sec.snippet}
              </pre>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
};
