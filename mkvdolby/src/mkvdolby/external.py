import os
import sys
import json
import shutil
import subprocess
import time
import re
from typing import List, Dict, Any, Optional

try:
    from ffmpeg_progress_yield import FfmpegProgress
except ImportError:
    FfmpegProgress = None

from .utils import print_color

# Simple cache to avoid redundant mediainfo calls per file
MI_JSON_CACHE: Dict[str, Dict[str, Any]] = {}


def run_command(command: List[str], log_file_path: str) -> bool:
    """
    Executes a command, logs its output, and checks for errors.
    """
    try:
        with open(log_file_path, "w") as log_file:
            process = subprocess.run(
                command,
                stdout=log_file,
                stderr=subprocess.STDOUT,
                check=True,
                text=True,
            )
        return process.returncode == 0
    except (subprocess.CalledProcessError, FileNotFoundError) as e:
        print_color("red", f"\nError executing command: {' '.join(command)}")
        print_color("red", f"See log for details: {log_file_path}")
        if os.path.exists(log_file_path):
            with open(log_file_path, "r") as log_file:
                print(log_file.read())
        else:
            print(f"Error details: {e}")
        return False


def run_command_live(command: List[str], log_file_path: str) -> bool:
    """
    Executes a command and streams its output to stdout and a log file simultaneously.
    Useful for long-running processes that provide their own progress bars.
    """
    try:
        with open(log_file_path, "w") as log_file:
            process = subprocess.Popen(
                command,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,  # Line buffered
            )

            while True:
                # Read character by character to ensure \r progress bars are updated immediately
                char = process.stdout.read(1)
                if not char and process.poll() is not None:
                    break
                if char:
                    sys.stdout.write(char)
                    sys.stdout.flush()
                    log_file.write(char)

            # Ensure log file is flushed
            log_file.flush()

            return process.returncode == 0
    except Exception as e:
        print_color("red", f"\nError executing command: {' '.join(command)}")
        print_color("red", f"Details: {e}")
        return False


def run_ffmpeg_with_progress(
    command: List[str],
    log_file_path: str,
    description: str,
    duration_override: Optional[float] = None,
) -> bool:
    """
    Run an ffmpeg command with a concise progress display.
    """
    if not command or os.path.basename(command[0]) != "ffmpeg":
        return run_command(command, log_file_path)

    if FfmpegProgress is None:
        print_color(
            "yellow",
            "ffmpeg-progress-yield not installed; running ffmpeg without progress.",
        )
        return run_command(command, log_file_path)

    try:
        with FfmpegProgress(command, exclude_progress=True) as ff:
            last_print = -1.0
            finalizing_hint_shown = False
            for pct in ff.run_command_with_progress(
                duration_override=duration_override
            ):
                if pct is None:
                    continue
                if pct == 0 or pct == 100 or pct - last_print >= 1.0:
                    if pct == 100 and not finalizing_hint_shown:
                        print(
                            f"\r{description}: {pct:6.2f}% (finalizing...)",
                            end="",
                            flush=True,
                        )
                        finalizing_hint_shown = True
                    else:
                        print(f"\r{description}: {pct:6.2f}%", end="", flush=True)
                    last_print = pct
        print()

        returncode = getattr(ff, "returncode", None)
        if returncode is not None and returncode != 0:
            print_color("red", f"\nFFmpeg command failed with return code {returncode}")
            try:
                with open(log_file_path, "w") as log_file:
                    log_file.write(f"Command: {' '.join(command)}\n\n")
                    if hasattr(ff, "stderr") and ff.stderr:
                        log_file.write(ff.stderr)
            except Exception:
                pass
            return False

        try:
            with open(log_file_path, "w") as log_file:
                log_file.write(f"Command: {' '.join(command)}\n\n")
                if hasattr(ff, "stderr") and ff.stderr:
                    log_file.write(ff.stderr)
        except Exception:
            pass
        return True
    except RuntimeError as e:
        print()
        print_color("red", f"\nError executing command: {' '.join(command)}")
        print_color("red", f"See log for details: {log_file_path}")
        return False


def get_mediainfo_json_cached(input_file: str) -> Optional[Dict[str, Any]]:
    """Return mediainfo JSON for a file, using a simple in-memory cache."""
    path = os.path.abspath(input_file)
    if path in MI_JSON_CACHE:
        return MI_JSON_CACHE[path]
    try:
        mi_output = subprocess.check_output(
            ["mediainfo", "--Output=JSON", input_file], text=True
        )
        mi_json = json.loads(mi_output)
        MI_JSON_CACHE[path] = mi_json
        return mi_json
    except Exception:
        return None


def find_local_tool(tool_name: str) -> Optional[str]:
    """Find a tool in the current working directory first, then in PATH."""
    local_path = os.path.join(".", tool_name)
    if os.path.isfile(local_path) and os.access(local_path, os.X_OK):
        return local_path
    return shutil.which(tool_name)


def check_dependencies():
    """Check base dependencies that are always required."""
    required_cmds = ["ffmpeg", "mediainfo", "mkvmerge"]
    all_found = True
    for cmd in required_cmds:
        if not shutil.which(cmd):
            print_color("red", f"Error: Required command '{cmd}' not found in PATH.")
            all_found = False

    if not find_local_tool("dovi_tool"):
        print_color(
            "red",
            "Error: Required command 'dovi_tool' not found in current directory or PATH.",
        )
        all_found = False

    if not all_found:
        sys.exit(1)


def find_analyzer_executable() -> Optional[str]:
    """Return the analyzer executable name if found (supports alias 'hdranalyze')."""
    for name in ["hdr_analyzer_mvp", "hdranalyze"]:
        if shutil.which(name):
            return name
    return None
