# qwen 实时翻译 WebSocket 协议：实测规格

> **这份文档的来源，按可信度排序：**
>
> 1. **旧 Python 客户端 `VRCQ\vrcq\cloud.py`** —— 上线跑过很久、踩过坑的东西。
>    凡是它发过、收过的字段，都是**实测确认**的。
> 2. **`crates/vox-core/src/cloud/protocol.rs`** —— 上面那份的逐条 Rust 移植，
>    带完整单测。它比旧版**多接了几个事件**（防御性补的），那些标了「推断」。
> 3. **阿里云百炼（DashScope）官方文档** —— 用来交叉核对，见 §10。
>
> 协议整体是 **OpenAI Realtime API 的形状**，但不完全一样。凡是本文档与
> OpenAI 官方文档冲突的地方，**以本文档为准**——因为这是实测的。
>
> **未确认的东西一律在 §10 单独列出**，并写清推断依据。别把推断当事实用。

---

## 1. 连接

代码里用的（**旧版实测可用**）：

```
wss://dashscope.aliyuncs.com/api-ws/v1/realtime?model=<模型名>
```

鉴权走 HTTP 头，**不是** query 参数（✅ 官方确认："使用 Bearer Token 鉴权"）：

```
Authorization: Bearer sk-xxxxxxxxxxxxxxxx
```

### ⚠️ 官方文档现在给的是另一个地址

官方《Realtime API》页面写的是**带 workspace 的分区域地址**：

```
wss://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/api-ws/v1/realtime      # 北京
wss://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/api-ws/v1/realtime  # 新加坡
```

两者的路径和 `?model=` 用法一致，只有 host 不同。

**以代码为准**：`dashscope.aliyuncs.com` 这个是旧版长期实测跑通的，
先不动。但要知道官方在推 workspace 地址，将来老地址可能下线，
或者某些新模型只在新地址上。**这是个已知的定时炸弹**，见 §10.6。

### 两个坑

**坑 1：模型名在 URL 里，不在 `session.update` 里。**
这直接决定了架构：**换模型必须重新连接**，没有别的办法。详见 §7。

**坑 2（Rust 侧特有）：** 建立连接时**必须**用 `IntoClientRequest` 构造请求再塞头，
直接把 URL 字符串传给 `connect_async` 会**把自定义头丢掉**，然后服务端以未鉴权拒绝你。
这个坑在 `vox-net/src/ws.rs` 里有注释钉住。Python 那边没这个问题
（`websockets` 库的 `additional_headers=` 参数直接就行）。

握手超时代码里定的是 **30 秒**。

---

## 2. 音频格式（两端不一样，别搞混）

| 方向 | 采样率 | 格式 | 声道 |
| --- | --- | --- | --- |
| **上行**（你发给服务端） | **16 000 Hz** | PCM16LE | 单声道 |
| **下行**（服务端发给你） | **24 000 Hz** | PCM16LE | 单声道 |

`session` 对象里两个格式字段都填字符串 `"pcm"`（不是 `"pcm16"`，
也不带采样率后缀）：

```json
"input_audio_format": "pcm",
"output_audio_format": "pcm"
```

采样率**不在协议里声明**，是隐含约定的 16k/24k。发错了不会报错，
只会让翻译结果变成鬼话（语速和音调都不对）。

### f32 → PCM16 的转换要照抄

```rust
let clipped = s.clamp(-1.0, 1.0);
let scaled = if clipped < 0.0 { clipped * 32768.0 } else { clipped * 32767.0 };
```

负半轴乘 **32768**、正半轴乘 **32767**。这样 `-1.0` 正好落在 `i16::MIN`，
`+1.0` 落在 `i16::MAX` 而**不会溢出翻符号**（用同一个 32768 会让 +1.0 变成 -32768，
听起来是一声爆音）。旧版就是这么写的，别"优化"成一个常数。

---

## 3. 上行事件

只有两种。**没有** `input_audio_buffer.commit`、
**没有** `response.create`——服务端自己靠 VAD 切段。

