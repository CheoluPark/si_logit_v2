import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ClipboardList, RefreshCw, Copy, Check } from "lucide-react";
import {
  api,
  type WorkItem,
  type TimelineSession,
} from "../../api/hindsight";
import { useCategories } from "../../state/categories";
import {
  type DayActivity,
  formatTime,
  keywordMatch,
  generateWorkLog,
} from "../../lib/workLogMatch";
import styles from "./WorkLogPage.module.css";

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface MappingState {
  workLogText: string;
  startedAt: string; // local datetime-local value: "YYYY-MM-DDTHH:MM"
  endedAt: string;
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function toLocalDatetime(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  const h = String(d.getHours()).padStart(2, "0");
  const min = String(d.getMinutes()).padStart(2, "0");
  return `${y}-${m}-${day}T${h}:${min}`;
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export default function WorkLogPage() {
  const { t } = useTranslation();
  const { getCategory } = useCategories();

  const [workItems, setWorkItems] = useState<WorkItem[]>([]);
  const [fetching, setFetching] = useState(false);
  const [sessions, setSessions] = useState<TimelineSession[]>([]);
  const [mappings, setMappings] = useState<Record<string, MappingState>>({});
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

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

  /* ---- Fetch today's sessions on mount ---- */
  useEffect(() => {
    const dateStr = new Date().toISOString().split("T")[0];
    api
      .getTimelineSessions(dateStr)
      .then(setSessions)
      .catch(console.error);
  }, []);

  /* ---- Fetch work items ---- */
  const handleFetch = useCallback(async () => {
    setFetching(true);
    try {
      const items = await api.fetchWorkItems();
      setWorkItems(items);

      // Initialize mapping state for each item
      const next: Record<string, MappingState> = {};
      for (const item of items) {
        const matched = keywordMatch(item.summary, dayActivities);
        const startMs =
          matched.length > 0
            ? Math.min(...matched.map((a) => a.startMs))
            : Date.now();
        const endMs =
          matched.length > 0
            ? Math.max(...matched.map((a) => a.endMs))
            : Date.now();
        next[item.key] = {
          workLogText: generateWorkLog(item, matched),
          startedAt: toLocalDatetime(new Date(startMs)),
          endedAt: toLocalDatetime(new Date(endMs)),
        };
      }
      setMappings(next);
    } catch (err) {
      console.error("Failed to fetch work items", err);
    } finally {
      setFetching(false);
    }
  }, [dayActivities]);

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

  /* ---- Render ---- */
  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <ClipboardList className={styles.icon} size={24} />
        <h1 className={styles.title}>{t("workLog.title")}</h1>
      </header>

      <p className={styles.description}>{t("workLog.description")}</p>

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

      {/* Empty state */}
      {workItems.length === 0 && !fetching && (
        <div className={styles.emptyState}>{t("workLog.noItems")}</div>
      )}

      {/* Work item cards */}
      {workItems.map((item) => {
        const mapping = mappings[item.key];
        const matched = matchedByItem.get(item.key) ?? [];

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

            {/* Matched activities */}
            <div>
              <div className={styles.sectionLabel}>
                {t("workLog.matchedActivities")} ({matched.length})
              </div>
              {matched.length > 0 ? (
                <div className={styles.activityList}>
                  {matched.map((act) => (
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
              ) : (
                <div className={styles.noActivities}>
                  {t("workLog.noActivities")}
                </div>
              )}
            </div>

            {/* Work log text */}
            <div>
              <div className={styles.sectionLabel}>
                {t("workLog.workLogText")}
              </div>
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

            {/* Time range */}
            <div className={styles.timeRange}>
              <div className={styles.timeInput}>
                <label className={styles.timeInputLabel}>
                  {t("workLog.startDate")}
                </label>
                <input
                  type="datetime-local"
                  value={mapping?.startedAt ?? ""}
                  onChange={(e) =>
                    updateMapping(item.key, { startedAt: e.target.value })
                  }
                />
              </div>
              <span className={styles.timeSeparator}>~</span>
              <div className={styles.timeInput}>
                <label className={styles.timeInputLabel}>
                  {t("workLog.endDate")}
                </label>
                <input
                  type="datetime-local"
                  value={mapping?.endedAt ?? ""}
                  onChange={(e) =>
                    updateMapping(item.key, { endedAt: e.target.value })
                  }
                />
              </div>
            </div>

            {/* Copy button */}
            <button
              className={styles.copyButton}
              onClick={() => {
                const text = mapping?.workLogText ?? "";
                if (text) {
                  navigator.clipboard.writeText(text);
                  setCopiedKey(item.key);
                  setTimeout(() => setCopiedKey(null), 2000);
                }
              }}
            >
              {copiedKey === item.key ? (
                <>
                  <Check size={14} />
                  {t("workLog.copied")}
                </>
              ) : (
                <>
                  <Copy size={14} />
                  {t("workLog.copy")}
                </>
              )}
            </button>
          </div>
        );
      })}
    </div>
  );
}
