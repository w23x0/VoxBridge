import { useEffect, useState } from "react";

export function AboutPage() {
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
          <span>产品</span>
          <span className="num num-muted">实时语音翻译</span>
        </div>
      </div>
    </div>
  );
}