### 3.1 `session.update`

连上之后**第一件事**就发这个。之后热更新（换语言/换音色）也发这个。

```json
{
  "event_id": "event_1735689600000_1",
  "type": "session.update",
  "session": { ...见 §4... }
}
```

### 3.2 `input_audio_buffer.append`

音频**base64 编码**放在 `audio` 字段里。注意字段名就叫 `audio`，
不是 OpenAI 那边的 `delta`。

```json
{
  "event_id": "event_1735689600001_2",
  "type": "input_audio_buffer.append",
  "audio": "<base64 的 PCM16LE 16kHz 单声道>"
}
```

### 3.3 `event_id` 的生成

服务端只要求**唯一**。

- 旧 Python：`f"event_{int(time.time() * 1000)}"` —— **只有毫秒时间戳**。
  同一毫秒内发两帧就会撞号（音频帧是 20～100 ms 一发，撞的概率不高但存在）。
- Rust 版修掉了：`event_{毫秒}_{递增序号}`，同毫秒内也唯一。

---

## 4. `session` 对象完整字段表

### 4.1 两个家族共有

| 字段 | 类型 | 值 | 说明 |
| --- | --- | --- | --- |
| `modalities` | 字符串数组 | `["text","audio"]` 或 `["text"]` | 要不要语音输出。只要文字就填后者，**省一半 token** |
| `input_audio_format` | 字符串 | `"pcm"` | 固定 |
| `output_audio_format` | 字符串 | `"pcm"` | 固定 |
| `voice` | 字符串 | 如 `"Tina"` | **只在 `modalities` 含 audio 时才发**。不要语音时**不发这个字段**（不是发 null） |

### 4.2 LiveTranslate 家族（`qwen3.5-livetranslate-*`）

有**原生翻译参数**，这是首选家族。以下**全部已对官方《客户端事件》文档核实**
（2026-08-05）：

| 字段 | 类型 | 值 | 说明 |
| --- | --- | --- | --- |
| `translation.language` | 字符串 | `"ja"` | 目标语言代码，**默认 `en`**。这是翻译的开关 |
| `translation.corpus.phrases` | 对象 | 见下 | **热词表**：原文词 → 指定译法。我们**没用**，见 `DECISIONS.md` B10 |
| `input_audio_transcription.language` | 字符串 | `"zh"` | **源语言**。⭐ **不填 = 服务端自动识别**（官方原话见 §10.1） |
| `input_audio_transcription.model` | 字符串 | `"qwen3-asr-flash-realtime"` | 源文 ASR 模型。不填就不出源文转写 |
| `sample_rate` | 整数 | `8000` \| `16000` | **输入**采样率，默认 `16000`。我们不发，吃默认值正好 |
| `input_audio_format` | 字符串 | `"pcm"` \| `"opus"` | 默认 `pcm`。官方**还支持 opus**，我们只用 pcm |
| `output_audio_format` | 字符串 | `"pcm"` | 只能是 `pcm` |
| `enable_voice_clone` | 布尔 | 默认 `false` | 见 §4.4 |

LiveTranslate **没有 `instructions` 字段**（官方确认），别发。

`turn_detection` 这个模型**是支持的**（默认开着 server VAD，可以传 `null` 切手动模式）。
我们不发它，吃服务端默认值——**这是对的**，但理由是"默认值够用"，
不是"这模型不认这个字段"。LiveTranslate 的 `threshold` 默认是 **0.2**，
`silence_duration_ms` 默认是 **1000**。

> 早先版本的本文档写"LiveTranslate 不要发 `turn_detection`（它自己带 VAD）"，
> 前半句结论对、理由错，已按官方文档改正。

```json
{
  "modalities": ["text", "audio"],
  "input_audio_format": "pcm",
  "output_audio_format": "pcm",
  "translation": { "language": "ja" },
  "voice": "Tina"
}
```

**源语言这件事**：我们全程不发 `input_audio_transcription.language`，
所以走的是自动识别。官方明确支持显式指定，所以"给不给用户选源语言"
是个真选项，不是技术限制——见 `DECISIONS.md` B5。

