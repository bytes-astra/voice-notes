//! 火山 provider 的协议层单测：结果映射与热词清洗。

use super::*;

/// 官方示例的响应：两个已定稿的句子。
const SAMPLE: &str = r#"{
  "audio_info": {"duration": 3696},
  "result": {
    "text": "这是字节跳动， 今日头条母公司。",
    "utterances": [
      {"definite": true, "end_time": 1705, "start_time": 0, "text": "这是字节跳动，"},
      {"definite": true, "end_time": 3696, "start_time": 2110, "text": "今日头条母公司。"}
    ]
  }
}"#;

/// 已定稿的句子 → 定稿事件（不是实时行）。
#[test]
fn definite_sentence_maps_to_final() {
    let mut emitted = 0;
    let events = map_response(SAMPLE.as_bytes(), &mut emitted);
    let finals: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            VolcengineEvent::Final(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        finals,
        vec!["这是字节跳动，".to_string(), "今日头条母公司。".to_string()]
    );
    assert_eq!(emitted, 2);
}

/// 最后一条未定稿的句子 → 实时行（当前句的累计串）。
#[test]
fn indefinite_sentence_maps_to_partial() {
    let payload = r#"{"result":{"utterances":[{"definite":false,"text":"正在说"}]}}"#;
    let mut emitted = 0;
    let events = map_response(payload.as_bytes(), &mut emitted);
    let partials: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            VolcengineEvent::Partial(PartialUpdate::Preview { confirmed, draft }) => {
                Some(format!("{confirmed}{draft}"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(partials, vec!["正在说".to_string()]);
}

/// 同一句定稿后不再重复上抛（服务端会重复发累积结果）。
#[test]
fn finalized_sentence_is_not_emitted_twice() {
    let mut emitted = 0;
    let first = map_response(SAMPLE.as_bytes(), &mut emitted);
    assert_eq!(first.len(), 2);
    let second = map_response(SAMPLE.as_bytes(), &mut emitted);
    assert!(second.is_empty(), "已上抛过的句子不该再来一遍");
}

/// 热词：控制字符丢弃、超上限（100 条）截断。
#[test]
fn keywords_are_cleaned_by_volcengine_rules() {
    let mut words: Vec<String> = (0..120).map(|i| format!("词{i}")).collect();
    words.push("带\n控制符".into());
    let (clean, ignored) = clean_volcengine_keywords(&words);
    assert_eq!(clean.len(), MAX_KEYWORDS, "超上限要截断");
    assert_eq!(ignored, 1 + 20, "控制符 1 条 + 截断 20 条");
}

/// 术语表 → `corpus.context`（JSON 字符串）。
#[test]
fn keywords_encoded_as_corpus_context() {
    let context = encode_corpus_context(&["玄戒芯片".into()]).expect("应有 context");
    let value: Value = serde_json::from_str(&context).expect("应是合法 JSON");
    assert_eq!(value["hotwords"][0]["word"], "玄戒芯片");
    assert!(
        encode_corpus_context(&[]).is_none(),
        "没有热词就不带 context"
    );
}

/// 能力声明：服务端断句、不需要本地 VAD、16 kHz。
#[test]
fn capabilities_are_server_endpoint() {
    let caps = VolcengineProvider::CAPABILITIES;
    assert_eq!(caps.endpoint_kind, EndpointKind::ServerEndpoint);
    assert!(!caps.needs_local_vad, "服务端断句不需要本地 VAD");
    assert_eq!(caps.target_sample_rate, 16_000, "与内部轴一致，不重采样");
}

/// 首次全量请求的内容：模型名、音频参数、断句参数、热词。
#[test]
fn first_request_payload_is_correct() {
    let config = VolcengineConfig {
        base_url: String::new(),
        api_key: "k".into(),
        resource_id: String::new(),
        keywords: vec!["玄戒芯片".into()],
        itn: false,
        punct: true,
    };
    let value: Value =
        serde_json::from_slice(&encode_full_request(&config)).expect("应是合法 JSON");
    assert_eq!(value["request"]["model_name"], "bigmodel");
    assert_eq!(value["audio"]["format"], "pcm");
    assert_eq!(value["audio"]["rate"], 16000);
    assert_eq!(value["audio"]["bits"], 16);
    assert_eq!(value["audio"]["channel"], 1);
    assert_eq!(value["request"]["show_utterances"], true);
    assert_eq!(value["request"]["end_window_size"], 800);
    // ITN / 标点按设置走。
    assert_eq!(value["request"]["enable_itn"], false);
    assert_eq!(value["request"]["enable_punc"], true);
    // 热词在 corpus.context 里（JSON 字符串）。
    let context: Value =
        serde_json::from_str(value["request"]["corpus"]["context"].as_str().unwrap())
            .expect("context 应是 JSON");
    assert_eq!(context["hotwords"][0]["word"], "玄戒芯片");
}

/// 错误码映射：空音频 → 没有结果；等包超时 → 超时；其余 → 协议错误（带细节）。
#[test]
fn error_code_maps_to_dictionary_key() {
    assert_eq!(
        map_error_code(45000002, "empty audio", "k"),
        "cloud.noResult"
    );
    assert_eq!(map_error_code(45000081, "timeout", "k"), "cloud.timeout");
    let other = map_error_code(45000151, "bad format", "k");
    assert!(other.starts_with("cloud.protocolError"), "实际 {other}");
    assert!(other.contains("45000151"), "细节要带上错误码：{other}");
}

/// 真服务冒烟：连上火山 → 发一段真实语音 → 收中间结果与定稿 → 末包收尾。
///
///   VOICE_NOTES_VOLCENGINE_KEY=... cargo test --manifest-path src-tauri/Cargo.toml \
///     --lib asr::cloud::volcengine::tests::真服务冒烟 -- --nocapture
///
/// **没有环境变量就跳过**（打印一行「跳过：未设置…」，不打印 key）。
/// key 只从环境变量读 —— 不写进仓库、不进日志、不进断言消息。
#[test]
fn live_smoke() {
    let Ok(key) = std::env::var("VOICE_NOTES_VOLCENGINE_KEY") else {
        println!("跳过：未设置 VOICE_NOTES_VOLCENGINE_KEY");
        return;
    };
    let resource_id = std::env::var("VOICE_NOTES_VOLCENGINE_RESOURCE_ID")
        .unwrap_or_else(|_| DEFAULT_RESOURCE_ID.to_string());

    // 真实语音 fixture（16 kHz / 单声道 / 5.6 秒）。
    let wave = sherpa_onnx::Wave::read("tests/fixtures/zh.wav").expect("读取测试音频");
    let samples: Vec<f32> = wave.samples().to_vec();
    assert_eq!(wave.sample_rate(), 16_000, "fixture 应该是 16 kHz");

    let config = VolcengineConfig {
        base_url: String::new(),
        api_key: key,
        resource_id,
        keywords: vec!["玄戒芯片".into()],
        itn: true,
        punct: true,
    };
    use crate::asr::cloud::{CloudStreamingSession, StreamEvent};
    use crate::asr::AsrSession;
    let mut session = CloudStreamingSession::new(Box::new(VolcengineProvider::new(config)));
    session.connect().expect("应能连上火山");

    // 按 60 ms 一批喂进去（编排层就是这个节奏）。
    let mut finals: Vec<String> = Vec::new();
    let mut partials: Vec<String> = Vec::new();
    let chunk = 16_000 / 50; // 20 ms
    for part in samples.chunks(chunk) {
        session.advance_samples(part.len());
        for event in session.push(part) {
            match event {
                StreamEvent::Partial(text) => partials.push(text),
                StreamEvent::Segment(segment) => finals.push(segment.text),
            }
        }
        // 收一收后台事件（final 可能晚于音频到达）。
        for event in session.poll() {
            match event {
                StreamEvent::Partial(text) => partials.push(text),
                StreamEvent::Segment(segment) => finals.push(segment.text),
            }
        }
    }
    // 收尾：force_end 提交这一句，finish 发末包并等最终响应。
    for event in session.force_end() {
        match event {
            StreamEvent::Partial(text) => partials.push(text),
            StreamEvent::Segment(segment) => finals.push(segment.text),
        }
    }
    for event in session.finish() {
        match event {
            StreamEvent::Partial(text) => partials.push(text),
            StreamEvent::Segment(segment) => finals.push(segment.text),
        }
    }
    // 再等一会儿：最终响应可能还在路上（finish 内部已经等过一轮）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        for event in session.poll() {
            match event {
                StreamEvent::Partial(text) => partials.push(text),
                StreamEvent::Segment(segment) => finals.push(segment.text),
            }
        }
        if !finals.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    println!("中间结果条数：{}", partials.len());
    println!("定稿句子：{finals:?}");
    println!("最后一条中间结果：{:?}", partials.last());
    assert!(!finals.is_empty(), "应该至少收到一句定稿");
    assert!(
        finals.iter().any(|text| !text.trim().is_empty()),
        "定稿不应全是空的：{finals:?}"
    );
}

/// 服务端错误消息里的凭据要脱敏。
///
/// 走 `error_detail` —— 那就是**生产路径上的那个函数**（错误帧 → 错误详情），
/// 删掉它里面的 `sanitize` 调用，这条守卫就会红。
///
/// **必须用 `_` 分支的错误码**（如 `45000151`）—— `45000002` / `45000081`
/// 那两个码 `map_error_code` 根本不看 `message`，只断言「不含 key」会**恒真**
/// （凭据压根没进过详情）。
#[test]
fn server_error_message_redacts_secret() {
    let key = "sk-secret-123";
    let detail = error_detail(45000151, "上传失败：sk-secret-123", key);
    assert!(
        detail.starts_with("cloud.protocolError"),
        "错误详情要带上字典码：{}",
        detail
    );
    assert!(detail.contains("***"), "凭据要脱敏：{}", detail);
    assert!(
        !detail.contains(key),
        "凭据不能出现在错误详情里：{}",
        detail
    );
}

// ── provider 契约守卫（RC-K1 / RC-K2）────────────────────────────────────
//
// 两条不变量：
// - RC-K1：声明的 `partial_semantics` 与协议实际形态一致；
// - RC-K2：上抛的中间结果是当前未定稿句的**累计文本**（句内整体替换，不追加）。
//
// 守卫要躲开三种「恒真」写法（OpenSpec change `cloud-contract-guards` 的
// design D2）：① 期望值不能从样例里再读一遍（那是 `x == x`）；② 第二次更新
// 必须**改写**第一次（否则「替换」与「追加」结果相同）；③ 形态要从**样例
// 本身**推导，不能只钉住 `CAPABILITIES` 里的常量。

/// 第一次响应：一条已定稿 + 一条未定稿（草稿）。
///
/// 那条**已定稿**是必需的：它把 `emitted` 游标推进到 1，第二次响应里下标 0
/// 的句子才会被跳过（否则第二次会把已定稿句又上抛一遍）。
const FIRST_RESPONSE: &str = r#"{
  "result": {
    "utterances": [
      {"definite": true, "text": "已定稿。"},
      {"definite": false, "text": "你好世界"}
    ]
  }
}"#;

/// 第二次响应：全量返回（与真实协议一致），最后一条未定句被**改写**。
const SECOND_RESPONSE: &str = r#"{
  "result": {
    "utterances": [
      {"definite": true, "text": "已定稿。"},
      {"definite": false, "text": "你好"}
    ]
  }
}"#;

