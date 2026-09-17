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
        jiraMcp: { ...jira, url: v, pat: jira?.pat ?? "", transport: jira?.transport ?? "remote" },
      });
    },
    [ai, jira, updateAi],
  );

  const updatePat = useCallback(
    (v: string) => {
      if (!ai) return;
      updateAi({
        jiraMcp: { ...jira, url: jira?.url ?? "", pat: v, transport: jira?.transport ?? "remote" },
      });
    },
    [ai, jira, updateAi],
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

        {/* Personal Access Token */}
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


      </Section>
    </div>
  );
}
