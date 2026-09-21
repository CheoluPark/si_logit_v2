import { useTranslation } from "react-i18next";
import { KeyRound, Link } from "lucide-react";
import { Section } from "../../../components/FormLayout/Section";
import { Row } from "../../../components/FormLayout/Row";
import { useAiSettings } from "../shared/useAiSettings";
import styles from "../AISettings.module.css";
import localStyles from "./JiraMcpTab.module.css";
import { useCallback, useState } from "react";

export default function JiraMcpTab() {
  const { t } = useTranslation();
  const { ai, updateAi } = useAiSettings();
  const [showPat, setShowPat] = useState(false);

  const jira = ai?.jiraMcp;

  const updateUrl = useCallback(
    (v: string) => {
      if (!ai) return;
      updateAi({
        jiraMcp: { url: v, pat: jira?.pat ?? "" },
      });
    },
    [ai, jira, updateAi],
  );

  const updatePat = useCallback(
    (v: string) => {
      if (!ai) return;
      updateAi({
        jiraMcp: { url: jira?.url ?? "", pat: v },
      });
    },
    [ai, jira, updateAi],
  );

  const updateWorklogPrompt = useCallback(
    (value: string) => {
      updateAi({ jiraWorklogPrompt: Array.from(value).slice(0, 1000).join("") });
    },
    [updateAi],
  );

  if (!ai) return null;

  return (
    <div className={styles.content}>
      <Section
        title={t("aiSettings.jiraMcp.title")}
        icon={Link}
        description={t("aiSettings.jiraMcp.description")}
      >
        {/* Server URL */}
        <div className={localStyles.wideField}>
          <Row label={t("aiSettings.jiraMcp.urlLabel")}>
            <input
              className={styles.externalInput}
              type="url"
              value={jira?.url ?? ""}
              onChange={(e) => updateUrl(e.target.value)}
              placeholder={t("aiSettings.jiraMcp.urlPlaceholder")}
              spellCheck={false}
              autoComplete="url"
            />
          </Row>
        </div>

        {/* Personal Access Token */}
        <div className={localStyles.wideField}>
          <Row label={t("aiSettings.jiraMcp.patLabel")}>
            <div className={localStyles.patField}>
              <input
                className={styles.externalInput}
                type={showPat ? "text" : "password"}
                value={jira?.pat ?? ""}
                onChange={(e) => updatePat(e.target.value)}
                placeholder={t("aiSettings.jiraMcp.patPlaceholder")}
                spellCheck={false}
                autoComplete="off"
              />
              <button
                type="button"
                className={localStyles.patToggle}
                onClick={() => setShowPat((p) => !p)}
                aria-label={showPat ? "Hide token" : "Show token"}
                tabIndex={-1}
              >
                <KeyRound size={14} strokeWidth={2} />
              </button>
            </div>
          </Row>
        </div>

        <Row
          label={t("aiSettings.jiraMcp.worklogPromptLabel")}
          description={t("aiSettings.jiraMcp.worklogPromptDescription")}
          block
        >
          <div className={localStyles.worklogPrompt}>
            <textarea
              className={styles.textarea}
              value={ai.jiraWorklogPrompt ?? ""}
              onChange={(e) => updateWorklogPrompt(e.target.value)}
              placeholder={t("aiSettings.jiraMcp.worklogPromptPlaceholder")}
              rows={6}
            />
            <span className={localStyles.characterCount} aria-live="polite">
              {t("aiSettings.jiraMcp.characterCount", {
                count: Array.from(ai.jiraWorklogPrompt ?? "").length,
              })}
            </span>
          </div>
        </Row>

      </Section>
    </div>
  );
}
