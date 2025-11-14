import os
import sys
import time
import re
import glob
import argparse
import atexit

from .utils import print_color, cleanup
from .external import check_dependencies
from .conversion import convert_file


def main():
    """Main function to parse arguments and process files."""
    parser = argparse.ArgumentParser(
        description="A script to convert HDR10/HDR10+ files to Dolby Vision.",
        formatter_class=argparse.RawTextHelpFormatter,
    )
    parser.add_argument(
        "input",
        nargs="*",
        help="One or more input video files. If not provided, processes all .mkv files in the current directory.",
    )
    parser.add_argument(
        "--peak-source",
        choices=["max-scl-luminance", "histogram", "histogram99"],
        default="max-scl-luminance",
        help="Controls the --hdr10plus-peak-source flag in dovi_tool generate.\n"
        "  max-scl-luminance: (Default) Use max-scl from metadata.\n"
        "  histogram: Use the max value from histogram.\n"
        "  histogram99: Use the 99th percentile from histogram.",
    )
    parser.add_argument(
        "--trim-targets",
        type=str,
        default="100,600,1000",
        help=(
            "Comma-separated list of nits values for the Dolby Vision trim pass "
            "(e.g., '100,400,1000'). Used when Details.txt is unavailable or when "
            "--no-trim-from-details is specified."
        ),
    )
    parser.add_argument(
        "--trim-from-details",
        dest="trim_from_details",
        action="store_true",
        default=True,
        help=(
            "Derive target_nits automatically from madVR Details.txt (uses real display "
            "peak and maximum target nits). Enabled by default."
        ),
    )
    parser.add_argument(
        "--no-trim-from-details",
        dest="trim_from_details",
        action="store_false",
        help="Disable deriving target_nits from Details.txt and use --trim-targets instead.",
    )
    parser.add_argument(
        "--drop-chapters",
        action="store_true",
        help="Drop chapters in the output file (kept by default).",
    )
    parser.add_argument(
        "--drop-tags",
        action="store_true",
        help="Drop global tags in the output file (kept by default).",
    )
    parser.add_argument(
        "--hlg-crf",
        type=int,
        default=17,
        help="CRF to use when converting HLG to PQ (default: 17).",
    )
    parser.add_argument(
        "--hlg-preset",
        type=str,
        default="medium",
        help="x265 preset to use for HLG->PQ (default: medium).",
    )
    parser.add_argument(
        "--hlg-peak-nits",
        type=int,
        default=1000,
        help=(
            "Nominal peak luminance for HLG content in cd/m² (default: 1000). "
            "Passed to hdr_analyzer_mvp for native HLG analysis, and used as 'npl' in zscale "
            "when re-encoding HLG→PQ for the Dolby Vision base layer."
        ),
    )
    parser.add_argument(
        "--verify",
        action="store_true",
        help=(
            "After muxing, run verification: our verifier on the measurements and DV checks "
            "(dovi_tool extract/info + mediainfo). Fails on inconsistencies."
        ),
    )
    parser.add_argument(
        "-b",
        "--boost",
        action="store_true",
        help=(
            "Enable a brighter Dolby Vision mapping preset.\n"
            "For HDR10+ sources this switches --peak-source to 'histogram99' when using "
            "the default peak source, which tends to lift overall brightness by ignoring "
            "extreme highlight outliers."
        ),
    )
    parser.add_argument(
        "--boost-experimental",
        action="store_true",
        help=(
            "Experimental boost mode that asks hdr_analyzer_mvp to use a more aggressive "
            "optimizer profile when generating madVR measurements (shot-by-shot target_nits). "
            "Only applies when mkvdolby needs to generate measurements itself; existing "
            "measurements.bin files are left untouched."
        ),
    )

    args = parser.parse_args()

    if getattr(args, "boost", False):
        if args.peak_source == "max-scl-luminance":
            print_color(
                "green",
                "Boost mode enabled: using --peak-source=histogram99 for HDR10+ peak detection.",
            )
            args.peak_source = "histogram99"
        else:
            print_color(
                "yellow",
                "Boost mode enabled but custom --peak-source was provided; leaving it unchanged.",
            )

    try:
        args.trim_targets = [int(t.strip()) for t in args.trim_targets.split(",")]
    except ValueError:
        print_color(
            "red", "Error: --trim-targets must be a comma-separated list of integers."
        )
        sys.exit(1)

    check_dependencies()

    temp_dir = f"./mkvdolby_temp_{int(time.time())}"
    os.makedirs(temp_dir, exist_ok=True)
    atexit.register(cleanup, temp_dir)

    files_to_process = args.input
    if not files_to_process:
        files_to_process = glob.glob("*.mkv")
        if not files_to_process:
            print("No MKV files found in the current directory.")
            sys.exit(0)

    had_failure = False
    for file_path in files_to_process:
        if not os.path.isfile(file_path):
            print_color(
                "yellow", f"Warning: Input file not found, skipping: {file_path}"
            )
            continue
        if file_path.endswith(".DV.mkv"):
            print_color("yellow", f"Skipping already converted file: {file_path}")
            continue

        file_temp_dir = os.path.join(
            temp_dir, re.sub(r"[^a-zA-Z0-9]", "_", os.path.basename(file_path))
        )
        os.makedirs(file_temp_dir, exist_ok=True)

        ok = convert_file(file_path, file_temp_dir, args)
        if not ok:
            had_failure = True

    print("\nMKV Dolby Vision conversion process finished!")
    if had_failure:
        sys.exit(1)


if __name__ == "__main__":
    main()
