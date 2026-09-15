import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import { createPortal } from "react-dom";
import { FileText, X } from "lucide-react";
import timelineDocument from "../../assets/timeline-document.svg";
import { api, type Category, type OffPcGap, type SuperCategory, type TimelineAppUsage, type TimelineBlockDetail, type TimelineSession } from "../../api/hindsight";
import { AppIcon } from "../../components/AppIcon/AppIcon";
import { DevicePicker } from "../../components/DevicePicker/DevicePicker";
import { useIsDark } from "../../hooks/useTheme";
import { useFocusTrap } from "../../hooks/useFocusTrap";
import { useCategories } from "../../state/categories";
import { useDeviceFilter } from "../../state/deviceFilter";
import { useSuperCategories } from "../../state/superCategories";
import { adjustCategoryColor } from "../../utils/categoryColor";
import { displaySuperCategoryName } from "../../utils/categoryName";
import { displayAppName } from "../../utils/displayName";
import { useDurationFormatter } from "../../utils/duration";

interface SessionBlock {
  id: string;
  superCategory: SuperCategory;
  startedAt: number;
  endedAt: number;
}

interface TimelineRow {
  superCategory: SuperCategory;
  totalMs: number;
  blocks: SessionBlock[];
}

interface AwayBlock {
  id: string;
  startedAt: number;
  endedAt: number;
  from: string;
  to: string;
}

const MIN_SCALE = 1;
const MAX_SCALE = 3;
const MIN_GRID_WIDTH = 288;
// 0 keeps capture-session fidelity; a positive value smooths into winning windows of this many minutes.
const TIMELINE_GROUPING_MINUTES = 10;
const TIMELINE_SLOT_MS = TIMELINE_GROUPING_MINUTES * 60_000;
const OFF_PC_SUPER_CATEGORY_ID = "off_pc";
const OFF_PC_COLOR = "#14b8a6";

function timelineCategoryColor(superCategory: SuperCategory, isDark: boolean): string {
  return superCategory.id === OFF_PC_SUPER_CATEGORY_ID
    ? OFF_PC_COLOR
    : adjustCategoryColor(superCategory.color || "#9ca3af", isDark);
}

function dateForOffset(offset: number): Date {
  const date = new Date();
  date.setDate(date.getDate() + offset);
  return date;
}

