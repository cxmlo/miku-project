import React, { useState } from 'react';
import type { VisualCaptureEntry, WeekOneDayPlan } from '../utils/cockpitLogic';
import { Calendar, Camera, CheckCircle2, Clock, Maximize2, X } from 'lucide-react';

interface WeekOnePlanPanelProps {
  days: WeekOneDayPlan[];
  captures: VisualCaptureEntry[];
}

export const WeekOnePlanPanel: React.FC<WeekOnePlanPanelProps> = ({ days, captures }) => {
  const [selectedDay, setSelectedDay] = useState<number>(days[0]?.day ?? 1);
  const [activePhaseGroup, setActivePhaseGroup] = useState<string>('TODAS');
  const [activeCategory, setActiveCategory] = useState<string>('TODAS');
  const [zoomCapture, setZoomCapture] = useState<VisualCaptureEntry | null>(null);

  const currentDay = days.find((d) => d.day === selectedDay) ?? days[0];
  const phaseGroups = ['TODAS', ...Array.from(new Set(captures.map((c) => c.phaseGroup || 'General')))];
  const categories = ['TODAS', ...Array.from(new Set(captures.map((c) => c.category)))];
  const visibleCaptures = captures.filter((c) => {
    const groupOk = activePhaseGroup === 'TODAS' || (c.phaseGroup || 'General') === activePhaseGroup;
    const catOk = activeCategory === 'TODAS' || c.category === activeCategory;
    return groupOk && catOk;
  });

  return (
    <section
      id="semana-1"
      aria-labelledby="heading-semana-1"
      className="scroll-mt-20 border border-slate-800 bg-slate-900/60 rounded-xl p-6 space-y-6"
    >
      <div className="flex flex-col lg:flex-row lg:items-center lg:justify-between gap-4 border-b border-slate-800 pb-4">
        <div className="space-y-1">
          <div className="text-xs font-mono text-cyan-400 flex items-center gap-1.5">
            <Calendar className="w-3.5 h-3.5" />
            <span>Hoja de Ruta &amp; Evidencia Visual</span>
          </div>
          <h2 id="heading-semana-1" className="text-lg font-semibold text-slate-100 tracking-tight">
            Avances por Día y Galería Histórica
          </h2>
        </div>

        <div className="flex flex-wrap items-center gap-2 text-xs font-mono">
          <span className="px-2.5 py-1 rounded-md border border-cyan-500/40 bg-cyan-950/30 text-cyan-300">
            {days.filter((d) => d.status === 'COMPLETADO').length}/{days.length} bloques listos
          </span>
          <span className="px-2.5 py-1 rounded-md border border-slate-700 bg-slate-950 text-slate-300">
            {captures.length} capturas guardadas
          </span>
        </div>
      </div>

      {/* Selector de días */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-3">
        {days.map((day) => {
          const doneCount = day.tasks.filter((t) => t.done).length;
          const isSelected = day.day === currentDay?.day;
          const statusStyle =
            day.status === 'COMPLETADO'
              ? 'text-emerald-300 border-emerald-500/40 bg-emerald-950/30'
              : day.status === 'SIGUIENTE'
                ? 'text-cyan-300 border-cyan-500/40 bg-cyan-950/30'
                : 'text-slate-400 border-slate-700 bg-slate-950';

          return (
            <button
              key={day.day}
              type="button"
              onClick={() => setSelectedDay(day.day)}
              className={`text-left p-4 rounded-lg border transition-colors flex flex-col justify-between gap-2.5 ${
                isSelected
                  ? 'border-cyan-400 bg-slate-900'
                  : 'border-slate-800 bg-slate-950/60 hover:border-slate-700'
              }`}
            >
              <div className="flex items-center justify-between gap-2 text-xs font-mono">
                <span className="text-cyan-400 font-semibold">Día {day.day}</span>
                <span className={`px-1.5 py-0.5 rounded border text-[10px] ${statusStyle}`}>
                  {day.status}
                </span>
              </div>
              <div className="text-xs font-semibold text-slate-100 leading-snug">{day.title}</div>
              <div className="flex items-center justify-between text-[11px] font-mono text-slate-400 pt-1">
                <span>{day.hoursTarget}</span>
                <span>
                  {doneCount}/{day.tasks.length} tareas
                </span>
              </div>
            </button>
          );
        })}
      </div>

      {/* Detalle del día seleccionado */}
      {currentDay && (
        <div className="border border-slate-800 bg-slate-950/70 rounded-xl p-5 space-y-4">
          <div className="flex flex-col md:flex-row md:items-center md:justify-between gap-2 border-b border-slate-800 pb-3">
            <div>
              <div className="text-xs font-mono text-cyan-400">
                Día {currentDay.day} · {currentDay.hoursTarget} · Foco: {currentDay.focus}
              </div>
              <h3 className="text-base font-semibold text-slate-100 mt-0.5">{currentDay.title}</h3>
            </div>
            <div className="text-xs font-mono text-slate-400">
              Entregable: <span className="text-slate-200">{currentDay.visualDeliverable}</span>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            {currentDay.tasks.map((task) => (
              <div
                key={task.id}
                className={`p-3.5 rounded-lg border text-xs space-y-1 ${
                  task.done
                    ? 'border-emerald-500/30 bg-emerald-950/15'
                    : 'border-slate-800 bg-slate-900/50'
                }`}
              >
                <div className="flex items-start gap-2">
                  {task.done ? (
                    <CheckCircle2 className="w-4 h-4 text-emerald-400 shrink-0 mt-0.5" />
                  ) : (
                    <Clock className="w-4 h-4 text-slate-500 shrink-0 mt-0.5" />
                  )}
                  <div className="space-y-1 min-w-0">
                    <div className="font-semibold text-slate-100 leading-snug">{task.label}</div>
                    <p className="text-slate-400 leading-relaxed">{task.detail}</p>
                  </div>
                </div>
              </div>
            ))}
          </div>

          <div className="p-3 rounded-lg bg-slate-900 border border-slate-800 text-xs font-mono text-slate-300">
            Compuerta de cierre del Día {currentDay.day}: {currentDay.verificationGate}
          </div>
        </div>
      )}

      {/* Galería de Capturas Reales por Fase / Día */}
      <div className="space-y-4 pt-2 border-t border-slate-800">
        <div className="flex flex-col gap-3">
          <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-2">
            <div className="flex items-center gap-2">
              <Camera className="w-4 h-4 text-cyan-400" />
              <h3 className="text-sm font-semibold text-slate-100">
                Archivo Histórico de Capturas ({visibleCaptures.length})
              </h3>
            </div>
            <div className="flex flex-wrap gap-1.5">
              {phaseGroups.map((grp) => (
                <button
                  key={grp}
                  type="button"
                  onClick={() => setActivePhaseGroup(grp)}
                  className={`px-2.5 py-1 text-[11px] font-mono rounded-md border transition-colors ${
                    activePhaseGroup === grp
                      ? 'border-emerald-400 bg-emerald-950/40 text-emerald-300'
                      : 'border-slate-800 bg-slate-950 text-slate-400 hover:text-slate-200'
                  }`}
                >
                  {grp}
                </button>
              ))}
            </div>
          </div>
          <div className="flex flex-wrap gap-1.5">
            {categories.map((cat) => (
              <button
                key={cat}
                type="button"
                onClick={() => setActiveCategory(cat)}
                className={`px-2.5 py-1 text-[11px] font-mono rounded-md border transition-colors ${
                  activeCategory === cat
                    ? 'border-cyan-400 bg-cyan-950/40 text-cyan-300'
                    : 'border-slate-800 bg-slate-950 text-slate-400 hover:text-slate-200'
                }`}
              >
                {cat}
              </button>
            ))}
          </div>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {visibleCaptures.map((cap) => (
            <article
              key={cap.id}
              className="border border-slate-800 bg-slate-950/80 rounded-xl overflow-hidden flex flex-col justify-between"
            >
              <div className="relative bg-black/90 border-b border-slate-800 aspect-[16/10] flex items-center justify-center overflow-hidden">
                <img
                  src={cap.imagePath}
                  alt={cap.title}
                  className="w-full h-full object-contain"
                  loading="lazy"
                />
                <button
                  type="button"
                  onClick={() => setZoomCapture(cap)}
                  aria-label={`Ampliar ${cap.title}`}
                  className="absolute bottom-2 right-2 p-1.5 rounded-md bg-slate-900/90 border border-slate-700 text-slate-200 hover:border-cyan-400"
                >
                  <Maximize2 className="w-3.5 h-3.5" />
                </button>
              </div>
              <div className="p-3.5 space-y-1.5 text-xs">
                <div className="flex items-center justify-between gap-2 font-mono text-[10px]">
                  <span className="text-cyan-300">{cap.category}</span>
                  <span className="text-slate-500">{cap.capturedAt}</span>
                </div>
                <div className="font-semibold text-slate-100">{cap.title}</div>
                <p className="text-slate-400 leading-relaxed">{cap.summary}</p>
              </div>
            </article>
          ))}
        </div>
      </div>

      {zoomCapture && (
        <div
          role="dialog"
          aria-modal="true"
          aria-label={zoomCapture.title}
          onClick={() => setZoomCapture(null)}
          className="fixed inset-0 z-50 bg-black/85 backdrop-blur-sm flex items-center justify-center p-6"
        >
          <div
            onClick={(e) => e.stopPropagation()}
            className="max-w-5xl w-full bg-slate-900 border border-slate-700 rounded-xl overflow-hidden shadow-2xl"
          >
            <div className="flex items-center justify-between px-4 py-3 border-b border-slate-800 text-xs">
              <span className="font-semibold text-slate-100">{zoomCapture.title}</span>
              <button
                type="button"
                onClick={() => setZoomCapture(null)}
                className="p-1 rounded hover:bg-slate-800 text-slate-300"
              >
                <X className="w-4 h-4" />
              </button>
            </div>
            <div className="bg-black p-4 flex items-center justify-center max-h-[75vh] overflow-auto">
              <img src={zoomCapture.imagePath} alt={zoomCapture.title} className="max-w-full max-h-[68vh] object-contain" />
            </div>
            <div className="px-4 py-3 text-xs text-slate-300 border-t border-slate-800">
              {zoomCapture.summary}
            </div>
          </div>
        </div>
      )}
    </section>
  );
};
