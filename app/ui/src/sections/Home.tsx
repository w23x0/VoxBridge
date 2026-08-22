/**
 * 首页：两个常驻功能的启停、听取目标和运行时状态。
 *
 * 单卡的配置与启停全部在 PipelineCard 里；这里只剩两张卡的 .map 骨架
 * 和底部的延迟面板 / 耳机提示。
 */

import { LatencyPanel } from "../components/Latency";
import { useLang } from "../i18n/context";
import { useStore } from "../store";
import { CARDS, PipelineCard } from "./PipelineCard";

export function HomePage() {
  const { t } = useLang();
  const { api, snapshot, settings } = useStore();
  const speak = settings.speak;
  const listen = settings.listen;

  return (
    <>
      <div className="stats-row cols-2">
        {CARDS.map((card) => (
          <PipelineCard key={card.id} pipeline={card.id} />
        ))}
      </div>

      <div className="panel">
        <div className="panel-top">
          <div className="panel-title">{t("home.panelTitle")}</div>
          {api.mock ? <span className="badge badge-warn">{t("home.demoBadge")}</span> : null}
        </div>
        <div className="panel-body">
          <div className="stats-row cols-2">
            <LatencyPanel
              label={t("pipeline.speak")}
              snapshot={snapshot?.speak ?? null}
              showTranslation={speak.show_translation}
              speakTranslation
            />
            <LatencyPanel
              label={t("pipeline.listen")}
              snapshot={snapshot?.listen ?? null}
              showTranslation={listen.show_translation}
              speakTranslation={listen.speak_translation}
            />
          </div>

          {snapshot?.headphones_advised ? (
            <div className="hint hint-warn" style={{ marginTop: 12 }}>
              {t("home.headphonesHint")}
            </div>
          ) : null}
        </div>
      </div>
    </>
  );
}
