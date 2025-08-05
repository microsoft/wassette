// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use spin_sdk::http::{send, Request, Response};

#[allow(warnings)]
mod bindings;

use bindings::Guest;
use serde_json::Value;

struct Component;

impl Guest for Component {
    fn fetch(url: String) -> Result<String, String> {
        spin_executor::run(async move {
            let request = Request::get(url);
            let response: Response = send(request).await.map_err(|e| e.to_string())?;
            let status = response.status();
            if !(200..300).contains(status) {
                return Err(format!("Request failed with status code: {}", status));
            }
            let body = String::from_utf8_lossy(response.body());

            if let Some(content_type) = response.header("content-type").and_then(|v| v.as_str()) {
                if content_type.contains("application/json") {
                    let json: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                    return Ok(json_to_markdown(&json));
                } else if content_type.contains("text/html") {
                    return Ok(html_to_markdown(&body));
                }
            }

            Ok(body.into_owned())
        })
    }
}

fn html_to_markdown(html: &str) -> String {
    let mut markdown = String::new();
    let fragment = scraper::Html::parse_fragment(html);
    let text_selector = scraper::Selector::parse("h1, h2, h3, h4, h5, h6, p, a, div").unwrap();

    for element in fragment.select(&text_selector) {
        let tag_name = element.value().name();
        let text = element.text().collect::<Vec<_>>().join(" ").trim().to_string();
        
        if text.is_empty() {
            continue;
        }

        match tag_name {
            "h1" => markdown.push_str(&format!("# {}\n\n", text)),
            "h2" => markdown.push_str(&format!("## {}\n\n", text)),
            "h3" => markdown.push_str(&format!("### {}\n\n", text)),
            "h4" => markdown.push_str(&format!("#### {}\n\n", text)),
            "h5" => markdown.push_str(&format!("##### {}\n\n", text)),
            "h6" => markdown.push_str(&format!("###### {}\n\n", text)),
            "p" => markdown.push_str(&format!("{}\n\n", text)),
            "a" => {
                if let Some(href) = element.value().attr("href") {
                    markdown.push_str(&format!("[{}]({})\n\n", text, href));
                } else {
                    markdown.push_str(&format!("{}\n\n", text));
                }
            },
            _ => markdown.push_str(&format!("{}\n\n", text)),
        }
    }

    markdown.trim().to_string()
}

fn json_to_markdown(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut markdown = String::new();
            for (key, val) in map {
                markdown.push_str(&format!("### {}\n\n{}\n\n", key, json_to_markdown(val)));
            }
            markdown
        }
        Value::Array(arr) => {
            let mut markdown = String::new();
            for (i, val) in arr.iter().enumerate() {
                markdown.push_str(&format!("1. {}\n", json_to_markdown(val)));
                if i < arr.len() - 1 {
                    markdown.push('\n');
                }
            }
            markdown
        }
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
    }
}
bindings::export!(Component with_types_in bindings);
