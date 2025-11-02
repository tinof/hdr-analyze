import os
import shutil

# ANSI color codes
class Colors:
    GREEN = "\033[32m"
    RED = "\033[31m"
    YELLOW = "\033[33m"
    RESET = "\033[0m"


def print_color(color: str, text: str):
    """Print colored text to the console."""
    color_map = {
        "green": Colors.GREEN,
        "red": Colors.RED,
        "yellow": Colors.YELLOW,
    }
    color_code = color_map.get(color, "")
    print(f"{color_code}{text}{Colors.RESET}")


def cleanup(temp_dir: str):
    """Clean up temporary files."""
    if os.path.exists(temp_dir):
        try:
            shutil.rmtree(temp_dir)
            print(f"Cleaned up temporary directory: {temp_dir}")
        except Exception as e:
            print_color(
                "yellow", f"Warning: Failed to clean up temporary directory: {e}"
            )
