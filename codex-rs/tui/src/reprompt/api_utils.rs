//! Shared utilities for reprompt API calls.
//!
//! Contains the SSE output text extraction logic used by both the refinement
//! and insights API calls.

/// Extract the concatenated output text from an SSE response body.
///
/// Parses `data: {...}` lines from the streaming response, collecting text
/// deltas from `response.output_text.delta` events. Falls back to reading
/// `output_text` from the `response.completed` event if no deltas were
/// collected.
///
/// Returns an empty string if no output text was found.
pub(crate) fn extract_output_text_from_sse(body: &str) -> String {
    let mut output_text = String::new();
    for line in body.lines() {
        let line = line.trim();
        if !line.starts_with("data: ") {
            continue;
        }
        let data = &line["data: ".len()..];
        if data == "[DONE]" {
            break;
        }
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
            let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match event_type {
                "response.output_text.delta" => {
                    if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                        output_text.push_str(delta);
                    }
                }
                "response.completed" => {
                    if output_text.is_empty()
                        && let Some(resp) = event.get("response")
                        && let Some(text) = resp.get("output_text").and_then(|t| t.as_str())
                    {
                        output_text = text.to_string();
                    }
                }
                _ => {}
            }
        }
    }
    output_text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_deltas_from_sse_stream() {
        let body = "\
            data: {\"type\": \"response.output_text.delta\", \"delta\": \"hello \"}\n\
            data: {\"type\": \"response.output_text.delta\", \"delta\": \"world\"}\n\
            data: [DONE]\n";
        assert_eq!(extract_output_text_from_sse(body), "hello world");
    }

    #[test]
    fn falls_back_to_completed_event() {
        let body = "\
            data: {\"type\": \"response.completed\", \"response\": {\"output_text\": \"fallback text\"}}\n\
            data: [DONE]\n";
        assert_eq!(extract_output_text_from_sse(body), "fallback text");
    }

    #[test]
    fn returns_empty_for_no_output() {
        let body = "data: {\"type\": \"response.created\"}\ndata: [DONE]\n";
        assert_eq!(extract_output_text_from_sse(body), "");
    }

    #[test]
    fn skips_non_data_lines() {
        let body = "\
            event: message\n\
            data: {\"type\": \"response.output_text.delta\", \"delta\": \"ok\"}\n\
            \n\
            data: [DONE]\n";
        assert_eq!(extract_output_text_from_sse(body), "ok");
    }

    #[test]
    fn prefers_deltas_over_completed() {
        let body = "\
            data: {\"type\": \"response.output_text.delta\", \"delta\": \"from deltas\"}\n\
            data: {\"type\": \"response.completed\", \"response\": {\"output_text\": \"from completed\"}}\n\
            data: [DONE]\n";
        assert_eq!(extract_output_text_from_sse(body), "from deltas");
    }
}
