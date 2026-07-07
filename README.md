# DiskUtil

DiskUtil is a fast and simple disk usage utility for Windows, written in Rust. It lists folders and files in a given directory by their disk usage size, and can organize or copy files to a destination with flexible layout modes (date-based, mirrored structure, or flat). Size calculations and file operations exclude junctions and symbolic links for accuracy.

## Features
- **Disk usage analysis**: Lists folders and files by size (KB, MB, GB, TB)
- **File discovery**: Recursively lists and sorts individual files by size
- **File organization**: Organize or copy files with three layout modes:
  - **Timestamp-based**: Organizes into `year/`, `year/month/`, or `year/month/day/` folders
  - **Mirrored structure**: Copies/moves files while preserving the source folder hierarchy
  - **Flattened layout**: Places all files directly in a single destination folder (with auto-rename conflict handling)
- **Copy or move**: Supports both copy (source stays intact) and move (source removed) operations
- **Conflict resolution**: Configurable per-run — auto-rename with ` (1)`, ` (2)` suffixes, skip, or overwrite
- **Timestamp preservation**: Preserves original file timestamps after any operation
- **Cleanup**: Removes empty directories left behind after moving files
- **Safe operations**: Dry-run mode to preview all changes before applying them
- **Flexible filtering**: Glob and regex pattern matching (repeatable, with full-path matching option)
- **Case-insensitive matching**: Optional case-insensitive pattern filtering
- **Safe file handling**: Excludes junctions and symbolic links from all operations
- **Live feedback**: Shows scanning progress updates during operations
- **Windows Terminal integration**: Clickable hyperlinks to files and folders in Windows Terminal

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

## Organize Files

The `organize` command moves or copies files from a source directory to a destination with flexible layout modes. When no destination is specified, files are reorganized **in-place** by their timestamp (the original behavior). Original timestamps are always preserved, and empty source directories are cleaned up after moves.

```powershell
diskutil.exe organize [DIR] [OPTIONS]
```

### Core Options

| Option | Description |
|--------|-------------|
| `[DIR]` | Source directory to organize (default: `.`) |
| `--dest <path>` | Destination root folder. If omitted, organizes files in-place (in-place mode ignores `--mode`). |
| `--mode <mode>` | Layout mode when `--dest` is given: `timestamp` (default), `structure`, or `flatten` |
| `--copy` | Copy files instead of moving them (source stays intact). Only meaningful with `--dest`. |
| `--on-conflict <strategy>` | Handle filename collisions: `rename` (default, auto-suffix), `skip`, or `overwrite` |

### Timestamp and Filtering Options

| Option | Description |
|--------|-------------|
| `--by <granularity>` | (Timestamp mode only) Folder depth: `year`, `month` (default), or `day` |
| `--timestamp <source>` | Timestamp to use: `modified` (default) or `created` |
| `--glob <pattern>` | Filter files by glob pattern (e.g., `*.jpg`). Can be repeated; filters are OR'd. |
| `--regex <pattern>` | Filter files by regex pattern. Can be repeated. |
| `--match-path` | Match glob/regex patterns against the full path instead of just the filename |
| `--ignore-case` | Case-insensitive glob and regex matching |
| `--dry-run` | Preview all changes without touching the filesystem |

### Layout Modes

#### 1. Timestamp Mode (default)
Organizes files into date-based subdirectories. Works both in-place (no `--dest`) and when copying/moving to a destination.

**Folder structure:**

| `--by` | Example path |
|--------|--------------|
| `year` | `Photos/2024/IMG_001.jpg` |
| `month` | `Photos/2024/03/IMG_001.jpg` |
| `day` | `Photos/2024/03/15/IMG_001.jpg` |

(Month and day use zero-padded numbers: `01`–`12`, `01`–`31`.)

#### 2. Structure Mode
Preserves the folder hierarchy from the source under the destination, mirroring relative paths.

**Example:**
- Source: `D:\Downloads\Photos\2024\IMG_001.jpg`
- Dest with `--mode structure`: `D:\Archive\Photos\2024\IMG_001.jpg`

