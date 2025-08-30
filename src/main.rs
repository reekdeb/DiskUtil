
use std::fs::{self, Metadata};
use std::path::Path;
use std::io;
use clap::Parser;
use std::collections::VecDeque;
use std::os::windows::fs::MetadataExt;
use std::ffi::OsString;
use std::time::Instant;

/// Disk usage utility
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Directory to scan
    #[arg(default_value = ".")]
    dir: String,

    /// Exclude files from listing (only show folders)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    exclude_files: bool,
}

fn is_symlink_or_junction(meta: &Metadata) -> bool {
    // On Windows, FILE_ATTRIBUTE_REPARSE_POINT means symlink/junction
    meta.file_attributes() & 0x400 != 0
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
                size += meta.file_size();
            }
        }
    }
    size
}

fn main() {
    let args = Args::parse();
    let root = Path::new(&args.dir);
    let start = Instant::now();
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
            io::Write::flush(&mut io::stdout()).ok();
            let size = get_folder_size(&folder_path);
            item_sizes.push((entry.file_name(), size, true));
        } else if !args.exclude_files {
            let size = meta.file_size();
            item_sizes.push((entry.file_name(), size, false));
        }
    }
    println!("\rDone scanning.\n");
    item_sizes.sort_by(|a, b| b.1.cmp(&a.1));
    println!("Items by size:");
    for (name, size, is_dir) in item_sizes {
        let kind = if is_dir { "[DIR]" } else { "[FILE]" };
        println!("{:>12} {}\t{:?}", format_size(size), kind, name);
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
    println!("Elapsed: {:.2?}", start.elapsed());
}
