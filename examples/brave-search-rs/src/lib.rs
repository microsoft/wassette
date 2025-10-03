// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use spin_sdk::http::{send, Request, Response};
use serde::{Deserialize, Serialize};

#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

#[derive(Debug, Deserialize, Serialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: Option<WebResults>,
    #[serde(default)]
    news: Option<NewsResults>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WebResults {
    #[serde(default)]
    results: Vec<WebResult>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WebResult {
    title: String,
    url: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct NewsResults {
    #[serde(default)]
    results: Vec<NewsResult>,
}

#[derive(Debug, Deserialize, Serialize)]
struct NewsResult {
    title: String,
    url: String,
    #[serde(default)]
    description: Option<String>,
}

impl Guest for Component {
    fn search(query: String) -> Result<String, String> {
        spin_executor::run(async move {
            // Get API key from environment variable
            let api_key = std::env::var("BRAVE_SEARCH_API_KEY")
                .map_err(|_| "BRAVE_SEARCH_API_KEY environment variable not set".to_string())?;

            // Construct the Brave Search API URL
            let url = format!(
                "https://api.search.brave.com/res/v1/web/search?q={}",
                urlencoding::encode(&query)
            );

            // Create request with API key header
            let request = Request::builder()
                .method(spin_sdk::http::Method::Get)
                .uri(&url)
                .header("X-Subscription-Token", &api_key)
                .header("Accept", "application/json")
                .build();

            // Send the request
            let response: Response = send(request).await.map_err(|e| e.to_string())?;
            let status = response.status();

            if !(200..300).contains(status) {
                let body = String::from_utf8_lossy(response.body());
                return Err(format!(
                    "Request failed with status code: {}. Body: {}",
                    status, body
                ));
            }

            // Parse the response
            let body = String::from_utf8_lossy(response.body());
            let search_response: BraveSearchResponse =
                serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {}", e))?;

            // Format results as markdown
            let mut markdown = String::new();
            markdown.push_str(&format!("# Search Results for: {}\n\n", query));

            // Add web results
            if let Some(web) = &search_response.web {
                if !web.results.is_empty() {
                    markdown.push_str("## Web Results\n\n");
                    for (i, result) in web.results.iter().enumerate() {
                        markdown.push_str(&format!("{}. **[{}]({})**\n", i + 1, result.title, result.url));
                        if let Some(desc) = &result.description {
                            markdown.push_str(&format!("   {}\n\n", desc));
                        } else {
                            markdown.push('\n');
                        }
                    }
                }
            }

            // Add news results
            if let Some(news) = &search_response.news {
                if !news.results.is_empty() {
                    markdown.push_str("## News Results\n\n");
                    for (i, result) in news.results.iter().enumerate() {
                        markdown.push_str(&format!("{}. **[{}]({})**\n", i + 1, result.title, result.url));
                        if let Some(desc) = &result.description {
                            markdown.push_str(&format!("   {}\n\n", desc));
                        } else {
                            markdown.push('\n');
                        }
                    }
                }
            }

            if markdown.trim().ends_with(&format!("# Search Results for: {}", query)) {
                markdown.push_str("\nNo results found.\n");
            }

            Ok(markdown)
        })
    }
}

bindings::export!(Component with_types_in bindings);

// urlencoding helper since we can't use external crate for this simple case
mod urlencoding {
    pub fn encode(input: &str) -> String {
        input
            .chars()
            .map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
                ' ' => "+".to_string(),
                _ => format!("%{:02X}", c as u8),
            })
            .collect()
    }
}