/// 上抛的中间结果文本（**过一遍归一器** —— 与 provider 实际走的是同一条路）。
///
/// 归一器算出的「当前句累计串」就是上层看到的东西；这里用同样的算法，
/// 守的仍然是「上抛给上层的文本」，不是「协议层长什么样」。
fn partial_texts(events: &[VolcengineEvent]) -> Vec<String> {
    let mut accumulator =
        PartialAccumulator::new(VolcengineProvider::CAPABILITIES.partial_semantics);
    let mut texts = Vec::new();
    for event in events {
        if let VolcengineEvent::Partial(update) = event {
            let text = accumulator.apply(None, update.clone());
            if !text.is_empty() {
                texts.push(text);
            }
        }
    }
    texts
}

/// 从两次中间结果推导协议形态。
///
/// **不接收任何声明值** —— 推导结果一旦被 `CAPABILITIES` 污染，断言就恒真。
///
/// 判据：第二次是第一次的**前缀扩展**（更长且以第一次为前缀）→ 增量拼接；
/// 否则 → 整句替换（火山的形态：确认前缀为空，整句就是草稿）。
///
/// 区间替换（`Range`）与整句替换在样例上分不开（没有区间信息就无从判断）；
/// 当前没有 provider 用它，推导不出 `Range` 是合理的。
fn derive_shape(first: &str, second: &str) -> PartialSemantics {
    if second.len() > first.len() && second.starts_with(first) {
        PartialSemantics::Delta
    } else {
        PartialSemantics::Preview
    }
}

