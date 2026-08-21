/** 假字幕流：把脚本一句一句按字吐出来，句末停一拍再清屏，下一句接上。 */

import type { Track } from "../types";
import type { VoxEvent } from "../types.snapshot";

type Emit = (event: VoxEvent) => void;

/** 模拟「订正」的台词：先吐一半，再用 replace 整句改正。 */
function correctedLine(line: number): { half: string; full: string } | null {
  if (line % 5 !== 1) return null;
  return {
    half: "这个翻译一开始是错的",
    full: "这段才是订正后的正确译文",
  };
}

const MOCK_SOURCES = ["ja", "en", "ja", "ko"];

export class FakeTyper {
  private line = 0;
  private cursor = 0;
  /** 下一次动作的时间戳；句子说完后用来压一拍。 */
  private nextAt = 0;
  private phase: "writing" | "hold" | "clear" = "writing";

  constructor(
    private readonly track: Track,
    private readonly script: string[],
  ) {}

  reset(emit: Emit): void {
    if (this.cursor > 0 || this.phase !== "writing") emit({ kind: "subtitle_cleared", track: this.track });
    this.line = 0;
    this.cursor = 0;
    this.nextAt = 0;
    this.phase = "writing";
  }

  /** 返回本帧新增的字符数（给用量计数用）。 */
  step(now: number, emit: Emit): number {
    if (now < this.nextAt) return 0;
    const text = this.script[this.line % this.script.length] ?? "";

    if (this.phase === "clear") {
      emit({ kind: "subtitle_cleared", track: this.track });
      this.line += 1;
      this.cursor = 0;
      this.phase = "writing";
      // 开头顺手报一次「识别成 X 语言」，让 Listen 页的小字活起来。
      if (this.track === "listen") {
        emit({
          kind: "source_detected",
          track: this.track,
          language: MOCK_SOURCES[this.line % MOCK_SOURCES.length] ?? "ja",
        });
      }
      this.nextAt = now + 320;
      return 0;
    }
    if (this.phase === "hold") {
      this.phase = "clear";
      this.nextAt = now + 1500;
      return 0;
    }

    // 订正演示：某一行先吐半句，到一半时整句 replace 换掉。
    const demo = correctedLine(this.line);
    if (demo && this.cursor >= demo.half.length) {
      this.cursor = demo.full.length;
      emit({ kind: "subtitle_delta", track: this.track, text: demo.full, done: false, replace: true });
      this.phase = "hold";
      this.nextAt = now + 900;
      return demo.full.length;
    }

    // 一次吐 1–2 个字，节奏 90–200ms，像流式返回
    const take = Math.random() < 0.25 ? 2 : 1;
    const chunk = demo ? demo.half.slice(this.cursor, this.cursor + take) : text.slice(this.cursor, this.cursor + take);
    this.cursor += chunk.length;
    const done = !demo && this.cursor >= text.length;
    emit({ kind: "subtitle_delta", track: this.track, text: chunk, done, replace: false });
    if (done) {
      this.phase = "hold";
      this.nextAt = now + 900;
    } else {
      this.nextAt = now + 90 + Math.random() * 110;
    }
    return chunk.length;
  }
}
