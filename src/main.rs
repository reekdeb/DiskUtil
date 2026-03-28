use std::fs;
use std::path::{Path, PathBuf};
use std::io::{self, Write};
use clap::Parser;
use std::collections::VecDeque;
use std::os::windows::fs::MetadataExt;
use std::ffi::OsString;
use std::time::Instant;
use glob::MatchOptions;
use regex::Regex;

/// Disk usage utility
#[derive(Parser)]
#[command(author, version, about = "List folders or files by disk usage (Windows)")]
struct Args {
    /// Directory to scan
    #[arg(default_value = ".")]
    dir: String,

    /// Exclude files from top-level listing (only show folders)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    exclude_files: bool,

    /// List only files (recursively) and show largest files found
    #[arg(long, action = clap::ArgAction::SetTrue)]
    list_files: bool,

    /// Minimum size filter for files (e.g. 10MB, 2GB). When used with --list-files
    #[arg(long)]
    min_size: Option<String>,

    /// Limit the number of results shown
    #[arg(long)]
    limit: Option<usize>,

    /// Glob pattern(s) to filter files/folders (e.g. *.rs, *-suffix.txt). Can be specified multiple times.
    #[arg(long = "glob", value_name = "PATTERN")]
    globs: Vec<String>,

    /// Regex pattern(s) to filter files/folders. Can be specified multiple times.
    #[arg(long = "regex", value_name = "PATTERN")]
    regexes: Vec<String>,

    /// Match patterns against the full path instead of just the file/folder name
    #[arg(long, action = clap::ArgAction::SetTrue)]
    match_path: bool,

    /// Use case-insensitive pattern matching (default is case-sensitive)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    ignore_case: bool,
}

// ---------------------------------------------------------------------------
// Pattern filtering
// ---------------------------------------------------------------------------

struct PatternFilter {
    globs: Vec<glob::Pattern>,
    regexes: Vec<Regex>,
    match_path: bool,
    match_opts: MatchOptions,
}

impl PatternFilter {
    fn build(args: &Args) -> Result<Self, String> {
        let match_opts = MatchOptions {
            case_sensitive: !args.ignore_case,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };
        let mut globs = Vec::new();
        for raw in &args.globs {
            match glob::Pattern::new(raw) {
                Ok(p) => globs.push(p),
                Err(e) => return Err(format!("Invalid glob pattern {:?}: {}", raw, e)),
            }
        }
        let mut regexes = Vec::new();
        for raw in &args.regexes {
            let pattern = if args.ignore_case {
                format!("(?i){}", raw)
            } else {
                raw.clone()
            };
            match Regex::new(&pattern) {
                Ok(r) => regexes.push(r),
                Err(e) => return Err(format!("Invalid regex pattern {:?}: {}", raw, e)),
            }
        }
        Ok(PatternFilter { globs, regexes, match_path: args.match_path, match_opts })
    }

    fn is_active(&self) -> bool {
        !self.globs.is_empty() || !self.regexes.is_empty()
    }

