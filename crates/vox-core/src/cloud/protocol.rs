//! qwen realtime WebSocket 协议。**收发的 JSON 长什么样，只写在这一个文件里。**
//!
//! 这份实现是旧版 `vrcq/cloud.py` 的逐条移植——那是踩过坑跑通过的东西，
//! 字段名、事件名、采样率都照抄，不"顺手改得更漂亮"。移植时保留的关键事实：
//!
//! 1. 产品只接专用 LiveTranslate 模型，使用原生 `translation.language` 参数。
//! 2. 文字增量事件有**三个**名字（`response.audio_transcript.delta`、
//!    `response.text.delta`、`response.output_text.delta`），字段名还不一样：
//!    transcript 类的在 `transcript`，text 类的在 `delta`。少接一个就丢字幕。
//! 3. 增量对外吐的是**到目前为止拼好的整句**，不是这一小片。字幕层要的是整句。
//! 4. done 事件里的文字可能是空的，这时拿累积的凑数（`or_else`），别直接清空。
//! 5. `session.update` 能热改语言/音色/要不要语音，**换模型不行**——那要新开
//!    socket，由上层重启会话。
//! 6. 输入 16 kHz、输出 24 kHz，都是 PCM16LE 单声道。
//!
//! 这里只做纯粹的编解码，不碰网络、不碰线程，好在平台无关的内核里做单测。

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::catalog;
use crate::usage::TurnUsage;

/// 上传音频的采样率。
pub const INPUT_SAMPLE_RATE: u32 = 16_000;
/// 服务端回的语音采样率。
pub const OUTPUT_SAMPLE_RATE: u32 = 24_000;

/// 拼出连接地址。模型名走 query 参数。
pub fn endpoint_url(model_name: &str) -> String {
    format!(
        "{}?model={}",
        catalog::API_BASE,
        catalog::normalize_model(model_name)
    )
}

/// 鉴权头的值（`Authorization: Bearer sk-...`）。
pub fn auth_header_value(api_key: &str) -> String {
    format!("Bearer {api_key}")
}

/// 声音复刻的采样频次。协议里是字符串，设置里存的是次数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloneFrequency {
    /// 只用第一段声音复刻。
    Once,
    /// 一直跟着说话人的声音走。
    Always,
}

impl CloneFrequency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Always => "always",
        }
    }

    /// 设置里存的是次数：1 次 = only once，多次 = 一直跟。0/None 表示不复刻。
    pub fn from_count(count: u32) -> Option<Self> {
        match count {
            0 => None,
            1 => Some(Self::Once),
            _ => Some(Self::Always),
        }
    }
}

/// 一次会话的协议参数。跟 `runtime::SessionConfig` 的区别是这里只留协议关心的东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionParams {
    pub model_name: String,
    pub target_language: String,
    /// `None` = 只要文字不要语音。
    pub voice: Option<String>,
    pub clone_frequency: Option<CloneFrequency>,
    /// 源语言；`None` = 服务端自动识别（默认，行为和旧版一致）。
    /// 填了会发 `input_audio_transcription.language`，把识别锁死到指定语言。
    pub source_language: Option<String>,
}

