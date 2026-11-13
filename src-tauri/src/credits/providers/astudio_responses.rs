//! OpenAI request conversion for AStudio's Responses API.
//! See NOTICE.md for the full contract and the measured event sequence.
use super::*;

fn invalid(message: &str) -> ProviderError {
    ProviderError { stage: "validation", ..ProviderError::new(400, message) }
}

fn user_content(content: &Value) -> Result<Vec<Value>, ProviderError> {
    if let Some(text) = content.as_str().filter(|s| !s.is_empty()) {
        return Ok(vec![json!({"type":"input_text","text":text})]);
    }
    let Some(parts) = content.as_array() else {
        return Ok(vec![json!({"type":"input_text","text":""})]);
    };
    let mut out = vec![];
    for part in parts {
        match part["type"].as_str() {
            Some("text") => {
                if let Some(text) = part["text"].as_str().filter(|s| !s.is_empty()) {
                    out.push(json!({"type":"input_text","text":text}));
                }
            }
            Some("image_url") => {
                let url = part["image_url"]["url"].as_str().ok_or_else(|| invalid("图片缺少 URL"))?;
                out.push(json!({"type":"input_image","image_url":url}));
            }
            _ => return Err(invalid("AStudio 仅支持文本和图片输入")),
        }
    }
    if out.is_empty() { out.push(json!({"type":"input_text","text":""})); }
    Ok(out)
}

