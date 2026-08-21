/** 字幕外观：悬浮窗的显示开关、字体、颜色、逐字淡出，加一块按真实参数渲染的预览。 */

import {
  BACKGROUND_ALPHA_RANGE,
  CHAR_FADE_RANGE,
  CHAR_TTL_RANGE,
  DIM_ALPHA_RANGE,
  FONT_SIZE_RANGE,
  clamp,
} from "../defaults";
import { useStore } from "../store";
import type { SubtitleSettings } from "../types";
import { Dropdown, SettingsItem, Slider, Toggle } from "../ui/controls";

const FONTS = [
  { value: "Microsoft YaHei UI", label: "微软雅黑" },
  { value: "Maple Mono Normal CN", label: "Maple Mono CN" },
  { value: "SimHei", label: "黑体" },
  { value: "KaiTi", label: "楷体" },
  { value: "DengXian", label: "等线" },
];

/** 底衬颜色 = 纯黑 + 设置里的 alpha，和 vox-overlay-win 的 PLATE_COLOR 一致。 */
function plateStyle(sub: SubtitleSettings) {
  return {
    background: `rgba(0, 0, 0, ${(sub.background_alpha / 255).toFixed(3)})`,
    borderRadius: "var(--radius)",
  };
}

/** 预览里固定摆两行：对外（暖）在上、听人（冷）在下，颜色跟设置走。 */
function SubtitlePreview({ sub }: { sub: SubtitleSettings }) {
  const line = (color: string, text: string) => (
    <div
      className="subtitle-preview__row"
      style={{
        ...plateStyle(sub),
        color,
        fontSize: Math.min(sub.font_size, 30),
        fontFamily: sub.font_family,
      }}
    >
      {text.split("").map((ch, i) => (
        <span key={i} className="subtitle-preview__ch">
          {ch}
        </span>
      ))}
    </div>
  );
  return (
    <div className="subtitle-preview" aria-hidden="true">
      <span className="subtitle-preview__tag">预览 · 实际悬浮窗为透明叠层</span>
      {line(sub.speak_color, "Hello，这句是对外说话的字幕")}
      {line(sub.listen_color, "这句是听人说话的字幕")}
    </div>
  );
}

