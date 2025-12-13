import os
import sys
import json
import shutil
import subprocess
from typing import List, Dict, Any, Optional

# FfmpegProgress removed


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
    Executes a command and streams its output to the terminal and a log file simultaneously.
    Handles both stdout and stderr for progress output.
    Properly handles carriage return (\r) for in-place progress bar updates.
    Useful for long-running processes that provide their own progress bars.
    """
    import select

    try:
        with open(log_file_path, "w", encoding="utf-8") as log_file:
            process = subprocess.Popen(
                command,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=False,  # Binary mode to avoid buffering issues/decoding lags
                bufsize=0,  # Unbuffered
            )

            while True:
                # Check for process exit if pipes are closed/empty
                if process.poll() is not None:
                    # Drain one last time
                    remainder_out = process.stdout.read() if process.stdout else b""
                    remainder_err = process.stderr.read() if process.stderr else b""
                    if remainder_out:
                        sys.stdout.buffer.write(remainder_out)
                        sys.stdout.buffer.flush()
                        log_file.write(remainder_out.decode("utf-8", errors="replace"))
                    if remainder_err:
                        sys.stderr.buffer.write(remainder_err)
                        sys.stderr.buffer.flush()
                        log_file.write(
                            remainder_err.decode("utf-8", errors="replace").replace(
                                "\r", "\n"
                            )
                        )
                    break

                readable, _, _ = select.select(
                    [process.stdout, process.stderr], [], [], 0.1
                )

                for pipe in readable:
                    # Read chunk (bytes)
                    chunk = pipe.read(4096)
                    if not chunk:
                        continue

                    # Direct binary pass-through to terminal
                    if pipe == process.stderr:
                        sys.stderr.buffer.write(chunk)
                        sys.stderr.buffer.flush()
                        # Log file: decode and replace \r with \n
                        log_file.write(
                            chunk.decode("utf-8", errors="replace").replace("\r", "\n")
                        )
                    else:
                        sys.stdout.buffer.write(chunk)
                        sys.stdout.buffer.flush()
                        log_file.write(chunk.decode("utf-8", errors="replace"))

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
    # Simply use run_command_live which already handles live output streaming
    # and \r carriage returns for progress bars.
    return run_command_live(command, log_file_path)


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


def get_ffprobe_data(input_file: str) -> Optional[Dict[str, Any]]:
    """Return ffprobe data for a file."""
    try:
        cmd = [
            "ffprobe",
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_frames",
            "-read_intervals",
            "%+#1",
            input_file,
        ]
        output = subprocess.check_output(cmd, text=True)
        return json.loads(output)
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
    required_cmds = ["ffmpeg", "mkvmerge"]
    all_found = True
    for cmd in required_cmds:
        if not shutil.which(cmd):
            print_color("red", f"Error: Required command '{cmd}' not found in PATH.")
            all_found = False

    if not shutil.which("mediainfo") and not shutil.which("ffprobe"):
        print_color(
            "red",
            "Error: Neither 'mediainfo' nor 'ffprobe' found in PATH. One is required.",
        )
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
