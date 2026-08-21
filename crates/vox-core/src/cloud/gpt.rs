//! OpenAI GPT Realtime Translation wire protocol (`gpt-realtime-translate`).
//!
//! This is the dedicated translation endpoint documented at
//! `https://developers.openai.com/api/docs/guides/realtime-translation`, not the
//! general Realtime voice-agent endpoint. It uses `session.*` events and does not
//! call `response.create`.

use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::catalog;
use crate::settings::ModelProvider;

use super::protocol::{ParsedEvent, ServerEvent, SessionParams};

/// The OpenAI translation endpoint is opened with the model selected via query.
pub fn endpoint_url() -> String {
    format!(
        "{}?model={}",
        catalog::GPT_API_BASE,
        catalog::GPT_MODEL_NAME
    )
}

/// OpenAI Realtime Translation accepts 24 kHz PCM16 mono over WebSocket.
pub fn input_sample_rate() -> u32 {
    crate::catalog::GPT_INPUT_SAMPLE_RATE
}

/// Builds the WebSocket session configuration for a translation session.
pub fn session_update(params: &SessionParams) -> String {
    let mut audio = Map::new();
    let mut output = Map::new();
    output.insert("language".to_string(), json!(params.target_language));
    audio.insert("output".to_string(), Value::Object(output));

    if catalog::supports_source_language(ModelProvider::Gpt) && params.source_language.is_some() {
        audio.insert(
            "input".to_string(),
            json!({
                "transcription": {
                    "model": "gpt-realtime-whisper"
                }
            }),
        );
    }

    let mut session = Map::new();
    session.insert("audio".to_string(), Value::Object(audio));

    if catalog::supports_voice_selection(ModelProvider::Gpt) {
        if let Some(voice) = &params.voice {
            session.insert("voice".to_string(), json!(voice));
        }
    }

    if catalog::supports_voice_clone(ModelProvider::Gpt) {
        if let Some(freq) = params.clone_frequency {
            session.insert("voice_clone".to_string(), json!(freq.as_str()));
        }
    }

    json!({
        "type": "session.update",
        "session": Value::Object(session),
    })
    .to_string()
}

/// Appends one base64-encoded 24 kHz PCM16 mono little-endian audio chunk.
pub fn audio_frame(pcm: &[u8]) -> String {
    json!({
        "type": "session.input_audio_buffer.append",
        "audio": base64::engine::general_purpose::STANDARD.encode(pcm),
    })
    .to_string()
}

/// Tells the server to flush remaining output and emit `session.closed`.
pub fn close_frame() -> String {
    json!({ "type": "session.close" }).to_string()
}

/// Decodes OpenAI Realtime Translation server events.
#[derive(Debug, Default)]
pub struct Decoder {
    output_parts: String,
    source_parts: String,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending(&self) -> &str {
        &self.output_parts
    }

    pub fn reset(&mut self) {
        self.output_parts.clear();
        self.source_parts.clear();
    }

    pub fn decode(&mut self, message: &str) -> Vec<ParsedEvent> {
        let Ok(value) = serde_json::from_str::<Value>(message) else {
            return Vec::new();
        };
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let event = match event_type.as_str() {
            "session.created" | "session.updated" => ServerEvent::SessionUpdated,
            "session.output_transcript.delta" => {
                self.output_transcript_delta(str_field(&value, "delta"))
            }
            "session.input_transcript.delta" => {
                self.input_transcript_delta(str_field(&value, "delta"))
            }
            "session.output_audio.delta" => audio_delta(&value, &event_type),
            "session.closed" => ServerEvent::Other {
                event_type: event_type.clone(),
            },
            "error" | "response.error" => {
                let err = value.get("error").unwrap_or(&value);
                ServerEvent::Error {
                    code: str_field(err, "code").map(str::to_string),
                    message: str_field(err, "message")
                        .unwrap_or("OpenAI Realtime 返回了错误，但没说原因")
                        .to_string(),
                }
            }
            _ => ServerEvent::Other {
                event_type: event_type.clone(),
            },
        };
        vec![ParsedEvent { event_type, event }]
    }

    fn output_transcript_delta(&mut self, piece: Option<&str>) -> ServerEvent {
        match piece.filter(|p| !p.is_empty()) {
            Some(piece) => {
                self.output_parts.push_str(piece);
                ServerEvent::TextDelta {
                    text: self.output_parts.clone(),
                }
            }
            None => ServerEvent::Other {
                event_type: "session.output_transcript.delta".into(),
            },
        }
    }