/// 契约守卫：火山的中间结果是**整句替换**，声明的形态与样例相符。
#[test]
fn partial_is_a_whole_sentence_replacement() {
    // 两次响应用**同一个 `emitted` 游标**（会话状态，不能各自从头数）。
    let mut emitted = 0usize;
    let first = map_response(FIRST_RESPONSE.as_bytes(), &mut emitted);
    let second = map_response(SECOND_RESPONSE.as_bytes(), &mut emitted);

    let first_texts = partial_texts(&first);
    let second_texts = partial_texts(&second);
    assert_eq!(
        first_texts.len(),
        1,
        "volcengine/Preview: 第一次响应该有一条实时行"
    );
    assert_eq!(
        second_texts.len(),
        1,
        "volcengine/Preview: 第二次响应该有一条实时行"
    );

    let derived = derive_shape(&first_texts[0], &second_texts[0]);
    // 期望值是**手写字面量**：不从样例 JSON 里再读一遍 `utterances[..].text`。
    assert_eq!(
        first_texts[0], "你好世界",
        "volcengine/{:?}: 第一次上抛的中间结果应是整句草稿",
        derived
    );
    assert_eq!(
        second_texts[0], "你好",
        "volcengine/{:?}: 第二次上抛的中间结果应**改写**第一次（整句替换，不是追加）",
        derived
    );
    // 声明值与**推导出的**形态比较（不是只钉住常量）。
    assert_eq!(
        VolcengineProvider::CAPABILITIES.partial_semantics,
        derived,
        "volcengine/{:?}: 声明的形态与协议实际形态不符",
        derived
    );
}

