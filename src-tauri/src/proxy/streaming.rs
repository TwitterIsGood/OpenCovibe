use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::IntoResponse;
use axum::response::Response;
use futures_util::StreamExt;

/// Same-protocol SSE passthrough: pipe upstream bytes directly to the client.
pub fn passthrough_stream(
    upstream: reqwest::Response,
    content_type: String,
) -> Response {
    let stream = upstream.bytes_stream().map(|result| {
        result.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    });
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_str(&content_type).unwrap());
    headers.insert("cache-control", HeaderValue::from_static("no-cache"));
    headers.insert("connection", HeaderValue::from_static("keep-alive"));

    (headers, body).into_response()
}

/// Cross-protocol streaming: translate OpenAI SSE events to Anthropic SSE events.
pub fn translate_openai_to_anthropic_stream(upstream: reqwest::Response) -> Response {
    let stream = upstream.bytes_stream().scan(
        String::new(),
        |buffer: &mut String, chunk_result: Result<axum::body::Bytes, reqwest::Error>| {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("[proxy/stream] chunk error: {e}");
                    return std::future::ready(Some(Ok::<_, std::io::Error>(axum::body::Bytes::new())));
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Safety: abort if buffer grows beyond 1 MB (malformed upstream)
            if buffer.len() > 1_048_576 {
                log::error!("[proxy/stream] SSE buffer exceeded 1MB, aborting stream");
                return std::future::ready(None);
            }

            let mut output_events = Vec::new();

            // Process complete SSE events from buffer
            while let Some(pos) = buffer.find("\n\n") {
                let event_text = buffer[..pos].to_string();
                buffer.drain(..pos + 2);

                for line in event_text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        let data = data.trim();
                        if data == "[DONE]" {
                            output_events.push("event: message_stop\n".to_string());
                            output_events
                                .push("data: {\"type\":\"message_stop\"}\n\n".to_string());
                            continue;
                        }

                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(translated) =
                                crate::proxy::translator::translate_openai_sse_chunk(&value)
                            {
                                for event in translated {
                                    output_events.push(event);
                                }
                            }
                        }
                    }
                }
            }

            let output = output_events.concat();
            std::future::ready(Some(Ok(axum::body::Bytes::from(output))))
        },
    );

    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("text/event-stream"));
    headers.insert("cache-control", HeaderValue::from_static("no-cache"));
    headers.insert("connection", HeaderValue::from_static("keep-alive"));

    (headers, body).into_response()
}
