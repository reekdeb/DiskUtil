use std::fs;
use std::path::{Path, PathBuf};
use std::io::{self, Write};
use clap::Parser;
use std::collections::VecDeque;
use std::os::windows::fs::MetadataExt;
use std::ffi::OsString;
use std::time::Instant;

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
}

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

/// Recursively collect files (path + size) under `root`, skipping reparse points.
fn collect_files_recursive(root: &Path, min_size: u64) -> Vec<(PathBuf, u64)> {
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
                    result.push((entry.path(), size));
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

        let mut files = collect_files_recursive(root, min_size);
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
            let size = get_folder_size(&folder_path);
            if size >= min_size_top {
                item_sizes.push((entry.file_name(), size, true));
            }
        } else if !args.exclude_files {
            let size = meta.len();
            if size >= min_size_top {
                item_sizes.push((entry.file_name(), size, false));
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
