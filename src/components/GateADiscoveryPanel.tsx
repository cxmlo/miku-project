import React, { useState } from 'react';
import {
  buildGateAReplySummary,
  type DiscoveryQuestion,
  type ExtraProposalItem,
} from '../utils/cockpitLogic';
import { Check, Copy, RotateCcw, FileText } from 'lucide-react';

interface GateADiscoveryPanelProps {
  questions: DiscoveryQuestion[];
  extraProposals: ExtraProposalItem[];
}

export const GateADiscoveryPanel: React.FC<GateADiscoveryPanelProps> = ({
  questions,
  extraProposals,
}) => {
  const [customAnswers, setCustomAnswers] = useState<Record<string, string>>({});
  const [selectedExtras, setSelectedExtras] = useState<Record<string, boolean>>(() => {
    const initial: Record<string, boolean> = {};
    for (const item of extraProposals) {
      initial[item.id] = item.defaultIncluded;
    }
    return initial;
  });
  const [copied, setCopied] = useState(false);

  const generatedReply = buildGateAReplySummary(questions, customAnswers, selectedExtras);

  const handleCopyReply = async () => {
    try {
      await navigator.clipboard.writeText(generatedReply);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  const handleResetDefaults = () => {
    setCustomAnswers({});
    const resetExtras: Record<string, boolean> = {};
    for (const item of extraProposals) {
      resetExtras[item.id] = item.defaultIncluded;
    }
    setSelectedExtras(resetExtras);
  };

  return (
    <section
      id="compuerta-a"
      aria-labelledby="heading-compuerta-a"
      className="border border-slate-800 bg-slate-900/60 rounded-xl p-6 space-y-6"
    >
      <div className="flex flex-col lg:flex-row lg:items-center lg:justify-between gap-4 pb-5 border-b border-slate-800">
        <div className="space-y-1">
          <div className="text-xs font-mono text-cyan-400">
            Compuerta A Activa · Bloqueo preventivo antes de Compuertas B y C
          </div>
          <h2
            id="heading-compuerta-a"
            className="text-xl font-semibold text-slate-100 tracking-tight"
          >
            01. Cuestionario de Descubrimiento y Preguntas Abiertas (Plan v1.2 §15)
          </h2>
          <p className="text-sm text-slate-400 max-w-3xl">
            Cada punto incluye una sugerencia por defecto entre paréntesis. Puedes responder
            simplemente <span className="font-mono text-slate-200">"ok"</span> en el chat para
            aprobar todos los valores por defecto, o personalizar aquí tus respuestas y copiar el
            bloque listo para pegar.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-3 shrink-0">
          <button
            type="button"
            onClick={handleResetDefaults}
            className="inline-flex items-center gap-2 px-3.5 py-2 text-xs font-medium text-slate-300 bg-slate-800/80 hover:bg-slate-800 border border-slate-700 rounded-lg transition-colors whitespace-nowrap shrink-0"
          >
            <RotateCcw className="w-3.5 h-3.5" />
            Restaurar valores por defecto
          </button>
          <button
            type="button"
            onClick={handleCopyReply}
            className="inline-flex items-center gap-2 px-4 py-2 text-xs font-semibold text-slate-950 bg-cyan-400 hover:bg-cyan-300 rounded-lg transition-colors whitespace-nowrap shrink-0"
          >
            {copied ? <Check className="w-3.5 h-3.5" /> : <Copy className="w-3.5 h-3.5" />}
            {copied ? 'Respuesta copiada' : 'Copiar respuesta Compuerta A'}
          </button>
        </div>
      </div>

      {/* Questions List */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        {questions.map((q) => {
          const currentVal = customAnswers[q.id] ?? '';
          return (
            <div
              key={q.id}
              className="border border-slate-800/90 bg-slate-950/60 rounded-lg p-4 flex flex-col justify-between gap-3"
            >
              <div className="space-y-1.5">
                <div className="flex items-center justify-between gap-2 text-xs text-slate-400 font-mono">
                  <span>
                    {q.id} · {q.group}
                  </span>
                  <span className="text-slate-500">{q.source}</span>
                </div>
                <label
                  htmlFor={`input-${q.id}`}
                  className="block text-sm font-medium text-slate-100 leading-snug"
                >
                  {q.question}
                </label>
                <p className="text-xs text-slate-400 leading-relaxed">
                  <span className="text-slate-300 font-medium">Por defecto (si dices "ok"):</span>{' '}
                  ({q.defaultSuggestion})
                </p>
              </div>

              <div className="pt-1">
                <input
                  id={`input-${q.id}`}
                  type="text"
                  value={currentVal}
                  onChange={(e) =>
                    setCustomAnswers((prev) => ({
                      ...prev,
                      [q.id]: e.target.value,
                    }))
                  }
                  placeholder='Dejar vacío para usar "ok" (valor por defecto)...'
                  className="w-full px-3 py-2 text-xs bg-slate-900 border border-slate-800 rounded-md text-slate-100 placeholder:text-slate-500 focus:outline-none focus:border-cyan-400"
                />
              </div>
            </div>
          );
        })}
      </div>

      {/* Extra Proposals */}
      <div className="pt-2 space-y-3">
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-semibold text-slate-200">
            Ideas extra propuestas por el Arquitecto (selecciona las que quieras incluir en el Plan Maestro)
          </h3>
          <span className="text-xs font-mono text-slate-400 tabular-nums">
            {Object.values(selectedExtras).filter(Boolean).length} de {extraProposals.length}{' '}
            activas
          </span>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
          {extraProposals.map((item) => {
            const isChecked = !!selectedExtras[item.id];
            return (
              <label
                key={item.id}
                htmlFor={`extra-${item.id}`}
                className={`flex items-start gap-3 p-3.5 rounded-lg border cursor-pointer transition-colors ${
                  isChecked
                    ? 'border-cyan-500/40 bg-cyan-950/15'
                    : 'border-slate-800 bg-slate-950/40 hover:border-slate-700'
                }`}
              >
                <input
                  id={`extra-${item.id}`}
                  type="checkbox"
                  checked={isChecked}
                  onChange={(e) =>
                    setSelectedExtras((prev) => ({
                      ...prev,
                      [item.id]: e.target.checked,
                    }))
                  }
                  className="mt-1 h-4 w-4 rounded border-slate-700 bg-slate-900 text-cyan-400 focus:ring-cyan-400"
                />
                <div className="space-y-1 text-xs">
                  <div className="font-medium text-slate-100">
                    <span className="font-mono text-cyan-400">{item.id}</span> · {item.title}
                  </div>
                  <p className="text-slate-400 leading-relaxed">{item.description}</p>
                  <div className="font-mono text-[11px] text-slate-500">
                    Ubicación propuesta: {item.targetStep}
                  </div>
                </div>
              </label>
            );
          })}
        </div>
      </div>

      {/* Generated Markdown Preview */}
      <div className="border-t border-slate-800 pt-4 space-y-2">
        <div className="flex items-center justify-between text-xs text-slate-400">
          <span className="inline-flex items-center gap-1.5 font-medium text-slate-300">
            <FileText className="w-3.5 h-3.5 text-cyan-400" />
            Vista previa de tu respuesta para cerrar la Compuerta A:
          </span>
          <span className="font-mono">Listo para copiar o responder "ok"</span>
        </div>
        <pre className="p-3.5 rounded-lg bg-slate-950 border border-slate-800/90 text-xs font-mono text-slate-300 overflow-x-auto leading-relaxed">
          {generatedReply}
        </pre>
      </div>
    </section>
  );
};