impl SessionParams {
    pub fn text_only(model_name: impl Into<String>, target_language: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
            target_language: target_language.into(),
            voice: None,
            clone_frequency: None,
            source_language: None,
        }
    }

    /// 这轮到底能不能出语音：要有音色，且目标语言支持语音输出。
    /// 目标语言不支持语音时，硬要 audio 会被服务端拒，所以这里先降级成纯文字。（坑）
    pub fn audio_enabled(&self) -> bool {
        self.voice.is_some() && catalog::supports_audio_output(&self.target_language)
    }

    /// 有效音色。复刻模式下协议要求音色写死 `"default"`。
    pub fn effective_voice(&self) -> Option<&str> {
        if !self.audio_enabled() {
            return None;
        }
        if self.clone_frequency.is_some() {
            Some("default")
        } else {
            self.voice.as_deref()
        }
    }

    /// `session.update` 里那个 `session` 对象。
    pub fn to_session_object(&self) -> Value {
        let audio = self.audio_enabled();
        let mut session = Map::new();
        session.insert(
            "modalities".into(),
            if audio {
                json!(["text", "audio"])
            } else {
                json!(["text"])
            },
        );
        session.insert("input_audio_format".into(), json!("pcm"));
        session.insert("output_audio_format".into(), json!("pcm"));

        session.insert(
            "translation".into(),
            json!({ "language": self.target_language }),
        );
        // 手动选过源语言就把它锁进会话，不填则让服务端自动识别。
        if let Some(lang) = &self.source_language {
            session.insert(
                "input_audio_transcription".into(),
                json!({ "language": lang }),
            );
        }

        if audio {
            if let Some(freq) = self.clone_frequency {
                session.insert("voice".into(), json!("default"));
                session.insert("enable_voice_clone".into(), json!(true));
                session.insert(
                    "voice_clone_options".into(),
                    json!({ "frequency": freq.as_str() }),
                );
            } else if let Some(voice) = self.voice.as_deref() {
                session.insert("voice".into(), json!(voice));
            }
        }

        Value::Object(session)
    }
}

/// 事件 id。服务端只要求唯一，用毫秒时间戳 + 递增号；时钟由外壳传进来。
pub fn event_id(now_ms: u64, seq: u64) -> String {
    format!("event_{now_ms}_{seq}")
}

/// 我们往上发的东西。
#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    /// 推会话配置（连上就发一次，之后热更新也发这个）。
    SessionUpdate(Box<SessionParams>),
    /// 追加一段上行音频（PCM16LE 单声道 16 kHz）。
    AppendAudio(Vec<u8>),
}

impl ClientEvent {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::SessionUpdate(_) => "session.update",
            Self::AppendAudio(_) => "input_audio_buffer.append",
        }
    }

    /// 序列化成要发出去的 JSON 文本。
    pub fn to_json(&self, event_id: &str) -> String {
        let value = match self {
            Self::SessionUpdate(params) => json!({
                "event_id": event_id,
                "type": self.type_name(),
                "session": params.to_session_object(),
            }),
            Self::AppendAudio(pcm) => json!({
                "event_id": event_id,
                "type": self.type_name(),
                "audio": base64::engine::general_purpose::STANDARD.encode(pcm),
            }),
        };
        value.to_string()
    }
}

/// 服务端回过来的东西。认得的都拆开，认不得的原样留着，方便调试面板显示。
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    /// 译文增量。`text` 是**到目前为止拼好的整句**。
    TextDelta { text: String },
    /// 这一句完了。
    TextDone { text: String },
    /// 服务端回报的**源文识别语种**。自动识别（`source_language` 为 None）时才有；
    /// 只给用户看识别到了什么语言，不改任何行为。
    SourceDetected { language: String },
    /// 一段译文语音（已解出的 PCM16LE 24 kHz 单声道字节）。
    AudioDelta { pcm: Vec<u8> },
    /// 服务端 VAD 在上传音频时间线上检测到语音开始。
    SpeechStarted {
        audio_start_ms: u64,
        item_id: Option<String>,
    },
    /// 服务端 VAD 检测到本轮语音结束。
    SpeechStopped {
        audio_end_ms: u64,
        item_id: Option<String>,
    },
    /// 一轮结束，带 token 用量。
    TurnDone { usage: TurnUsage },
    /// 会话配置已生效。
    SessionUpdated,
    /// 服务端报错。
    Error {
        code: Option<String>,
        message: String,
    },
    /// 没接的事件类型，留着给调试面板看。
    Other { event_type: String },
}

/// 解析出的东西 + 原始事件名（日志/调试面板要显示原名）。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEvent {
    pub event_type: String,
    pub event: ServerEvent,
}

/// 增量拼装器。服务端一小片一小片地给，字幕层要的是整句。
#[derive(Debug, Default)]
pub struct Decoder {
    parts: String,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前拼到哪了。
    pub fn pending(&self) -> &str {
        &self.parts
    }