### 4.3 声音复刻

开了复刻之后，`voice` **必须写死 `"default"`**，真正的音色由复刻结果决定：

```json
{
  "voice": "default",
  "enable_voice_clone": true,
  "voice_clone_options": { "frequency": "once" }
}
```

`frequency` 只有两个合法值：

| 值 | 含义 |
| --- | --- |
| `"once"` | 只用第一段声音复刻，之后固定不变 |
| `"always"` | 一直跟着说话人的声音走 |

设置里存的是**次数**（整数），映射规则：`0`/`None` → 不复刻，
`1` → `once`，`≥2` → `always`。

### 4.5 一个必须自己防的坑：目标语言不支持语音

**并非所有目标语言都能出语音。** 如果目标语言不支持语音输出，
而你还发了 `modalities: ["text","audio"]`，**服务端会拒掉整个会话**。

所以发之前必须自己判断，不支持就**降级成纯文字**：

```rust
fn audio_enabled(&self) -> bool {
    self.voice.is_some() && catalog::supports_audio_output(&self.target_language)
}
```

支持语音输出的语言，代码里列了 **29** 个（`catalog::AUDIO_OUTPUT_LANGUAGES`）：

```
zh en ar de fr es pt id it ko ru th vi ja tr hi ms nl ur nb sv da he fi pl is cs fil fa
```

> ✅ **已对官方模型页核实（2026-08-05），29 个代码逐个对得上，顺序都一样。**
> 官方原话：`qwen3.5-livetranslate-flash-realtime` 支持
> "60 种语言互译（其中 **29 种支持音频+文本输出**、31 种仅支持文本输出）"。
> 界面上实际可选的目标语言只有 13 个（`LANGUAGE_LABELS`），是这 29 个的子集。

**但这个列表只对 `qwen3.5-livetranslate-*` 成立**，而 `AUDIO_OUTPUT_LANGUAGES`
是**全局唯一一份**。老模型 `qwen3-livetranslate-flash-realtime-2025-09-22`
官方只支持 **18** 种：

```
en zh ru fr de pt es it id ko ja vi th ar yue hi el tr
```

注意它**不是**那 29 个的子集——`yue`（粤语）和 `el`（希腊语）在 29 里没有，
但在老模型的 18 里有。所以：

- 选老模型 + 目标语言 `sv`（瑞典语）→ 我们判断"支持语音"，实际不支持 → **服务端可能拒会话**。
- 选老模型 + 目标语言 `yue` → 我们判断"不支持"，白白降级成纯文字。

日常路径踩不到（默认模型是 3.5，界面 13 种语言里也没有 `sv`/`yue`），
但这是个真缺陷，见 `DECISIONS.md` B11。

---

## 5. 下行事件 ⚠️ 这一节是全文最容易出错的地方

### 5.0 LiveTranslate 的流式文字事件

官方事件名是 `response.text.text` 和 `response.audio_transcript.text`，字段为
`text` + `stash`。`text` 是已确认前缀，`stash` 是可能被后续改写的尾巴。
当前解码器已经接入这两种事件，并继续兼容旧服务曾返回的 `.delta` 形状。

`stash` 的语义（官方）：`text` 是**已确认**的前缀，`stash` 是**可能被改写**的尾巴，
界面上应该显示 `text + stash`，最终值以 `.done` 为准。

### 5.1 总表

下表是**我们代码现在接的**主要事件。

| 事件名 | 取哪个字段 | 含义 |
| --- | --- | --- |
| `response.text.text` | `text` + `stash` | 仅文本模式的流式译文 |
| `response.audio_transcript.text` | `text` + `stash` | 带译音模式的流式译文 |
| `response.audio.delta` | `delta`（base64） | 一段译文语音 |
| `response.audio_transcript.done` | `transcript` | 这句说完了 |
| `response.text.done` | `text` | 这句说完了 |
| `response.done` | `response.usage` | 一轮结束，带 token 用量 |
| `session.updated` | — | 会话配置已生效 |
| `error` | `error.{code,message}` | 服务端报错 |