function fmtDate(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function dayBounds(date: Date) {
  const start = new Date(date);
  start.setHours(0, 0, 0, 0);
  const end = new Date(start);
  end.setDate(end.getDate() + 1);
  return { start: start.getTime(), end: end.getTime() };
}

function wholeSecondIso(timestamp: number): string {
  return new Date(timestamp).toISOString().replace(/\.\d{3}Z$/, "Z");
}

function normalizeSessions(
  sessions: TimelineSession[],
  categories: Category[],
  supers: SuperCategory[],
  bounds: { start: number; end: number },
): SessionBlock[] {
  const categoryById = new Map(categories.map((category) => [category.id, category]));
  const superById = new Map(supers.map((superCategory) => [superCategory.id, superCategory]));
  const resolved = sessions
    .map((session, index) => {
      const category = categoryById.get(session.categoryId);
      const superCategory = category?.superCategoryId
        ? superById.get(category.superCategoryId)
        : undefined;
      const startedAt = new Date(session.startedAt).getTime();
      const endedAt = new Date(session.endedAt).getTime();
      if (!superCategory || !Number.isFinite(startedAt) || !Number.isFinite(endedAt)) return null;
      return { id: `${index}-${session.startedAt}-${session.endedAt}-${session.categoryId}`, superCategory, startedAt, endedAt };
    })
    .filter((session): session is SessionBlock => session !== null)
    .sort((a, b) => a.startedAt - b.startedAt || a.endedAt - b.endedAt || a.id.localeCompare(b.id));

  let previousEnd = bounds.start;
  return resolved.flatMap((session) => {
    const startedAt = Math.max(bounds.start, session.startedAt, previousEnd);
    const endedAt = Math.min(bounds.end, session.endedAt);
    if (endedAt <= startedAt) return [];
    previousEnd = endedAt;
    return [{ ...session, startedAt, endedAt }];
  });
}

function mergeAdjacent(blocks: SessionBlock[]): SessionBlock[] {
  return blocks.reduce<SessionBlock[]>((merged, block) => {
    const previous = merged[merged.length - 1];
    if (previous && previous.superCategory.id === block.superCategory.id && previous.endedAt === block.startedAt) {
      previous.endedAt = block.endedAt;
    } else {
      merged.push({ ...block });
    }
    return merged;
  }, []);
}

function smoothBlocks(blocks: SessionBlock[], bounds: { start: number; end: number }): SessionBlock[] {
  if (TIMELINE_GROUPING_MINUTES <= 0) return mergeAdjacent(blocks);
  const windowMs = TIMELINE_GROUPING_MINUTES * 60_000;
  const smoothed: SessionBlock[] = [];
  for (let windowStart = bounds.start; windowStart < bounds.end; windowStart += windowMs) {
    const windowEnd = Math.min(bounds.end, windowStart + windowMs);
    const occupancy = new Map<string, { superCategory: SuperCategory; ms: number }>();
    for (const block of blocks) {
      const overlap = Math.min(windowEnd, block.endedAt) - Math.max(windowStart, block.startedAt);
      if (overlap <= 0) continue;
      const entry = occupancy.get(block.superCategory.id) ?? { superCategory: block.superCategory, ms: 0 };
      entry.ms += overlap;
      occupancy.set(block.superCategory.id, entry);
    }
    const winner = [...occupancy.values()].sort((a, b) => b.ms - a.ms || a.superCategory.id.localeCompare(b.superCategory.id))[0];
    if (winner) smoothed.push({ id: `window-${windowStart}-${winner.superCategory.id}`, superCategory: winner.superCategory, startedAt: windowStart, endedAt: windowEnd });
  }
  return mergeAdjacent(smoothed);
}

function slotAwayBlocks(
  gaps: OffPcGap[],
  displayedBlocks: SessionBlock[],
  bounds: { start: number; end: number },
): AwayBlock[] {
  if (TIMELINE_GROUPING_MINUTES <= 0) return [];
  const slotMs = TIMELINE_GROUPING_MINUTES * 60_000;
  const slots: { startedAt: number; endedAt: number }[] = [];
  for (const gap of gaps) {
    const rawStart = new Date(gap.from).getTime();
    const rawEnd = new Date(gap.to).getTime();
    if (!Number.isFinite(rawStart) || !Number.isFinite(rawEnd)) continue;
    const startedAt = Math.max(bounds.start, bounds.start + Math.ceil((rawStart - bounds.start) / slotMs) * slotMs);
    const endedAt = Math.min(bounds.end, bounds.start + Math.floor((rawEnd - bounds.start) / slotMs) * slotMs);
    for (let slotStart = startedAt; slotStart < endedAt; slotStart += slotMs) {
      const slotEnd = slotStart + slotMs;
      if (!displayedBlocks.some((block) => block.startedAt < slotEnd && block.endedAt > slotStart)) {
        slots.push({ startedAt: slotStart, endedAt: slotEnd });
      }
    }
  }
  slots.sort((a, b) => a.startedAt - b.startedAt || a.endedAt - b.endedAt);
  return slots.reduce<AwayBlock[]>((merged, slot) => {
    const previous = merged[merged.length - 1];
    if (previous && previous.endedAt >= slot.startedAt) {
      if (slot.endedAt > previous.endedAt) {
        previous.endedAt = slot.endedAt;
        previous.to = new Date(slot.endedAt).toISOString();
      }
    } else {
      merged.push({ id: `away-${slot.startedAt}`, startedAt: slot.startedAt, endedAt: slot.endedAt, from: new Date(slot.startedAt).toISOString(), to: new Date(slot.endedAt).toISOString() });
    }
    return merged;
  }, []);
}

function timelinePosition(
  startedAt: number,
  endedAt: number,
  bounds: { start: number; end: number },
): { left: number; width: number } | null {
  const duration = bounds.end - bounds.start;
  if (duration <= 0) return null;
  const left = Math.min(100, Math.max(0, ((startedAt - bounds.start) / duration) * 100));
  const right = Math.min(100, Math.max(0, ((endedAt - bounds.start) / duration) * 100));
  return right > left ? { left, width: right - left } : null;
}

export default function TimelinePage() {
  const { t, i18n } = useTranslation();
  const { selectedDeviceId, selfId } = useDeviceFilter();
  const { categories, refresh: refreshCategories } = useCategories();
  const { supers } = useSuperCategories();
  const isDark = useIsDark();
  const fmtHM = useDurationFormatter();
  const [offset, setOffset] = useState(0);
  const [scale, setScale] = useState(1);
  const [fitWidth, setFitWidth] = useState(MIN_GRID_WIDTH);
  const [sessions, setSessions] = useState<TimelineSession[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<SessionBlock | null>(null);
  const [detail, setDetail] = useState<TimelineBlockDetail | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [selectedApp, setSelectedApp] = useState<TimelineAppUsage | null>(null);
  const [appDetail, setAppDetail] = useState<TimelineBlockDetail | null>(null);
  const [appDetailError, setAppDetailError] = useState<string | null>(null);
  const [selectedAway, setSelectedAway] = useState<AwayBlock | null>(null);
  const [offPcType, setOffPcType] = useState("");
  const [offPcDetail, setOffPcDetail] = useState("");
  const [offPcStart, setOffPcStart] = useState(0);
  const [offPcEnd, setOffPcEnd] = useState(0);
  const [offPcBusy, setOffPcBusy] = useState(false);
  const [offPcError, setOffPcError] = useState<string | null>(null);
  const [recordableGaps, setRecordableGaps] = useState<OffPcGap[]>([]);
  const [scrollElement, setScrollElement] = useState<HTMLDivElement | null>(null);
  const scaleRef = useRef(scale);
  const sessionRequestRef = useRef(0);
  const gapRequestRef = useRef(0);
  const scopeRef = useRef<{ date: string; deviceId: string | undefined; selfId: string | null }>({ date: "", deviceId: undefined, selfId: null });
  const offPcDrawerRef = useRef<HTMLElement>(null);
  scaleRef.current = scale;
  const setScrollRef = useCallback((element: HTMLDivElement | null) => {
    setScrollElement(element);
  }, []);

  const day = useMemo(() => dateForOffset(offset), [offset]);
  const date = fmtDate(day);
  const bounds = useMemo(() => dayBounds(day), [day]);
  const canRecordOffPc = selectedDeviceId !== undefined && selectedDeviceId === selfId;
  scopeRef.current = { date, deviceId: selectedDeviceId, selfId };
  useFocusTrap(selectedAway !== null, offPcDrawerRef);
  const dayLabel = (value: number) => {
    if (value === 0) return t("timeline.dateNav.today");
    if (value === -1) return t("timeline.dateNav.yesterday");
    return t(value < -1 ? "timeline.dateNav.daysAgo" : "timeline.dateNav.daysLater", { count: Math.abs(value) });
  };

  const reloadSessions = useCallback(async () => {
    const request = ++sessionRequestRef.current;
    const scope = { date, deviceId: selectedDeviceId };
    setSessions(null);
    setError(null);
    try {
      const nextSessions = await api.getTimelineSessions(date, selectedDeviceId);
      if (
        request === sessionRequestRef.current &&
        scopeRef.current.date === scope.date &&
        scopeRef.current.deviceId === scope.deviceId
      ) {
        setSessions(nextSessions);
      }
    } catch (reason) {
      if (
        request === sessionRequestRef.current &&
        scopeRef.current.date === scope.date &&
        scopeRef.current.deviceId === scope.deviceId
      ) {
        setError(reason instanceof Error ? reason.message : String(reason));
      }
    }
  }, [date, selectedDeviceId]);

  const reloadRecordableGaps = useCallback(async () => {
    const scope = { date, deviceId: selectedDeviceId };
    if (!canRecordOffPc || !selectedDeviceId) {
      if (scopeRef.current.date === scope.date && scopeRef.current.deviceId === scope.deviceId) {
        setRecordableGaps([]);
        setSelectedAway(null);
      }
      return;
    }
    const request = ++gapRequestRef.current;
    try {
      const nextGaps = await api.getRecordableOffPcGaps(date, selectedDeviceId);
      if (
        request === gapRequestRef.current &&
        scopeRef.current.date === scope.date &&
        scopeRef.current.deviceId === scope.deviceId
      ) {
        setRecordableGaps(nextGaps);
      }
    } catch {
      if (
        request === gapRequestRef.current &&
        scopeRef.current.date === scope.date &&
        scopeRef.current.deviceId === scope.deviceId
      ) {
        setRecordableGaps([]);
        setSelectedAway(null);
      }
    }
  }, [canRecordOffPc, date, selectedDeviceId]);

  useEffect(() => {
    setSelected(null);
    setSelectedAway(null);
    void reloadSessions();
    void reloadRecordableGaps();
    return () => {
      sessionRequestRef.current += 1;
      gapRequestRef.current += 1;
    };
  }, [reloadRecordableGaps, reloadSessions]);

  useEffect(() => {
    if (!selected) {
      setDetail(null);
      setDetailError(null);
      return;
    }
    let active = true;
    setDetail(null);
    setDetailError(null);
    void api.getTimelineBlockDetail(
      new Date(selected.startedAt).toISOString(),
      new Date(selected.endedAt).toISOString(),
      selected.superCategory.id,
      selectedDeviceId,
    ).then(
      (nextDetail) => { if (active) setDetail(nextDetail); },
      (reason: unknown) => { if (active) setDetailError(reason instanceof Error ? reason.message : String(reason)); },
    );
    return () => { active = false; };
  }, [date, selected, selectedDeviceId]);

  useEffect(() => {
    setSelectedApp(null);
  }, [selected]);

  useEffect(() => {
    if (!selected || !selectedApp) {
      setAppDetail(null);
      setAppDetailError(null);
      return;
    }
    let active = true;
    setAppDetail(null);
    setAppDetailError(null);
    void api.getTimelineAppBlockDetail(
      new Date(selected.startedAt).toISOString(),
      new Date(selected.endedAt).toISOString(),
      selected.superCategory.id,
      selectedApp.iconProcess,
      selectedDeviceId,
    ).then(
      (nextDetail) => { if (active) setAppDetail(nextDetail); },
      (reason: unknown) => { if (active) setAppDetailError(reason instanceof Error ? reason.message : String(reason)); },
    );
    return () => { active = false; };
  }, [date, selected, selectedApp, selectedDeviceId]);

  useEffect(() => {
    if (!selectedApp) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSelectedApp(null);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedApp]);

  useEffect(() => {
    if (!selectedAway) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !offPcBusy) setSelectedAway(null);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [offPcBusy, selectedAway]);

  const rawBlocks = useMemo(
    () => normalizeSessions(sessions ?? [], categories, supers, bounds),
    [bounds, categories, sessions, supers],
  );
  const blocks = useMemo(() => smoothBlocks(rawBlocks, bounds), [bounds, rawBlocks]);
  const rows = useMemo<TimelineRow[]>(() => {
    const byId = new Map<string, TimelineRow>();
    for (const block of blocks) {
      const row = byId.get(block.superCategory.id) ?? { superCategory: block.superCategory, totalMs: 0, blocks: [] };
      row.totalMs += block.endedAt - block.startedAt;
      row.blocks.push(block);
      byId.set(block.superCategory.id, row);
    }
    return [...byId.values()].sort((a, b) => b.totalMs - a.totalMs || a.superCategory.id.localeCompare(b.superCategory.id));
  }, [blocks]);
  const totalMinutes = useMemo(() => Math.round(blocks.reduce((total, block) => total + block.endedAt - block.startedAt, 0) / 60_000), [blocks]);
  const selectedIsOffPc = selected?.superCategory.id === OFF_PC_SUPER_CATEGORY_ID;
  const awayBlocks = useMemo(() => canRecordOffPc ? slotAwayBlocks(recordableGaps, blocks, bounds) : [], [blocks, bounds, canRecordOffPc, recordableGaps]);
  const awayMinutes = useMemo(() => Math.round(awayBlocks.reduce((total, block) => total + block.endedAt - block.startedAt, 0) / 60_000), [awayBlocks]);
  const offPcRangeOptions = useMemo(() => {
    if (!selectedAway || TIMELINE_SLOT_MS <= 0) return [];
    return Array.from({ length: (selectedAway.endedAt - selectedAway.startedAt) / TIMELINE_SLOT_MS + 1 }, (_, index) => selectedAway.startedAt + index * TIMELINE_SLOT_MS);
  }, [selectedAway]);
  const offPcRangeValid = Boolean(
    selectedAway &&
    offPcStart >= selectedAway.startedAt &&
    offPcEnd <= selectedAway.endedAt &&
    offPcEnd > offPcStart &&
    (offPcStart - selectedAway.startedAt) % TIMELINE_SLOT_MS === 0 &&
    (offPcEnd - selectedAway.startedAt) % TIMELINE_SLOT_MS === 0,
  );

  useEffect(() => {
    setSelectedAway((current) =>
      current && awayBlocks.some((block) => block.from === current.from && block.to === current.to)
        ? current
        : null,
    );
  }, [awayBlocks]);

  const openOffPcDrawer = (block: AwayBlock) => {
    if (!canRecordOffPc || !selectedDeviceId) return;
    setSelectedAway(block);
    setOffPcStart(block.startedAt);
    setOffPcEnd(block.endedAt);
    setOffPcType("");
    setOffPcDetail("");
    setOffPcError(null);
  };

  const saveOffPcWork = () => {
    if (!selectedAway || offPcBusy || !canRecordOffPc || !selectedDeviceId) return;
    if (!offPcRangeValid) {
      setOffPcError(t("timeline.offPc.validation.timeRangeInvalid"));
      return;
    }
    if (!offPcType) {
      setOffPcError(t("timeline.offPc.validation.taskTypeRequired"));
      return;
    }
    if (!offPcDetail.trim()) {
      setOffPcError(t("timeline.offPc.validation.detailRequired"));
      return;
    }
    setOffPcBusy(true);
    setOffPcError(null);
    const saveScope = { date, deviceId: selectedDeviceId, selfId };
    const scopeMatches = () =>
      scopeRef.current.date === saveScope.date &&
      scopeRef.current.deviceId === saveScope.deviceId &&
      scopeRef.current.selfId === saveScope.selfId;
    void (async () => {
      try {
        await api.recordOffPcWork(
          wholeSecondIso(offPcStart),
          wholeSecondIso(offPcEnd),
          selectedDeviceId,
          offPcType,
          offPcDetail.trim(),
        );
        setSelectedAway(null);
        await refreshCategories();
        if (scopeMatches() && saveScope.deviceId === saveScope.selfId) {
          await reloadSessions();
          await reloadRecordableGaps();
        }
      } catch (reason) {
        setOffPcError(t("timeline.offPc.errors.saveFailed", { message: reason instanceof Error ? reason.message : String(reason) }));
        if (scopeMatches() && saveScope.deviceId === saveScope.selfId) await reloadRecordableGaps();
      } finally {
        setOffPcBusy(false);
      }
    })();
  };

  useEffect(() => {
    const element = scrollElement;
    if (!element) return;
    const updateFitWidth = () => {
      const labelWidth = element.querySelector<HTMLElement>(".timeline-label")?.getBoundingClientRect().width ?? 0;
      setFitWidth(Math.max(MIN_GRID_WIDTH, element.clientWidth - labelWidth));
    };
    updateFitWidth();
    const observer = new ResizeObserver(updateFitWidth);
    observer.observe(element);
    return () => observer.disconnect();
  }, [scrollElement]);

  useEffect(() => {
    const element = scrollElement;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest("[data-timeline-hours]")) return;
      if (event.ctrlKey || event.deltaY === 0) return;
      event.preventDefault();
      const oldScale = scaleRef.current;
      const nextScale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, oldScale * Math.exp(-event.deltaY * 0.0015)));
      if (nextScale === oldScale) return;
      const pointerX = event.clientX - element.getBoundingClientRect().left;
      const grid = element.querySelector<HTMLElement>("[data-timeline-hours]");
      const gridOffset = grid?.offsetLeft ?? 0;
      const unscaledX = (element.scrollLeft + pointerX - gridOffset) / oldScale;
      scaleRef.current = nextScale;
      setScale(nextScale);
      requestAnimationFrame(() => { element.scrollLeft = gridOffset + unscaledX * nextScale - pointerX; });
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [scrollElement]);

  const timelineWidth = fitWidth * scale;
  const hourWidth = timelineWidth / 24;
  const timeFormatter = useMemo(() => new Intl.DateTimeFormat(i18n.language, { hour: "2-digit", minute: "2-digit" }), [i18n.language]);

  return (
    <main style={pageStyle}>
      <style>{`
        .timeline-scroll { overflow-x: auto; overscroll-behavior: contain; scrollbar-color: var(--border-default) transparent; }
        .timeline-block { appearance: none; box-sizing: border-box; border: 0; cursor: pointer; overflow: hidden; padding: 0; transition: filter 180ms var(--ease-out), opacity 180ms var(--ease-out), transform 140ms ease; }
        .timeline-block:hover, .timeline-block:focus-visible { filter: brightness(1.05); outline: none; transform: translateY(-1px); }
        .timeline-block:focus-visible { outline: 2px solid var(--accent, #6d5dfc); outline-offset: 2px; z-index: 2; }
        .timeline-block[data-selected=true] { filter: brightness(1.1) saturate(1.08); }
        .timeline-block[data-off-pc=true][data-selected=true] { box-shadow: inset 0 0 0 2px #042f2e; }
        .timeline-block[data-off-pc=true]:focus-visible { outline-color: #042f2e; }
        .timeline-block[data-dimmed=true] { opacity: .32; filter: saturate(.6); }
        .timeline-away-block { appearance: none; box-sizing: border-box; border: 0; cursor: pointer; overflow: hidden; transition: opacity 160ms var(--ease-out), filter 160ms var(--ease-out); }
        .timeline-away-block:hover, .timeline-away-block:focus-visible { opacity: .9 !important; filter: brightness(.96); outline: none; }
        .timeline-away-block:focus-visible { outline: 2px solid var(--accent, #6d5dfc); outline-offset: 2px; z-index: 2; }
        .timeline-detail-app:hover { background: color-mix(in srgb, var(--surface-card-elevated) 88%, var(--text-muted)) !important; }
        .timeline-detail-app:focus-visible { outline: 2px solid var(--accent, #6d5dfc); outline-offset: 1px; border-radius: 5px; }
        .timeline-nav-button:hover:not(:disabled) { border-color: var(--text-muted) !important; color: var(--text-strong) !important; }
        .timeline-nav-button:disabled { opacity: .46; cursor: default !important; }
        @media (max-width: 640px) { .timeline-label { width: 132px !important; min-width: 0 !important; flex-basis: 132px !important; } }
      `}</style>

      <header style={pageHeaderStyle}>
        <h1 style={titleStyle}>{t("timeline.pageTitle")}</h1>
        <p style={metaStyle}>
          {t("timeline.meta", { date: new Intl.DateTimeFormat(i18n.language, { weekday: "short", month: "long", day: "numeric" }).format(day) })}
        </p>
      </header>

      <div style={navigationStyle}>
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <button className="timeline-nav-button" type="button" onClick={() => setOffset((value) => value - 1)} aria-label={t("timeline.dateNav.prevAria")} style={navButtonStyle}>←</button>
          <button className="timeline-nav-button" type="button" onClick={() => setOffset(0)} disabled={offset === 0} style={{ ...navButtonStyle, minWidth: 94 }}>{dayLabel(offset)}</button>
          <button className="timeline-nav-button" type="button" onClick={() => setOffset((value) => Math.min(0, value + 1))} disabled={offset >= 0} aria-label={t("timeline.dateNav.nextAria")} style={navButtonStyle}>→</button>
        </div>
        <DevicePicker />
      </div>

      <section style={cardStyle} aria-label={t("timeline.cardTitle")}>
        <header style={cardHeaderStyle}>
          <h2 style={cardTitleStyle}>{t("timeline.title")}</h2>
          <span style={totalStyle}>{fmtHM(totalMinutes)}</span>
        </header>
        {error ? (
          <p role="alert" style={stateStyle}>{t("timeline.errors.load", { message: error })}</p>
        ) : sessions === null ? (
          <p role="status" style={stateStyle}>{t("timeline.loading")}</p>
        ) : rows.length === 0 && awayBlocks.length === 0 ? (
          <p style={stateStyle}>{t("timeline.empty")}</p>
        ) : (
          <div ref={setScrollRef} className="timeline-scroll" aria-label={t("timeline.gridAria")}>
            <div style={{ minWidth: "max-content" }} role="table" aria-label={t("timeline.gridAria")}>
              <div style={headRowStyle} role="row">
                <div className="timeline-label" style={cornerStyle}>{t("timeline.category")}</div>
                <div data-timeline-hours style={{ width: timelineWidth, position: "relative", transition: "width 100ms ease-out" }} aria-hidden="true">
                  <div style={{ display: "grid", gridTemplateColumns: `repeat(24, ${hourWidth}px)` }}>
                    {Array.from({ length: 24 }, (_, hour) => <div key={hour} style={hourStyle}>{String(hour).padStart(2, "0")}</div>)}
                  </div>
                </div>
              </div>
              {awayBlocks.length > 0 && <div style={rowStyle} role="row">
                <div className="timeline-label" style={labelStyle}>
                  <span style={{ width: 8, height: 8, borderRadius: "50%", background: "#a8a29e", boxShadow: "0 0 0 3px rgba(168, 162, 158, .16)", flex: "0 0 auto" }} />
                  <span style={{ flex: 1 }}>{t("timeline.away")}</span>
                  <span style={rowTotalStyle}>{fmtHM(awayMinutes)}</span>
                </div>
                <div data-timeline-hours style={{ width: timelineWidth, position: "relative", transition: "width 100ms ease-out", backgroundImage: "repeating-linear-gradient(to right, transparent 0, transparent calc(4.166667% - 1px), var(--border-subtle) calc(4.166667% - 1px), var(--border-subtle) 4.166667%)" }}>
                  {awayBlocks.map((block) => {
                    const position = timelinePosition(block.startedAt, block.endedAt, bounds);
                    if (!position) return null;
                    const duration = Math.round((block.endedAt - block.startedAt) / 60_000);
                    return <button key={block.id} type="button" className="timeline-away-block" onClick={() => openOffPcDrawer(block)} aria-label={t("timeline.barAria", { category: t("timeline.away"), start: timeFormatter.format(block.startedAt), end: timeFormatter.format(block.endedAt), duration: fmtHM(duration) })} style={{ position: "absolute", left: `${position.left}%`, width: `${position.width}%`, top: 11, height: 36, borderRadius: 6, background: "#a8a29e", opacity: 0.72, boxShadow: "inset 0 -1px rgba(0, 0, 0, .1)" }} />;
                  })}
                </div>
              </div>}
              {rows.map((row) => {
                const name = displaySuperCategoryName(row.superCategory, t);
                const color = timelineCategoryColor(row.superCategory, isDark);
                const isOffPc = row.superCategory.id === OFF_PC_SUPER_CATEGORY_ID;
                return <div key={row.superCategory.id} style={rowStyle} role="row">
                  <div className="timeline-label" style={labelStyle}>
                    <span style={{ width: 8, height: 8, borderRadius: "50%", background: color, boxShadow: `0 0 0 3px color-mix(in srgb, ${color} 18%, transparent)`, flex: "0 0 auto" }} />
                    <span title={name} style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1 }}>{name}</span>
                    <span style={rowTotalStyle}>{fmtHM(Math.round(row.totalMs / 60_000))}</span>
                  </div>
                  <div data-timeline-hours style={{ width: timelineWidth, position: "relative", transition: "width 100ms ease-out", backgroundImage: "repeating-linear-gradient(to right, transparent 0, transparent calc(4.166667% - 1px), var(--border-subtle) calc(4.166667% - 1px), var(--border-subtle) 4.166667%)" }}>
                    {row.blocks.map((block) => {
                      const selectedBlock = selected?.id === block.id;
                      const dimmed = selected !== null && !selectedBlock;
                      const position = timelinePosition(block.startedAt, block.endedAt, bounds);
                      if (!position) return null;
                      const duration = Math.round((block.endedAt - block.startedAt) / 60_000);
                      return <button key={block.id} type="button" className="timeline-block" data-off-pc={isOffPc || undefined} data-selected={selectedBlock || undefined} data-dimmed={dimmed || undefined} onClick={() => setSelected(selectedBlock ? null : block)} aria-label={t("timeline.barAria", { category: name, start: timeFormatter.format(block.startedAt), end: timeFormatter.format(block.endedAt), duration: fmtHM(duration) })} style={{ position: "absolute", left: `${position.left}%`, width: `${position.width}%`, maxWidth: "100%", top: 11, height: 36, borderRadius: 6, background: color }} />;
                    })}
                  </div>
                </div>;
              })}
            </div>
          </div>
        )}
      </section>
      {selected && <section style={detailSectionStyle} aria-label={t("timeline.selectedSummary", { category: displaySuperCategoryName(selected.superCategory, t), start: timeFormatter.format(selected.startedAt), end: timeFormatter.format(selected.endedAt), duration: fmtHM(Math.round((selected.endedAt - selected.startedAt) / 60_000)) })}>
        <p role="status" style={detailStyle}>{t("timeline.selectedSummary", { category: displaySuperCategoryName(selected.superCategory, t), start: timeFormatter.format(selected.startedAt), end: timeFormatter.format(selected.endedAt), duration: fmtHM(Math.round((selected.endedAt - selected.startedAt) / 60_000)) })}</p>
        <div style={detailGridStyle}>
          <section style={detailCardStyle}>
            <h2 style={detailCardTitleStyle}>{t("timeline.detail.topApps")}</h2>
            {detailError ? <p role="alert" style={detailStateStyle}>{t("timeline.detail.error", { message: detailError })}</p> : detail === null ? <p role="status" style={detailStateStyle}>{t("timeline.detail.loading")}</p> : detail.apps.length === 0 ? <p style={detailStateStyle}>{t("timeline.detail.empty")}</p> : <div>{detail.apps.map((app, index) => {
              const maxSecs = Math.max(...detail.apps.map((item) => item.secs), 1);
              const appName = displayAppName(app.process);
              return <button key={`${app.process}-${index}`} className="timeline-detail-app" type="button" onClick={() => setSelectedApp(app)} style={appRowStyle}><span style={rankStyle}>{index + 1}</span><AppIcon processName={app.iconProcess} fallbackColor="#94a3b8" size={22} /><span style={appNameStyle} title={appName}>{appName}</span><span aria-hidden="true" style={appProgressTrackStyle}><span style={{ ...appProgressFillStyle, width: `${(app.secs / maxSecs) * 100}%`, ...(selectedIsOffPc ? { background: OFF_PC_COLOR } : {}) }} /></span><span style={detailDurationStyle}>{fmtHM(Math.round(app.secs / 60))}</span></button>;
            })}</div>}
          </section>
          <section style={detailCardStyle}>
            <h2 style={detailCardTitleStyle}>{t("timeline.detail.titles")}</h2>
            {detailError ? <p role="alert" style={detailStateStyle}>{t("timeline.detail.error", { message: detailError })}</p> : detail === null ? <p role="status" style={detailStateStyle}>{t("timeline.detail.loading")}</p> : detail.titles.length === 0 ? <p style={detailStateStyle}>{t("timeline.detail.noTitles")}</p> : <div>{detail.titles.map((title, index) => {
              const maxSecs = detail.titles[0]?.secs || 1;
              return <div key={`${title.title}-${index}`} style={titleRowStyle}><img src={timelineDocument} alt="" aria-hidden="true" style={titleDocumentStyle} /><span style={detailNameStyle} title={title.title}>{title.title}</span><span style={detailDurationStyle}>{fmtHM(Math.round(title.secs / 60))}</span><span aria-hidden="true" style={{ ...progressStyle, width: `${(title.secs / maxSecs) * 100}%`, ...(selectedIsOffPc ? { background: OFF_PC_COLOR } : {}) }} /></div>;
            })}</div>}
          </section>
        </div>
      </section>}
      {selected && selectedApp && createPortal(
        <div style={drawerBackdropStyle}>
          <button type="button" aria-label={t("timeline.detail.closeAria")} onClick={() => setSelectedApp(null)} style={drawerDismissStyle} />
          <aside role="dialog" aria-modal="true" aria-label={t("timeline.detail.dialogAria", { app: displayAppName(selectedApp.process) })} style={drawerStyle}>
            <header style={drawerHeaderStyle}>
              <div style={drawerAppStyle}><AppIcon processName={selectedApp.iconProcess} fallbackColor="#94a3b8" size={30} /><div style={{ minWidth: 0 }}><h2 style={drawerTitleStyle}>{t("timeline.detail.appTitle", { app: displayAppName(selectedApp.process) })}</h2><p style={drawerDurationStyle}>{t("timeline.detail.usageDuration", { duration: fmtHM(Math.round(selectedApp.secs / 60)) })}</p></div></div>
              <button type="button" onClick={() => setSelectedApp(null)} aria-label={t("timeline.detail.closeAria")} style={drawerCloseStyle}><X size={18} aria-hidden="true" /><span style={{ position: "absolute", width: 1, height: 1, overflow: "hidden", clipPath: "inset(50%)" }}>{t("timeline.detail.close")}</span></button>
            </header>
            <div style={drawerBodyStyle}>
              <h3 style={detailCardTitleStyle}>{t("timeline.detail.titles")}</h3>
              {appDetailError ? <p role="alert" style={detailStateStyle}>{t("timeline.detail.error", { message: appDetailError })}</p> : appDetail === null ? <p role="status" style={detailStateStyle}>{t("timeline.detail.loading")}</p> : appDetail.titles.length === 0 ? <p style={detailStateStyle}>{t("timeline.detail.noTitles")}</p> : appDetail.titles.map((title, index) => <div key={`${title.title}-${index}`} style={drawerTitleRowStyle}><FileText size={16} aria-hidden="true" /><span style={detailNameStyle} title={title.title}>{title.title}</span><span style={detailDurationStyle}>{fmtHM(Math.round(title.secs / 60))}</span></div>)}
            </div>
          </aside>
        </div>,
        document.body,
      )}
      {selectedAway && createPortal(
        <div style={offPcDrawerBackdropStyle}>
          <button type="button" aria-label={t("timeline.offPc.closeAria")} onClick={() => !offPcBusy && setSelectedAway(null)} style={drawerDismissStyle} disabled={offPcBusy} />
          <aside ref={offPcDrawerRef} role="dialog" aria-modal="true" aria-label={t("timeline.offPc.dialogAria")} style={offPcDrawerStyle}>
            <header style={drawerHeaderStyle}>
              <div style={{ minWidth: 0 }}><h2 style={drawerTitleStyle}>{t("timeline.offPc.drawerTitle")}</h2><p style={drawerDurationStyle}>{t("timeline.offPc.drawerSubtitle")}</p><p style={drawerRangeStyle}>{timeFormatter.format(selectedAway.startedAt)}–{timeFormatter.format(selectedAway.endedAt)}</p></div>
              <button type="button" onClick={() => setSelectedAway(null)} aria-label={t("timeline.offPc.closeAria")} style={drawerCloseStyle} disabled={offPcBusy}><X size={18} aria-hidden="true" /></button>
            </header>
            <div style={drawerBodyStyle}>
              <div style={offPcFixedCategoryStyle}>{t("timeline.offPc.superCategoryLabel")}</div>
              <fieldset style={offPcRangeStyle} disabled={offPcBusy}>
                <legend style={offPcRangeLegendStyle}>{t("timeline.offPc.timeRangeLabel")}</legend>
                <label style={offPcRangeFieldStyle}>{t("timeline.offPc.startTimeLabel")}
                  <select value={offPcStart} onChange={(event) => {
                    const start = Number(event.target.value);
                    setOffPcStart(start);
                    if (start >= offPcEnd) setOffPcEnd(start + TIMELINE_SLOT_MS);
                    setOffPcError(null);
                  }} style={offPcSelectStyle}>
                    {offPcRangeOptions.slice(0, -1).map((value) => <option key={value} value={value}>{timeFormatter.format(value)}</option>)}
                  </select>
                </label>
                <span aria-hidden="true" style={offPcRangeDashStyle}>→</span>
                <label style={offPcRangeFieldStyle}>{t("timeline.offPc.endTimeLabel")}
                  <select value={offPcEnd} onChange={(event) => {
                    const end = Number(event.target.value);
                    setOffPcEnd(end);
                    if (end <= offPcStart) setOffPcStart(end - TIMELINE_SLOT_MS);
                    setOffPcError(null);
                  }} style={offPcSelectStyle}>
                    {offPcRangeOptions.slice(1).map((value) => <option key={value} value={value}>{timeFormatter.format(value)}</option>)}
                  </select>
                </label>
              </fieldset>
              <label style={offPcFieldStyle}>{t("timeline.offPc.taskTypeLabel")}
                <select value={offPcType} disabled={offPcBusy} onChange={(event) => setOffPcType(event.target.value)} style={offPcSelectStyle}>
                  <option value="" />
                  <option value="meeting">{t("timeline.offPc.taskTypes.meeting")}</option>
                  <option value="business_trip">{t("timeline.offPc.taskTypes.businessTrip")}</option>
                  <option value="exam">{t("timeline.offPc.taskTypes.exam")}</option>
                </select>
              </label>
              <label style={offPcFieldStyle}>{t("timeline.offPc.detailLabel")}
                <textarea value={offPcDetail} disabled={offPcBusy} rows={4} placeholder={t("timeline.offPc.detailPlaceholder")} onChange={(event) => setOffPcDetail(event.target.value)} style={offPcTextareaStyle} />
              </label>
              {offPcError && <p role="alert" style={offPcErrorStyle}>{offPcError}</p>}
              <div style={offPcActionsStyle}>
                <button type="button" onClick={() => setSelectedAway(null)} disabled={offPcBusy} style={offPcCancelStyle}>{t("timeline.offPc.cancel")}</button>
                <button type="button" onClick={saveOffPcWork} disabled={offPcBusy || !offPcRangeValid} style={offPcSaveStyle}>{offPcBusy ? t("timeline.offPc.saving") : t("timeline.offPc.save")}</button>
              </div>
            </div>
          </aside>
        </div>,
        document.body,
      )}
    </main>
  );
}

