/** 假数据素材：设备、进程、字幕脚本、初始用量。只在 dev / 浏览器预览里用。 */

import type { AudioApp, DeviceInfo, UsageLedger } from "../types.snapshot";
import { DEFAULT_MODEL_NAME } from "../catalog";

export const MOCK_INPUTS: DeviceInfo[] = [
  { name: "麦克风 (Realtek(R) Audio)", is_default: true },
  { name: "耳机麦克风 (Arctis Nova 7)", is_default: false },
  { name: "CABLE Output (VB-Audio Virtual Cable)", is_default: false },
  { name: "线路输入 (Focusrite Scarlett Solo)", is_default: false },
];

export const MOCK_OUTPUTS: DeviceInfo[] = [
  { name: "扬声器 (Realtek(R) Audio)", is_default: true },
  { name: "耳机 (Arctis Nova 7)", is_default: false },
  { name: "CABLE Input (VB-Audio Virtual Cable)", is_default: false },
  { name: "LG HDR 4K (NVIDIA High Definition Audio)", is_default: false },
];

export const MOCK_APPS: AudioApp[] = [
  { executable: "VRChat.exe", display_name: "VRChat", pid: 18420, active: true },
  { executable: "Discord.exe", display_name: "Discord", pid: 9312, active: true },
  { executable: "chrome.exe", display_name: "Google Chrome", pid: 4488, active: true },
  { executable: "steam.exe", display_name: "Steam", pid: 7704, active: false },
  { executable: "Spotify.exe", display_name: "Spotify", pid: 15992, active: false },
];

/** 对外说话：中文进、外语出，字幕轨显示的是译文。 */
export const SPEAK_SCRIPT: string[] = [
  "はじめまして、よろしくお願いします。",
  "この部屋のライトはどうやって変えるんですか？",
  "ちょっと待ってください、マイクを直します。",
  "さっきの話、もう一度言ってもらえますか？",
  "今日は付き合ってくれてありがとう、また明日ね。",
];

/** 听人说话：抓别人的声音，翻成中文。 */
export const LISTEN_SCRIPT: string[] = [
  "欢迎来玩，随便找个位置坐吧。",
  "我这边的麦好像有点小，你能听清吗？",
  "那个镜子后面有个隐藏房间，要不要一起去看看。",
  "等一下，我去换个头像，两分钟就回来。",
  "今天人有点多，语音有点卡，抱歉啊。",
];

const today = new Date();
const pad = (n: number) => String(n).padStart(2, "0");
export const MOCK_TODAY = `${today.getFullYear()}-${pad(today.getMonth() + 1)}-${pad(today.getDate())}`;
export const MOCK_MONTH = `${today.getFullYear()}-${pad(today.getMonth() + 1)}`;

export function mockUsage(): UsageLedger {
  return {
    [DEFAULT_MODEL_NAME]: {
      input_tokens: 412_866,
      output_tokens: 118_204,
      total_tokens: 531_070,
      turns: 1_284,
      daily: { input_tokens: 24_118, output_tokens: 7_402, total_tokens: 31_520, turns: 86 },
      daily_date: MOCK_TODAY,
      monthly: { input_tokens: 186_530, output_tokens: 54_998, total_tokens: 241_528, turns: 604 },
      monthly_month: MOCK_MONTH,
      updated_at: Math.floor(Date.now() / 1000) - 90,
    },
  };
}