#### 3. Flatten Mode
Places all matching files directly in the destination folder, with no subdirectories. Filename collisions are resolved via the `--on-conflict` strategy (default: auto-rename).

**Example:**
- Source: `D:\Downloads\Photos\Folder1\IMG_001.jpg` and `D:\Downloads\Photos\Folder2\IMG_001.jpg`
- Dest with `--mode flatten`: `D:\Archive\IMG_001.jpg` and `D:\Archive\IMG_001 (1).jpg`

### Conflict Handling

| Strategy | Behavior |
|----------|----------|
| `rename` (default) | Append ` (1)`, ` (2)`, etc., before the file extension to find a free name. E.g., `photo.jpg` → `photo (1).jpg` |
| `skip` | Skip files that would collide; log as "Skipped (conflict)" |
| `overwrite` | Overwrite any existing file at the destination (silent) |

### In-Place Mode (No `--dest`)

When `--dest` is omitted, files are reorganized in-place into date-based subdirectories (timestamp mode), matching the original behavior:

```powershell
# In-place: moves files within D:\Photos into year/month subfolders
diskutil.exe organize "D:\Photos" --by month
```

The `--mode` and `--copy` flags are ignored in in-place mode.

### Examples

#### In-place timestamp organization (classic behavior)
```powershell
# Preview
diskutil.exe organize "D:\Photos" --by day --dry-run

# Organize by year/month using last-modified time (default)
diskutil.exe organize "D:\Photos" --by month

# Organize by year, using file creation time
diskutil.exe organize "D:\Downloads" --by year --timestamp created
```

#### Copy to destination with mirrored folder structure
```powershell
# Copy (not move) all JPEGs, preserving folder structure
diskutil.exe organize "D:\Photos" --dest "D:\Archive" --mode structure --copy --glob "*.jpg"
```

#### Flatten all PDFs into a single folder
```powershell
# Move (not copy) all PDFs to a single destination; auto-rename on collision
diskutil.exe organize "D:\Documents" --dest "D:\PDFArchive" --mode flatten --glob "*.pdf"

# Preview the flattening with conflict renaming
diskutil.exe organize "D:\Documents" --dest "D:\PDFArchive" --mode flatten --glob "*.pdf" --dry-run
```

#### Organize by timestamp into a different location
```powershell
# Copy files to destination, organized by date (matching original creation date)
diskutil.exe organize "D:\Photos" --dest "D:\PhotoArchive" --mode timestamp --timestamp created --copy
```

### Example Output

**In-place mode:**
```
Moved: D:\Photos\IMG_001.jpg -> D:\Photos\2024\03\15\IMG_001.jpg
Moved: D:\Photos\IMG_002.jpg -> D:\Photos\2024\03\15\IMG_002.jpg
Skipped (conflict): D:\Photos\IMG_003.jpg

Done: 2 moved, 1 skipped (conflict), 1 dir(s) removed, 0 error(s). Elapsed: 3.20ms
```

**Flatten mode with rename:**
```
Copied: D:\Downloads\Folder1\photo.jpg -> D:\Archive\photo.jpg
Copied: D:\Downloads\Folder2\photo.jpg -> D:\Archive\photo (1).jpg
Copied: D:\Downloads\Folder3\photo.jpg -> D:\Archive\photo (2).jpg

Done: 3 copied, 0 skipped (conflict), 1 renamed to avoid conflict, 0 dir(s) removed, 0 error(s). Elapsed: 5.42ms
```

**Dry-run output:**
```
[DRY RUN] No changes will be made.

Would move: \\?\D:\Photos\a\photo.jpg -> \\?\D:\Archive\a\photo.jpg
Would move: \\?\D:\Photos\b\photo.jpg -> \\?\D:\Archive\b\photo.jpg
Would remove empty dir: \\?\D:\Photos\a
Would remove empty dir: \\?\D:\Photos\b

[DRY RUN] 2 file(s) would be moved, 0 skipped (conflict), 0 renamed to avoid conflict, 2 dir(s) would be removed.
```

---

## License
GNU General Public License v3.0

---

*Created by Reekdeb Mal*
