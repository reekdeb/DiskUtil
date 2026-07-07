use std::fs;
use std::path::{Path, PathBuf};
use std::io::{self, Write};
use clap::{Parser, Subcommand, Args as ClapArgs};
use std::collections::VecDeque;
use std::os::windows::fs::MetadataExt;
use std::ffi::OsString;
use std::time::{Instant, SystemTime};
use glob::MatchOptions;
use regex::Regex;
use chrono::{DateTime, Local};
use filetime::{FileTime, set_file_times};

/// Disk usage utility
#[derive(Parser)]
#[command(author, version, about = "List folders or files by disk usage (Windows)")]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
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

#[derive(Subcommand)]
enum Commands {
    /// Organize files into date-based subdirectories
    Organize(OrganizeArgs),
}

/// Arguments for the organize subcommand
#[derive(ClapArgs)]
struct OrganizeArgs {
    /// Directory to organize
    #[arg(default_value = ".")]
    dir: String,

    /// Destination root directory. If omitted, files are reorganized in place within `dir`
    /// (existing behavior, always by timestamp).
    #[arg(long)]
    dest: Option<String>,

    /// Layout mode when --dest is given: recreate the source folder structure ("structure"),
    /// organize into date-based subdirectories ("timestamp"), or put everything directly in
    /// the destination directory ("flatten"). Ignored (always "timestamp") when --dest is omitted.
    #[arg(long, default_value = "timestamp", value_parser = ["timestamp", "structure", "flatten"])]
    mode: String,

    /// Folder granularity: year, month (year/month), or day (year/month/day)
    #[arg(long, default_value = "month", value_parser = ["year", "month", "day"])]
    by: String,

    /// Which file timestamp to use for date determination
    #[arg(long, default_value = "modified", value_parser = ["modified", "created"])]
    timestamp: String,

    /// Copy files instead of moving them. Only meaningful with --dest; source is left untouched.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    copy: bool,

    /// How to resolve a filename collision at the destination: auto-rename with a " (1)"
    /// suffix, skip the file, or overwrite the existing file.
    #[arg(long = "on-conflict", default_value = "rename", value_parser = ["rename", "skip", "overwrite"])]
    on_conflict: String,

    /// Glob pattern(s) to filter which files are organized (e.g. *.jpg). Can be specified multiple times.
    #[arg(long = "glob", value_name = "PATTERN")]
    globs: Vec<String>,

    /// Regex pattern(s) to filter which files are organized. Can be specified multiple times.
    #[arg(long = "regex", value_name = "PATTERN")]
    regexes: Vec<String>,

    /// Match glob/regex patterns against the full path instead of just the file name
    #[arg(long, action = clap::ArgAction::SetTrue)]
    match_path: bool,

    /// Use case-insensitive pattern matching (default is case-sensitive)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    ignore_case: bool,

