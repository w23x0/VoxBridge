import { useEffect, useState } from "react";
import { useT } from "../i18n/context";

export function AboutPage() {
  const t = useT();
  const [version, setVersion] = useState("0.1.0");

  useEffect(() => {
    void import("@tauri-apps/api/app")
      .then(({ getVersion }) => getVersion())
      .then(setVersion)
      .catch(() => undefined);
  }, []);

  return (
    <div className="panel">
      <div className="panel-top">
        <div className="panel-title">VoxBridge</div>
        <span className="badge badge-running">v{version}</span>
      </div>
      <div className="panel-body">
        <div className="sub-row">
          <span>{t("about.product")}</span>
          <span className="num num-muted">{t("about.productValue")}</span>
        </div>
      </div>
    </div>
  );
}