**我们没接、但官方有的**（按重要性排）：

| 事件名 | 家族 | 为什么值得管 |
| --- | --- | --- |
| `session.created` | 连上后**第一个**事件，带服务端默认值。可以用它确认握手成功 |
| `session.finished` | 优雅收尾的回执 |
| `conversation.item.input_audio_transcription.*` | **源文**转写（带 `language`、`emotion`） |
| `input_audio_buffer.speech_started` / `.speech_stopped` | 服务端 VAD 的判定结果 |
| `response.audio.done` | 这句的语音发完了 |

`response.output_text.delta` / `.done` 这两个名字**官方没有**，
是旧版从 OpenAI 形状带过来的。留着不亏（多一个 match 分支不要钱），
但别指望它们会来。

### 5.2 坑 1：文字增量有三个事件名，字段名还不一样

这是最阴的一条。三个事件都可能出现，**少接一个就整段丢字幕**：

| 事件名 | 字段名 |
| --- | --- |
| `response.audio_transcript.delta` | `transcript` |
| `response.text.delta` | `delta` |
| `response.output_text.delta` | `delta` |

**transcript 类的文字在 `transcript` 里，text 类的在 `delta` 里。**
按事件名去取对应字段，别写一个通用的"随便找个字符串字段"。

done 事件同理，而且**又换了一个字段名**：

| 事件名 | 字段名 |
| --- | --- |
| `response.audio_transcript.done` | `transcript` |
| `response.text.done` | **`text`** |
| `response.output_text.done` | **`text`** |

也就是说 `.delta` 用 `delta`，但 `.done` 用 `text`。没有规律，认命照抄。

### 5.3 坑 2：线上是片段，但 `TextDelta` 对外吐的是**整句**

这里有两层，**分清楚了才不会写错**：

- **线上（wire）**：服务端给的是**增量片段**，跟 OpenAI 一样。
  官方服务端事件文档把 `delta` 明确写成"返回的增量文本"/"增量文本"。
- **内核 API**：解码器**自己累加**，`ServerEvent::TextDelta { text }` 里的 `text`
  是**到目前为止拼好的整句**，不是刚来的那几个字。

服务端一小片一小片地给（`"こん"`、`"にち"`、`"は"`），
我们的解码器**内部累加**，然后**对外吐到目前为止拼好的整句**：

| 服务端给的 | 解码器对外吐的 |
| --- | --- |
| `こん` | `こん` |
| `にち` | `こんにち` |
| `は` | `こんにちは` |

代码依据：`Decoder::accumulate()` 是 `self.parts.push_str(piece)` 然后
`TextDelta { text: self.parts.clone() }`；测试
`all_three_delta_event_names_accumulate_into_one_sentence` 把 `こん` → `こんにち`
→ `こんにちは` 钉死了。

**为什么这么设计**：字幕层要的是"当前这句话是什么"，不是"刚才多了哪几个字"。
让解码器累加，字幕层就能无脑整句替换，不用自己维护拼接状态。
累加状态只有一份（`Decoder::parts`），谁也别再自己攒一份。

**所以前端/字幕层收到 `subtitle_delta` 时，`text` 是完整的一句，
直接整句替换，不要 append。** 这条在 `DECISIONS.md` 的后端契约里也钉了一遍。

### 5.4 坑 3：done 事件里的文字**可能是空的**

服务端有时候发 `{"type":"response.audio_transcript.done","transcript":""}`。
如果直接用它的值，字幕就**被清空了**——用户看着字打出来，然后一瞬间全没了。

正确做法：**空的时候拿累积值兜底**。

```rust
let final_text = match text.filter(|t| !t.is_empty()) {
    Some(t) => t.to_string(),
    None => std::mem::take(&mut self.parts),  // 拿累积的凑
};
```

### 5.5 坑 4：解不开的音频**不要伪造静音**

