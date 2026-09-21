import { describe, expect, it } from "vitest";
import {
  type DayActivity,
  keywordMatch,
  generateWorkLog,
} from "./workLogMatch";
import type { WorkItem, TimelineSession } from "../api/hindsight";

// ── 2026-09-16 실제 활동 데이터 (hindsight.sqlite에서 추출) ──────────
// startMs/endMs는 실제 DB의 started_at/ended_at (KST) 기준
const T0 = new Date("2026-09-16T08:53:26+09:00").getTime();

// TimelineSession 형태의 원본 데이터 (DB에서 오는 실제 형태)
const rawSessions: TimelineSession[] = [
  { startedAt: new Date(T0).toISOString(), endedAt: new Date(T0 + 45_000).toISOString(), categoryId: "dev", processName: "opencode", windowTitle: "OpenCode" },
  { startedAt: new Date(T0 + 45_000).toISOString(), endedAt: new Date(T0 + 75_000).toISOString(), categoryId: "dev", processName: "opencode", windowTitle: "OpenCode" },
  { startedAt: new Date(T0 + 95_000).toISOString(), endedAt: new Date(T0 + 115_000).toISOString(), categoryId: "dev", processName: "opencode", windowTitle: "OpenCode" },
  { startedAt: new Date(T0 + 115_000).toISOString(), endedAt: new Date(T0 + 120_000).toISOString(), categoryId: "comm", processName: "brave", windowTitle: "CheoluPark/si_logit_v2 - Brave" },
  { startedAt: new Date(T0 + 120_000).toISOString(), endedAt: new Date(T0 + 125_000).toISOString(), categoryId: "comm", processName: "brave", windowTitle: "Private New Tab - Brave" },
  { startedAt: new Date(T0 + 7_471_000).toISOString(), endedAt: new Date(T0 + 7_496_000).toISOString(), categoryId: "util", processName: "Everything", windowTitle: "Everything" },
  { startedAt: new Date(T0 + 7_506_000).toISOString(), endedAt: new Date(T0 + 7_537_000).toISOString(), categoryId: "comm", processName: "brave", windowTitle: "CheoluPark/si_logit_v2 - Brave" },
  { startedAt: new Date(T0 + 7_542_000).toISOString(), endedAt: new Date(T0 + 7_547_000).toISOString(), categoryId: "dev", processName: "opencode", windowTitle: "OpenCode" },
  { startedAt: new Date(T0 + 7_547_000).toISOString(), endedAt: new Date(T0 + 7_567_000).toISOString(), categoryId: "dev", processName: "code", windowTitle: "README.md - Hindsight - Visual Studio Code" },
  { startedAt: new Date(T0 + 7_567_000).toISOString(), endedAt: new Date(T0 + 7_582_000).toISOString(), categoryId: "dev", processName: "opencode", windowTitle: "OpenCode" },
  { startedAt: new Date(T0 + 7_587_000).toISOString(), endedAt: new Date(T0 + 7_601_000).toISOString(), categoryId: "dev", processName: "code", windowTitle: "DEVELOPMENT.md - Hindsight - Visual Studio Code" },
  { startedAt: new Date(T0 + 20_340_000).toISOString(), endedAt: new Date(T0 + 20_350_000).toISOString(), categoryId: "dev", processName: "code", windowTitle: "Preview DEVELOPMENT.md - Hindsight - Visual Studio Code" },
  { startedAt: new Date(T0 + 20_350_000).toISOString(), endedAt: new Date(T0 + 20_355_000).toISOString(), categoryId: "comm", processName: "brave", windowTitle: "CheoluPark/si_logit_v2 - Brave" },
];

// WorkLogPage.tsx와 동일한 변환 로직: TimelineSession → DayActivity
// appName: processName || categoryName || categoryId, title: windowTitle
function toDayActivities(sessions: TimelineSession[]): DayActivity[] {
  return sessions.map((s, i) => ({
    id: `act-${i}`,
    appName: s.processName || s.categoryId,
    title: s.windowTitle || "",
    startMs: new Date(s.startedAt).getTime(),
    endMs: new Date(s.endedAt).getTime(),
    superCategory: s.categoryId,
  }));
}

const todayActivities = toDayActivities(rawSessions);

