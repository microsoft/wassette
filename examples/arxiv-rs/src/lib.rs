// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use serde::{Deserialize, Serialize};
use spin_sdk::http::{send, Request, Response};

#[allow(warnings)]
mod bindings;

struct Component;

#[derive(Debug, Deserialize, Serialize)]
struct ArxivFeed {
    #[serde(rename = "entry", default)]
    entries: Vec<ArxivEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArxivEntry {
    id: String,
    title: String,
    summary: String,
    published: String,
    updated: String,
    #[serde(rename = "author", default)]
    authors: Vec<ArxivAuthor>,
    #[serde(rename = "category", default)]
    categories: Vec<ArxivCategory>,
    #[serde(rename = "link", default)]
    links: Vec<ArxivLink>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArxivAuthor {
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArxivCategory {
    #[serde(rename = "@term")]
    term: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArxivLink {
    #[serde(rename = "@href")]
    href: String,
    #[serde(rename = "@type", default)]
    link_type: Option<String>,
}

impl bindings::Guest for Component {
    fn search_papers(
        query: String,
        max_results: u32,
        date_from: String,
        categories: String,
    ) -> Result<String, String> {
        spin_executor::run(async move {
            // Build the search query
            let mut search_query = query.clone();

            // Add category filter if provided
            if !categories.is_empty() {
                let cats: Vec<&str> = categories.split(',').collect();
                let cat_query = cats
                    .iter()
                    .map(|c| format!("cat:{}", c.trim()))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                search_query = if search_query.contains("cat:") {
                    search_query
                } else {
                    format!("({}) AND ({})", search_query, cat_query)
                };
            }

            // Add date filter if provided
            if !date_from.is_empty() {
                search_query = format!("{} AND submittedDate:[{} TO *]", search_query, date_from);
            }

            // Construct the arXiv API URL
            let url = format!(
                "http://export.arxiv.org/api/query?search_query={}&start=0&max_results={}&sortBy=submittedDate&sortOrder=descending",
                urlencoding::encode(&search_query),
                max_results
            );

            // Create and send request
            let request = Request::builder()
                .method(spin_sdk::http::Method::Get)
                .uri(&url)
                .build();

            let response: Response = send(request).await.map_err(|e| e.to_string())?;
            let status = response.status();

            if !(200..300).contains(status) {
                let body = String::from_utf8_lossy(response.body());
                return Err(format!(
                    "Request failed with status code: {}. Body: {}",
                    status, body
                ));
            }

            // Parse XML response
            let body = String::from_utf8_lossy(response.body());
            let feed = parse_arxiv_xml(&body)?;

            // Format results as markdown
            format_search_results(&query, &feed.entries)
        })
    }

    fn download_paper(id: String) -> Result<Vec<u8>, String> {
        spin_executor::run(async move {
            // Construct PDF URL
            let url = format!("http://arxiv.org/pdf/{}.pdf", sanitize_arxiv_id(&id));

            // Create and send request
            let request = Request::builder()
                .method(spin_sdk::http::Method::Get)
                .uri(&url)
                .build();

            let response: Response = send(request).await.map_err(|e| e.to_string())?;
            let status = response.status();

            if !(200..300).contains(status) {
                return Err(format!(
                    "Failed to download paper with status code: {}",
                    status
                ));
            }

            Ok(response.body().to_vec())
        })
    }

    fn read_paper(id: String) -> Result<String, String> {
        spin_executor::run(async move {
            // Construct API URL for single paper
            let url = format!(
                "http://export.arxiv.org/api/query?id_list={}",
                sanitize_arxiv_id(&id)
            );

            // Create and send request
            let request = Request::builder()
                .method(spin_sdk::http::Method::Get)
                .uri(&url)
                .build();

            let response: Response = send(request).await.map_err(|e| e.to_string())?;
            let status = response.status();

            if !(200..300).contains(status) {
                let body = String::from_utf8_lossy(response.body());
                return Err(format!(
                    "Request failed with status code: {}. Body: {}",
                    status, body
                ));
            }

            // Parse XML response
            let body = String::from_utf8_lossy(response.body());
            let feed = parse_arxiv_xml(&body)?;

            if feed.entries.is_empty() {
                return Err(format!("Paper not found: {}", id));
            }

            // Format paper details as markdown
            format_paper_details(&feed.entries[0])
        })
    }
}

fn sanitize_arxiv_id(id: &str) -> String {
    // Remove common prefixes and clean up the ID
    id.trim()
        .replace("http://arxiv.org/abs/", "")
        .replace("https://arxiv.org/abs/", "")
        .replace("arxiv:", "")
        .replace("arXiv:", "")
}

fn parse_arxiv_xml(xml: &str) -> Result<ArxivFeed, String> {
    // Simple XML parsing for arXiv API response
    let mut entries = Vec::new();

    // Split by entry tags
    let entry_parts: Vec<&str> = xml.split("<entry>").collect();

    for (i, part) in entry_parts.iter().enumerate() {
        if i == 0 {
            continue; // Skip the header
        }

        let entry_end = part.find("</entry>").unwrap_or(part.len());
        let entry_xml = &part[..entry_end];

        // Extract fields using simple string parsing
        let id = extract_tag(entry_xml, "id").unwrap_or_default();
        let title = extract_tag(entry_xml, "title")
            .unwrap_or_default()
            .replace("\n", " ")
            .trim()
            .to_string();
        let summary = extract_tag(entry_xml, "summary")
            .unwrap_or_default()
            .replace("\n", " ")
            .trim()
            .to_string();
        let published = extract_tag(entry_xml, "published").unwrap_or_default();
        let updated = extract_tag(entry_xml, "updated").unwrap_or_default();

        // Extract authors
        let mut authors = Vec::new();
        let author_parts: Vec<&str> = entry_xml.split("<author>").collect();
        for (j, author_part) in author_parts.iter().enumerate() {
            if j == 0 {
                continue;
            }
            if let Some(name) = extract_tag(author_part, "name") {
                authors.push(ArxivAuthor { name });
            }
        }

        // Extract categories
        let mut categories = Vec::new();
        let mut search_pos = 0;
        while let Some(cat_start) = entry_xml[search_pos..].find("<category term=\"") {
            let abs_start = search_pos + cat_start + 16;
            if let Some(cat_end) = entry_xml[abs_start..].find("\"") {
                let term = entry_xml[abs_start..abs_start + cat_end].to_string();
                categories.push(ArxivCategory { term });
                search_pos = abs_start + cat_end;
            } else {
                break;
            }
        }

        // Extract links
        let mut links = Vec::new();
        let mut search_pos = 0;
        while let Some(link_start) = entry_xml[search_pos..].find("<link ") {
            let abs_start = search_pos + link_start;
            if let Some(link_end) = entry_xml[abs_start..].find("/>") {
                let link_tag = &entry_xml[abs_start..abs_start + link_end];
                if let Some(href_pos) = link_tag.find("href=\"") {
                    let href_start = abs_start + href_pos + 6;
                    if let Some(href_end) = entry_xml[href_start..].find("\"") {
                        let href = entry_xml[href_start..href_start + href_end].to_string();
                        let link_type = if link_tag.contains("type=\"") {
                            link_tag.find("type=\"").and_then(|type_pos| {
                                let type_start = type_pos + 6;
                                link_tag[type_start..].find("\"").map(|type_end| {
                                    link_tag[type_start..type_start + type_end].to_string()
                                })
                            })
                        } else {
                            None
                        };
                        links.push(ArxivLink { href, link_type });
                    }
                }
                search_pos = abs_start + link_end;
            } else {
                break;
            }
        }

        entries.push(ArxivEntry {
            id,
            title,
            summary,
            published,
            updated,
            authors,
            categories,
            links,
        });
    }

    Ok(ArxivFeed { entries })
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let start_tag = format!("<{}>", tag);
    let end_tag = format!("</{}>", tag);

    if let Some(start_pos) = xml.find(&start_tag) {
        let content_start = start_pos + start_tag.len();
        if let Some(end_pos) = xml[content_start..].find(&end_tag) {
            return Some(xml[content_start..content_start + end_pos].to_string());
        }
    }
    None
}

fn format_search_results(query: &str, entries: &[ArxivEntry]) -> Result<String, String> {
    let mut markdown = String::new();
    markdown.push_str(&format!("# arXiv Search Results for: {}\n\n", query));
    markdown.push_str(&format!("Found {} papers\n\n", entries.len()));

    for (i, entry) in entries.iter().enumerate() {
        markdown.push_str(&format!("## {}. {}\n\n", i + 1, entry.title));

        // Extract arXiv ID from the ID URL
        let arxiv_id = entry.id.split('/').next_back().unwrap_or(&entry.id);
        markdown.push_str(&format!("**arXiv ID:** {}\n\n", arxiv_id));

        // Authors
        if !entry.authors.is_empty() {
            let author_names: Vec<String> = entry.authors.iter().map(|a| a.name.clone()).collect();
            markdown.push_str(&format!("**Authors:** {}\n\n", author_names.join(", ")));
        }

        // Categories
        if !entry.categories.is_empty() {
            let cat_names: Vec<String> = entry.categories.iter().map(|c| c.term.clone()).collect();
            markdown.push_str(&format!("**Categories:** {}\n\n", cat_names.join(", ")));
        }

        // Published date
        markdown.push_str(&format!(
            "**Published:** {}\n\n",
            entry
                .published
                .split('T')
                .next()
                .unwrap_or(&entry.published)
        ));

        // Abstract
        markdown.push_str(&format!("**Abstract:** {}\n\n", entry.summary));

        // Links
        for link in &entry.links {
            if link.link_type.as_deref() == Some("application/pdf") {
                markdown.push_str(&format!("**PDF:** {}\n\n", link.href));
            }
        }

        markdown.push_str("---\n\n");
    }

    if entries.is_empty() {
        markdown.push_str("No papers found matching your query.\n");
    }

    Ok(markdown)
}

fn format_paper_details(entry: &ArxivEntry) -> Result<String, String> {
    let mut markdown = String::new();
    markdown.push_str(&format!("# {}\n\n", entry.title));

    // Extract arXiv ID
    let arxiv_id = entry.id.split('/').next_back().unwrap_or(&entry.id);
    markdown.push_str(&format!("**arXiv ID:** {}\n\n", arxiv_id));

    // Authors
    if !entry.authors.is_empty() {
        let author_names: Vec<String> = entry.authors.iter().map(|a| a.name.clone()).collect();
        markdown.push_str(&format!("**Authors:** {}\n\n", author_names.join(", ")));
    }

    // Categories
    if !entry.categories.is_empty() {
        let cat_names: Vec<String> = entry.categories.iter().map(|c| c.term.clone()).collect();
        markdown.push_str(&format!("**Categories:** {}\n\n", cat_names.join(", ")));
    }

    // Dates
    markdown.push_str(&format!(
        "**Published:** {}\n\n",
        entry
            .published
            .split('T')
            .next()
            .unwrap_or(&entry.published)
    ));
    markdown.push_str(&format!(
        "**Updated:** {}\n\n",
        entry.updated.split('T').next().unwrap_or(&entry.updated)
    ));

    // Abstract
    markdown.push_str(&format!("## Abstract\n\n{}\n\n", entry.summary));

    // Links
    markdown.push_str("## Links\n\n");
    for link in &entry.links {
        if let Some(link_type) = &link.link_type {
            if link_type == "application/pdf" {
                markdown.push_str(&format!("- [PDF]({})\n", link.href));
            }
        } else {
            markdown.push_str(&format!("- [Abstract]({})\n", link.href));
        }
    }

    Ok(markdown)
}

bindings::export!(Component with_types_in bindings);

// URL encoding helper
mod urlencoding {
    pub fn encode(input: &str) -> String {
        input
            .chars()
            .map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
                ' ' => "+".to_string(),
                _ => {
                    let mut buf = [0; 4];
                    let s = c.encode_utf8(&mut buf);
                    s.bytes().map(|b| format!("%{:02X}", b)).collect::<String>()
                }
            })
            .collect()
    }
}
