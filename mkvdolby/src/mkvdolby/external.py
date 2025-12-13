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
        with open(log_file_path, "w") as log_file:
            process = subprocess.Popen(
                command,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                bufsize=1,  # Line buffered
            )

            # Track line buffers to handle \r properly
            stdout_line = ""
            stderr_line = ""

            def write_with_cr_handling(
                char: str, output_stream, line_buffer: str
            ) -> str:
                """Handle carriage return by clearing and rewriting the line."""
                if char == "\r":
                    # Clear current line and move cursor to start
                    # Use ANSI escape: \033[2K clears line, \r moves to start
                    output_stream.write("\r\033[K")
                    output_stream.flush()
                    return ""
                elif char == "\n":
                    output_stream.write(char)
                    output_stream.flush()
                    return ""
                else:
                    output_stream.write(char)
                    output_stream.flush()
                    return line_buffer + char

            # Use select for multiplexing stdout/stderr
            while process.poll() is None:
                readable, _, _ = select.select(
                    [process.stdout, process.stderr], [], [], 0.1
                )

                for pipe in readable:
                    # Read one character at a time for responsive progress updates
                    char = pipe.read(1)
                    if char:
                        # Write to appropriate output stream with CR handling
                        if pipe == process.stderr:
                            stderr_line = write_with_cr_handling(
                                char, sys.stderr, stderr_line
                            )
                        else:
                            stdout_line = write_with_cr_handling(
                                char, sys.stdout, stdout_line
                            )
                        # Log file gets raw output (newlines for each update for readability)
                        if char == "\r":
                            log_file.write("\n")
                        else:
                            log_file.write(char)

            # Drain any remaining output after process exits
            for pipe, out, line_buf in [
                (process.stdout, sys.stdout, stdout_line),
                (process.stderr, sys.stderr, stderr_line),
            ]:
                remaining = pipe.read()
                if remaining:
                    for char in remaining:
                        if char == "\r":
                            out.write("\r\033[K")
                            log_file.write("\n")
                        elif char == "\n":
                            out.write(char)
                            log_file.write(char)
                        else:
                            out.write(char)
                            log_file.write(char)
                    out.flush()

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
