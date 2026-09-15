// i18next 实例初始化
// - 已显式选择过语言：用 localStorage 持久化的值（key=hindsight.locale）
// - 首启未选择：渲染前 ensureInitialLocale() 按系统 locale 选语言（zh/ja→对应，其余→en）
// - 不引入 backend / detector 子包，资源直接静态 import
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { locale as osLocale } from "@tauri-apps/plugin-os";
import en from "./locales/en.json";
import ko from "./locales/ko.json";

export const LOCALE_STORAGE_KEY = "hindsight.locale";
export const FALLBACK_LOCALE = "en";

type Supported = "en" | "ko";

function mapToSupported(loc: string | null | undefined): Supported {
  const l = (loc ?? "").toLowerCase();
  if (l.startsWith("ko")) return "ko";
  return "en";
}

const stored = localStorage.getItem(LOCALE_STORAGE_KEY);

void i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    ko: { translation: ko },
  },
  supportedLngs: ["en", "ko"],
  lng: stored ?? FALLBACK_LOCALE,
  fallbackLng: FALLBACK_LOCALE,
  interpolation: {
    escapeValue: false,
  },
});

/**
 * 首启（未显式选择过语言）按系统 locale 设初始语言。必须在 React 首次渲染前 await，
 * 避免闪一下兜底语言。已有显式选择则原样尊重；自动识别**不写** localStorage——
 * 这样系统语言变了下次还能继续跟随。
 */
export async function ensureInitialLocale(): Promise<void> {
  if (localStorage.getItem(LOCALE_STORAGE_KEY)) return;
  let sys: string | null = null;
  try {
    sys = await osLocale();
  } catch {
    sys = null;
  }
  const target = mapToSupported(sys);
  if (i18n.language !== target) {
    await i18n.changeLanguage(target);
  }
}

export default i18n;
