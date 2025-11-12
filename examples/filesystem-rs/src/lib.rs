// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#[allow(warnings)]
mod bindings;

use std::path::{Path, PathBuf};
use std::{env, fs};

use anyhow::{anyhow, Result};
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
                        return Err(format!(
                            "Failed to read directory '{}': {}",
                            path.display(),
                            e
                        ));
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
                            text.push(format!("{prefix} {}\n", file_name.to_string_lossy()));
                        }
                        Err(e) => {
                            text.push(format!("Error reading entry: {e}\n"));
                        }
                    }
                }
                Ok(text)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn read_file(path: String) -> Result<String, String> {
        match get_path(&path) {
            Ok(path) => match fs::read_to_string(&path) {
                Ok(text) => Ok(text),
                Err(e) => {
                    return Err(format!("Failed to read file '{}': {}", path.display(), e));
                }
            },
            Err(e) => {
                return Err(e.to_string());
            }
        }
    }

    fn write_file(path: String, content: String) -> Result<String, String> {
        match get_path(&path) {
            Ok(path) => {
                // Ensure parent directory exists
                if let Some(parent) = path.parent() {
                    if !parent.exists() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            return Err(format!(
                                "Failed to create parent directory '{}': {}",
                                parent.display(),
                                e
                            ));
                        }
                    }
                }

                match fs::write(&path, content.as_bytes()) {
                    Ok(_) => Ok(format!("Successfully wrote to file '{}'", path.display())),
                    Err(e) => Err(format!(
                        "Failed to write to file '{}': {}",
                        path.display(),
                        e
                    )),
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn create_directory(path: String) -> Result<String, String> {
        match get_path(&path) {
            Ok(path) => match fs::create_dir_all(&path) {
                Ok(_) => Ok(format!(
                    "Successfully created directory '{}'",
                    path.display()
                )),
                Err(e) => Err(format!(
                    "Failed to create directory '{}': {}",
                    path.display(),
                    e
                )),
            },
            Err(e) => Err(e.to_string()),
        }
    }

    fn move_path(source: String, destination: String) -> Result<String, String> {
        let source_path = match get_path(&source) {
            Ok(p) => p,
            Err(e) => return Err(e.to_string()),
        };

        let dest_path = match get_path(&destination) {
            Ok(p) => p,
            Err(e) => return Err(e.to_string()),
        };

        if !source_path.exists() {
            return Err(format!(
                "Source path '{}' does not exist",
                source_path.display()
            ));
        }

        // Ensure parent directory of destination exists
        if let Some(parent) = dest_path.parent() {
            if !parent.exists() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Err(format!(
                        "Failed to create destination parent directory '{}': {}",
                        parent.display(),
                        e
                    ));
                }
            }
        }

        match fs::rename(&source_path, &dest_path) {
            Ok(_) => Ok(format!(
                "Successfully moved '{}' to '{}'",
                source_path.display(),
                dest_path.display()
            )),
            Err(e) => Err(format!(
                "Failed to move '{}' to '{}': {}",
                source_path.display(),
                dest_path.display(),
                e
            )),
        }
    }

    fn delete_file(path: String) -> Result<String, String> {
        match get_path(&path) {
            Ok(path) => {
                if !path.exists() {
                    return Err(format!("File '{}' does not exist", path.display()));
                }

                if path.is_dir() {
                    return Err(format!(
                        "'{}' is a directory, use delete-directory instead",
                        path.display()
                    ));
                }

                match fs::remove_file(&path) {
                    Ok(_) => Ok(format!("Successfully deleted file '{}'", path.display())),
                    Err(e) => Err(format!("Failed to delete file '{}': {}", path.display(), e)),
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn delete_directory(path: String) -> Result<String, String> {
        match get_path(&path) {
            Ok(path) => {
                if !path.exists() {
                    return Err(format!("Directory '{}' does not exist", path.display()));
                }

                if !path.is_dir() {
                    return Err(format!(
                        "'{}' is not a directory, use delete-file instead",
                        path.display()
                    ));
                }

                match fs::remove_dir(&path) {
                    Ok(_) => Ok(format!(
                        "Successfully deleted directory '{}'",
                        path.display()
                    )),
                    Err(e) => {
                        let error_msg =
                            format!("Failed to delete directory '{}': {}", path.display(), e);
                        // Directory not empty errors often contain "not empty" in the message
                        if e.to_string().to_lowercase().contains("not empty") {
                            Err(format!("{} Remove all contents first.", error_msg))
                        } else {
                            Err(error_msg)
                        }
                    }
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn file_exists(path: String) -> Result<bool, String> {
        match get_path(&path) {
            Ok(path) => Ok(path.exists()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn get_directory_tree(path: String, max_depth: u32) -> Result<String, String> {
        match get_path(&path) {
            Ok(path) => {
                if !path.exists() {
                    return Err(format!("Path '{}' does not exist", path.display()));
                }

                if !path.is_dir() {
                    return Err(format!("'{}' is not a directory", path.display()));
                }

                let mut output = String::new();
                if let Err(e) = build_tree(&path, &mut output, 0, max_depth, "") {
                    return Err(format!("Failed to build directory tree: {}", e));
                }
                Ok(output)
            }
            Err(e) => Err(e.to_string()),
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

        if matches.is_empty() {
            Ok(format!(
                "No files matching pattern '{}' found in '{}'",
                pattern,
                path.display()
            ))
        } else {
            Ok(matches.join("\n"))
        }
    }

    fn get_file_info(path: String) -> Result<String, String> {
        match get_path(&path) {
            Ok(path) => match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    let file_type = if metadata.is_dir() {
                        "Directory"
                    } else if metadata.is_file() {
                        "File"
                    } else if metadata.is_symlink() {
                        "Symlink"
                    } else {
                        "Unknown"
                    };

                    let size = metadata.len();
                    let size_str = format_size(size);

                    let permissions = metadata.permissions();

                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| {
                            let secs = d.as_secs();
                            format!("{} seconds since epoch", secs)
                        })
                        .unwrap_or_else(|| "Unknown".to_string());

                    let readonly = if permissions.readonly() { "yes" } else { "no" };

                    Ok(format!(
                        "Path: {}\nType: {}\nSize: {} ({} bytes)\nRead-only: {}\nModified: {}",
                        path.display(),
                        file_type,
                        size_str,
                        size,
                        readonly,
                        modified
                    ))
                }
                Err(e) => Err(format!(
                    "Failed to get metadata for '{}': {}",
                    path.display(),
                    e
                )),
            },
            Err(e) => Err(e.to_string()),
        }
    }
}

fn build_tree(
    dir: &Path,
    output: &mut String,
    current_depth: u32,
    max_depth: u32,
    prefix: &str,
) -> Result<()> {
    if current_depth > max_depth {
        return Ok(());
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            return Err(anyhow!(
                "Failed to read directory '{}': {}",
                dir.display(),
                e
            ))
        }
    };

    let mut entries: Vec<_> = entries.collect();
    entries.sort_by_key(|e| {
        e.as_ref()
            .ok()
            .and_then(|e| e.file_name().into_string().ok())
    });

    let count = entries.len();
    for (idx, entry_result) in entries.into_iter().enumerate() {
        let entry = match entry_result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let is_last = idx == count - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let extension = if is_last { "    " } else { "│   " };

        let file_name = entry.file_name();
        let file_type = match entry.file_type() {
            Ok(ft) if ft.is_dir() => "[DIR] ",
            Ok(_) => "",
            Err(_) => "[?] ",
        };

        output.push_str(&format!(
            "{}{}{}{}\n",
            prefix,
            connector,
            file_type,
            file_name.to_string_lossy()
        ));

        if entry.path().is_dir() {
            let new_prefix = format!("{}{}", prefix, extension);
            build_tree(
                &entry.path(),
                output,
                current_depth + 1,
                max_depth,
                &new_prefix,
            )?;
        }
    }

    Ok(())
}

fn format_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size_f = size as f64;
    let mut unit_idx = 0;

    while size_f >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size_f /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", size, UNITS[unit_idx])
    } else {
        format!("{:.2} {}", size_f, UNITS[unit_idx])
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
        let home_dir =
            env::var("HOME").map_err(|_| anyhow!("Cannot determine home directory from $HOME"))?;

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
