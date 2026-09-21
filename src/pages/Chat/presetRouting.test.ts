import { describe, expect, it, vi } from "vitest";
import { Clock, Mail, Search } from "lucide-react";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { buildPresets } from "./ChatView";

const t = ((key: string) => key) as never;

describe("chat example preset routing", () => {
  it("maps all eight example cards to their deterministic backend IDs", () => {
    const fixed = buildPresets(t, { key: "mail", id: "top_app_today", icon: Mail });
    const titles = buildPresets(t, { key: "searchkw", id: "titles_today", icon: Search });
    const peak = buildPresets(t, { key: "hours", id: "peak_hour", icon: Clock });

    expect(fixed.map((preset) => preset.id)).toEqual([
      "today", "confluence", "jira_duration", "week_category", "trend_14d", "top_app_today",
    ]);
    expect(titles[titles.length - 1]?.id).toBe("titles_today");
    expect(peak[peak.length - 1]?.id).toBe("peak_hour");
  });
});