`response.audio.delta` 的 `delta` 是 base64。如果解不开（或者解出来是空的），
**当未知事件丢掉**，不要塞一段静音给播放器——那会在译文语音里插入一段
听得出来的断裂。代码里这条有单测钉住
（`broken_audio_delta_does_not_become_fake_silence`）。

### 5.6 `response.done` 与 usage

```json
{
  "type": "response.done",
  "response": {
    "usage": { "input_tokens": 30, "output_tokens": 12, "total_tokens": 42 }
  }
}
```

三个字段**全部可缺**。缺 `total_tokens` 时自己加
（`resolved_total() = input + output`）。

**官方位置就是 `response.usage`**（✅ 已对官方《服务端事件》文档核实）。
Rust 版额外多看一眼顶层：

```rust
value.get("response").and_then(|r| r.get("usage"))
     .or_else(|| value.get("usage"))
```

> 顶层 `usage` 这个兜底是 **Rust 版推断补的**——旧 Python 只读
> `response.usage`（`event.get("response", {}).get("usage", {})`），
> 官方文档里也**只有** `response.usage`。属于白拿的保险，不花钱，留着。

**官方还有两个我们没读的细分字段**（✅ 已核实存在）：

```json
"usage": {
  "total_tokens": 377, "input_tokens": 336, "output_tokens": 41,
  "input_tokens_details":  { "text_tokens": 228, "audio_tokens": 108 },
  "output_tokens_details": { "text_tokens": 9,   "audio_tokens": 32  }
}
```

我们现在只记三个总数。**如果音频 token 和文字 token 单价不同，
按总数估的钱就是错的**——见 `DECISIONS.md` 待拍板 B8。
注意 `output_tokens_details` 里的 `audio_tokens` **可能整个缺失**
（纯文字回复时），别假设它一定在。

`response.done` 还有一个副作用：**它标志一轮结束，要把没说完的半句作废**
（`self.parts.clear()`），否则下一轮的第一个增量会接在上一轮的残句后面。

### 5.7 错误事件（**已对官方文档核实**）

⚠️ 旧 Python 客户端完全没有错误处理——它的 handler 字典里
根本没注册 `error`，服务端报错就静默丢掉。所以这一节**不是**从旧版实测来的。
但 **2026-08-05 已对官方《服务端事件》文档核实**，形状确认如下：

```json
{
  "event_id": "event_RoUu4T8yExPMI37GKwaOC",
  "type": "error",
  "error": {
    "type": "invalid_request_error",
    "code": "invalid_value",
    "message": "Invalid modalities: ['audio']. Supported combinations are: ['text'] and ['audio', 'text'].",
    "param": "session.modalities"
  }
}
```

要点：

- **是嵌套的**，`error` 是子对象。`event_id` 和 `type` 是它的**兄弟**，不在里面。
- 子对象里有**四个**字段：`type`、`code`、`message`、`param`。
- `param` 指出是哪个参数错了（例：`session.modalities`）。
  **我们现在没读 `param`**——只读 `code` + `message`。
  这不是 bug，但 `param` 对排查配置错误很有用，值得以后加进日志。

Rust 版兼容"嵌套"和"平铺"两种形状：

```rust
value.get("error").unwrap_or(&value)   // 有 error 子对象就进去取，没有就顶层取
```

嵌套那条现在是**官方确认的正路**；平铺那条是白送的保险，留着不亏。
`message` 缺失时给一句兜底文案，**绝不能让错误提示是空的**。

**顺带确认的一条**：`session.update` 失败时，服务端**不发** `session.updated`，
而是发 `error`。所以不能只等 `session.updated`，也得听 `error`。

### 5.8 致命错误 vs 可重试错误

不是所有错误都该重连。**这些是致命的，重连一万次也没用，直接停下来告诉用户**：

```
invalidapikey        invalidauthorization   authenticationerror
accessdenied         modelnotfound          invalidmodel
arrearage            insufficientquota      allocatedquotaexceeded
```

