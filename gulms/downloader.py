"""
GULMS File Downloader - Streaming downloader with token auth and progress bar.
"""

import os
import sys
import re
import urllib.parse
from pathlib import Path
import requests

def format_size(num_bytes: int) -> str:
    """Format bytes into readable human format."""
    if not num_bytes:
        return "0 B"
    for unit in ['B', 'KB', 'MB', 'GB']:
        if num_bytes < 1024.0:
            return f"{num_bytes:.1f} {unit}" if unit != 'B' else f"{num_bytes} B"
        num_bytes /= 1024.0
    return f"{num_bytes:.1f} TB"

def sanitize_filename(name: str) -> str:
    """Sanitize string to be safe for filenames across Linux/Windows/macOS."""
    name = re.sub(r'<[^>]+>', '', name or "")
    name = re.sub(r'[\\/*?:"<>|]', "_", name)
    return name.strip()

def download_file(
    session: requests.Session,
    file_url: str,
    token: str,
    dest_path: Path,
    filename: str = None,
    show_progress: bool = True
) -> Path:
    """
    Download a file from Moodle pluginfile.php with token authentication.
    Returns the Path to the downloaded file.
    """
    if not file_url:
        raise ValueError("No file URL provided")

    # Ensure token is attached
    parsed = urllib.parse.urlparse(file_url)
    params = urllib.parse.parse_qs(parsed.query)
    if "token" not in params:
        params["token"] = [token]
        new_query = urllib.parse.urlencode(params, doseq=True)
        file_url = urllib.parse.urlunparse((
            parsed.scheme, parsed.netloc, parsed.path, parsed.params, new_query, parsed.fragment
        ))

    dest_path = Path(dest_path)
    dest_path.mkdir(parents=True, exist_ok=True)
    if not filename:
        filename = os.path.basename(parsed.path) or "downloaded_file"

    target_file = dest_path / sanitize_filename(filename)

    with session.get(file_url, stream=True) as r:
        r.raise_for_status()
        total_length = r.headers.get('content-length')
        total_bytes = int(total_length) if total_length and total_length.isdigit() else 0

        # Skip if file exists and matches size
        if target_file.exists() and total_bytes and target_file.stat().st_size == total_bytes:
            if show_progress:
                print(f"\033[2mSkipping (already exists): {target_file.name}\033[0m")
            return target_file

        if show_progress:
            print(f"Downloading \033[1m{target_file.name}\033[0m ({format_size(total_bytes) if total_bytes else 'stream'})...")

        downloaded = 0
        with open(target_file, 'wb') as f:
            for chunk in r.iter_content(chunk_size=65536):
                if chunk:
                    f.write(chunk)
                    downloaded += len(chunk)
                    if show_progress:
                        if total_bytes:
                            percent = (downloaded / total_bytes) * 100
                            bar_len = 30
                            filled = int(bar_len * downloaded // total_bytes)
                            bar = '█' * filled + '░' * (bar_len - filled)
                            sys.stdout.write(f"\r[{bar}] {percent:.1f}% ({format_size(downloaded)} / {format_size(total_bytes)})")
                            sys.stdout.flush()
                        else:
                            sys.stdout.write(f"\rDownloaded {format_size(downloaded)}...")
                            sys.stdout.flush()

        if show_progress:
            print(f"\n\033[92m✓ Download complete:\033[0m {target_file}")

    return target_file