const localFixtureItem: WorkItem = {
  key: "TEST-LOCAL-1",
  summary: "OpenCode 및 Hindsight 한국어 AI 요약·채팅 도구 라우팅 개선",
  status: "TEST FIXTURE",
  assignee: "local",
  issueType: "Work Item",
  background: "한국어 AI 요약 및 채팅 도구 작업 흐름을 확인합니다.",
  info: "오늘 수집된 활동으로 Work Log 매칭을 검증합니다.",
  objective: "MCP 없이 기존 추천 Work Log 흐름을 점검합니다.",
  output: "로컬 매칭 및 Work Log 검증 결과",
};

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
  it("로컬 fixture: OpenCode 앱과 Hindsight 제목을 매칭하고 Work Log를 추천", () => {
    const matched = keywordMatch(localFixtureItem.summary, todayActivities);
    const text = generateWorkLog(localFixtureItem, matched);

    expect(matched.some((a) => a.appName === "opencode")).toBe(true);
    expect(matched.some((a) => a.title.includes("Hindsight"))).toBe(true);
    expect(text).toContain(localFixtureItem.summary);
    expect(text).toMatch(/총 \d+(?:시간(?: \d+분)?|분) 수행/);
  });

  it("MOCK-101: OpenCode 활동과 매칭", () => {
    const matched = keywordMatch(mockItems[0].summary, todayActivities);
    expect(matched.length).toBeGreaterThan(0);
    // "logit"이 si_logit_v2 브라우저 탭에도 부분 매칭될 수 있음 — OpenCode 활동은 반드시 포함
    expect(matched.some((a) => a.appName === "opencode")).toBe(true);
  });

  it("MOCK-102: README/DEVELOPMENT 문서 편집 활동과 매칭", () => {
    const matched = keywordMatch(mockItems[1].summary, todayActivities);
    expect(matched.length).toBeGreaterThan(0);
    expect(matched.every((a) => a.appName === "code")).toBe(true);
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

describe("TimelineSession → DayActivity 변환 후 매칭", () => {
  it("processName/windowTitle이 빈 문자열이면 categoryId로 폴백", () => {
    const fallback: TimelineSession[] = [
      { startedAt: new Date(T0).toISOString(), endedAt: new Date(T0 + 1000).toISOString(), categoryId: "dev", processName: "", windowTitle: "" },
    ];
    const acts = toDayActivities(fallback);
    expect(acts[0].appName).toBe("dev");
    expect(acts[0].title).toBe("");
  });

  it("windowTitle이 있으면 keywordMatch에서 윈도우 제목으로 매칭", () => {
    // windowTitle에 "si_logit_v2" 포함된 세션 → "si_logit" 키워드로 매칭
    const matched = keywordMatch("si_logit 프로젝트 작업", todayActivities);
    expect(matched.length).toBeGreaterThan(0);
    expect(matched.every((a) => a.title.includes("si_logit_v2"))).toBe(true);
  });

  it("processName이 있으면 keywordMatch에서 appName으로 매칭", () => {
    // "Everything" 프로세스명으로 매칭
    const matched = keywordMatch("Everything 유틸리티 사용", todayActivities);
    expect(matched.length).toBeGreaterThan(0);
    expect(matched.some((a) => a.appName === "Everything")).toBe(true);
  });

  it("processName이 약어일 때도 매칭 (brave → Brave Browser 탭)", () => {
    const matched = keywordMatch("Brave 브라우저 사용", todayActivities);
    // "brave" processName이 appName으로 설정됨 → 키워드 매칭 확인
    expect(matched.some((a) => a.appName === "brave")).toBe(true);
  });
});

describe("generateWorkLog", () => {
  it("매칭 활동으로 워크로그 텍스트 생성", () => {
    const matched = keywordMatch(mockItems[1].summary, todayActivities);
    const text = generateWorkLog(mockItems[1], matched);

    expect(text).toContain(mockItems[1].summary);
    expect(text).toContain("Visual Studio Code");
    // 10분 단위 시간 표시 (총 수행 시간)
    expect(text).toMatch(/총 \d+분/);
  });

  it("매칭 없으면 빈 문자열", () => {
    expect(generateWorkLog(mockItems[0], [])).toBe("");
  });

  it("각 도구별 수행 시간 표시", () => {
    const matched = keywordMatch(mockItems[1].summary, todayActivities);
    const text = generateWorkLog(mockItems[1], matched);

    expect(text).toContain("Visual Studio Code:");
    expect(text).toMatch(/- Visual Studio Code: \d+분/);
  });

  it("총 수행 시간이 도구별 시간 합과 일치", () => {
    // 현실적인 시간 데이터 (분 단위)로 직접 구성
    const realistic: DayActivity[] = [
      { id: "a1", appName: "code", title: "README.md - Hindsight - Visual Studio Code", startMs: T0, endMs: T0 + 25 * 60_000, superCategory: "dev" },
      { id: "a2", appName: "code", title: "DEVELOPMENT.md - Hindsight - Visual Studio Code", startMs: T0 + 30 * 60_000, endMs: T0 + 55 * 60_000, superCategory: "dev" },
    ];
    const text = generateWorkLog(mockItems[1], realistic);

    const totalMatch = text.match(/총 (\d+)분 수행/);
    expect(totalMatch).not.toBeNull();
    expect(Number(totalMatch![1])).toBeGreaterThan(0);
    // 25 + 25 = 50분 → 10분 단위 반올림 시 50분
    expect(totalMatch![1]).toBe("50");
  });
});