比对方式很重要：**先小写、再删掉所有非字母数字字符**再比。
因为服务端同一个错误会用好几种拼法回来——`InvalidApiKey`、`invalid_api_key`、
`INVALID_API_KEY` 都见过。归一化之后它们都变成 `invalidapikey`。

其它错误（网络抖动、超时、5xx）走**指数退避重连**：
首次 400 ms，每次翻倍，**上限 15 s**。

---

## 6. 会话生命周期

```
连接（带 Authorization 头）
   │
   ├─→ 立刻发 session.update              ← 必须第一件事
   │
   ├─← session.updated                    ← 配置生效了
   │
   ├─→ input_audio_buffer.append × N      ← 持续灌音频
   │
   ├─← response.*.delta × N               ← 译文一点点回来（片段；解码器累加成整句）
   ├─← response.audio.delta × N           ← 译文语音一段段回来
   ├─← response.*.done                    ← 这句完了
   ├─← response.done                      ← 这轮完了，带 usage
   │
   └─→ 关闭
```

### 握手之前的音频要**丢掉**，不要排队

会话还没握手完（没收到 `session.updated`）就来的音频帧，**直接丢**。

理由：排队攒着然后一次性灌进去，会让服务端收到一大坨"过去的声音"，
翻出来的是几秒前说的话，比丢掉更糟。代码里 `Session::audio_frame`
就是这个行为。

---

## 7. 热更新：什么能改，什么不能

`session.update` 可以**在会话中途重发**，用来改配置而不断线。

| 想改什么 | 能不能热改 | 怎么做 |
| --- | --- | --- |
| 目标语言 | ✅ 能 | 重发 `session.update`，改 `translation.language` |
| 音色 | ✅ 能 | 重发 `session.update`，改 `voice` |
| 要不要语音输出 | ✅ 能 | 重发 `session.update`，改 `modalities` |
| 声音复刻开关/频次 | ✅ 能 | 重发 `session.update` |

模型由 `catalog/aliyun.json` 固定为唯一的专用实时翻译模型，界面不提供模型选择。
如果以后维护表更换模型 ID，需要发布新版并重开 WebSocket。

另外两条实现细节（`cloud/mod.rs` 的 `hot_update`，都有测试钉住）：

1. **没变化就别发**。新值跟旧值一样时 `hot_update` 返回 `None`——省一次往返，
   也避免服务端重新初始化会话状态。
2. **还没握手就别发，但要把参数记下来**。握手前调 `hot_update` 同样返回 `None`，
   新值写进 `params`，由后面的 `handshake()` 一次带上。
   所以「连接还在起、用户就改了语言」这个竞态是安全的——**不会丢改动，也不会发废帧**。

---

## 8. 心跳与保活

代码现状：**没有主动心跳**。只靠 tungstenite **自动回应服务端的 Ping**
（tungstenite 0.30 会自动回 Pong，不用我们管）。

`recv` 用的是 `tokio::time::timeout` 包装，所以**超时返回 `Ok(None)` 而不是错误**——
"这段时间没消息"是正常状态，不能当成连接坏了去触发重连。这个区分很重要，
搞错了会在没人说话时疯狂重连。

### ✅ 官方确认：单次会话最长 120 分钟

官方《Realtime API》原话：**"单次会话最长可持续 120 分钟，达到此上限后服务将主动关闭连接。"**

所以长时间挂着的连接**一定会被踢**，不是 if 而是 when。好消息是我们已经能扛：

服务端主动关闭走的是 `Incoming::Closed`，而 `Closed` **完全不经过
`is_fatal_error()`**（那个函数只判 `error` 事件的 code）。所以它落在
**可重试**分支上 → 走 400 ms 起步的退避重连。代码依据：
`vox-net/src/ws.rs` 把 `Message::Close` 转成 `ReaderMsg::Closed`，
`cloud/mod.rs` 的 `is_fatal_error` 只吃 `(code, message)`。

**代价**：两小时一次、几百毫秒的字幕断档。挂机场景下用户基本察觉不到。
不需要为此做主动心跳。

## 9. 与 OpenAI Realtime API 的差异（别照着 OpenAI 的文档写）

