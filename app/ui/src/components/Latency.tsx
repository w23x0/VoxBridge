/**
 * 延迟统计面板：只显示时间和计数——没有音频、没有字幕内容。
 *
 * 结构分三组，按「要不要实时盯着」来分：
 *   · 实时 —— 一条「话到话」的分段横条，每段宽度按耗时成比例，
 *     颜色按本段阈值（绿=快 / 琥珀=偏慢 / 红=慢）。
 *     起点是服务端确认说话（speech_started），不是按键或采集起点。
 *     根据「显示译文」「播放译音」开关状态决定测量哪些段：
 *       - 播放译音 ✓：到首字 + 到首声 + 播放
 *       - 播放译音 ✗ 但显示译文 ✓：只测到首字
 *       - 两个都 ✗：不测量（横条不显示）
 *   · 健康度 —— 丢块计数（非零=处理跟不上采集，红了）。
 *   · 一次性 —— 冷启动的连接与会话就绪，启动后不再变，弱化放在最底。
 *
 * 视觉全部沿用 tokens.css / components.css 里已有的 class（sub-card、
 * sub-row、num 系列、label-xs），不在这里另起配色。
 */

import type { LatencyMetric, LatencySnapshot, PipelineSnapshot } from "../types.snapshot";

function ms(value: number | null): string {
  return value === null ? "—" : `${value}ms`;
}

function lastOf(metric: LatencyMetric | null | undefined): number | null {
  return metric?.last_ms ?? null;
}

/** 输入排队多久算「要注意」。队列满 8 块 ≈160ms 就开始丢最旧的块，留点余量。 */
const INPUT_WARN_MS = 100;
/** send() 阻塞超过多少算网络背压。好连接下恒为 0。 */
const UPLOAD_WARN_MS = 50;

type Tone = "green" | "amber" | "red";

/** 数值补色：全部走 tokens/components 里已有的 num 系列类。 */
const TONE_TEXT: Record<Tone, string> = { green: "num-green", amber: "num-amber", red: "num-red" };
/** 分段条/色块直接吃设计令牌的语义色。 */
const TONE_BG: Record<Tone, string> = {
  green: "var(--success)",
  amber: "var(--warn)",
  red: "var(--danger)",
};

/** 一段的定义：[标签, 绿色上限, 琥珀上限]；超过琥珀上限就是红。 */
interface SegmentDef {
  label: string;
  hint: string;
  good: number;
  warn: number;
}

const SEGMENT_DEFS: SegmentDef[] = [
  {
    label: "到首字",
    hint: "首个译文",
    good: 800,
    warn: 1500,
  },
  {
    label: "到首声",
    hint: "首个译音",
    good: 1500,
    warn: 3000,
  },
  {
    label: "播放",
    hint: "播放译音",
    good: 100,
    warn: 300,
  },
];

function toneFor(valueMs: number, def: SegmentDef): Tone {
  if (valueMs <= def.good) return "green";
  if (valueMs <= def.warn) return "amber";
  return "red";
}

/** 话到话总时长的整体语境。 */
function overallTone(totalMs: number): Tone {
  if (totalMs <= 4000) return "green";
  if (totalMs <= 6000) return "amber";
  return "red";
}

interface Segment {
  def: SegmentDef;
  ms: number;
  tone: Tone;
  pct: number;
}

/**
 * 把各里程碑的累积耗时差分成分段。起点是服务端确认说话（speech_started），
 * 不是按键——所以所有累积时间都要减去 server_vad 的值。
 *
 * 根据开关状态决定测哪些段：
 * - 播放译音 ✓：到首字 + 到首声 + 播放
 * - 播放译音 ✗ 但显示译文 ✓：只到首字
 * - 两个都 ✗：不测（返回空）
 */
function segmentsFor(
  lat: LatencySnapshot | null,
  showTranslation: boolean,
  speakTranslation: boolean,
): { segments: Segment[]; totalMs: number | null; headline: string } {
  const empty = { segments: [], totalMs: null, headline: "话到话" };
  if (!lat) return empty;

  // 两个开关都关了，不测
  if (!showTranslation && !speakTranslation) return empty;

  const vadMs = lastOf(lat.server_vad);
  if (vadMs === null) return empty; // 还没有 VAD 回报，整条链都测不了

  // 所有累积时间都减去 VAD，得到从 speech_started 算起的相对时间
  const firstTextAbs = lastOf(lat.first_text);
  const firstAudioAbs = lastOf(lat.first_audio);
  const firstPlaybackAbs = lastOf(lat.first_playback);

  const firstTextRel = firstTextAbs !== null ? firstTextAbs - vadMs : null;
  const firstAudioRel = firstAudioAbs !== null ? firstAudioAbs - vadMs : null;
  const firstPlaybackRel = firstPlaybackAbs !== null ? firstPlaybackAbs - vadMs : null;

  // 只显示译文、不播放译音：只测到首字
  if (showTranslation && !speakTranslation) {
    if (firstTextRel === null) return empty;
    const seg: Segment = {
      def: SEGMENT_DEFS[0],
      ms: firstTextRel,
      tone: toneFor(firstTextRel, SEGMENT_DEFS[0]),
      pct: 100,
    };
    return {
      segments: [seg],
      totalMs: firstTextRel,
      headline: "话到字",
    };
  }

  // 播放译音 ✓：测完整链路（到首字 + 到首声 + 播放）
  // 端点取最远可达的里程碑
  const reachable = firstPlaybackRel ?? firstAudioRel ?? firstTextRel;
  if (reachable === null) return empty;

  const segments: Segment[] = [];
  const sums = [firstTextRel, firstAudioRel, firstPlaybackRel];
  let prev = 0;
  for (let i = 0; i < sums.length; i += 1) {
    const sum = sums[i];
    if (sum === null) break; // 链断了
    const delta = Math.max(0, sum - prev);
    prev = sum;
    if (delta <= 0) continue; // 同刻/乱序样本，这截没有可展示的耗时
    const def = SEGMENT_DEFS[i];
    segments.push({
      def,
      ms: delta,
      tone: toneFor(delta, def),
      pct: (delta / reachable) * 100,
    });
  }

  const headline = firstPlaybackRel !== null ? "话到话" : firstAudioRel !== null ? "话到声" : "话到字";
  return { segments, totalMs: reachable, headline };
}

