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

export function generateWorkLog(
  item: WorkItem,
  matched: DayActivity[],
): string {
  if (matched.length === 0) return "";
  const sorted = [...matched].sort((a, b) => a.startMs - b.startMs);
  const lines = sorted.map((act) => {
    const start = formatTime(act.startMs);
    const end = formatTime(act.endMs);
    return `  - ${act.appName}: ${act.title || "(no title)"} (${start}~${end})`;
  });
  return `${item.summary}\n\n수행 업무:\n${lines.join("\n")}`;
}