    /// 丢掉没说完的半句（换会话、被抢占时用）。
    pub fn reset(&mut self) {
        self.parts.clear();
    }

    /// 吃一条服务端消息。解析不出 JSON 就返回 `None`——网络上的垃圾不该让流水线炸。
    pub fn decode(&mut self, message: &str) -> Option<ParsedEvent> {
        let value: Value = serde_json::from_str(message).ok()?;
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();

        let event = match event_type.as_str() {
            // transcript 类的文字在 `transcript`，text 类的在 `delta`。（坑 2）
            "response.audio_transcript.delta" => self.accumulate(str_field(&value, "transcript")),
            "response.text.delta" | "response.output_text.delta" => {
                self.accumulate(str_field(&value, "delta"))
            }
            // LiveTranslate 的流式译文走 `.text`，字段是 `text` + 可能被改写的 `stash`。
            // `text` 是已确认的前缀（**整句替换**累积的那一半），`stash` 是临时尾巴。
            "response.text.text"
            | "response.output_text.text"
            | "response.audio_transcript.text" => self.set_text_full(
                str_field(&value, "text").unwrap_or_default(),
                str_field(&value, "stash").unwrap_or_default(),
            ),
            "response.audio.delta" => match str_field(&value, "delta") {
                Some(b64) => match base64::engine::general_purpose::STANDARD.decode(b64) {
                    Ok(pcm) if !pcm.is_empty() => ServerEvent::AudioDelta { pcm },
                    // 空的或解不开的，当没接过的事件处理，不要伪造静音塞给播放器。
                    _ => ServerEvent::Other {
                        event_type: event_type.clone(),
                    },
                },
                None => ServerEvent::Other {
                    event_type: event_type.clone(),
                },
            },
            "input_audio_buffer.speech_started" => ServerEvent::SpeechStarted {
                audio_start_ms: value
                    .get("audio_start_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                item_id: str_field(&value, "item_id").map(str::to_string),
            },
            "input_audio_buffer.speech_stopped" => ServerEvent::SpeechStopped {
                audio_end_ms: value
                    .get("audio_end_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                item_id: str_field(&value, "item_id").map(str::to_string),
            },
            "response.audio_transcript.done" => self.finish(str_field(&value, "transcript")),
            "response.text.done" | "response.output_text.done" => {
                self.finish(str_field(&value, "text"))
            }
            "response.done" => {
                // 一轮说完了，半句作废。
                self.parts.clear();
                ServerEvent::TurnDone {
                    usage: usage_from_response(&value),
                }
            }
            "session.updated" => ServerEvent::SessionUpdated,
            // 源文转写回报识别到的语种（自动识别时才有）。字段名没钉死过，
            // 顶层有 `language` 就认，嵌套在 `input_audio_transcription` 里也认。
            // 这只是给用户看的小字，识别不到就当成没接过的事件，绝不打断流水线。
            "conversation.item.input_audio_transcription.delta"
            | "conversation.item.input_audio_transcription.done"
            | "conversation.item.input_audio_transcription.completed" => {
                let language = str_field(&value, "language")
                    .or_else(|| {
                        value
                            .get("input_audio_transcription")
                            .and_then(|v| v.get("language"))
                            .and_then(Value::as_str)
                    })
                    .filter(|l| !l.is_empty());
                match language {
                    Some(lang) => ServerEvent::SourceDetected {
                        language: lang.to_string(),
                    },
                    None => ServerEvent::Other {
                        event_type: event_type.clone(),
                    },
                }
            }
            "error" | "response.error" => {
                let err = value.get("error").unwrap_or(&value);
                ServerEvent::Error {
                    code: str_field(err, "code").map(str::to_string),
                    message: str_field(err, "message")
                        .unwrap_or("服务端返回了错误，但没说原因")
                        .to_string(),
                }
            }
            _ => ServerEvent::Other {
                event_type: event_type.clone(),
            },
        };

        Some(ParsedEvent { event_type, event })
    }

    /// 攒一片，吐出到目前为止的整句。（坑 3）
    fn accumulate(&mut self, piece: Option<&str>) -> ServerEvent {
        match piece.filter(|p| !p.is_empty()) {
            Some(piece) => {
                self.parts.push_str(piece);
                ServerEvent::TextDelta {
                    text: self.parts.clone(),
                }
            }
            // 空增量当没发生过。
            None => ServerEvent::Other {
                event_type: "response.text.delta".to_string(),
            },
        }
    }

    /// LiveTranslate 的 `.text` 事件：`confirmed` 是**已确认的整句前缀**（整体替换
    /// 累积那半），`stash` 是可能被改写的临时尾巴。渲染值 = `confirmed + stash`。
    ///
    /// `text` 是确认前缀而不是增量片段，所以**替换** `self.parts`（不能往上追加），
    /// 否则订正会叠字。`stash` 每次来都重算、不落进 `parts`——它只临时拼进渲染值，
    /// 最后以 `.done` 的终稿为准（§5.0）。
    fn set_text_full(&mut self, confirmed: &str, stash: &str) -> ServerEvent {
        let render = format!("{confirmed}{stash}");
        // 空渲染值（例如只剩空 stash）当没接过，别把已确认的字清掉。
        if render.is_empty() {
            return ServerEvent::Other {
                event_type: "response.text.text".to_string(),
            };
        }
        self.parts = confirmed.to_string();
        ServerEvent::TextDelta { text: render }
    }

    /// 收尾。done 里没带文字就拿累积的凑。（坑 4）
    fn finish(&mut self, text: Option<&str>) -> ServerEvent {
        let final_text = match text.filter(|t| !t.is_empty()) {
            Some(t) => t.to_string(),
            None => std::mem::take(&mut self.parts),
        };
        self.parts.clear();
        ServerEvent::TextDone { text: final_text }
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// 从 `response.done` 里掏 usage。字段名跟服务端一致，缺的当 0。
/// 服务端偶尔把 usage 放在顶层而不是 `response` 里，两处都看一眼。
fn usage_from_response(value: &Value) -> TurnUsage {
    let usage = value
        .get("response")
        .and_then(|r| r.get("usage"))
        .or_else(|| value.get("usage"));
    usage
        .and_then(|u| serde_json::from_value::<TurnUsage>(u.clone()).ok())
        .unwrap_or_default()
}

// --- PCM 转换 --------------------------------------------------------------

/// f32 → PCM16LE。**照抄旧版的做法**：先夹到 ±1.0，负半轴乘 32768、
/// 正半轴乘 32767，这样 -1.0 正好落在 i16::MIN 而 +1.0 不会溢出翻符号。
pub fn float_to_pcm16(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let clipped = s.clamp(-1.0, 1.0);
        let scaled = if clipped < 0.0 {
            clipped * 32768.0
        } else {
            clipped * 32767.0
        };
        out.extend_from_slice(&(scaled as i16).to_le_bytes());
    }
    out
}

/// PCM16LE → f32，给播放侧用。半个样本的尾巴直接丢掉。
pub fn pcm16_to_float(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = "qwen3.5-livetranslate-flash-realtime";

    fn speak_params() -> SessionParams {
        SessionParams {
            model_name: LIVE.to_string(),
            target_language: "ja".to_string(),
            voice: Some("Tina".to_string()),
            clone_frequency: None,
            source_language: None,
        }
    }

    #[test]
    fn endpoint_carries_the_model_and_normalizes_junk() {
        assert_eq!(
            endpoint_url(LIVE),
            format!("{}?model={LIVE}", catalog::API_BASE)
        );
        // 认不出的模型名要归一到默认模型，不能原样拼进 URL。
        assert!(endpoint_url("不存在的模型").ends_with(catalog::DEFAULT_MODEL_NAME));
    }

    #[test]
    fn auth_header_is_a_bearer_token() {
        assert_eq!(auth_header_value("sk-abc"), "Bearer sk-abc");
    }

    #[test]
    fn livetranslate_uses_the_native_translation_param() {
        let session = speak_params().to_session_object();
        assert_eq!(session["translation"]["language"], "ja");
        assert_eq!(session["modalities"], json!(["text", "audio"]));
        assert_eq!(session["voice"], "Tina");
        assert_eq!(session["input_audio_format"], "pcm");
        assert_eq!(session["output_audio_format"], "pcm");
        // 专用翻译模型不靠提示词驱动。
        assert!(session.get("instructions").is_none());
        assert!(session.get("turn_detection").is_none());
        // 没选源语言就不发 `input_audio_transcription`——服务端自动识别。
        assert!(session.get("input_audio_transcription").is_none());
    }

    #[test]
    fn livetranslate_sends_source_language_when_opted_in() {
        let mut params = speak_params();
        params.source_language = Some("en".to_string());
        let session = params.to_session_object();
        assert_eq!(session["input_audio_transcription"]["language"], "en");
    }

    #[test]
    fn text_only_session_defaults_to_auto_detected_source_language() {
        let text_only = SessionParams::text_only(LIVE, "en");
        assert!(text_only
            .to_session_object()
            .get("input_audio_transcription")
            .is_none());
    }

    #[test]
    fn text_only_session_asks_for_no_audio_modality() {
        let session = SessionParams::text_only(LIVE, "en").to_session_object();
        assert_eq!(session["modalities"], json!(["text"]));
        assert!(session.get("voice").is_none(), "不要语音就别报音色");
    }

    #[test]
    fn language_without_voice_support_degrades_to_text() {
        let mut params = speak_params();
        // 挑一个不在 AUDIO_OUTPUT_LANGUAGES 里的语言。
        params.target_language = "yue".to_string();
        assert!(
            !catalog::supports_audio_output("yue"),
            "前提：这个语言不支持语音输出"
        );
        assert!(!params.audio_enabled());
        let session = params.to_session_object();
        assert_eq!(session["modalities"], json!(["text"]));
        assert!(session.get("voice").is_none());
    }

    #[test]
    fn voice_clone_pins_the_voice_to_default() {
        let mut params = speak_params();
        params.clone_frequency = Some(CloneFrequency::Always);
        let session = params.to_session_object();
        assert_eq!(session["voice"], "default", "复刻时音色必须写死 default");
        assert_eq!(session["enable_voice_clone"], true);
        assert_eq!(session["voice_clone_options"]["frequency"], "always");
        assert_eq!(params.effective_voice(), Some("default"));
    }

    #[test]
    fn clone_frequency_maps_from_the_settings_count() {
        assert_eq!(CloneFrequency::from_count(0), None);
        assert_eq!(CloneFrequency::from_count(1), Some(CloneFrequency::Once));
        assert_eq!(CloneFrequency::from_count(9), Some(CloneFrequency::Always));
        assert_eq!(CloneFrequency::Once.as_str(), "once");
    }

    #[test]
    fn session_update_wraps_the_config_with_an_event_id() {
        let event = ClientEvent::SessionUpdate(Box::new(speak_params()));
        let sent: Value = serde_json::from_str(&event.to_json("event_1_1")).unwrap();
        assert_eq!(sent["type"], "session.update");
        assert_eq!(sent["event_id"], "event_1_1");
        assert_eq!(sent["session"]["translation"]["language"], "ja");
    }

    #[test]
    fn audio_append_is_base64_pcm() {
        let pcm = float_to_pcm16(&[0.0, 0.5]);
        let event = ClientEvent::AppendAudio(pcm.clone());
        let sent: Value = serde_json::from_str(&event.to_json("e")).unwrap();
        assert_eq!(sent["type"], "input_audio_buffer.append");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(sent["audio"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, pcm);
    }

    #[test]
    fn event_ids_are_unique_within_a_millisecond() {
        assert_ne!(event_id(1_700_000, 1), event_id(1_700_000, 2));
    }

    /// 坑 2 + 坑 3：三个增量事件名都要认，且吐的是拼好的整句。
    #[test]
    fn all_three_delta_event_names_accumulate_into_one_sentence() {
        let mut dec = Decoder::new();

        let a = dec
            .decode(r#"{"type":"response.audio_transcript.delta","transcript":"こん"}"#)
            .unwrap();
        assert_eq!(
            a.event,
            ServerEvent::TextDelta {
                text: "こん".into()
            }
        );

        let b = dec
            .decode(r#"{"type":"response.text.delta","delta":"にち"}"#)
            .unwrap();
        assert_eq!(
            b.event,
            ServerEvent::TextDelta {
                text: "こんにち".into()
            }
        );

        let c = dec
            .decode(r#"{"type":"response.output_text.delta","delta":"は"}"#)
            .unwrap();
        assert_eq!(
            c.event,
            ServerEvent::TextDelta {
                text: "こんにちは".into()
            },
            "增量要吐到目前为止的整句"
        );
        assert_eq!(dec.pending(), "こんにちは");
    }

    #[test]
    fn done_finalizes_and_clears_the_buffer() {
        let mut dec = Decoder::new();
        dec.decode(r#"{"type":"response.text.delta","delta":"hel"}"#);
        let done = dec
            .decode(r#"{"type":"response.text.done","text":"hello"}"#)
            .unwrap();
        assert_eq!(
            done.event,
            ServerEvent::TextDone {
                text: "hello".into()
            }
        );
        assert_eq!(dec.pending(), "", "收尾后不能留半句");
    }

    /// 坑 4：done 里的文字是空的，要拿累积的凑，不能吐空串。
    #[test]
    fn empty_done_falls_back_to_the_accumulated_text() {
        let mut dec = Decoder::new();
        dec.decode(r#"{"type":"response.audio_transcript.delta","transcript":"你好"}"#);
        let done = dec
            .decode(r#"{"type":"response.audio_transcript.done","transcript":""}"#)
            .unwrap();
        assert_eq!(
            done.event,
            ServerEvent::TextDone {
                text: "你好".into()
            }
        );
        assert_eq!(dec.pending(), "");
    }

    /// LiveTranslate 的流式文字走 `.text`：`text` 是确认前缀（整体替换累积那半），
    /// `stash` 是可能被改写的尾巴。对外吐的是渲染值 `text + stash`（§5.0）。
    #[test]
    fn livetranslate_text_event_renders_confirmed_plus_stash() {
        let mut dec = Decoder::new();
        let first = dec
            .decode(r#"{"type":"response.text.text","text":"こん","stash":"にち"}"#)
            .unwrap();
        assert_eq!(
            first.event,
            ServerEvent::TextDelta {
                text: "こんにち".into()
            },
            "渲染值 = 确认前缀 + 尾巴"
        );
        assert_eq!(dec.pending(), "こん", "确认前缀才进累积，stash 不落账");

        // 前缀长出来、stash 缩回去，整段还是完整一句。
        let second = dec
            .decode(r#"{"type":"response.text.text","text":"こんにち","stash":"は"}"#)
            .unwrap();
        assert_eq!(
            second.event,
            ServerEvent::TextDelta {
                text: "こんにちは".into()
            }
        );
        assert_eq!(dec.pending(), "こんにち");
    }

    /// 服务端订正整句时，`.text` 的 `text` 前缀整体变了——必须替换而不是追加。
    /// 上层的 `push_text` 靠 `strip_prefix` 失败触发 replace，这里保证 Decoder
    /// 把确认前缀整体换掉，不残留旧句的尾巴。
    #[test]
    fn text_event_replaces_the_accumulated_prefix_on_rewrite() {
        let mut dec = Decoder::new();
        dec.decode(r#"{"type":"response.text.text","text":"これは最初の訳","stash":""}"#);
        assert_eq!(dec.pending(), "これは最初の訳");
        let corrected = dec
            .decode(r#"{"type":"response.text.text","text":"訂正後の訳","stash":"文"}"#)
            .unwrap();
        assert_eq!(
            corrected.event,
            ServerEvent::TextDelta {
                text: "訂正後の訳文".into()
            }
        );
        assert_eq!(
            dec.pending(),
            "訂正後の訳",
            "前缀被整体替换，不能残留旧的确认部分"
        );
    }

    #[test]
    fn audio_transcript_text_is_rendered_the_same_way() {
        let mut dec = Decoder::new();
        let ev = dec
            .decode(r#"{"type":"response.audio_transcript.text","text":"你好","stash":"呀"}"#)
            .unwrap();
        assert_eq!(
            ev.event,
            ServerEvent::TextDelta {
                text: "你好呀".into()
            }
        );
        assert_eq!(dec.pending(), "你好");
    }

    /// 服务端回报识别到的源文语种（自动识别下才有）。顶层或嵌套
    /// `input_audio_transcription.language` 都认；没有就不当回事。
    #[test]
    fn source_language_is_read_from_transcription_events() {
        let mut dec = Decoder::new();
        let flat = dec
            .decode(r#"{"type":"conversation.item.input_audio_transcription.delta","language":"ja","delta":"こん"}"#)
            .unwrap();
        assert_eq!(
            flat.event,
            ServerEvent::SourceDetected {
                language: "ja".into()
            }
        );

        let nested = dec
            .decode(r#"{"type":"conversation.item.input_audio_transcription.done","input_audio_transcription":{"language":"en"}}"#)
            .unwrap();
        assert_eq!(
            nested.event,
            ServerEvent::SourceDetected {
                language: "en".into()
            }
        );

        // 没有 language 字段就当没接过，绝不炸流水线。
        let silent = dec
            .decode(r#"{"type":"conversation.item.input_audio_transcription.delta"}"#)
            .unwrap();
        assert!(matches!(silent.event, ServerEvent::Other { .. }));
    }

    #[test]
    fn speech_boundaries_keep_audio_offsets_and_item_ids() {
        let mut dec = Decoder::new();
        let started = dec
            .decode(
                r#"{"type":"input_audio_buffer.speech_started","audio_start_ms":568,"item_id":"item-a"}"#,
            )
            .unwrap();
        assert_eq!(
            started.event,
            ServerEvent::SpeechStarted {
                audio_start_ms: 568,
                item_id: Some("item-a".into())
            }
        );
        let stopped = dec
            .decode(
                r#"{"type":"input_audio_buffer.speech_stopped","audio_end_ms":3900,"item_id":"item-a"}"#,
            )
            .unwrap();
        assert_eq!(
            stopped.event,
            ServerEvent::SpeechStopped {
                audio_end_ms: 3900,
                item_id: Some("item-a".into())
            }
        );
    }

    #[test]
    fn audio_delta_is_decoded_into_pcm_bytes() {
        let pcm = float_to_pcm16(&[0.25, -0.25]);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm);
        let mut dec = Decoder::new();
        let ev = dec
            .decode(&format!(
                r#"{{"type":"response.audio.delta","delta":"{b64}"}}"#
            ))
            .unwrap();
        assert_eq!(ev.event, ServerEvent::AudioDelta { pcm });
    }

    #[test]
    fn broken_audio_delta_does_not_become_fake_silence() {
        let mut dec = Decoder::new();
        let ev = dec
            .decode(r#"{"type":"response.audio.delta","delta":"!!!not-base64!!!"}"#)
            .unwrap();
        assert!(
            matches!(ev.event, ServerEvent::Other { .. }),
            "解不开的音频不该塞给播放器，实际是 {:?}",
            ev.event
        );
    }

    #[test]
    fn response_done_carries_usage_and_wipes_the_half_sentence() {
        let mut dec = Decoder::new();
        dec.decode(r#"{"type":"response.text.delta","delta":"半句"}"#);
        let ev = dec
            .decode(
                r#"{"type":"response.done","response":{"usage":{"input_tokens":30,"output_tokens":12,"total_tokens":42}}}"#,
            )
            .unwrap();
        assert_eq!(
            ev.event,
            ServerEvent::TurnDone {
                usage: TurnUsage {
                    input_tokens: 30,
                    output_tokens: 12,
                    total_tokens: 42,
                }
            }
        );
        assert_eq!(dec.pending(), "");
    }

    #[test]
    fn usage_is_also_read_from_the_top_level_and_tolerates_missing_fields() {
        let mut dec = Decoder::new();
        let ev = dec
            .decode(r#"{"type":"response.done","usage":{"input_tokens":5}}"#)
            .unwrap();
        let ServerEvent::TurnDone { usage } = ev.event else {
            panic!("应该是 TurnDone");
        };
        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.resolved_total(), 5, "服务端没给总数就自己加");

        // 完全没有 usage 的 response.done 也要收得下。
        let ev = dec
            .decode(r#"{"type":"response.done","response":{}}"#)
            .unwrap();
        assert_eq!(
            ev.event,
            ServerEvent::TurnDone {
                usage: TurnUsage::default()
            }
        );
    }

    #[test]
    fn error_events_surface_code_and_message() {
        let mut dec = Decoder::new();
        let ev = dec
            .decode(
                r#"{"type":"error","error":{"code":"invalid_api_key","message":"Incorrect API key provided."}}"#,
            )
            .unwrap();
        assert_eq!(
            ev.event,
            ServerEvent::Error {
                code: Some("invalid_api_key".into()),
                message: "Incorrect API key provided.".into(),
            }
        );
    }

    #[test]
    fn error_without_a_message_still_says_something() {
        let mut dec = Decoder::new();
        let ev = dec.decode(r#"{"type":"error"}"#).unwrap();
        let ServerEvent::Error { message, code } = ev.event else {
            panic!("应该是 Error");
        };
        assert!(code.is_none());
        assert!(!message.is_empty(), "错误消息不能是空的");
    }

    #[test]
    fn session_updated_and_unknown_events_are_kept_apart() {
        let mut dec = Decoder::new();
        assert_eq!(
            dec.decode(r#"{"type":"session.updated","session":{}}"#)
                .unwrap()
                .event,
            ServerEvent::SessionUpdated
        );
        let ev = dec.decode(r#"{"type":"response.created"}"#).unwrap();
        assert_eq!(
            ev.event,
            ServerEvent::Other {
                event_type: "response.created".into()
            }
        );
        assert_eq!(ev.event_type, "response.created", "原名要留着给调试面板");
    }

    #[test]
    fn garbage_json_is_dropped_without_panicking() {
        let mut dec = Decoder::new();
        assert!(dec.decode("这不是 JSON").is_none());
        assert!(dec.decode("").is_none());
        // 是 JSON 但没有 type，当未知事件收下。
        assert_eq!(dec.decode(r#"{"foo":1}"#).unwrap().event_type, "unknown");
    }

    #[test]
    fn reset_drops_the_half_sentence() {
        let mut dec = Decoder::new();
        dec.decode(r#"{"type":"response.text.delta","delta":"半句"}"#);
        dec.reset();
        assert_eq!(dec.pending(), "");
    }

    #[test]
    fn pcm_conversion_hits_the_rails_without_wrapping() {
        // -1.0 落在 i16::MIN，+1.0 落在 i16::MAX，都不能翻符号。
        let bytes = float_to_pcm16(&[-1.0, 1.0, 0.0]);
        assert_eq!(bytes.len(), 6);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), i16::MIN);
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[4], bytes[5]]), 0);

        // 超范围的输入先夹再转。
        let clipped = float_to_pcm16(&[-9.0, 9.0]);
        assert_eq!(i16::from_le_bytes([clipped[0], clipped[1]]), i16::MIN);
        assert_eq!(i16::from_le_bytes([clipped[2], clipped[3]]), i16::MAX);
    }

    #[test]
    fn pcm_roundtrips_closely_enough_and_ignores_a_dangling_byte() {
        let original = [0.0f32, 0.5, -0.5, 0.999];
        let back = pcm16_to_float(&float_to_pcm16(&original));
        assert_eq!(back.len(), original.len());
        for (a, b) in original.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
        // 奇数字节数（网络上截断的包）不能 panic。
        assert_eq!(pcm16_to_float(&[0x00, 0x00, 0x7f]).len(), 1);
    }

    #[test]
    fn sample_rates_match_the_old_client() {
        assert_eq!(INPUT_SAMPLE_RATE, 16_000);
        assert_eq!(OUTPUT_SAMPLE_RATE, 24_000);
    }
}