pub(super) fn request_body(body: &Value, model: &ModelInfo) -> Result<Value, ProviderError> {
    // Responses API has no equivalent for these chat/completions parameters.
    for field in ["presence_penalty", "frequency_penalty", "seed", "logprobs", "stop"] {
        if body.get(field).is_some_and(|value| !value.is_null()) {
            return Err(invalid(&format!("AStudio 暂不支持 {field}")));
        }
    }
    if body.get("response_format").is_some_and(|value| !value.is_null()) {
        return Err(invalid("AStudio 暂不支持结构化输出"));
    }
    let mut instructions: Vec<String> = vec![];
    let mut input: Vec<Value> = vec![];
    for message in body["messages"].as_array().ok_or_else(|| invalid("messages 必须是数组"))? {
        match message["role"].as_str().unwrap_or("") {
            "system" | "developer" => {
                let text = message["content"].as_str().filter(|s| !s.is_empty())
                    .or_else(|| message["content"].as_array().and_then(|parts| {
                        parts.iter().find_map(|p| p["text"].as_str().filter(|s| !s.is_empty()))
                    }))
                    .ok_or_else(|| invalid("AStudio 系统指令仅支持文本"))?;
                instructions.push(text.to_owned());
            }
            "user" => {
                let content = user_content(&message["content"])?;
                input.push(json!({"type":"message","role":"user","content":content}));
            }
            "assistant" => {
                let mut content = vec![];
                match message["content"].as_str() {
                    Some(text) if !text.is_empty() => {
                        content.push(json!({"type":"output_text","text":text}));
                    }
                    _ => {
                        if let Some(parts) = message["content"].as_array() {
                            for part in parts {
                                if let Some(text) = part["text"].as_str().filter(|s| !s.is_empty()) {
                                    content.push(json!({"type":"output_text","text":text}));
                                }
                            }
                        }
                    }
                }
                if !content.is_empty() {
                    input.push(json!({"type":"message","role":"assistant","content":content}));
                }
                if let Some(calls) = message["tool_calls"].as_array() {
                    for call in calls {
                        let call_id = call["id"].as_str().filter(|id| !id.is_empty())
                            .ok_or_else(|| invalid("历史工具调用缺少 id"))?;
                        let name = call["function"]["name"].as_str()
                            .ok_or_else(|| invalid("历史工具调用缺少函数名"))?;
                        let arguments = call["function"]["arguments"].as_str().unwrap_or("{}");
                        input.push(json!({"type":"function_call","call_id":call_id,"name":name,"arguments":arguments}));
                    }
                }
            }
            "tool" => {
                let call_id = message["tool_call_id"].as_str().filter(|id| !id.is_empty())
                    .ok_or_else(|| invalid("工具结果缺少 tool_call_id"))?;
                let output = message["content"].as_str().unwrap_or("");
                input.push(json!({"type":"function_call_output","call_id":call_id,"output":output}));
            }
            _ => return Err(invalid("AStudio 消息角色不受支持")),
        }
    }
    let max_output_tokens = body["max_tokens"].as_u64()
        .or_else(|| body["max_completion_tokens"].as_u64())
        .unwrap_or(4096);
    if model.max_output_tokens.is_some_and(|maximum| max_output_tokens > maximum) {
        return Err(invalid("请求输出上限超过 AStudio 目录声明的最大输出 Token"));
    }
    let mut output = json!({
        "model": model.upstream_id.as_deref().unwrap_or(&model.id),
        "input": input,
        "max_output_tokens": max_output_tokens,
        "stream": true,
    });
    if !instructions.is_empty() {
        output["instructions"] = json!(instructions.join("\n"));
    }
    for field in ["temperature", "top_p"] {
        if let Some(value) = body.get(field) { output[field] = value.clone(); }
    }
    if let Some(tools) = body["tools"].as_array().filter(|tools| !tools.is_empty()) {
        output["tools"] = Value::Array(tools.iter().map(|tool| {
            let function = &tool["function"];
            let mut result = json!({"type":"function","name":function["name"],"parameters":function["parameters"]});
            if let Some(description) = function.get("description") { result["description"] = description.clone(); }
            result
        }).collect());
        let choice = &body["tool_choice"];
        output["tool_choice"] = match choice.as_str() {
            Some("none") => json!("none"),
            Some("required") => json!("required"),
            _ if choice.is_object() => json!({"type":"function","name":choice["function"]["name"]}),
            _ => json!("auto"),
        };
    }
    if model.supports_reasoning == Some(true) {
        if let Some(effort) = body["reasoning_effort"].as_str().or(model.default_reasoning_effort.as_deref()) {
            let effort = match effort { "disabled" | "none" => "none", other => other };
            if !model.reasoning_effort_options.is_empty()
                && !model.reasoning_effort_options.iter().any(|allowed| allowed == effort) {
                return Err(invalid("AStudio 推理等级不在当前模型目录允许的范围内"));
            }
            output["reasoning"] = json!({"effort": effort});
        }
    } else if body.get("reasoning_effort").is_some() {
        return Err(invalid("AStudio 当前模型未声明思考能力"));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spark() -> ModelInfo {
        let mut item = model("spark-x2.5".into(), "Spark X2.5".into(), Some(1048576));
        item.supports_images = Some(true);
        item.supports_reasoning = Some(true);
        item.reasoning_effort_options = vec!["none".into(), "high".into(), "max".into()];
        item.default_reasoning_effort = Some("high".into());
        item
    }
    #[test]
    fn converts_messages_tools_and_reasoning() {
        let image = "data:image/png;base64,c3ludGhldGlj";
        let input = json!({"max_completion_tokens":64,"reasoning_effort":"high",
            "messages":[{"role":"system","content":"sys"},{"role":"user","content":[{"type":"text","text":"hi"},{"type":"image_url","image_url":{"url":image}}]},
                {"role":"assistant","content":"call","tool_calls":[{"id":"c1","type":"function","function":{"name":"echo","arguments":"{\"v\":1}"}}]},
                {"role":"tool","tool_call_id":"c1","content":"ok"},{"role":"user","content":"continue"}],
            "tools":[{"type":"function","function":{"name":"echo","description":"e","parameters":{"type":"object"}}}],
            "tool_choice":{"type":"function","function":{"name":"echo"}}});
        let converted = request_body(&input, &spark()).unwrap();
        assert_eq!(converted["model"], "spark-x2.5");
        assert_eq!(converted["max_output_tokens"], 64);
        assert_eq!(converted["instructions"], "sys");
        assert_eq!(converted["stream"], true);
        assert_eq!(converted["reasoning"]["effort"], "high");
        let items = converted["input"].as_array().unwrap();
        assert_eq!(items[0]["type"], "message");
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[0]["content"][1]["type"], "input_image");
        assert_eq!(items[0]["content"][1]["image_url"], image);
        assert_eq!(items[1]["content"][0]["type"], "output_text");
        assert_eq!(items[2]["type"], "function_call");
        assert_eq!(items[2]["call_id"], "c1");
        assert_eq!(items[3]["type"], "function_call_output");
        assert_eq!(items[3]["call_id"], "c1");
        assert_eq!(items[3]["output"], "ok");
        assert_eq!(items[4]["role"], "user");
        assert_eq!(converted["tools"][0]["type"], "function");
        assert_eq!(converted["tools"][0]["name"], "echo");
        assert_eq!(converted["tool_choice"]["type"], "function");
        assert_eq!(converted["tool_choice"]["name"], "echo");
    }
    #[test]
    fn rejects_unsupported_parameters_and_invalid_efforts() {
        let base = json!({"messages":[{"role":"user","content":"hi"}]});
        for (key, value) in [("presence_penalty",json!(1)), ("frequency_penalty",json!(0.5)),
            ("seed",json!(42)), ("logprobs",json!(true)), ("stop",json!("x")),
            ("response_format",json!({"type":"json_object"}))] {
            let mut body = base.clone(); body[key] = value;
            assert_eq!(request_body(&body, &spark()).unwrap_err().status, 400, "accepted {key}");
        }
        let mut bad = base.clone(); bad["reasoning_effort"] = json!("turbo");
        assert!(request_body(&bad, &spark()).is_err());
        let no_reason = model("m".into(), "M".into(), None);
        assert!(request_body(&bad, &no_reason).is_err());
        let mut limited = spark(); limited.max_output_tokens = Some(16);
        let mut too_big = base.clone(); too_big["max_tokens"] = json!(64);
        assert!(request_body(&too_big, &limited).is_err());
        let mut none = base.clone(); none["reasoning_effort"] = json!("none");
        assert_eq!(request_body(&none, &spark()).unwrap()["reasoning"]["effort"], "none");
        let mut disabled = base.clone(); disabled["reasoning_effort"] = json!("disabled");
        assert_eq!(request_body(&disabled, &spark()).unwrap()["reasoning"]["effort"], "none");
    }
    #[test]
    fn tool_choice_strings_and_empty_assistant_content() {
        let body = json!({"messages":[{"role":"assistant","content":null,"tool_calls":[{"id":"c","type":"function","function":{"name":"f","arguments":"{}"}}]},{"role":"tool","tool_call_id":"c","content":"r"}],
            "tools":[{"type":"function","function":{"name":"f","parameters":{"type":"object"}}}],"tool_choice":"required"});
        let converted = request_body(&body, &spark()).unwrap();
        let items = converted["input"].as_array().unwrap();
        assert_eq!(items[0]["type"], "function_call");
        assert!(converted["input"].as_array().unwrap().iter().all(|item| item["type"] != "message" || item.get("content").is_none() || item["content"].as_array().unwrap().is_empty() || item["role"] != "assistant"));
        assert_eq!(converted["tool_choice"], "required");
    }
}
