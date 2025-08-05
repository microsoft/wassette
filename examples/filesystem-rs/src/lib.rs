// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#[allow(warnings)]
mod bindings;

use std::{env, fs, path::{Path, PathBuf}};

use anyhow::{Result, anyhow};

use bindings::Guest;

struct Component;

impl Guest for Component {
    fn list_directory(path: String) -> Result<Vec<String>, String> {
        match get_path(&path) {
            Ok(path) => {
                let mut text = vec![];
                let entries = match fs::read_dir(&path) {
                    Ok(e) => e,
                    Err(e) => {
                        return Err(format!("Failed to read directory: {}", e));
                    }
                };
                for entry_result in entries {
                    match entry_result {
                        Ok(entry) => {
                            let prefix = match entry.file_type() {
                                Ok(ft) if ft.is_dir() => "[DIR]",
                                Ok(_) => "[FILE]",
                                Err(_) => "[UNKNOWN]",
                            };
                            let file_name = entry.file_name();
                            text.push(format!(
                                "{prefix} {}\n",
                                file_name.to_string_lossy()
                            ));
                        }
                        Err(e) => {
                            text.push(format!("Error reading entry: {e}\n"));
                        }
                    }
                }
                Ok(text)
            }
            Err(e) => {
                Err(e.to_string())
            }
        }
    }

    fn read_file(path: String) -> Result<String, String> {
        match get_path(&path) {
            Ok(path) => match fs::read_to_string(&path) {
                Ok(text) => Ok(text),
                Err(e) => {
                    return Err(format!("Failed to read file: {}", e));
                }
            },
            Err(e) => {
                return Err(e.to_string());
            }
        }
    }

    fn search_file(path: String, pattern: String) -> Result<String, String> {
        let path = match get_path(&path) {
            Ok(p) => p,
            Err(e) => {
                return Err(e.to_string());
            }
        };
        let mut matches = Vec::new();
        if let Err(e) = search_directory(&path, &pattern, &mut matches) {
            return Err(format!("Failed to search directory: {}", e));
        }
        Ok(matches.join("\n"))
    }

    fn get_file_info(path: String) -> Result<String, String> {
        match get_path(&path) {
            Ok(path) => match fs::metadata(&path) {
                Ok(metadata) => {
                    Ok(format!("{:?}", metadata))
                }
                Err(e) => {
                    Err(format!("Failed to get metadata: {}", e))
                }
            },
            Err(e) => Err(e.to_string())
        }
    }
}


fn search_directory(dir: &Path, pattern: &str, matches: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();

        if name.contains(&pattern.to_lowercase()) {
            matches.push(path.to_string_lossy().to_string());
        }
        if path.is_dir() {
            search_directory(&path, pattern, matches)?;
        }
    }
    Ok(())
}

fn get_path(path_str: &str) -> Result<PathBuf> {
    if path_str == "~" || path_str.starts_with("~/") {
        let home_dir = env::var("HOME")
            .map_err(|_| anyhow!("Cannot determine home directory from $HOME"))?;

        if path_str == "~" {
            return Ok(PathBuf::from(home_dir));
        }
        let suffix = &path_str[2..];
        let combined = Path::new(&home_dir).join(suffix);
        return Ok(combined);
    }

    Ok(PathBuf::from(path_str))
}


bindings::export!(Component with_types_in bindings);
