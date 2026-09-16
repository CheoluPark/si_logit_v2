import { describe, expect, it } from "vitest";
import {
  type DayActivity,
  keywordMatch,
  generateWorkLog,
} from "./workLogMatch";
import type { WorkItem } from "../api/hindsight";

// ── 2026-09-16 실제 활동 데이터 (hindsight.sqlite에서 추출) ──────────
// startMs/endMs는 실제 DB의 started_at/ended_at (KST) 기준
const T0 = new Date("2026-09-16T08:53:26+09:00").getTime();

const todayActivities: DayActivity[] = [
  { id: "a1", appName: "OpenCode", title: "OpenCode", startMs: T0, endMs: T0 + 45_000, superCategory: "dev" },
  { id: "a2", appName: "OpenCode", title: "OpenCode", startMs: T0 + 45_000, endMs: T0 + 75_000, superCategory: "dev" },
  { id: "a3", appName: "OpenCode", title: "OpenCode", startMs: T0 + 95_000, endMs: T0 + 115_000, superCategory: "dev" },
  { id: "a4", appName: "Brave Browser", title: "CheoluPark/si_logit_v2 - Brave", startMs: T0 + 115_000, endMs: T0 + 120_000, superCategory: "comm" },
  { id: "a5", appName: "Brave Browser", title: "Private New Tab - Brave", startMs: T0 + 120_000, endMs: T0 + 125_000, superCategory: "comm" },
  { id: "a6", appName: "Everything", title: "Everything", startMs: T0 + 7_471_000, endMs: T0 + 7_496_000, superCategory: "util" },
  { id: "a7", appName: "Brave Browser", title: "CheoluPark/si_logit_v2 - Brave", startMs: T0 + 7_506_000, endMs: T0 + 7_537_000, superCategory: "comm" },
  { id: "a8", appName: "OpenCode", title: "OpenCode", startMs: T0 + 7_542_000, endMs: T0 + 7_547_000, superCategory: "dev" },
  { id: "a9", appName: "Visual Studio Code", title: "README.md - Hindsight - Visual Studio Code", startMs: T0 + 7_547_000, endMs: T0 + 7_567_000, superCategory: "dev" },
  { id: "a10", appName: "OpenCode", title: "OpenCode", startMs: T0 + 7_567_000, endMs: T0 + 7_582_000, superCategory: "dev" },
  { id: "a11", appName: "Visual Studio Code", title: "DEVELOPMENT.md - Hindsight - Visual Studio Code", startMs: T0 + 7_587_000, endMs: T0 + 7_601_000, superCategory: "dev" },
  { id: "a12", appName: "Visual Studio Code", title: "Preview DEVELOPMENT.md - Hindsight - Visual Studio Code", startMs: T0 + 20_340_000, endMs: T0 + 20_350_000, superCategory: "dev" },
  { id: "a13", appName: "Brave Browser", title: "CheoluPark/si_logit_v2 - Brave", startMs: T0 + 20_350_000, endMs: T0 + 20_355_000, superCategory: "comm" },
];

// ── mock_work_items() (worklog.rs)와 동일한 테스트 Work Item ─────────
const mockItems: WorkItem[] = [
  {
    key: "MOCK-101",
    summary: "SI Logit 프로젝트 개발 및 OpenCode 작업",
    status: "In Progress",
    assignee: "me",
    issueType: "Work Item",
    background: "사내 폐쇄망 배포 전환에 따른 개발 작업",
    info: "OpenCode에서 작업 수행",
    objective: "SI Logit 기능 개발 완료",
    output: "개발 완료된 코드",
  },
  {
    key: "MOCK-102",
    summary: "README 및 DEVELOPMENT 문서 작성",
    status: "In Progress",
    assignee: "me",
    issueType: "Work Item",
    background: "프로젝트 문서화 필요",
    info: "Visual Studio Code에서 문서 편집",
    objective: "개발 가이드 문서 완성",
    output: "README.md, DEVELOPMENT.md",
  },
  {
    key: "MOCK-103",
    summary: "GitHub 저장소 push 및 si_logit_v2 관리",
    status: "In Progress",
    assignee: "me",
    issueType: "Work Item",
    background: "코드 원격 저장소 반영 필요",
    info: "GitHub 저장소 확인",
    objective: "최신 코드 push 완료",
    output: "si_logit_v2 저장소 최신화",
  },
];

describe("keywordMatch (실제 오늘 활동 데이터 기반)", () => {
  it("MOCK-101: OpenCode 활동과 매칭", () => {
    const matched = keywordMatch(mockItems[0].summary, todayActivities);
    expect(matched.length).toBeGreaterThan(0);
    // "logit"이 si_logit_v2 브라우저 탭에도 부분 매칭될 수 있음 — OpenCode 활동은 반드시 포함
    expect(matched.some((a) => a.appName === "OpenCode")).toBe(true);
  });

  it("MOCK-102: README/DEVELOPMENT 문서 편집 활동과 매칭", () => {
    const matched = keywordMatch(mockItems[1].summary, todayActivities);
    expect(matched.length).toBeGreaterThan(0);
    expect(matched.every((a) => a.appName === "Visual Studio Code")).toBe(true);
    expect(matched.some((a) => a.title.includes("README.md"))).toBe(true);
    expect(matched.some((a) => a.title.includes("DEVELOPMENT.md"))).toBe(true);
  });

  it("MOCK-103: si_logit_v2 GitHub 활동과 매칭", () => {
    const matched = keywordMatch(mockItems[2].summary, todayActivities);
    expect(matched.length).toBeGreaterThan(0);
    expect(matched.every((a) => a.title.includes("si_logit_v2"))).toBe(true);
  });

  it("매칭 없는 요약은 빈 결과", () => {
    const matched = keywordMatch("완전히 다른 주제의 작업", todayActivities);
    expect(matched).toEqual([]);
  });
});

describe("generateWorkLog", () => {
  it("매칭 활동으로 워크로그 텍스트 생성", () => {
    const matched = keywordMatch(mockItems[1].summary, todayActivities);
    const text = generateWorkLog(mockItems[1], matched);

    expect(text).toContain(mockItems[1].summary);
    expect(text).toContain("수행 업무:");
    expect(text).toContain("Visual Studio Code");
    expect(text).toContain("README.md");
    expect(text).toContain("DEVELOPMENT.md");
  });

  it("매칭 없으면 빈 문자열", () => {
    expect(generateWorkLog(mockItems[0], [])).toBe("");
  });

  it("시간순 정렬", () => {
    const matched = keywordMatch(mockItems[1].summary, todayActivities);
    const text = generateWorkLog(mockItems[1], matched);
    const readmeIdx = text.indexOf("README.md");
    const devIdx = text.indexOf("DEVELOPMENT.md");
    expect(readmeIdx).toBeGreaterThan(-1);
    expect(devIdx).toBeGreaterThan(readmeIdx);
  });
});