export function SubtitlePage() {
  const { snapshot, settings, patch } = useStore();
  const loading = snapshot === null;
  const sub = settings.subtitle;

  return (
    <>
      <div className="settings-group">
        <SettingsItem
          title="显示字幕"
          desc="关闭后悬浮窗整体隐藏，两条管线仍会照常工作"
          control={
            <Toggle
              checked={sub.visible}
              disabled={loading}
              label="显示字幕"
              onChange={(visible) => patch({ subtitle: { visible } })}
            />
          }
        />
        <div className="subtitle-preview-wrap">
          <SubtitlePreview sub={sub} />
        </div>
      </div>

      <div className="settings-group">
        <SettingsItem
          wide
          htmlFor="dd-subtitle-font"
          title="字体"
          control={
            <Dropdown
              id="dd-subtitle-font"
              label="字幕字体"
              value={sub.font_family}
              options={
                FONTS.some((f) => f.value === sub.font_family)
                  ? FONTS
                  : [...FONTS, { value: sub.font_family, label: sub.font_family }]
              }
              onChange={(font_family) => patch({ subtitle: { font_family } })}
            />
          }
        />
        <SettingsItem
          wide
          title="字号"
          control={
            <Slider
              value={sub.font_size}
              min={FONT_SIZE_RANGE.min}
              max={FONT_SIZE_RANGE.max}
              step={1}
              disabled={loading}
              label="字幕字号"
              format={(v) => `${v} px`}
              onChange={(font_size) => patch({ subtitle: { font_size } })}
            />
          }
        />
        <SettingsItem
          title="对外说话 · 颜色"
          desc="暖色行"
          control={
            <input
              type="color"
              className="color-input"
              value={sub.speak_color}
              disabled={loading}
              aria-label="对外说话字幕颜色"
              onChange={(e) => patch({ subtitle: { speak_color: e.target.value } })}
            />
          }
        />
        <SettingsItem
          title="听人说话 · 颜色"
          desc="冷色行"
          control={
            <input
              type="color"
              className="color-input"
              value={sub.listen_color}
              disabled={loading}
              aria-label="听人说话字幕颜色"
              onChange={(e) => patch({ subtitle: { listen_color: e.target.value } })}
            />
          }
        />
        <SettingsItem
          wide
          title="底衬不透明度"
          desc="0 = 只显字幕、无底衬"
          control={
            <Slider
              value={sub.background_alpha}
              min={BACKGROUND_ALPHA_RANGE.min}
              max={BACKGROUND_ALPHA_RANGE.max}
              step={1}
              disabled={loading}
              label="底衬不透明度"
              format={(v) => `${v} / 255`}
              onChange={(background_alpha) => patch({ subtitle: { background_alpha } })}
            />
          }
        />
      </div>

      <div className="settings-group">
        <SettingsItem
          wide
          title="字符停留"
          desc="每个字在屏幕上保留多久"
          control={
            <Slider
              value={sub.char_ttl_ms}
              min={CHAR_TTL_RANGE.min}
              max={CHAR_TTL_RANGE.max}
              step={100}
              disabled={loading}
              label="字符停留时长"
              format={(v) => `${(v / 1000).toFixed(1)} 秒`}
              onChange={(char_ttl_ms) =>
                patch({
                  subtitle: {
                    char_ttl_ms,
                    // 后端归一化会把淡出夹到不超过停留，这里提前跟上，
                    // 免得拖完两个滑块读数看着像没反应。
                    char_fade_ms: Math.min(sub.char_fade_ms, char_ttl_ms),
                  },
                })
              }
            />
          }
        />
        <SettingsItem
          wide
          title="淡出时长"
          desc="停留结束后的渐隐速度，0 = 直接消失"
          control={
            <Slider
              value={sub.char_fade_ms}
              min={CHAR_FADE_RANGE.min}
              max={clamp(CHAR_FADE_RANGE.max, CHAR_FADE_RANGE.min, sub.char_ttl_ms)}
              step={100}
              disabled={loading}
              label="淡出时长"
              format={(v) => (v === 0 ? "直接消失" : `${(v / 1000).toFixed(1)} 秒`)}
              onChange={(char_fade_ms) => patch({ subtitle: { char_fade_ms } })}
            />
          }
        />
        <SettingsItem
          title="保留 0 类字幕"
          desc="纯噪声/填充词/无意义发音在 Lifetime 结束后淡成浅灰长期保留，而不是消失"
          control={
            <Toggle
              checked={sub.dim_zeros}
              disabled={loading}
              label="保留 0 类字幕"
              onChange={(dim_zeros) => patch({ subtitle: { dim_zeros } })}
            />
          }
        />
        <SettingsItem
          wide
          title="淡化程度"
          desc="启用“保留 0 类字幕”后才能调整；数值越小颜色越淡"
          control={
            <Slider
              value={sub.dim_alpha}
              min={DIM_ALPHA_RANGE.min}
              max={DIM_ALPHA_RANGE.max}
              step={0.05}
              disabled={loading || !sub.dim_zeros}
              label="淡化程度"
              format={(v) => `${Math.round(v * 100)}%`}
              onChange={(dim_alpha) => patch({ subtitle: { dim_alpha } })}
            />
          }
        />
      </div>

      <div className="settings-group">
        <SettingsItem
          title="位置与大小"
          desc="拖动字幕区域移动，拖动窗口边缘调整大小；位置和大小会自动记住"
          control={
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => patch({ subtitle: { geometry: null } })}
            >
              恢复默认
            </button>
          }
        />
      </div>
    </>
  );
}
