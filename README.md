# DiskUtil

DiskUtil is a fast and simple disk usage utility for Windows, written in Rust. It lists folders and files in a given directory by their disk usage size, and can organize files into date-based subdirectories. Size calculations and file operations exclude junctions and symbolic links for accuracy.

## Features
- Lists folders and files by size (KB, MB, GB, TB)
- Recursively lists and sorts individual files by size
- Organizes files into `year/`, `year/month/`, or `year/month/day/` folders based on file timestamps
- Preserves original file timestamps after moving
- Removes empty directories left behind after organizing
- Dry-run mode to preview all changes before applying them
- Glob and regex pattern filtering
- Excludes junctions and symbolic links from all operations
- Shows a live status update while scanning

## Usage

### Build and Install

```powershell
cargo build --release
cargo install --path . --force
```

---

## Disk Usage Listing

```powershell
# Scan current directory (default)
diskutil.exe

# Scan a specific directory
diskutil.exe "C:\Path\To\Folder"

# Exclude files from listing (only show folders)
diskutil.exe "C:\Path\To\Folder" --exclude-files

# Recursively list largest files >= 5 MB, show top 20
diskutil.exe "C:\Path\To\Folder" --list-files --min-size 5MB --limit 20
```

### Options

| Option | Description |
|--------|-------------|
| `[DIR]` | Directory to scan (default: `.`) |
| `--exclude-files` | Only show folders in the top-level listing |
| `--list-files` | Recursively list individual files sorted by size |
| `--min-size <value>` | Filter to items at or above this size (`10MB`, `1.5G`, `1024`, etc.) |
| `--limit <n>` | Show only the top `n` results |
| `--glob <pattern>` | Filter by glob pattern (repeatable, e.g. `*.rs`) |
| `--regex <pattern>` | Filter by regex pattern (repeatable) |
| `--match-path` | Match patterns against the full path instead of just the name |
| `--ignore-case` | Case-insensitive pattern matching |

### Example Output

```
Items by size:
   150.10 MB [DIR]      target
    41.02 KB [DIR]      .git
     6.81 KB [FILE]     Cargo.lock
     3.58 KB [DIR]      src
   130 bytes [FILE]     Cargo.toml
Elapsed: 11.99ms
```

---

## Organize Files by Date

Recursively moves files in a folder into date-based subdirectories determined by each file's timestamp. Original timestamps are restored after every move, and empty source directories are deleted.

```powershell
diskutil.exe organize [DIR] [OPTIONS]
```

### Options

| Option | Description |
|--------|-------------|
| `[DIR]` | Directory to organize (default: `.`) |
| `--by <granularity>` | Folder depth: `year`, `month` (default), or `day` |
| `--timestamp <source>` | Timestamp to use: `modified` (default) or `created` |
| `--dry-run` | Preview all changes without touching the filesystem |

### Folder structure created

| `--by` | Example path |
|--------|--------------|
| `year` | `Photos/2024/IMG_001.jpg` |
| `month` | `Photos/2024/03/IMG_001.jpg` |
| `day` | `Photos/2024/03/15/IMG_001.jpg` |

Month and day folders use zero-padded numbers (`01`–`12`, `01`–`31`).

### Conflict handling

If a file with the same name already exists at the destination, the source file is **skipped** and logged — it is never overwritten.

### Examples

```powershell
# Preview what would happen (no changes made)
diskutil.exe organize "D:\Photos" --by day --dry-run

# Organize by year/month using last-modified time (default)
diskutil.exe organize "D:\Photos" --by month

# Organize by year only, using file creation time
diskutil.exe organize "D:\Downloads" --by year --timestamp created

# Organize current directory by year/month/day
diskutil.exe organize --by day
```

### Example Output

```
Moved: D:\Photos\IMG_001.jpg -> D:\Photos\2024\03\15\IMG_001.jpg
Moved: D:\Photos\IMG_002.jpg -> D:\Photos\2024\03\15\IMG_002.jpg
Skipped (conflict): D:\Photos\IMG_003.jpg -> D:\Photos\2024\03\15\IMG_003.jpg
Removed empty dir: D:\Photos\Unsorted

Done: 2 moved, 1 skipped (conflict), 1 dir(s) removed, 0 error(s). Elapsed: 3.20ms
```

---

## License
MIT

---

*Created by Reekdeb Mal*
