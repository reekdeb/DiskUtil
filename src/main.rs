
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
    let mut folder_sizes: Vec<(OsString, u64)> = Vec::new();
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
        if meta.is_dir() && !is_symlink_or_junction(&meta) {
            let folder_path = entry.path();
            print!("\rScanning: {}", folder_path.display());
            io::Write::flush(&mut io::stdout()).ok();
            let size = get_folder_size(&folder_path);
            folder_sizes.push((entry.file_name(), size));
        }
    }
    println!("\rDone scanning.\n");
    folder_sizes.sort_by(|a, b| b.1.cmp(&a.1));
    println!("Folders by size:");
    for (name, size) in folder_sizes {
        println!("{:>12} bytes\t{:?}", size, name);
    }
    println!("Elapsed: {:.2?}", start.elapsed());
}