export function LatencyPanel({
  snapshot,
  label,
  showTranslation,
  speakTranslation,
}: {
  snapshot: PipelineSnapshot | null;
  label: string;
  showTranslation: boolean;
  speakTranslation: boolean;
}) {
  const lat = snapshot?.latency ?? null;
  const noSample = lat === null || lat.completed_turns === 0;
  const { segments, totalMs, headline } = segmentsFor(lat, showTranslation, speakTranslation);

  const inputMs = lastOf(lat?.input_queue);
  const uploadMs = lastOf(lat?.upload_send);
  const inputBad = inputMs !== null && inputMs >= INPUT_WARN_MS;
  const uploadBad = uploadMs !== null && uploadMs >= UPLOAD_WARN_MS;
  const dropped = lat?.dropped_chunks ?? 0;

  // 两个开关都关了，不显示延迟统计
  if (!showTranslation && !speakTranslation) {
    return (
      <div className="sub-card" style={{ flex: "none" }}>
        <div className="sub-card-head">
          {label}
          <span className="num num-muted">关闭</span>
        </div>
      </div>
    );
  }

  return (
    <div className="sub-card" style={{ flex: "none" }}>
      <div className="sub-card-head">
        {label}
        <span className="num num-muted">{noSample ? "无数据" : `${lat.completed_turns} 轮`}</span>
      </div>

      {/* ---- 实时：话到话/话到字 + 分段横条 ---- */}
      <div className="sub-row" title="从说话确认到首个输出">
        <span>{headline}</span>
        <span className={totalMs === null ? "num num-muted" : `num ${TONE_TEXT[overallTone(totalMs)]}`}>
          {totalMs === null ? "—" : ms(totalMs)}
        </span>
      </div>

      <div
        className="lat-seg"
        role="img"
        aria-label="延迟分段"
      >
        {segments.map((seg) => (
          <span
            key={seg.def.label}
            className="lat-seg-seg"
            style={{ width: `${Math.max(seg.pct, 1)}%`, background: TONE_BG[seg.tone] }}
          />
        ))}
      </div>

      <div className="lat-legend">
        {segments.map((seg) => (
          <span className="lat-item" key={seg.def.label} title={seg.def.hint}>
            <span className="lat-swatch" style={{ background: TONE_BG[seg.tone] }} />
            <span>{seg.def.label}</span>
            <span className={`num ${TONE_TEXT[seg.tone]}`}>{seg.ms}ms</span>
          </span>
        ))}
        {segments.length === 0 ? <span className="hint">暂无数据</span> : null}
      </div>

      <div className="sub-row" title="整轮完成时间">
        <span>整轮</span>
        <span className="num num-muted">{ms(lastOf(lat?.turn_complete))}</span>
      </div>

      {/* ---- 健康度：丢块常驻，输入/上传只在异常时冒出来 ---- */}
      <div className="label-xs" style={{ margin: "10px 0 0" }}>健康度</div>
      {inputBad ? (
        <div
          className="sub-row"
          title="输入处理拥堵"
        >
          <span>输入排队</span>
          <span className="num num-red">{inputMs}ms</span>
        </div>
      ) : null}
      {uploadBad ? (
        <div
          className="sub-row"
          title="上传拥堵"
        >
          <span>上传阻塞</span>
          <span className="num num-red">{uploadMs}ms</span>
        </div>
      ) : null}
      <div className="sub-row" title="丢弃数 / 处理数">
        <span>丢块 / 处理</span>
        <span className={dropped > 0 ? "num num-red" : "num num-green"}>
          {noSample ? "—" : `${dropped} / ${lat?.processed_chunks ?? 0}`}
        </span>
      </div>

      {/* ---- 冷启动连接 ---- */}
      <div className="sub-row" title="连接与会话就绪时间">
        <span>启动</span>
        <span className="num num-muted">
          连接 {ms(lat?.connect_ms ?? null)} · 就绪 {ms(lat?.session_ready_ms ?? null)}
        </span>
      </div>
    </div>
  );
}
