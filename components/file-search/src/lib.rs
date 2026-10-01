// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! `wassette:file-search`, a default Wassette tool that searches file contents
//! with the ripgrep search crates.
//!
//! The component has no ambient filesystem access: it can only read the
//! directories the host preopens for it according to its storage policy.

pub mod search;

#[allow(clippy::too_many_arguments)]
mod bindings {
    wit_bindgen::generate!({
        world: "file-search",
        path: "wit",
    });
}

use bindings::{Guest, SearchMatch, SearchResult};

struct Component;

impl Guest for Component {
    fn search_files(
        root: String,
        pattern: String,
        literal: bool,
        case_insensitive: bool,
        glob: Option<String>,
        max_results: Option<u32>,
    ) -> Result<SearchResult, String> {
        let query = search::Query {
            pattern,
            literal,
            case_insensitive,
            glob,
            max_results,
        };
        let outcome = search::search(&root, &query)?;
        Ok(SearchResult {
            matches: outcome
                .matches
                .into_iter()
                .map(|m| SearchMatch {
                    path: m.path,
                    line_number: m.line_number,
                    line: m.line,
                })
                .collect(),
            truncated: outcome.truncated,
            files_searched: outcome.files_searched,
        })
    }
}

bindings::export!(Component with_types_in bindings);
