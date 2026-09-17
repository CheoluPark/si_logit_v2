import type { WorkItem } from "../api/hindsight";

export interface DayActivity {
  id: string;
  appName: string;
  title: string;
  startMs: number;
  endMs: number;
  superCategory: string;
}

export function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function keywordMatch(
  itemSummary: string,
  activities: DayActivity[],
): DayActivity[] {
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

interface MergedRange {
  startMs: number;
  endMs: number;
  title: string;
}

function mergeRanges(activities: DayActivity[], gapMs: number): MergedRange[] {
  const sorted = [...activities].sort((a, b) => a.startMs - b.startMs);
  const ranges: MergedRange[] = [];

  for (const act of sorted) {
    const last = ranges[ranges.length - 1];
    // Same title and overlapping/near-consecutive → merge
    if (last && last.title === act.title && act.startMs - last.endMs <= gapMs) {
      last.endMs = Math.max(last.endMs, act.endMs);
    } else {
      ranges.push({ startMs: act.startMs, endMs: act.endMs, title: act.title });
    }
  }
  return ranges;
}

/** 밀리초를 10분 단위로 반올림하여 한국어 시간 문자열로 변환 (예: "10분", "30분", "1시간", "1시간 30분") */
export function formatDuration(totalMs: number): string {
  const TEN_MIN = 10 * 60 * 1000;
  const roundedMin = Math.round(totalMs / TEN_MIN) * 10;
  const hours = Math.floor(roundedMin / 60);
  const minutes = roundedMin % 60;
  if (hours === 0) return `${minutes}분`;
  if (minutes === 0) return `${hours}시간`;
  return `${hours}시간 ${minutes}분`;
}

export function generateWorkLog(
  item: WorkItem,
  matched: DayActivity[],
): string {
  if (matched.length === 0) return "";

  const sorted = [...matched].sort((a, b) => a.startMs - b.startMs);
  const gapMs = 2 * 60 * 1000; // 2 minutes

  // Group by appName
  const byApp = new Map<string, DayActivity[]>();
  for (const act of sorted) {
    const key = act.appName;
    const list = byApp.get(key) || [];
    list.push(act);
    byApp.set(key, list);
  }

  // For each app group, merge consecutive/same-title ranges and sum duration
  const groups: { app: string; totalMs: number }[] = [];
  for (const [app, acts] of byApp) {
    const ranges = mergeRanges(acts, gapMs);
    const totalMs = ranges.reduce((sum, r) => sum + (r.endMs - r.startMs), 0);
    groups.push({ app, totalMs });
  }

  // Sort groups by total duration descending (가장 많이 작업한 도구 우선)
  groups.sort((a, b) => b.totalMs - a.totalMs);

  // 총 수행 시간 계산
  const grandTotal = groups.reduce((s, g) => s + g.totalMs, 0);

  // Bullet lines: 각 도구별 수행 시간
  const lines = groups.map(({ app, totalMs }) => {
    const appLabel = appNameLabel(app);
    return `- ${appLabel}: ${formatDuration(totalMs)}`;
  });

  return `${item.summary}\n\n총 ${formatDuration(grandTotal)} 수행\n\n${lines.join("\n")}`;
}

function appNameLabel(app: string): string {
  switch (app) {
    case "opencode":
      return "OpenCode";
    case "code":
      return "Visual Studio Code";
    case "brave":
      return "Brave 브라우저";
    case "Everything":
      return "Everything";
    default:
      return app;
  }
}