    /// Preview changes without making them
    #[arg(long, action = clap::ArgAction::SetTrue)]
    dry_run: bool,
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
        Self::build_from(&args.globs, &args.regexes, args.match_path, args.ignore_case)
    }

    fn build_from(globs: &[String], regexes: &[String], match_path: bool, ignore_case: bool) -> Result<Self, String> {
        let match_opts = MatchOptions {
            case_sensitive: !ignore_case,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };
        let mut compiled_globs = Vec::new();
        for raw in globs {
            match glob::Pattern::new(raw) {
                Ok(p) => compiled_globs.push(p),
                Err(e) => return Err(format!("Invalid glob pattern {:?}: {}", raw, e)),
            }
        }
        let mut compiled_regexes = Vec::new();
        for raw in regexes {
            let pattern = if ignore_case {
                format!("(?i){}", raw)
            } else {
                raw.clone()
            };
            match Regex::new(&pattern) {
                Ok(r) => compiled_regexes.push(r),
                Err(e) => return Err(format!("Invalid regex pattern {:?}: {}", raw, e)),
            }
        }
        Ok(PatternFilter { globs: compiled_globs, regexes: compiled_regexes, match_path, match_opts })
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

// ---------------------------------------------------------------------------
// organize subcommand
// ---------------------------------------------------------------------------

/// Derive the destination directory path for a file given its date and granularity.
fn date_subdir(root: &Path, year: i32, month: u32, day: u32, by: &str) -> PathBuf {
    match by {
        "year" => root.join(format!("{:04}", year)),
        "month" => root.join(format!("{:04}", year)).join(format!("{:02}", month)),
        _ => root
            .join(format!("{:04}", year))
            .join(format!("{:02}", month))
            .join(format!("{:02}", day)),
    }
}

/// Read the relevant SystemTime from metadata (modified or created).
fn file_timestamp(meta: &fs::Metadata, use_created: bool) -> Option<SystemTime> {
    if use_created {
        meta.created().ok()
    } else {
        meta.modified().ok()
    }
}

fn organize_files(args: &OrganizeArgs) {
    match &args.dest {
        None => organize_in_place(args),
        Some(dest) => organize_to_dest(args, dest),
    }
}

fn organize_in_place(args: &OrganizeArgs) {
    let root = Path::new(&args.dir);
    if !root.exists() {
        eprintln!("Path does not exist: {}", root.display());
        std::process::exit(2);
    }
    let root = match fs::canonicalize(root) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Cannot canonicalize path: {}", e);
            std::process::exit(2);
        }
    };
    let use_created = args.timestamp == "created";
    let dry_run = args.dry_run;
    let start = Instant::now();

    if dry_run {
        println!("[DRY RUN] No changes will be made.\n");
    }

    // --- Phase 1: collect all files recursively ---
    let mut all_files: Vec<PathBuf> = Vec::new();
    let mut stack: VecDeque<PathBuf> = VecDeque::new();
    stack.push_back(root.clone());
    while let Some(current) = stack.pop_front() {
        let rd = match fs::read_dir(&current) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd {
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
            let path = entry.path();
            if meta.is_dir() {
                stack.push_back(path);
            } else {
                all_files.push(path);
            }
        }
    }

    // --- Phase 2: build move plan ---
    // moved_count, skipped_count track totals
    let mut moved_count: u64 = 0;
    let mut skipped_conflict: u64 = 0;
    let mut error_count: u64 = 0;

    // For dry-run empty-dir detection: track which source dirs will lose all their files.
    // We map each dir to (total_file_count, files_that_will_move_count).
    use std::collections::HashMap;
    let mut dir_file_counts: HashMap<PathBuf, (usize, usize)> = HashMap::new();

    if dry_run {
        // Pre-populate counts for all directories under root
        for file in &all_files {
            if let Some(parent) = file.parent() {
                let entry = dir_file_counts.entry(parent.to_path_buf()).or_insert((0, 0));
                entry.0 += 1;
            }
        }
    }

    for file in &all_files {
        let meta = match fs::metadata(file) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Cannot read metadata for {}: {}", file.display(), e);
                error_count += 1;
                continue;
            }
        };

        let sys_time = match file_timestamp(&meta, use_created) {
            Some(t) => t,
            None => {
                eprintln!("Cannot read timestamp for {}: skipping", file.display());
                error_count += 1;
                continue;
            }
        };

        let local_dt: DateTime<Local> = sys_time.into();
        let year = local_dt.format("%Y").to_string().parse::<i32>().unwrap_or(1970);
        let month = local_dt.format("%m").to_string().parse::<u32>().unwrap_or(1);
        let day = local_dt.format("%d").to_string().parse::<u32>().unwrap_or(1);

        let dest_dir = date_subdir(&root, year, month, day, &args.by);

        // Skip files that are already in (or directly under) the correct destination dir.
        // This avoids moving a file that is already organised.
        if file.parent() == Some(dest_dir.as_path()) {
            continue;
        }

        let file_name = match file.file_name() {
            Some(n) => n,
            None => continue,
        };
        let dest_file = dest_dir.join(file_name);

        if dest_file.exists() {
            println!("Skipped (conflict): {}", file.display());
            skipped_conflict += 1;
            continue;
        }

        if dry_run {
            println!("Would move: {} -> {}", file.display(), dest_file.display());
            moved_count += 1;
            // Mark this file's parent dir as having one more file that will move
            if let Some(parent) = file.parent() {
                let entry = dir_file_counts.entry(parent.to_path_buf()).or_insert((0, 0));
                entry.1 += 1;
            }
        } else {
            // Read timestamps before moving so we can restore them after
            let mtime = FileTime::from_last_modification_time(&meta);
            let atime = FileTime::from_last_access_time(&meta);

            if let Err(e) = fs::create_dir_all(&dest_dir) {
                eprintln!("Cannot create directory {}: {}", dest_dir.display(), e);
                error_count += 1;
                continue;
            }
            if let Err(e) = fs::rename(file, &dest_file) {
                eprintln!("Cannot move {} -> {}: {}", file.display(), dest_file.display(), e);
                error_count += 1;
                continue;
            }
            // Restore original timestamps
            if let Err(e) = set_file_times(&dest_file, atime, mtime) {
                eprintln!("Warning: could not restore timestamps on {}: {}", dest_file.display(), e);
            }
            println!("Moved: {} -> {}", file.display(), dest_file.display());
            moved_count += 1;
        }
    }

    // --- Phase 3: remove empty directories ---
    let mut removed_dirs: u64 = 0;

    if dry_run {
        // A directory would become empty if all its files would be moved
        // and it has no remaining subdirectories with files.
        // Simple heuristic: report dirs where every file would move.
        // We do a bottom-up pass on discovered dirs.
        let mut candidate_dirs: Vec<PathBuf> = dir_file_counts
            .iter()
            .filter(|(dir, (total, moving))| *moving == *total && **dir != root)
            .map(|(dir, _)| dir.clone())
            .collect();
        // Sort deepest first
        candidate_dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
        for dir in candidate_dirs {
            println!("Would remove empty dir: {}", dir.display());
            removed_dirs += 1;
        }
    } else {
        // Collect all subdirectories (excluding root itself)
        let mut all_dirs: Vec<PathBuf> = Vec::new();
        let mut stack: VecDeque<PathBuf> = VecDeque::new();
        stack.push_back(root.clone());
        while let Some(current) = stack.pop_front() {
            let rd = match fs::read_dir(&current) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for entry in rd {
                let entry = match entry { Ok(e) => e, Err(_) => continue };
                let meta = match entry.metadata() { Ok(m) => m, Err(_) => continue };
                if meta.is_dir() && !is_symlink_or_junction(&meta) {
                    let path = entry.path();
                    all_dirs.push(path.clone());
                    stack.push_back(path);
                }
            }
        }
        // Sort deepest first so children are removed before parents
        all_dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
        for dir in all_dirs {
            if dir == root {
                continue;
            }
            match fs::remove_dir(&dir) {
                Ok(_) => {
                    println!("Removed empty dir: {}", dir.display());
                    removed_dirs += 1;
                }
                Err(_) => {} // Not empty or access error — leave it
            }
        }
    }

    // --- Summary ---
    println!();
    if dry_run {
        println!(
            "[DRY RUN] {} file(s) would be moved, {} skipped (conflict), {} dir(s) would be removed.",
            moved_count, skipped_conflict, removed_dirs
        );
    } else {
        println!(
            "Done: {} moved, {} skipped (conflict), {} dir(s) removed, {} error(s). Elapsed: {:.2?}",
            moved_count, skipped_conflict, removed_dirs, error_count, start.elapsed()
        );
    }
}