    /// Returns true if the given entry (either a file or directory) matches any pattern.
    /// `name` is just the file/folder name; `full_path` is the canonical path.
    fn matches(&self, name: &str, full_path: &Path) -> bool {
        // On Windows normalise separators to forward-slash so glob patterns like **/src/*.rs work.
        let path_str = full_path.to_string_lossy().replace('\\', "/");
        let subject_path: &str = &path_str;
        let subject_name: &str = name;

        for g in &self.globs {
            if self.match_path {
                if g.matches_with(subject_path, self.match_opts) {
                    return true;
                }
            } else {
                if g.matches_with(subject_name, self.match_opts) {
                    return true;
                }
            }
        }
        for r in &self.regexes {
            let haystack = if self.match_path { subject_path } else { subject_name };
            if r.is_match(haystack) {
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------

fn is_symlink_or_junction(meta: &fs::Metadata) -> bool {
    // On Windows, FILE_ATTRIBUTE_REPARSE_POINT means symlink/junction
    meta.file_attributes() & 0x400 != 0
}

fn format_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;
    if size >= TB {
        format!("{:.2} TB", size as f64 / TB as f64)
    } else if size >= GB {
        format!("{:.2} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.2} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.2} KB", size as f64 / KB as f64)
    } else {
        format!("{} bytes", size)
    }
}

fn parse_size(s: &str) -> Option<u64> {
    // Accepts values like: 1024, 10K, 10KB, 1.5MB, 2G, 3TB
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // find position where suffix starts (first non-digit and non-dot)
    let mut idx = s.len();
    for (i, c) in s.char_indices() {
        if !(c.is_ascii_digit() || c == '.') {
            idx = i;
            break;
        }
    }
    let (num_part, suf_part) = s.split_at(idx);
    let num: f64 = match num_part.parse() {
        Ok(n) => n,
        Err(_) => return None,
    };
    let suf = suf_part.trim().to_ascii_uppercase();
    let bytes = if suf.is_empty() || suf == "B" {
        num
    } else if suf == "K" || suf == "KB" {
        num * 1024f64
    } else if suf == "M" || suf == "MB" {
        num * 1024f64 * 1024f64
    } else if suf == "G" || suf == "GB" {
        num * 1024f64 * 1024f64 * 1024f64
    } else if suf == "T" || suf == "TB" {
        num * 1024f64 * 1024f64 * 1024f64 * 1024f64
    } else {
        return None;
    };
    Some(bytes as u64)
}

fn get_folder_size(path: &Path) -> u64 {
    let mut size = 0u64;
    let mut stack = VecDeque::new();
    stack.push_back(path.to_path_buf());
    while let Some(current) = stack.pop_front() {
        let read_dir = match fs::read_dir(&current) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if is_symlink_or_junction(&meta) {
                continue;
            }
            if meta.is_dir() {
                stack.push_back(entry.path());
            } else {
                size = size.saturating_add(meta.len());
            }
        }
    }
    size
}

/// Like `get_folder_size` but only counts files that match the given filter.
fn get_filtered_folder_size(path: &Path, filter: &PatternFilter) -> u64 {
    let mut size = 0u64;
    let mut stack = VecDeque::new();
    stack.push_back(path.to_path_buf());
    while let Some(current) = stack.pop_front() {
        let read_dir = match fs::read_dir(&current) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if is_symlink_or_junction(&meta) {
                continue;
            }
            if meta.is_dir() {
                stack.push_back(entry.path());
            } else {
                let file_path = entry.path();
                let name = file_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if filter.matches(&name, &file_path) {
                    size = size.saturating_add(meta.len());
                }
            }
        }
    }
    size
}

/// Recursively collect files (path + size) under `root`, skipping reparse points.
fn collect_files_recursive(root: &Path, min_size: u64, filter: Option<&PatternFilter>) -> Vec<(PathBuf, u64)> {
    let mut result = Vec::new();
    let mut stack = VecDeque::new();
    stack.push_back(root.to_path_buf());
    while let Some(current) = stack.pop_front() {
        print!("\rScanning: {}", current.display());
        io::stdout().flush().ok();
        let read_dir = match fs::read_dir(&current) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if is_symlink_or_junction(&meta) {
                continue;
            }
            if meta.is_dir() {
                stack.push_back(entry.path());
            } else {
                let size = meta.len();
                if size >= min_size {
                    let file_path = entry.path();
                    let include = match filter {
                        Some(f) => {
                            let name = file_path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            f.matches(&name, &file_path)
                        }
                        None => true,
                    };
                    if include {
                        result.push((file_path, size));
                    }
                }
            }
        }
    }
    result
}

fn supports_hyperlinks() -> bool {
    std::env::var("WT_SESSION").is_ok()
}

fn path_to_file_uri(path: &Path) -> String {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.to_string_lossy();
    // Strip Windows extended-length prefix "\\?\" (4 chars: \, \, ?, \)
    let s = s.strip_prefix("\\\\?\\").unwrap_or(&s).to_owned();
    let s = s.replace('\\', "/");
    let s = s.replace(' ', "%20").replace('#', "%23").replace('?', "%3F");
    format!("file:///{}", s)
}

fn osc8_link(uri: &str, text: &str) -> String {
    format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", uri, text)
}