    fn input_transcript_delta(&mut self, piece: Option<&str>) -> ServerEvent {
        match piece.filter(|p| !p.is_empty()) {
            Some(piece) => {
                self.source_parts.push_str(piece);
                ServerEvent::SourceTranscriptDelta {
                    text: self.source_parts.clone(),
                }
            }
            None => ServerEvent::Other {
                event_type: "session.input_transcript.delta".into(),
            },
        }
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn audio_delta(value: &Value, event_type: &str) -> ServerEvent {
    match str_field(value, "delta") {
        Some(b64) => match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(pcm) if !pcm.is_empty() => ServerEvent::AudioDelta { pcm },
            _ => ServerEvent::Other {
                event_type: event_type.to_string(),
            },
        },
        None => ServerEvent::Other {
            event_type: event_type.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::protocol::ServerEvent as Ev;

    fn params() -> SessionParams {
        SessionParams {
            model_name: catalog::GPT_MODEL_NAME.into(),
            target_language: "ja".into(),
            voice: Some("auto".into()),
            clone_frequency: None,
            source_language: None,
        }
    }

    #[test]
    fn endpoint_uses_the_dedicated_translation_path() {
        assert_eq!(
            endpoint_url(),
            "wss://api.openai.com/v1/realtime/translations?model=gpt-realtime-translate"
        );
    }

    #[test]
    fn input_sample_rate_is_catalog_driven_24k() {
        assert_eq!(input_sample_rate(), 24_000);
        assert_eq!(input_sample_rate(), crate::catalog::GPT_INPUT_SAMPLE_RATE);
    }

    #[test]
    fn session_update_uses_translation_config_shape() {
        let value: Value = serde_json::from_str(&session_update(&params())).unwrap();
        assert_eq!(value["type"], "session.update");
        assert_eq!(value["session"]["audio"]["output"]["language"], "ja");
        assert!(value["session"].get("modalities").is_none());
        assert!(value["session"].get("instructions").is_none());
        assert!(value["session"].get("input_audio_format").is_none());
        assert!(value["session"].get("output_audio_format").is_none());
        assert!(
            value["session"].get("voice").is_none(),
            "GPT catalog declares voice_selection=false"
        );
    }

    #[test]
    fn audio_is_base64_pcm16_with_session_prefix() {
        let value: Value = serde_json::from_str(&audio_frame(&[1, 2, 3])).unwrap();
        assert_eq!(value["type"], "session.input_audio_buffer.append");
        assert_eq!(value["audio"], "AQID");
    }

    #[test]
    fn close_frame_is_session_close() {
        let value: Value = serde_json::from_str(&close_frame()).unwrap();
        assert_eq!(value["type"], "session.close");
        assert_eq!(value.as_object().unwrap().len(), 1);
    }

    #[test]
    fn output_transcript_deltas_accumulate_into_one_sentence() {
        let mut decoder = Decoder::new();
        let a = decoder.decode(r#"{"type":"session.output_transcript.delta","delta":"こん"}"#);
        assert!(matches!(
            &a[0].event, Ev::TextDelta { text } if text == "こん"
        ));

        let b = decoder.decode(r#"{"type":"session.output_transcript.delta","delta":"にちは"}"#);
        assert!(matches!(
            &b[0].event, Ev::TextDelta { text } if text == "こんにちは"
        ));
        assert_eq!(decoder.pending(), "こんにちは");
    }

    #[test]
    fn input_transcript_deltas_are_kept_apart_from_output() {
        let mut decoder = Decoder::new();
        let events = decoder.decode(r#"{"type":"session.input_transcript.delta","delta":"您好"}"#);
        assert!(matches!(
            &events[0].event, Ev::SourceTranscriptDelta { text } if text == "您好"
        ));
        assert_eq!(decoder.pending(), "");
    }

    #[test]
    fn audio_delta_is_decoded_into_pcm_bytes() {
        let pcm = vec![1u8, 0, 2, 0];
        let message = format!(
            r#"{{"type":"session.output_audio.delta","delta":"{}","sample_rate":24000,"channels":1,"format":"pcm16"}}"#,
            base64::engine::general_purpose::STANDARD.encode(&pcm)
        );
        let mut decoder = Decoder::new();
        let events = decoder.decode(&message);
        assert!(events
            .iter()
            .any(|event| matches!(&event.event, Ev::AudioDelta { pcm: got } if got == &pcm)));
    }

    #[test]
    fn lifecycle_and_error_events_are_recognized() {
        let mut decoder = Decoder::new();
        assert!(matches!(
            decoder.decode(r#"{"type":"session.created"}"#)[0].event,
            Ev::SessionUpdated
        ));
        assert!(matches!(
            decoder.decode(r#"{"type":"session.updated"}"#)[0].event,
            Ev::SessionUpdated
        ));
        assert!(matches!(
            decoder.decode(r#"{"type":"session.closed"}"#)[0].event,
            Ev::Other { .. }
        ));

        let error = decoder.decode(
            r#"{"type":"error","error":{"code":"invalid_api_key","message":"Incorrect API key provided."}}"#,
        );
        assert_eq!(
            error[0].event,
            Ev::Error {
                code: Some("invalid_api_key".into()),
                message: "Incorrect API key provided.".into(),
            }
        );
    }
}
