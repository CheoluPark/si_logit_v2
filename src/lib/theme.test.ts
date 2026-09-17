// node 环境手工桩:theme.ts 只用到 localStorage 与 documentElement.dataset,
// 不必为此挂 jsdom(vitest 配置保持纯函数策略)。
import { beforeEach, describe, expect, it, vi } from "vitest";

const store = new Map<string, string>();
vi.stubGlobal("localStorage", {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
  removeItem: (k: string) => void store.delete(k),
});
vi.stubGlobal("document", { documentElement: { dataset: {} as Record<string, string> } });

import {
  applyTheme,
  getCurrentTheme,
  getStoredTheme,
  setStoredTheme,
  subscribeTheme,
} from "./theme";

describe("theme", () => {
  beforeEach(() => {
    store.clear();
    (document.documentElement.dataset as Record<string, string>).theme = "";
  });

  it("未存 / 非法值回退 default", () => {
    expect(getStoredTheme()).toBe("default");
    store.set("hindsight.theme", "neon");
    expect(getStoredTheme()).toBe("default");
  });
  it("setStoredTheme 持久化 + 应用到 dataset + 通知订阅者", () => {
    const seen: string[] = [];
    const off = subscribeTheme(() => seen.push(getCurrentTheme()));
    setStoredTheme("dark");
    expect(store.get("hindsight.theme")).toBe("dark");
    expect(getCurrentTheme()).toBe("dark");
    expect(seen).toEqual(["dark"]);
    off();
    setStoredTheme("default");
    expect(seen).toEqual(["dark"]); // 退订后不再通知
  });
  it("applyTheme 只改 dataset;getCurrentTheme 对非法值回退 default", () => {
    applyTheme("minimal");
    expect(getCurrentTheme()).toBe("minimal");
    (document.documentElement.dataset as Record<string, string>).theme = "junk";
    expect(getCurrentTheme()).toBe("default");
  });
});