/// Outcome of resolving a potential filename collision at a destination path.
enum ConflictOutcome {
    /// Proceed using this (possibly renamed) destination path.
    Proceed(PathBuf),
    /// Skip this file entirely.
    Skip,
}

/// Given a desired destination path, check for collisions against both the filesystem and
/// paths already claimed earlier in this run (`planned`), and resolve according to `strategy`
/// ("rename", "skip", or "overwrite"). On success, the returned path is inserted into `planned`.
fn resolve_conflict(dest: &Path, planned: &mut std::collections::HashSet<PathBuf>, strategy: &str) -> ConflictOutcome {
    let collides = |p: &Path| p.exists() || planned.contains(p);
    if !collides(dest) {
        planned.insert(dest.to_path_buf());
        return ConflictOutcome::Proceed(dest.to_path_buf());
    }
    match strategy {
        "skip" => ConflictOutcome::Skip,
        "overwrite" => {
            planned.insert(dest.to_path_buf());
            ConflictOutcome::Proceed(dest.to_path_buf())
        }
        _ => {
            // "rename": append " (1)", " (2)", ... before the extension until free.
            let parent = dest.parent().unwrap_or_else(|| Path::new(""));
            let stem = dest.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let ext = dest.extension().map(|e| e.to_string_lossy().to_string());
            let mut n = 1u64;
            loop {
                let candidate_name = match &ext {
                    Some(e) => format!("{} ({}).{}", stem, n, e),
                    None => format!("{} ({})", stem, n),
                };
                let candidate = parent.join(candidate_name);
                if !collides(&candidate) {
                    planned.insert(candidate.clone());
                    return ConflictOutcome::Proceed(candidate);
                }
                n += 1;
            }
        }
    }
}