/// 映射器产出的更新必须落在**声明的那一档**。
///
/// 上面的 `partial_is_a_whole_sentence_replacement` 守的是**上抛的文本**
/// —— 文本对了不代表归一档对了：`Preview` 与 `Delta` 在「第二次比第一次短」
/// 的样例上**表现一样**（都是替换），把档换成 `Delta` 它照样绿。
///
/// 这条直接看**映射器产出的更新变体**：火山声明 `Preview`，映射器就必须
/// 产 `PartialUpdate::Preview`；换成 `Delta` 就红。
#[test]
fn mapper_emits_the_declared_normalization_tier() {
    let payload = r#"{"result":{"utterances":[{"definite":false,"text":"正在说"}]}}"#;
    let mut emitted = 0;
    let events = map_response(payload.as_bytes(), &mut emitted);
    let tiers: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            VolcengineEvent::Partial(PartialUpdate::Preview { .. }) => Some("Preview"),
            VolcengineEvent::Partial(PartialUpdate::Delta(_)) => Some("Delta"),
            VolcengineEvent::Partial(PartialUpdate::Replace { .. }) => Some("Range"),
            _ => None,
        })
        .collect();
    assert_eq!(
        tiers,
        vec!["Preview"],
        "volcengine: 映射器产出的归一档与声明的 {:?} 不符",
        VolcengineProvider::CAPABILITIES.partial_semantics
    );
}

/// 句定稿后，下一句的草稿**不能**带上上一句的残留。
///
/// 火山没有句 id → 归一器走**单桶**。单桶的清空靠 `finish_sentence` ——
/// 不清的话，**增量档**的下一句会变成「上一句 + 下一句」。
///
/// **为什么这条对火山用不上文本断言**：火山的协议形态是 `Preview`（整句替换），
/// 草稿每次都被整体覆盖 —— 不清桶在文本上**看不出来**。所以这条直接测
/// **归一器的桶状态**（定稿后没有活跃句），那才是「清了」的证据。
///
/// 走完整流程（映射 → 归一 → 上层事件），与 provider 实际走的是同一条路。
#[test]
fn finalized_sentence_clears_the_normalizer_bucket() {
    let mut emitted = 0usize;
    let mut accumulator =
        PartialAccumulator::new(VolcengineProvider::CAPABILITIES.partial_semantics);

    // **先草稿、后定稿**（真实顺序：先说话，出草稿；停顿后定稿）。
    // 草稿在前时桶是活的，定稿后才该清掉。
    let payload = r#"{"result":{"utterances":[
        {"definite": false, "text": "正在说"},
        {"definite": true, "text": "正在说。"}
    ]}}"#;
    let events = map_response(payload.as_bytes(), &mut emitted);
    let normalized = normalize_events(&mut accumulator, events);
    assert!(
        normalized
            .iter()
            .any(|event| matches!(event, CloudEvent::Final { .. })),
        "响应里应有一条定稿"
    );

    // 定稿后桶必须清掉：下一句的草稿不能带上这一句的残留。
    assert!(
        accumulator.current().is_none(),
        "定稿后归一器的桶没清 —— 下一句的草稿会带上这一句的残留"
    );
}