const pageStyle: CSSProperties = { maxWidth: 940, margin: "0 auto", padding: "32px clamp(16px, 4vw, 40px) 56px", color: "var(--text-strong)" };
const pageHeaderStyle: CSSProperties = { marginBottom: 20 };
const titleStyle: CSSProperties = { margin: 0, fontSize: 26, fontWeight: "var(--font-weight-semibold)", letterSpacing: "-.02em", lineHeight: 1.2 };
const metaStyle: CSSProperties = { margin: "5px 0 0", color: "var(--text-muted)", fontSize: 12 };
const navigationStyle: CSSProperties = { display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12, marginBottom: 18, flexWrap: "wrap" };
const cardStyle: CSSProperties = { background: "var(--surface-card-elevated)", border: "1px solid var(--border-default)", borderRadius: "var(--radius-md)", overflow: "hidden", boxShadow: "0 1px 2px rgba(20, 20, 40, .04)" };
const cardHeaderStyle: CSSProperties = { display: "flex", justifyContent: "space-between", gap: 16, alignItems: "center", padding: "15px 18px 12px", borderBottom: "1px solid var(--border-subtle)" };
const cardTitleStyle: CSSProperties = { margin: 0, fontSize: 13, fontWeight: "var(--font-weight-600)" };
const totalStyle: CSSProperties = { color: "var(--text-strong)", fontSize: 12, fontWeight: "var(--font-weight-600)", fontVariantNumeric: "tabular-nums" };
const navButtonStyle: CSSProperties = { minHeight: 32, padding: "0 10px", border: "1px solid var(--border-default)", borderRadius: 8, background: "var(--surface-card-elevated)", color: "var(--text-muted)", cursor: "pointer", fontSize: 12 };
const headRowStyle: CSSProperties = { display: "flex", borderBottom: "1px solid var(--border-subtle)", background: "color-mix(in srgb, var(--surface-card-elevated) 94%, var(--text-muted))" };
const cornerStyle: CSSProperties = { width: 170, minWidth: 0, flex: "0 0 170px", padding: "0 12px", display: "flex", alignItems: "center", color: "var(--text-muted)", borderRight: "1px solid var(--border-subtle)", fontSize: 11, position: "sticky", left: 0, zIndex: 3, background: "color-mix(in srgb, var(--surface-card-elevated) 94%, var(--text-muted))", boxShadow: "2px 0 4px rgba(20, 20, 40, .035)" };
const labelStyle: CSSProperties = { width: 170, minWidth: 0, flex: "0 0 170px", padding: "0 12px", display: "flex", alignItems: "center", gap: 7, borderRight: "1px solid var(--border-subtle)", fontSize: 12, position: "sticky", left: 0, zIndex: 2, background: "var(--surface-card-elevated)", boxShadow: "2px 0 4px rgba(20, 20, 40, .035)" };
const rowStyle: CSSProperties = { display: "flex", minHeight: 58, borderBottom: "1px solid var(--border-subtle)" };
const rowTotalStyle: CSSProperties = { color: "var(--text-muted)", fontSize: 11, fontVariantNumeric: "tabular-nums" };
const hourStyle: CSSProperties = { padding: "10px 0", textAlign: "center", color: "var(--text-muted)", fontSize: 10, fontVariantNumeric: "tabular-nums", borderLeft: "1px solid var(--border-subtle)" };
const stateStyle: CSSProperties = { margin: 0, padding: "52px 22px", textAlign: "center", color: "var(--text-muted)", fontSize: 13 };
const detailSectionStyle: CSSProperties = { marginTop: 16, background: "var(--surface-card-elevated)", border: "1px solid var(--border-default)", borderRadius: "var(--radius-md)", overflow: "hidden" };
const detailStyle: CSSProperties = { margin: 0, padding: "11px 14px", color: "var(--text-muted)", fontSize: 12, borderBottom: "1px solid var(--border-subtle)" };
const detailGridStyle: CSSProperties = { display: "grid", gridTemplateColumns: "repeat(2, minmax(0, 1fr))", gap: 12, padding: 12 };
const detailCardStyle: CSSProperties = { minWidth: 0, padding: "12px 14px", border: "1px solid var(--border-subtle)", borderRadius: 8, background: "color-mix(in srgb, var(--surface-card-elevated) 94%, var(--text-muted))" };
const detailCardTitleStyle: CSSProperties = { margin: "0 0 8px", fontSize: 12, fontWeight: "var(--font-weight-600)" };
const detailStateStyle: CSSProperties = { margin: 0, color: "var(--text-muted)", fontSize: 12 };
const detailRowStyle: CSSProperties = { display: "flex", alignItems: "center", gap: 10, minWidth: 0, padding: "6px 0", borderTop: "1px solid var(--border-subtle)", fontSize: 12 };
const appRowStyle: CSSProperties = { ...detailRowStyle, width: "100%", position: "relative", border: 0, background: "transparent", color: "inherit", cursor: "pointer", textAlign: "left", padding: "8px 2px", overflow: "hidden" };
const titleRowStyle: CSSProperties = { ...detailRowStyle, position: "relative", padding: "8px 2px", overflow: "hidden" };
const rankStyle: CSSProperties = { width: 14, flex: "0 0 14px", color: "var(--text-muted)", fontSize: 11, fontVariantNumeric: "tabular-nums", textAlign: "center" };
const appNameStyle: CSSProperties = { minWidth: 0, flex: "1 1 72px", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" };
const appProgressTrackStyle: CSSProperties = { minWidth: 28, flex: "1 1 64px", height: 5, overflow: "hidden", borderRadius: 999, background: "color-mix(in srgb, var(--text-muted) 16%, transparent)" };
const appProgressFillStyle: CSSProperties = { display: "block", height: "100%", borderRadius: "inherit", background: "color-mix(in srgb, var(--accent, #6d5dfc) 78%, var(--text-muted))", transition: "width 180ms var(--ease-out)" };
const titleDocumentStyle: CSSProperties = { width: 24, height: 24, flex: "0 0 24px", objectFit: "contain" };
const progressStyle: CSSProperties = { position: "absolute", left: 0, bottom: 1, height: 2, borderRadius: 999, background: "color-mix(in srgb, var(--text-muted) 38%, transparent)", transition: "width 180ms var(--ease-out)" };
const detailNameStyle: CSSProperties = { minWidth: 0, flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" };
const detailDurationStyle: CSSProperties = { flex: "0 0 auto", color: "var(--text-muted)", fontSize: 11, fontVariantNumeric: "tabular-nums" };
const drawerBackdropStyle: CSSProperties = { position: "fixed", inset: 0, zIndex: 100, display: "flex", justifyContent: "flex-end", background: "rgba(20, 20, 28, .34)" };
const drawerDismissStyle: CSSProperties = { position: "absolute", inset: 0, border: 0, background: "transparent", cursor: "default" };
const drawerStyle: CSSProperties = { position: "relative", zIndex: 1, width: "min(420px, 100%)", height: "100%", background: "var(--surface-card-elevated)", borderLeft: "1px solid var(--border-default)", boxShadow: "-12px 0 32px rgba(20, 20, 40, .16)", display: "flex", flexDirection: "column" };
const offPcDrawerBackdropStyle: CSSProperties = { position: "fixed", inset: 0, zIndex: "var(--z-modal-backdrop)", display: "flex", justifyContent: "flex-end", padding: 14, background: "rgba(20, 20, 40, .32)", backdropFilter: "blur(4px)", WebkitBackdropFilter: "blur(4px)" };
const offPcDrawerStyle: CSSProperties = { position: "relative", zIndex: "var(--z-modal)", width: "min(560px, 92vw)", background: "var(--surface-card)", borderRadius: "var(--radius-lg)", overflow: "hidden", boxShadow: "0 12px 40px rgba(20, 20, 40, .22), 0 2px 8px rgba(20, 20, 40, .12)", display: "flex", flexDirection: "column" };
const drawerHeaderStyle: CSSProperties = { display: "flex", justifyContent: "space-between", gap: 12, alignItems: "flex-start", padding: "20px 18px 16px", borderBottom: "1px solid var(--border-subtle)" };
const drawerAppStyle: CSSProperties = { display: "flex", alignItems: "center", gap: 10, minWidth: 0 };
const drawerTitleStyle: CSSProperties = { margin: 0, fontSize: 15, fontWeight: "var(--font-weight-semibold)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" };
const drawerDurationStyle: CSSProperties = { margin: "4px 0 0", color: "var(--text-muted)", fontSize: 12 };
const drawerRangeStyle: CSSProperties = { margin: "8px 0 0", color: "var(--text-strong)", fontSize: 12, fontVariantNumeric: "tabular-nums" };
const drawerCloseStyle: CSSProperties = { position: "relative", width: 32, height: 32, display: "grid", placeItems: "center", flex: "0 0 auto", border: "1px solid var(--border-default)", borderRadius: 7, background: "transparent", color: "var(--text-muted)", cursor: "pointer" };
const drawerBodyStyle: CSSProperties = { minHeight: 0, overflowY: "auto", padding: "18px" };
const drawerTitleRowStyle: CSSProperties = { display: "flex", alignItems: "center", gap: 10, minWidth: 0, padding: "10px 0", borderTop: "1px solid var(--border-subtle)", fontSize: 12, color: "var(--text-muted)" };
const offPcFieldStyle: CSSProperties = { display: "grid", gap: 7, marginBottom: 16, color: "var(--text-strong)", fontSize: 12, fontWeight: "var(--font-weight-500)" };
const offPcFixedCategoryStyle: CSSProperties = { minHeight: 36, display: "flex", alignItems: "center", marginBottom: 16, padding: "0 9px", border: "1px solid var(--border-default)", borderRadius: 7, background: "color-mix(in srgb, var(--surface-card-elevated) 92%, var(--text-muted))", color: "var(--text-muted)", fontWeight: "var(--font-weight-400)" };
const offPcRangeStyle: CSSProperties = { display: "grid", gridTemplateColumns: "minmax(0, 1fr) auto minmax(0, 1fr)", alignItems: "end", gap: 8, margin: "0 0 16px", padding: "10px", border: "1px solid var(--border-subtle)", borderRadius: 8, background: "color-mix(in srgb, var(--surface-card-elevated) 94%, var(--text-muted))" };
const offPcRangeLegendStyle: CSSProperties = { padding: "0 4px", color: "var(--text-muted)", fontSize: 11, fontWeight: "var(--font-weight-500)" };
const offPcRangeFieldStyle: CSSProperties = { display: "grid", gap: 5, minWidth: 0, color: "var(--text-muted)", fontSize: 11, fontWeight: "var(--font-weight-500)" };
const offPcRangeDashStyle: CSSProperties = { paddingBottom: 9, color: "var(--text-muted)", fontSize: 14 };
const offPcSelectStyle: CSSProperties = { width: "100%", minHeight: 36, padding: "0 9px", border: "1px solid var(--border-default)", borderRadius: 7, background: "var(--surface-card-elevated)", color: "var(--text-strong)" };
const offPcTextareaStyle: CSSProperties = { width: "100%", boxSizing: "border-box", resize: "vertical", padding: 9, border: "1px solid var(--border-default)", borderRadius: 7, background: "var(--surface-card-elevated)", color: "var(--text-strong)", font: "inherit" };
const offPcErrorStyle: CSSProperties = { margin: "0 0 12px", color: "var(--danger, #b42318)", fontSize: 12 };
const offPcActionsStyle: CSSProperties = { display: "flex", justifyContent: "flex-end", gap: 8, marginTop: 20 };
const offPcCancelStyle: CSSProperties = { minHeight: 34, padding: "0 12px", border: "1px solid var(--border-default)", borderRadius: 7, background: "transparent", color: "var(--text-strong)", cursor: "pointer" };
const offPcSaveStyle: CSSProperties = { minHeight: 34, padding: "0 12px", border: 0, borderRadius: 7, background: "var(--accent, #6d5dfc)", color: "white", cursor: "pointer" };