| | OpenAI Realtime | qwen（本协议） |
| --- | --- | --- |
| 模型指定 | `session.update` 里的 `model` | **URL query `?model=`** |
| 音频上行字段 | `audio` | `audio`（一样） |
| 提交音频 | 要发 `input_audio_buffer.commit` | **不用**，服务端自己 VAD 切段 |
| 触发回复 | 要发 `response.create` | **不用** |
| 增量语义（线上） | 增量 = 新片段 | **一样**，也是新片段 |
| 增量语义（我们内核 API） | — | `TextDelta` 吐**整句累计**，是我们自己加的一层（见 §5.3） |
| 翻译 | 靠 `instructions` | LiveTranslate 有**原生 `translation.language`** |
| 音频格式串 | `pcm16` | **`pcm`** |
| 下行采样率 | 24 kHz | 24 kHz（一样） |
| 上行采样率 | 24 kHz | **16 kHz**（不一样！） |

**最容易踩的**：上行采样率不是 24k 而是 **16k**，以及格式串是 `pcm` 不是 `pcm16`。

---

## 10. 核实清单（2026-08-05 调研结果）

调研方式：直接抓阿里云官方文档页面。
**WebSearch 这次全程失效**（任何查询都返回零结果），所以是靠 URL 逐层
爬文档树进去的，路径记在 §10.8 备查。

### 10.1 ✅ 源语言支持自动识别 —— 已确认

官方《LiveTranslate 客户端事件》原话：

> `input_audio_transcription.language` —— 翻译源语种。
> **默认不填写，此时模型会自动识别源语种。**

- 我们**全程不发**这个字段 → 走自动识别。行为和旧版一致。
- 官方**也支持显式指定**源语种，用的是那份 60 语言代码表
  （"下表中的语种代码可用于指定源语种与目标语种"）。
- 转写事件会**回报检测到的 `language`**，也就是说自动识别的结果是**可读的**。
  想做"识别错了给用户提示"的话，材料是有的。

⚠️ 仍未确认：短句 / 中日混说时的识别准确率。这个只能实测，文档不会写。
决策见 `DECISIONS.md` B5。

### 10.2 ⚠️ 一个 key 能同时开几条 WS —— 官方**没有**并发条数限制

这是任务里标"最关键"的一条。结论：

- 《Realtime API》页面提到并发时，**只是把你转走**："并发限流条件请参考限流"。
- 《限流》页面**通篇只有 RPM 和 TPM 两个维度**，
  **没有任何"并发连接数"或"并发会话数"的字段**。
- 所以：**没有查到条数限制。**

顺带查到两条**真限制**，比条数限制更该关心：

| 维度 | 值 | 谁的额度 |
| --- | --- | --- |
| RPM | **60**（realtime 系列，北京） | **主账号**，不是 key |
| TPM | **100 000**（北京）；老 turbo 系在新加坡只有 **10 000** | 同上 |

官方原话："限流按主账号维度计算，账号下所有 RAM 子账号、业务空间和
**API Key 的调用量合并计算**。"另外"不同模型限流额度相互独立"。

**推断（注意是推断）**：既然限流按请求数算，一条 WS 连接大概只算 1 次请求，
那 2 条连接 = 2 RPM，离 60 差得远，**开 2 条应该没问题**。

**但 TPM 是真瓶颈**：2 条连接共享同一个 100 000 TPM 池子。
按官方计费口径，LiveTranslate-Flash 的音频是
**输入 7 token/秒、输出 12.5 token/秒**。粗算一条连接满负荷说话
≈ 20 token/秒 ≈ 1 200 token/分钟，两条 ≈ 2 400 token/分钟——
离 100 000 也差得远。**所以日常用量下 TPM 也不是问题。**

还有一条**要小心的**：官方说限流"可能按秒级 RPS（RPM/60）与 TPS（TPM/60）执行"，
而且快速起量会撞一个单独的保护，报 `Request rate increased too quickly`。
60 RPM ÷ 60 = **1 RPS**。**我们开机时两条连接几乎同时发起握手 → 瞬时 2 RPS**，
理论上可能被这个保护打中。**建议两条连接的握手之间错开 300～500 ms**，
反正用户也感觉不到。