fn main() {
    let args = Args::parse();
    let root = Path::new(&args.dir);
    let start = Instant::now();

    if !root.exists() {
        eprintln!("Path does not exist: {}", root.display());
        std::process::exit(2);
    }
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let hyperlinks = supports_hyperlinks();

    let filter = match PatternFilter::build(&args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };
    let filter_ref = if filter.is_active() { Some(&filter) } else { None };

    // If --list-files is specified, do a recursive file scan and show largest files
    if args.list_files {
        let min_size = match &args.min_size {
            Some(s) => match parse_size(s) {
                Some(v) => v,
                None => {
                    eprintln!("Invalid --min-size value: {}", s);
                    return;
                }
            },
            None => 0u64,
        };

        let mut files = collect_files_recursive(root, min_size, filter_ref);
        // sort desc by size
        files.sort_by(|a, b| b.1.cmp(&a.1));
        if let Some(limit) = args.limit {
            files.truncate(limit);
        }
        // Clear status line
        print!("\r{:width$}\r", "", width = 120);
        println!("Largest files:");
        for (path, size) in files {
            if hyperlinks {
                let file_uri = path_to_file_uri(&path);
                let parent_uri = path_to_file_uri(path.parent().unwrap_or(&path));
                println!(
                    "{:>12} {}\t{}",
                    format_size(size),
                    osc8_link(&parent_uri, "[FILE]"),
                    osc8_link(&file_uri, &path.display().to_string()),
                );
            } else {
                println!("{:>12} [FILE]\t{}", format_size(size), path.display());
            }
        }
        println!("Elapsed: {:.2?}", start.elapsed());
        return;
    }

    // Otherwise, default behaviour: list top-level items (folders and optionally files)
    // Apply min_size and limit to top-level listing as well.
    let min_size_top = match &args.min_size {
        Some(s) => match parse_size(s) {
            Some(v) => v,
            None => {
                eprintln!("Invalid --min-size value: {}", s);
                return;
            }
        },
        None => 0u64,
    };

    let mut item_sizes: Vec<(OsString, u64, bool)> = Vec::new(); // (name, size, is_dir)
    let read_dir = match fs::read_dir(root) {
        Ok(rd) => rd,
        Err(e) => {
            eprintln!("Failed to read directory: {}", e);
            return;
        }
    };
    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if is_symlink_or_junction(&meta) {
            continue;
        }
        if meta.is_dir() {
            let folder_path = entry.path();
            print!("\rScanning: {}", folder_path.display());
            io::stdout().flush().ok();
            let size = match filter_ref {
                Some(f) => get_filtered_folder_size(&folder_path, f),
                None => get_folder_size(&folder_path),
            };
            // When a filter is active, skip folders whose filtered size is zero
            if size >= min_size_top && (size > 0 || filter_ref.is_none()) {
                item_sizes.push((entry.file_name(), size, true));
            }
        } else if !args.exclude_files {
            let size = meta.len();
            if size >= min_size_top {
                let name = entry.file_name();
                let include = match filter_ref {
                    Some(f) => {
                        let name_str = name.to_string_lossy();
                        f.matches(&name_str, &entry.path())
                    }
                    None => true,
                };
                if include {
                    item_sizes.push((name, size, false));
                }
            }
        }
    }
    // Clear the status line by overwriting with spaces, then return to start
    print!("\r{:width$}\r", "", width = 120);
    item_sizes.sort_by(|a, b| b.1.cmp(&a.1));
    if let Some(limit) = args.limit {
        item_sizes.truncate(limit);
    }
    println!("Items by size:");
    for (name, size, is_dir) in item_sizes {
        let name_str = name.to_string_lossy();
        if hyperlinks {
            let full_path = canonical_root.join(Path::new(&name));
            if is_dir {
                let dir_uri = path_to_file_uri(&full_path);
                println!(
                    "{:>12} {}\t{}",
                    format_size(size),
                    osc8_link(&dir_uri, "[DIR]"),
                    osc8_link(&dir_uri, &name_str),
                );
            } else {
                let file_uri = path_to_file_uri(&full_path);
                let parent_uri = path_to_file_uri(&canonical_root);
                println!(
                    "{:>12} {}\t{}",
                    format_size(size),
                    osc8_link(&parent_uri, "[FILE]"),
                    osc8_link(&file_uri, &name_str),
                );
            }
        } else {
            let kind = if is_dir { "[DIR]" } else { "[FILE]" };
            println!("{:>12} {}\t{}", format_size(size), kind, name_str);
        }
    }
    println!("Elapsed: {:.2?}", start.elapsed());
}
