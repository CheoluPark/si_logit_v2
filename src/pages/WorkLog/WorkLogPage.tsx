import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ClipboardList,
  RefreshCw,
  Copy,
  Check,
  ChevronLeft,
  ChevronRight,
  X,
} from "lucide-react";
import {
  api,
  type WorkItem,
  type TimelineSession,
} from "../../api/hindsight";
import { useCategories } from "../../state/categories";
import {
  type DayActivity,
  formatTime,
  formatDuration,
  keywordMatch,
  generateWorkLog,
} from "../../lib/workLogMatch";
import styles from "./WorkLogPage.module.css";

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface MappingState {
  workLogText: string;
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

/** dayOffset (0=today, -1=yesterday) → local "YYYY-MM-DD" */
function offsetToDateStr(dayOffset: number): string {
  const d = new Date();
  d.setDate(d.getDate() + dayOffset);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export default function WorkLogPage() {
  const { t } = useTranslation();
  const { getCategory } = useCategories();

  const [workItems, setWorkItems] = useState<WorkItem[]>([]);
  const [fetching, setFetching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sessions, setSessions] = useState<TimelineSession[]>([]);
  const [mappings, setMappings] = useState<Record<string, MappingState>>({});
  const [copiedKey, setCopiedKey] = useState<string | null>(null);
  const [dayOffset, setDayOffset] = useState(0);
  const [analyzingKey, setAnalyzingKey] = useState<string | null>(null);
  const [activitiesModalKey, setActivitiesModalKey] = useState<string | null>(null);

  const date = useMemo(() => offsetToDateStr(dayOffset), [dayOffset]);

  /* ---- Derive DayActivity[] from sessions + category names ---- */
  const dayActivities = useMemo<DayActivity[]>(() => {
    return sessions.map((s, i) => {
      const cat = getCategory(s.categoryId);
      return {
        id: `act-${i}`,
        appName: s.processName || cat?.name || s.categoryId,
        title: s.windowTitle || "",
        startMs: new Date(s.startedAt).getTime(),
        endMs: new Date(s.endedAt).getTime(),
        superCategory: s.categoryId,
      };
    });
  }, [sessions, getCategory]);

  /* ---- Keyword match per work item ---- */
  const matchedByItem = useMemo(() => {
    const map = new Map<string, DayActivity[]>();
    for (const item of workItems) {
      map.set(item.key, keywordMatch(item.summary, dayActivities));
    }
    return map;
  }, [workItems, dayActivities]);

  /* ---- Total duration per work item ---- */
  const totalDurationByItem = useMemo(() => {
    const map = new Map<string, number>();
    for (const item of workItems) {
      const matched = matchedByItem.get(item.key) ?? [];
      if (matched.length === 0) {
        map.set(item.key, 0);
        continue;
      }
      const startMs = Math.min(...matched.map((a) => a.startMs));
      const endMs = Math.max(...matched.map((a) => a.endMs));
      map.set(item.key, endMs - startMs);
    }
    return map;
  }, [workItems, matchedByItem]);

  /* ---- Fetch sessions for current date ---- */
  useEffect(() => {
    api
      .getTimelineSessions(date)
      .then(setSessions)
      .catch(console.error);
  }, [date]);

  /* ---- Build mappings from items + activities ---- */
  const buildMappings = useCallback(
    (items: WorkItem[], acts: DayActivity[]) => {
      const next: Record<string, MappingState> = {};
      for (const item of items) {
        const matched = keywordMatch(item.summary, acts);
        next[item.key] = {
          workLogText: generateWorkLog(item, matched),
        };
      }
      return next;
    },
    [],
  );

  /* ---- Rebuild mappings when sessions change (date navigation) ---- */
  useEffect(() => {
    if (workItems.length === 0) return;
    setMappings(buildMappings(workItems, dayActivities));
  }, [dayActivities]);

  /* ---- Fetch work items ---- */
  const handleFetch = useCallback(async () => {
    setFetching(true);
    try {
      const items = await api.fetchWorkItems();
      setWorkItems(items);
      setMappings(buildMappings(items, dayActivities));
      setError(null);
    } catch (err) {
      console.error("Failed to fetch work items", err);
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setFetching(false);
    }
  }, [dayActivities, buildMappings]);

  /* ---- Update a mapping field ---- */
  const updateMapping = useCallback(
    (key: string, patch: Partial<MappingState>) => {
      setMappings((prev) => ({
        ...prev,
        [key]: { ...prev[key], ...patch },
      }));
    },
    [],
  );

  /* ---- AI work description analysis ---- */
  const handleAnalyze = useCallback(
    async (item: WorkItem) => {
      const matched = matchedByItem.get(item.key) ?? [];
      if (matched.length === 0) return;
      const startMs = Math.min(...matched.map((a) => a.startMs));
      const endMs = Math.max(...matched.map((a) => a.endMs));
      setAnalyzingKey(item.key);
      try {
        const text = await api.generateWorkDescription(
          date,
          startMs,
          endMs,
          item.summary,
        );
        updateMapping(item.key, { workLogText: text });
      } catch (err) {
        console.error("AI analysis failed", err);
      } finally {
        setAnalyzingKey(null);
      }
    },
    [date, matchedByItem, updateMapping],
  );

  /* ---- Date label ---- */
  const offsetLabel = (off: number): string => {
    if (off === 0) return t("workLog.dateNav.today");
    if (off === -1) return t("workLog.dateNav.yesterday");
    if (off < -1) return t("workLog.dateNav.daysAgo", { count: -off });
    return t("workLog.dateNav.daysLater", { count: off });
  };

  /* ---- Modal activities data ---- */
  const modalActivities = activitiesModalKey
    ? matchedByItem.get(activitiesModalKey) ?? []
    : [];
  const modalItem = activitiesModalKey
    ? workItems.find((it) => it.key === activitiesModalKey) ?? null
    : null;

  /* ---- Render ---- */
  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <ClipboardList className={styles.icon} size={24} />
        <h1 className={styles.title}>{t("workLog.title")}</h1>
      </header>

      <p className={styles.description}>{t("workLog.description")}</p>

      <div className={styles.dateNav}>
        <button
          type="button"
          className={styles.navBtn}
          onClick={() => setDayOffset((v) => v - 1)}
          aria-label={t("workLog.dateNav.prevAria")}
        >
          <ChevronLeft size={14} strokeWidth={1.75} />
        </button>
        <button
          type="button"
          className={`${styles.dayPill} ${dayOffset !== 0 ? styles.dayPillClickable : ""}`}
          onClick={() => setDayOffset(0)}
          disabled={dayOffset === 0}
        >
          {offsetLabel(dayOffset)}
        </button>
        <button
          type="button"
          className={styles.navBtn}
          onClick={() => setDayOffset((v) => v + 1)}
          disabled={dayOffset >= 0}
          aria-label={t("workLog.dateNav.nextAria")}
        >
          <ChevronRight size={14} strokeWidth={1.75} />
        </button>
      </div>

      <button
        className={styles.fetchButton}
        onClick={handleFetch}
        disabled={fetching}
      >
        {fetching ? (
          <>
            <RefreshCw size={16} className={styles.spinner} />
            {t("workLog.fetching")}
          </>
        ) : (
          <>
            <RefreshCw size={16} />
            {t("workLog.fetchItems")}
          </>
        )}
      </button>

      {/* Error state */}
      {error && (
        <div className={styles.errorState} role="alert">
          {error}
        </div>
      )}

      {/* Empty state */}
      {workItems.length === 0 && !fetching && !error && (
        <div className={styles.emptyState}>{t("workLog.noItems")}</div>
      )}

      {/* Work item cards */}
      {workItems.map((item) => {
        const mapping = mappings[item.key];
        const matched = matchedByItem.get(item.key) ?? [];
        const totalMs = totalDurationByItem.get(item.key) ?? 0;

        return (
          <div key={item.key} className={styles.itemCard}>
            {/* Header */}
            <div className={styles.itemHeader}>
              <span className={styles.itemKey}>{item.key}</span>
              <span className={styles.itemSummary}>{item.summary}</span>
              {item.status && (
                <span className={`${styles.badge} ${styles.statusBadge}`}>
                  {item.status}
                </span>
              )}
              {item.issueType && (
                <span className={`${styles.badge} ${styles.typeBadge}`}>
                  {item.issueType}
                </span>
              )}
            </div>

            {/* Custom fields */}
            {(item.background || item.info || item.objective || item.output) && (
              <div className={styles.customFields}>
                {item.background && (
                  <div className={styles.customField}>
                    <span className={styles.customFieldLabel}>{t("workLog.background")}</span>
                    <span className={styles.customFieldValue}>{item.background}</span>
                  </div>
                )}
                {item.info && (
                  <div className={styles.customField}>
                    <span className={styles.customFieldLabel}>{t("workLog.info")}</span>
                    <span className={styles.customFieldValue}>{item.info}</span>
                  </div>
                )}
                {item.objective && (
                  <div className={styles.customField}>
                    <span className={styles.customFieldLabel}>{t("workLog.objective")}</span>
                    <span className={styles.customFieldValue}>{item.objective}</span>
                  </div>
                )}
                {item.output && (
                  <div className={styles.customField}>
                    <span className={styles.customFieldLabel}>{t("workLog.output")}</span>
                    <span className={styles.customFieldValue}>{item.output}</span>
                  </div>
                )}
              </div>
            )}

            {/* Matched activities button */}
            <div className={styles.activityRow}>
              <button
                type="button"
                className={styles.viewActivitiesButton}
                onClick={() => setActivitiesModalKey(item.key)}
                disabled={matched.length === 0}
              >
                {t("workLog.viewActivities")} ({matched.length})
              </button>
              {totalMs > 0 && (
                <span className={styles.totalTime}>
                  {t("workLog.totalTime")}: {formatDuration(totalMs)}
                </span>
              )}
            </div>

            {/* Work log text */}
            <div>
              <div className={styles.sectionLabel}>
                {t("workLog.workLogText")}
              </div>
              <button
                type="button"
                className={styles.analyzeButton}
                disabled={matched.length === 0 || analyzingKey === item.key}
                onClick={() => handleAnalyze(item)}
              >
                {analyzingKey === item.key ? (
                  <>
                    <RefreshCw size={14} className={styles.spinner} />
                    {t("workLog.analyzing")}
                  </>
                ) : (
                  t("workLog.analyze")
                )}
              </button>
              <textarea
                className={styles.textarea}
                value={mapping?.workLogText ?? ""}
                onChange={(e) =>
                  updateMapping(item.key, { workLogText: e.target.value })
                }
                placeholder={t("workLog.placeholder")}
                rows={5}
              />
            </div>

            {/* Copy button */}
            <button
              className={styles.copyButton}
              onClick={() => {
                const text = mapping?.workLogText ?? "";
                if (text) {
                  void navigator.clipboard.writeText(text);
                  setCopiedKey(item.key);
                  setTimeout(() => setCopiedKey(null), 2000);
                }
              }}
            >
              {copiedKey === item.key ? (
                <>
                  <Check size={12} />
                  {t("workLog.copied")}
                </>
              ) : (
                <>
                  <Copy size={12} />
                  {t("workLog.copy")}
                </>
              )}
            </button>
          </div>
        );
      })}

      {/* Activities modal */}
      {activitiesModalKey && (
        <div
          className={styles.modalBackdrop}
          onMouseDown={() => setActivitiesModalKey(null)}
          role="presentation"
        >
          {/* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions */}
          <div
            className={styles.modalDialog}
            role="dialog"
            aria-modal="true"
            aria-labelledby="activities-modal-title"
            onMouseDown={(e) => e.stopPropagation()}
          >
            <h2 id="activities-modal-title" className={styles.modalTitle}>
              {t("workLog.matchedActivities")} — {modalItem?.key ?? ""}
            </h2>
            <div className={styles.modalContent}>
              {modalActivities.length === 0 ? (
                <div className={styles.noActivities}>{t("workLog.noActivities")}</div>
              ) : (
                <div className={styles.activityList}>
                  {modalActivities.map((act) => (
                    <div key={act.id} className={styles.activityItem}>
                      <span className={styles.activityTime}>
                        {formatTime(act.startMs)} ~ {formatTime(act.endMs)}
                      </span>
                      <span className={styles.activityName}>{act.appName}</span>
                      {act.title && (
                        <span className={styles.activityTitle}>{act.title}</span>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </div>
            <div className={styles.modalActions}>
              <button
                type="button"
                className={styles.modalCloseBtn}
                onClick={() => setActivitiesModalKey(null)}
              >
                <X size={14} />
                {t("common.close")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