⚠️ **仍未实测**：没有真 key 抓过"同时开 2 条"。文档没禁止 ≠ 服务端不拦。

### 10.3 ✅ 空闲超时 / 心跳 —— 已确认，且我们已经扛得住

- **没有**空闲超时的说法，官方文档里查不到。
- **有一个硬上限**：单次会话最长 **120 分钟**，到点服务端主动断。
- **不需要主动心跳**：tungstenite 自动回 Pong；120 分钟那次断连会落到
  可重试分支，自动退避重连。
- ⚠️ `idle_timeout_ms` **不是**连接超时，是"模型主动搭话"，**绝对别开**。

细节和代码依据见 §8。

### 10.4 ✅ 29 个支持语音输出的语言 —— 已确认，逐个对得上

官方模型页原话："60 种语言互译（其中 **29 种支持音频+文本输出**、
31 种仅支持文本输出）"，29 个代码和 `catalog::AUDIO_OUTPUT_LANGUAGES`
**完全一致，顺序都一样**。

⚠️ **但发现一个真缺陷**：这 29 个只对 3.5 成立，老模型
`qwen3-livetranslate-flash-realtime-2025-09-22` 只支持 18 种，
而且**不是子集**（多了 `yue`/`el`，少了一堆）。我们只有一份全局列表。
详见 §4.5，决策见 `DECISIONS.md` B11。

### 10.5 ✅ 错误事件和 usage 的官方字段名 —— 已确认

- **错误**：嵌套形状，`error` 子对象里有 `type` / `code` / `message` / **`param`**，
  `event_id` 和 `type` 是它的兄弟。我们没读 `param`（不是 bug，但值得加日志）。
  详见 §5.7。
- **usage**：位置就是 `response.usage`（**没有**顶层 `usage`，我们那个兜底是白送的保险）。
  除了三个总数，官方还有 **`input_tokens_details` / `output_tokens_details`**，
  里面分 `text_tokens` / `audio_tokens`——**我们没读**。
  又因为音频和文字的计费口径不同（音频按秒折算 token），
  只记总数**可能算不准钱**。详见 §5.6，决策见 `DECISIONS.md` B8。

### 10.6 ⚠️ 端点地址 —— 我们用的和官方现在写的不一样

官方现在写 `wss://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/...`，
我们用 `wss://dashscope.aliyuncs.com/...`。

- **以代码为准**（旧版长期实测跑通）。
- 未确认：老地址会不会下线、新模型是否只在新地址上。
- 这是个定时炸弹，但**现在不该动**——动了要重测，而且没有真 key 也测不了。

### 10.7 🔴 未确认里最要紧的一条：LiveTranslate 的流式事件名

见 §5.0。官方说默认模型走 `.text` + `stash`，我们只接 `.delta`。
静态证据互相矛盾，**必须拿真 key 抓一次报文**。
这是所有未确认项里**唯一会影响可见功能**的一条。

### 10.8 调研留痕（下次接着查用）

| 页面 | URL |
| --- | --- |
| 限流（RPM/TPM，唯一的并发口径） | `help.aliyun.com/zh/model-studio/rate-limit` |
| Realtime API 总览（端点、120 分钟、上下文上限） | `help.aliyun.com/zh/model-studio/realtime` |
| **LiveTranslate 客户端事件** | `help.aliyun.com/zh/model-studio/live-translator-client-events` |
| **LiveTranslate 服务端事件** | `help.aliyun.com/zh/model-studio/live-translator-server-events` |
| **模型页（60 语言表、计费口径）** | `help.aliyun.com/zh/model-studio/qwen3-5-livetranslate-flash-realtime` |

从零找路：`model-api-reference/` → `音频` → `语音翻译` → `实时音视频翻译`。
LiveTranslate 的文档位于实时音视频翻译分类下。
