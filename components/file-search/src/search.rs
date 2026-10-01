// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! Host-independent search logic, so it can be unit tested natively.

use std::path::Path;

use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

/// Matches returned when the caller does not choose a limit.
pub const DEFAULT_MAX_RESULTS: u32 = 200;
/// Upper bound on matches, keeping responses a reasonable size for a model.
pub const MAX_RESULTS_LIMIT: u32 = 10_000;
/// Longest line returned, in bytes; longer lines are cut at a character boundary.
pub const MAX_LINE_BYTES: usize = 1000;

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub pattern: String,
    pub literal: bool,
    pub case_insensitive: bool,
    pub glob: Option<String>,
    pub max_results: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub path: String,
    pub line_number: u64,
    pub line: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub matches: Vec<Match>,
    pub truncated: bool,
    pub files_searched: u64,
}

/// Search the files below `root` for lines matching `query`.
///
/// Only paths reachable through the component's WASI preopens are visible, so
/// a root the host has not granted fails here rather than escaping the sandbox.
pub fn search(root: &str, query: &Query) -> Result<Outcome, String> {
    if root.is_empty() {
        return Err("root must not be empty".into());
    }
    if query.pattern.is_empty() {
        return Err("pattern must not be empty".into());
    }
    let root = Path::new(root);
    std::fs::metadata(root).map_err(|e| {
        format!(
            "cannot access '{}': {e}; grant read access with grant-storage-permission \
             using the uri 'fs://{}'",
            root.display(),
            root.display()
        )
    })?;

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(query.case_insensitive)
        .fixed_strings(query.literal)
        .line_terminator(Some(b'\n'))
        .build(&query.pattern)
        .map_err(|e| format!("invalid pattern: {e}"))?;
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .build();

    let mut walker = WalkBuilder::new(root);
    // Honour .gitignore even when the granted root is below the repository's .git.
    walker.require_git(false).sort_by_file_name(|a, b| a.cmp(b));
    if let Some(glob) = query.glob.as_deref().filter(|g| !g.is_empty()) {
        let overrides = OverrideBuilder::new(root)
            .add(glob)
            .and_then(|builder| builder.build())
            .map_err(|e| format!("invalid glob: {e}"))?;
        walker.overrides(overrides);
    }

    let max = query
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, MAX_RESULTS_LIMIT) as usize;
    let mut outcome = Outcome::default();
    for entry in walker.build() {
        // Unreadable entries below a granted root are skipped, like ripgrep does.
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let display = path.display().to_string();
        let matches = &mut outcome.matches;
        let mut truncated = false;
        let searched = searcher.search_path(
            &matcher,
            path,
            Lossy(|line_number, line| {
                if matches.len() >= max {
                    truncated = true;
                    return Ok(false);
                }
                matches.push(Match {
                    path: display.clone(),
                    line_number,
                    line: clip(line.trim_end_matches(['\r', '\n'])),
                });
                Ok(true)
            }),
        );
        if searched.is_ok() {
            outcome.files_searched += 1;
        }
        if truncated {
            outcome.truncated = true;
            break;
        }
    }
    Ok(outcome)
}

fn clip(line: &str) -> String {
    if line.len() <= MAX_LINE_BYTES {
        return line.to_owned();
    }
    let mut end = MAX_LINE_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n// Needle here\n").unwrap();
        fs::write(root.join("src/nested/lib.rs"), "pub fn needle() {}\n").unwrap();
        fs::write(root.join("notes.txt"), "a.b needle\naxb\n").unwrap();
        fs::write(root.join("ignored.log"), "needle\n").unwrap();
        fs::write(root.join(".hidden"), "needle\n").unwrap();
        fs::write(root.join("binary.bin"), b"needle\0\x01").unwrap();
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        dir
    }

    fn query(pattern: &str) -> Query {
        Query {
            pattern: pattern.into(),
            ..Query::default()
        }
    }

    fn run(dir: &tempfile::TempDir, query: &Query) -> Outcome {
        search(dir.path().to_str().unwrap(), query).unwrap()
    }

    fn files(outcome: &Outcome) -> Vec<String> {
        outcome
            .matches
            .iter()
            .map(|m| m.path.rsplit('/').next().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn finds_matches_in_sorted_order_and_skips_ignored_hidden_and_binary_files() {
        let dir = tree();
        let outcome = run(&dir, &query("needle"));
        assert_eq!(files(&outcome), ["notes.txt", "lib.rs"]);
        assert_eq!(outcome.matches[1].line_number, 1);
        assert_eq!(outcome.matches[1].line, "pub fn needle() {}");
        assert!(!outcome.truncated);
    }

    #[test]
    fn case_insensitive_matching() {
        let dir = tree();
        let outcome = run(
            &dir,
            &Query {
                case_insensitive: true,
                ..query("NEEDLE")
            },
        );
        assert_eq!(files(&outcome), ["notes.txt", "main.rs", "lib.rs"]);
        assert_eq!(outcome.matches[1].line_number, 2);
    }

    #[test]
    fn literal_patterns_escape_regex_syntax() {
        let dir = tree();
        assert_eq!(run(&dir, &query("a.b")).matches.len(), 2);
        let literal = run(
            &dir,
            &Query {
                literal: true,
                ..query("a.b")
            },
        );
        assert_eq!(literal.matches.len(), 1);
        assert_eq!(literal.matches[0].line, "a.b needle");
    }

    #[test]
    fn glob_restricts_paths() {
        let dir = tree();
        let outcome = run(
            &dir,
            &Query {
                glob: Some("*.rs".into()),
                case_insensitive: true,
                ..query("needle")
            },
        );
        assert_eq!(files(&outcome), ["main.rs", "lib.rs"]);
    }

    #[test]
    fn max_results_truncates() {
        let dir = tree();
        let outcome = run(
            &dir,
            &Query {
                max_results: Some(1),
                case_insensitive: true,
                ..query("needle")
            },
        );
        assert_eq!(outcome.matches.len(), 1);
        assert!(outcome.truncated);
    }

    #[test]
    fn single_file_root() {
        let dir = tree();
        let path = dir.path().join("notes.txt");
        let outcome = search(path.to_str().unwrap(), &query("axb")).unwrap();
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.files_searched, 1);
    }

    #[test]
    fn reports_invalid_inputs() {
        let dir = tree();
        let root = dir.path().to_str().unwrap();
        assert!(search(root, &query("("))
            .unwrap_err()
            .contains("invalid pattern"));
        assert!(search(root, &query("")).is_err());
        let missing = dir.path().join("missing");
        let error = search(missing.to_str().unwrap(), &query("x")).unwrap_err();
        assert!(error.contains("grant-storage-permission"), "{error}");
        let bad_glob = Query {
            glob: Some("[".into()),
            ..query("x")
        };
        assert!(search(root, &bad_glob)
            .unwrap_err()
            .contains("invalid glob"));
    }

    #[test]
    fn long_lines_are_clipped_on_char_boundaries() {
        let line = "é".repeat(MAX_LINE_BYTES);
        let clipped = clip(&line);
        assert!(clipped.len() <= MAX_LINE_BYTES);
        assert!(clipped.chars().all(|c| c == 'é'));
    }
}