fn organize_to_dest(args: &OrganizeArgs, dest: &str) {
    let source_root = Path::new(&args.dir);
    if !source_root.exists() {
        eprintln!("Path does not exist: {}", source_root.display());
        std::process::exit(2);
    }
    let source_root = match fs::canonicalize(source_root) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Cannot canonicalize path: {}", e);
            std::process::exit(2);
        }
    };

    let dest_root = PathBuf::from(dest);
    if !args.dry_run {
        if let Err(e) = fs::create_dir_all(&dest_root) {
            eprintln!("Cannot create destination directory {}: {}", dest_root.display(), e);
            std::process::exit(2);
        }
    }
    let dest_root = match fs::canonicalize(&dest_root) {
        Ok(p) => p,
        Err(_) => dest_root, // dry-run: destination may not exist yet
    };

    let filter = match PatternFilter::build_from(&args.globs, &args.regexes, args.match_path, args.ignore_case) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(2);
        }
    };

    let use_created = args.timestamp == "created";
    let dry_run = args.dry_run;
    let copy = args.copy;
    let start = Instant::now();

    if dry_run {
        println!("[DRY RUN] No changes will be made.\n");
    }

    // --- Phase 1: collect all files recursively, applying the pattern filter ---
    // Also tally the *unfiltered* file count per directory (dir_total_counts), since a
    // directory only truly becomes empty once every file in it (matched or not) is gone —
    // not just the ones that happen to match --glob/--regex.
    let mut all_files: Vec<PathBuf> = Vec::new();
    use std::collections::HashMap;
    let mut dir_total_counts: HashMap<PathBuf, usize> = HashMap::new();
    let mut stack: VecDeque<PathBuf> = VecDeque::new();
    stack.push_back(source_root.clone());
    while let Some(current) = stack.pop_front() {
        let rd = match fs::read_dir(&current) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd {
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
            let path = entry.path();
            if meta.is_dir() {
                stack.push_back(path);
            } else {
                if let Some(parent) = path.parent() {
                    *dir_total_counts.entry(parent.to_path_buf()).or_insert(0) += 1;
                }
                if filter.is_active() {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if !filter.matches(&name, &path) {
                        continue;
                    }
                }
                all_files.push(path);
            }
        }
    }

    // --- Phase 2: build copy/move plan ---
    let mut done_count: u64 = 0;
    let mut skipped_conflict: u64 = 0;
    let mut renamed_count: u64 = 0;
    let mut error_count: u64 = 0;
    let mut planned: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    // For move + dry-run empty-dir prediction: track (total, moving) files per source dir.
    // `total` is seeded from dir_total_counts (ALL files, not just filtered ones), so a
    // directory is only predicted as removable when nothing — matched or not — is left in it.
    let mut dir_file_counts: HashMap<PathBuf, (usize, usize)> = HashMap::new();
    if dry_run && !copy {
        for (dir, total) in &dir_total_counts {
            dir_file_counts.insert(dir.clone(), (*total, 0));
        }
    }

    let verb = if copy { "copy" } else { "move" };
    let verb_past = if copy { "Copied" } else { "Moved" };
    let verb_would = if copy { "Would copy" } else { "Would move" };

    for file in &all_files {
        let meta = match fs::metadata(file) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Cannot read metadata for {}: {}", file.display(), e);
                error_count += 1;
                continue;
            }
        };

        let desired_dest = match args.mode.as_str() {
            "structure" => {
                let rel = match file.strip_prefix(&source_root) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                dest_root.join(rel)
            }
            "flatten" => {
                let file_name = match file.file_name() {
                    Some(n) => n,
                    None => continue,
                };
                dest_root.join(file_name)
            }
            _ => {
                // "timestamp"
                let sys_time = match file_timestamp(&meta, use_created) {
                    Some(t) => t,
                    None => {
                        eprintln!("Cannot read timestamp for {}: skipping", file.display());
                        error_count += 1;
                        continue;
                    }
                };
                let local_dt: DateTime<Local> = sys_time.into();
                let year = local_dt.format("%Y").to_string().parse::<i32>().unwrap_or(1970);
                let month = local_dt.format("%m").to_string().parse::<u32>().unwrap_or(1);
                let day = local_dt.format("%d").to_string().parse::<u32>().unwrap_or(1);
                let dest_dir = date_subdir(&dest_root, year, month, day, &args.by);
                let file_name = match file.file_name() {
                    Some(n) => n,
                    None => continue,
                };
                dest_dir.join(file_name)
            }
        };

        let dest_file = match resolve_conflict(&desired_dest, &mut planned, &args.on_conflict) {
            ConflictOutcome::Proceed(p) => {
                if p != desired_dest {
                    renamed_count += 1;
                }
                p
            }
            ConflictOutcome::Skip => {
                println!("Skipped (conflict): {}", file.display());
                skipped_conflict += 1;
                continue;
            }
        };
        let dest_dir = match dest_file.parent() {
            Some(d) => d.to_path_buf(),
            None => continue,
        };

        if dry_run {
            println!("{}: {} -> {}", verb_would, file.display(), dest_file.display());
            done_count += 1;
            if !copy {
                if let Some(parent) = file.parent() {
                    let entry = dir_file_counts.entry(parent.to_path_buf()).or_insert((0, 0));
                    entry.1 += 1;
                }
            }
        } else {
            let mtime = FileTime::from_last_modification_time(&meta);
            let atime = FileTime::from_last_access_time(&meta);

            if let Err(e) = fs::create_dir_all(&dest_dir) {
                eprintln!("Cannot create directory {}: {}", dest_dir.display(), e);
                error_count += 1;
                continue;
            }

            let op_result = if copy {
                fs::copy(file, &dest_file).map(|_| ())
            } else {
                fs::rename(file, &dest_file)
            };
            if let Err(e) = op_result {
                eprintln!("Cannot {} {} -> {}: {}", verb, file.display(), dest_file.display(), e);
                error_count += 1;
                continue;
            }
            if let Err(e) = set_file_times(&dest_file, atime, mtime) {
                eprintln!("Warning: could not restore timestamps on {}: {}", dest_file.display(), e);
            }
            println!("{}: {} -> {}", verb_past, file.display(), dest_file.display());
            done_count += 1;
        }
    }

    // --- Phase 3: remove now-empty source directories (move mode only) ---
    let mut removed_dirs: u64 = 0;
    if !copy {
        if dry_run {
            let mut candidate_dirs: Vec<PathBuf> = dir_file_counts
                .iter()
                .filter(|(dir, (total, moving))| *moving == *total && **dir != source_root)
                .map(|(dir, _)| dir.clone())
                .collect();
            candidate_dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
            for dir in candidate_dirs {
                println!("Would remove empty dir: {}", dir.display());
                removed_dirs += 1;
            }
        } else {
            let mut all_dirs: Vec<PathBuf> = Vec::new();
            let mut stack: VecDeque<PathBuf> = VecDeque::new();
            stack.push_back(source_root.clone());
            while let Some(current) = stack.pop_front() {
                let rd = match fs::read_dir(&current) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                for entry in rd {
                    let entry = match entry { Ok(e) => e, Err(_) => continue };
                    let meta = match entry.metadata() { Ok(m) => m, Err(_) => continue };
                    if meta.is_dir() && !is_symlink_or_junction(&meta) {
                        let path = entry.path();
                        all_dirs.push(path.clone());
                        stack.push_back(path);
                    }
                }
            }
            all_dirs.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
            for dir in all_dirs {
                if dir == source_root {
                    continue;
                }
                match fs::remove_dir(&dir) {
                    Ok(_) => {
                        println!("Removed empty dir: {}", dir.display());
                        removed_dirs += 1;
                    }
                    Err(_) => {}
                }
            }
        }
    }

    // --- Summary ---
    println!();
    if dry_run {
        println!(
            "[DRY RUN] {} file(s) would be {}, {} skipped (conflict), {} renamed to avoid conflict, {} dir(s) would be removed.",
            done_count, if copy { "copied" } else { "moved" }, skipped_conflict, renamed_count, removed_dirs
        );
    } else {
        println!(
            "Done: {} {}, {} skipped (conflict), {} renamed to avoid conflict, {} dir(s) removed, {} error(s). Elapsed: {:.2?}",
            done_count, if copy { "copied" } else { "moved" }, skipped_conflict, renamed_count, removed_dirs, error_count, start.elapsed()
        );
    }
}

fn main() {
    let args = Args::parse();

    // Dispatch to organize subcommand if present
    if let Some(Commands::Organize(ref org_args)) = args.command {
        organize_files(org_args);
        return;
    }

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
