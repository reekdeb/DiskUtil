# DiskUtil

DiskUtil is a fast and simple disk usage utility for Windows, written in Rust. It lists folders and files in a given directory by their disk usage size, in descending order. The size calculation includes all subfolders and files, but excludes junctions and symbolic links for accuracy.

## Features
- Lists folders and files by size (KB, MB, GB, TB)
- Excludes junctions and symbolic links from size calculation
- Shows a live status update while scanning
- Command line option to exclude files from the listing
- Uses the fastest available method for size calculation

## Usage

### Build and Install

```powershell
cargo build --release
cargo install --path . --force
```

### Run

```powershell
# Scan current directory (default)
diskutil.exe

# Scan a specific directory
diskutil.exe "C:\Path\To\Folder"

# Exclude files from listing (only show folders)
diskutil.exe "C:\Path\To\Folder" --exclude-files
```

## Command Line Options

- `dir` (optional): Directory to scan. Defaults to current directory.
- `--exclude-files`: Exclude files from the listing (only show folders).

## Example Output

```
Items by size:
   150.10 MB [DIR]      "target"
    41.02 KB [DIR]      ".git"
     6.81 KB [FILE]     "Cargo.lock"
     3.58 KB [DIR]      "src"
   130 bytes [FILE]     "Cargo.toml"
     8 bytes [FILE]     ".gitignore"
Elapsed: 11.99ms
```

## License
MIT

---

*Created by Reekdeb Mal*
