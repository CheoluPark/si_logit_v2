import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ClipboardList, RefreshCw, Send, Check } from "lucide-react";
import {
  api,
  type WorkItem,
  type WorkLogDraft,
  type TimelineSession,
} from "../../api/hindsight";
import { useCategories } from "../../state/categories";
import styles from "./WorkLogPage.module.css";

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface DayActivity {
  id: string;
  appName: string;
  title: string;
  startMs: number;
  endMs: number;
  superCategory: string;
}

interface MappingState {
  workLogText: string;
  startedAt: string; // local datetime-local value: "YYYY-MM-DDTHH:MM"
  endedAt: string;
  status: "idle" | "registering" | "registered" | "error";
  error?: string;
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

function toIso(dt: string): string {
  if (!dt) return new Date().toISOString();
  const d = new Date(dt);
  return Number.isNaN(d.getTime()) ? new Date().toISOString() : d.toISOString();
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}

function keywordMatch(itemSummary: string, activities: DayActivity[]): DayActivity[] {
  const keywords = itemSummary
    .toLowerCase()
    .split(/[\s\-_/,.;:()[\]{}]+/)
    .filter((w) => w.length >= 3);

  if (keywords.length === 0) return [];

  return activities.filter((act) => {
    const haystack = `${act.appName} ${act.title}`.toLowerCase();
    return keywords.some((kw) => haystack.includes(kw));
  });
}

function generateWorkLog(item: WorkItem, matched: DayActivity[]): string {
  if (matched.length === 0) return "";
  const sorted = [...matched].sort((a, b) => a.startMs - b.startMs);
  const lines = sorted.map((act) => {
    const start = formatTime(act.startMs);
    const end = formatTime(act.endMs);
    return `  - ${act.appName}: ${act.title || "(no title)"} (${start}~${end})`;
  });
  return `${item.summary}\n\n수행 업무:\n${lines.join("\n")}`;
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

  /* ---- Derive DayActivity[] from sessions + category names ---- */
  const dayActivities = useMemo<DayActivity[]>(() => {
    return sessions.map((s, i) => {
      const cat = getCategory(s.categoryId);
      return {
        id: `act-${i}`,
        appName: cat?.name ?? s.categoryId,
        title: "",
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
          status: "idle",
        };
      }
      setMappings(next);
    } catch (err) {
      console.error("Failed to fetch work items", err);
    } finally {
      setFetching(false);
    }
  }, [dayActivities]);

  /* ---- Register a single work item ---- */
  const handleRegister = useCallback(
    async (item: WorkItem) => {
      const mapping = mappings[item.key];
      if (!mapping) return;

      setMappings((prev) => ({
        ...prev,
        [item.key]: { ...prev[item.key], status: "registering" },
      }));

      try {
        const draft: WorkLogDraft = {
          workItemKey: item.key,
          summary: mapping.workLogText,
          startedAt: toIso(mapping.startedAt),
          endedAt: toIso(mapping.endedAt),
        };
        const result = await api.registerWorkLog(draft);
        setMappings((prev) => ({
          ...prev,
          [item.key]: {
            ...prev[item.key],
            status: result.success ? "registered" : "error",
            error: result.success ? undefined : result.message,
          },
        }));
      } catch (err) {
        setMappings((prev) => ({
          ...prev,
          [item.key]: {
            ...prev[item.key],
            status: "error",
            error: err instanceof Error ? err.message : String(err),
          },
        }));
      }
    },
    [mappings],
  );

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

  /* ---- Derived states ---- */
  const allRegistered =
    workItems.length > 0 &&
    workItems.every((item) => mappings[item.key]?.status === "registered");

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

      {/* All registered banner */}
      {allRegistered && (
        <div className={styles.successBanner}>
          <Check size={16} />
          {t("workLog.allItemsRegistered")}
        </div>
      )}

      {/* Work item cards */}
      {workItems.map((item) => {
        const mapping = mappings[item.key];
        const matched = matchedByItem.get(item.key) ?? [];
        const isRegistered = mapping?.status === "registered";

        return (
          <div
            key={item.key}
            className={`${styles.itemCard} ${isRegistered ? styles.itemCardRegistered : ""}`}
          >
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

            {/* Error message */}
            {mapping?.status === "error" && mapping.error && (
              <div className={styles.errorText}>{mapping.error}</div>
            )}

            {/* Register button */}
            <button
              className={`${styles.registerButton} ${isRegistered ? styles.registeredButton : ""}`}
              onClick={() => handleRegister(item)}
              disabled={isRegistered || mapping?.status === "registering"}
            >
              {mapping?.status === "registering" ? (
                t("workLog.registering")
              ) : isRegistered ? (
                <>
                  <Check size={14} />
                  {t("workLog.registered")}
                </>
              ) : (
                <>
                  <Send size={14} />
                  {t("workLog.register")}
                </>
              )}
            </button>
          </div>
        );
      })}
    </div>
  );
}
