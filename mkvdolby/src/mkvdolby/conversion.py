import os
import re
import shutil
import argparse
from typing import Optional, List

from .utils import print_color
from .external import (
    run_command,
    run_command_live,
    run_ffmpeg_with_progress,
    find_local_tool,
    find_analyzer_executable,
)
from .metadata import (
    HdrFormat,
    check_hdr_format,
    find_measurements_file,
    find_details_file,
    get_static_metadata,
    validate_static_metadata,
    parse_madvr_details_for_trims,
    generate_extra_json,
    get_duration_from_mediainfo,
    get_frame_count_from_mediainfo,
)
from .verify import verify_post_mux


def _build_analyzer_boost_args(args: argparse.Namespace) -> List[str]:
    """Return extra hdr_analyzer_mvp arguments for experimental boost mode."""
    boost_args: List[str] = ["--optimizer-profile", "aggressive"]
    print_color(
        "green",
        "Experimental boost: hdr_analyzer_mvp will use the 'aggressive' optimizer "
        "profile for shot-by-shot target_nits.",
    )
    return boost_args


def run_hdr_analyzer(
    source_file: str,
    temp_dir: str,
    extra_args: Optional[List[str]] = None,
    fast_mode: bool = True,
) -> Optional[str]:
    """Run hdr_analyzer_mvp on a source file and return the measurements path.

    extra_args: optional list of additional flags to pass to the analyzer
    (e.g., ["--hlg-peak-nits", "1000"]).
    fast_mode: if True, use optimized settings (--downscale 2 --sample-rate 3) for faster processing.
    """
    exe = find_analyzer_executable()
    if not exe:
        print_color(
            "red",
            "Error: hdr_analyzer_mvp not found (try adding target/release to PATH or using alias 'hdranalyze').",
        )
        return None

    directory = os.path.dirname(os.path.abspath(source_file))
    base_no_ext = os.path.splitext(os.path.basename(source_file))[0]
    out_path = os.path.join(directory, f"{base_no_ext}_measurements.bin")
    log_path = os.path.join(temp_dir, "hdr_analyzer.log")
    cmd = [exe, source_file, "-o", out_path]

    # Add fast mode defaults for quicker processing
    if fast_mode:
        cmd.extend(["--downscale", "2", "--sample-rate", "3"])

    if extra_args:
        cmd.extend(extra_args)

    print_color("green", "Generating measurements (hdr_analyzer_mvp)...")
    if run_command_live(cmd, log_path) and os.path.isfile(out_path):
        print_color("green", f"Generated measurements: {os.path.basename(out_path)}")
        return os.path.abspath(out_path)

    print_color("red", "hdr_analyzer_mvp failed to create measurements. See log.")
    return None


def convert_hlg_to_pq(
    input_file: str, temp_dir: str, args: argparse.Namespace
) -> Optional[str]:
    """Convert an HLG video to PQ (ST2084) for analysis and BL creation."""
    base_no_ext = os.path.splitext(os.path.basename(input_file))[0]
    pq_path = os.path.join(temp_dir, f"{base_no_ext}_HLG_to_PQ.mkv")
    ffmpeg_log = os.path.join(temp_dir, "ffmpeg_hlg2pq.log")

    static_meta = get_static_metadata(input_file)
    max_dml = int(static_meta.get("max_dml", 1000))
    min_dml = float(static_meta.get("min_dml", 0.005))
    max_cll = int(static_meta.get("max_cll", 1000))
    max_fall = int(static_meta.get("max_fall", 400))

    master_display = (
        f"G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)"
        f"L({max_dml * 10000},{int(min_dml * 10000)})"
    )
    x265_params = (
        f"colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:"
        f"master-display={master_display}:max-cll={max_cll},{max_fall}:"
        f"hdr-opt=1:repeat-headers=1"
    )

    crf_value = str(getattr(args, "hlg_crf", 17))
    preset_value = str(getattr(args, "hlg_preset", "slow"))
    hlg_peak_nits = int(getattr(args, "hlg_peak_nits", 1000))

    if hlg_peak_nits < 100 or hlg_peak_nits > 10000:
        print_color(
            "yellow",
            f"Warning: hlg_peak_nits={hlg_peak_nits} is outside typical range (100-10000)",
        )

    vf = (
        f"zscale=transferin=arib-std-b67:transfer=smpte2084:primaries=bt2020:"
        f"matrix=bt2020nc:rangein=tv:range=tv:npl={hlg_peak_nits},format=yuv420p10le"
    )

    cmd = [
        "ffmpeg",
        "-i",
        input_file,
        "-y",
        "-map",
        "0:v:0",
        "-an",
        "-sn",
        "-vf",
        vf,
        "-c:v",
        "libx265",
        "-preset",
        preset_value,
        "-crf",
        crf_value,
        "-pix_fmt",
        "yuv420p10le",
        "-profile:v",
        "main10",
        "-x265-params",
        x265_params,
        pq_path,
    ]

    duration = get_duration_from_mediainfo(input_file)
    if run_ffmpeg_with_progress(
        cmd, ffmpeg_log, "HLG->PQ encode", duration_override=duration
    ) and os.path.exists(pq_path):
        print_color("green", "Converted HLG to PQ successfully.")
        return os.path.abspath(pq_path)

    print_color("red", "HLG to PQ conversion failed. See log.")
    return None


def extract_hdr10plus_metadata(input_file: str, temp_dir: str) -> Optional[str]:
    """Extracts HDR10+ metadata from a video file."""
    hevc_output = os.path.join(temp_dir, "video.hevc")
    ffmpeg_log = os.path.join(temp_dir, "ffmpeg_extract.log")

    if not run_ffmpeg_with_progress(
        [
            "ffmpeg",
            "-i",
            input_file,
            "-y",
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-f",
            "hevc",
            hevc_output,
        ],
        ffmpeg_log,
        "Extracting HEVC stream",
        duration_override=get_duration_from_mediainfo(input_file),
    ):
        return None

    metadata_file = os.path.join(temp_dir, "hdr10plus_metadata.json")
    hdr10plus_log = os.path.join(temp_dir, "hdr10plus_tool.log")
    hdr10plus_tool_path = find_local_tool("hdr10plus_tool") or "hdr10plus_tool"

    cmd = [hdr10plus_tool_path, "extract", "-i", hevc_output, "-o", metadata_file]
    if run_command(cmd, hdr10plus_log):
        if os.path.exists(metadata_file) and os.path.getsize(metadata_file) > 0:
            print_color("green", "HDR10+ metadata extracted successfully.")
            return metadata_file

    # Check for "no dynamic metadata" case
    if os.path.exists(hdr10plus_log):
        with open(hdr10plus_log, "r") as log:
            if "doesn't contain dynamic metadata" in log.read().lower():
                print_color(
                    "yellow",
                    "File is tagged as HDR10+ but contains no dynamic metadata.",
                )
                return "NO_DYNAMIC_METADATA"

    print_color("red", "HDR10+ metadata extraction failed.")
    return None


def generate_rpu(
    hdr_type: HdrFormat,
    temp_dir: str,
    peak_source: str,
    metadata_file: Optional[str] = None,
    measurements_file: Optional[str] = None,
) -> Optional[str]:
    """Generates the RPU.bin file using dovi_tool."""
    extra_json = os.path.join(temp_dir, "extra.json")
    rpu_bin = os.path.join(temp_dir, "RPU.bin")
    dovi_log = os.path.join(temp_dir, "dovi_tool_generate.log")
    dovi_tool_path = find_local_tool("dovi_tool") or "dovi_tool"

    cmd_base = [dovi_tool_path, "generate", "-j", extra_json, "--rpu-out", rpu_bin]
    if hdr_type == HdrFormat.HDR10_PLUS:
        if not metadata_file:
            print_color("red", "Error: HDR10+ processing requires a metadata file.")
            return None
        cmd = cmd_base + [
            "--hdr10plus-json",
            metadata_file,
            "--hdr10plus-peak-source",
            peak_source,
        ]
    elif hdr_type == HdrFormat.HDR10_WITH_MEASUREMENTS:
        if not measurements_file:
            print_color("red", "Error: madVR processing requires a measurements file.")
            return None
        cmd = cmd_base + ["--madvr-file", measurements_file, "--use-custom-targets"]
    else:
        return None

    if run_command(cmd, dovi_log):
        print_color("green", "RPU.bin generated successfully.")
        return rpu_bin
    return None


def convert_file(input_file: str, temp_dir: str, args: argparse.Namespace) -> bool:
    """Main conversion workflow for a single file.

    Returns True on success, False on failure.
    """
    output_file = f"{os.path.splitext(input_file)[0]}.DV.mkv"
    if os.path.exists(output_file):
        print_color("yellow", f"Output file '{output_file}' already exists. Skipping.")
        return True

    print_color("green", f"\n----- Processing: {os.path.basename(input_file)} -----")
    hdr_type = check_hdr_format(input_file)

    measurements_file: Optional[str] = None
    hdr10plus_json: Optional[str] = None
    bl_source_file: str = input_file

    if hdr_type == HdrFormat.HDR10_PLUS:
        hdr10plus_json = extract_hdr10plus_metadata(input_file, temp_dir)
        if hdr10plus_json == "NO_DYNAMIC_METADATA":
            hdr_type = HdrFormat.HDR10_UNSUPPORTED  # Fallback
            hdr10plus_json = None
        elif not hdr10plus_json:
            return False

    if hdr_type in (HdrFormat.HDR10_WITH_MEASUREMENTS, HdrFormat.HDR10_UNSUPPORTED):
        measurements_file = find_measurements_file(input_file)
        if measurements_file:
            if getattr(args, "boost_experimental", False):
                print_color(
                    "yellow",
                    "Experimental boost requested, but an existing measurements file was found "
                    "and will be used as-is. Delete or move it if you want to regenerate using "
                    "the boosted analyzer profile.",
                )
        elif hdr_type == HdrFormat.HDR10_WITH_MEASUREMENTS:
            print_color("red", "Expected madVR measurements file not found.")
            return False
        else:
            extra_args: Optional[List[str]] = None
            if getattr(args, "boost_experimental", False):
                extra_args = _build_analyzer_boost_args(args)
            measurements_file = run_hdr_analyzer(input_file, temp_dir, extra_args)
            if not measurements_file:
                return False

    elif hdr_type == HdrFormat.HLG:
        # Use native HLG analysis path (no analysis re-encode):
        # Generate measurements directly from the original HLG source,
        # passing the configured HLG peak nits to the analyzer.
        analyzer_extra = ["--hlg-peak-nits", str(getattr(args, "hlg_peak_nits", 1000))]
        if getattr(args, "boost_experimental", False):
            analyzer_extra.extend(_build_analyzer_boost_args(args))
        print_color(
            "green",
            f"HLG detected. Running analyzer natively with --hlg-peak-nits={analyzer_extra[1]}...",
        )
        measurements_file = run_hdr_analyzer(input_file, temp_dir, analyzer_extra)
        if not measurements_file:
            return False

        # For Dolby Vision P8.1 output, a PQ (HDR10) base layer is still required for injection and muxing.
        # Re-encode HLG→PQ for the BL only.
        pq_path = convert_hlg_to_pq(input_file, temp_dir, args)
        if not pq_path:
            return False
        bl_source_file = pq_path

    elif hdr_type == HdrFormat.UNSUPPORTED:
        print_color("red", f"Unsupported HDR format for conversion. Skipping.")
        return False

    static_meta = get_static_metadata(input_file)
    if not validate_static_metadata(static_meta, os.path.basename(input_file)):
        print_color("red", "Metadata validation failed. Proceeding with caution...")

    trim_targets = args.trim_targets
    if args.trim_from_details:
        details_path = find_details_file(input_file)
        derived = parse_madvr_details_for_trims(details_path) if details_path else None
        if derived:
            trim_targets = derived

    extra_json_path = os.path.join(temp_dir, "extra.json")
    if not generate_extra_json(extra_json_path, static_meta, trim_targets):
        return False

    rpu_bin_path = generate_rpu(
        HdrFormat.HDR10_PLUS if hdr10plus_json else HdrFormat.HDR10_WITH_MEASUREMENTS,
        temp_dir,
        args.peak_source,
        metadata_file=hdr10plus_json,
        measurements_file=measurements_file,
    )
    if not rpu_bin_path:
        return False

    bl_hevc = os.path.join(temp_dir, "BL.hevc")
    bl_rpu_hevc = os.path.join(temp_dir, "BL_RPU.hevc")

    if not run_ffmpeg_with_progress(
        [
            "ffmpeg",
            "-i",
            bl_source_file,
            "-y",
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-f",
            "hevc",
            bl_hevc,
        ],
        os.path.join(temp_dir, "ffmpeg_bl_extract.log"),
        "Extracting BL to HEVC",
        duration_override=get_duration_from_mediainfo(bl_source_file),
    ):
        return False

    dovi_tool_path = find_local_tool("dovi_tool") or "dovi_tool"
    if not run_command(
        [
            dovi_tool_path,
            "inject-rpu",
            "-i",
            bl_hevc,
            "--rpu-in",
            rpu_bin_path,
            "-o",
            bl_rpu_hevc,
        ],
        os.path.join(temp_dir, "dovi_inject.log"),
    ):
        return False

    mkvmerge_cmd = ["mkvmerge", "-q", "-o", output_file]
    if args.drop_tags:
        mkvmerge_cmd.append("--no-global-tags")
    if args.drop_chapters:
        mkvmerge_cmd.append("--no-chapters")
    mkvmerge_cmd += [bl_rpu_hevc, "--no-video", os.path.abspath(input_file)]

    if not run_command(mkvmerge_cmd, os.path.join(temp_dir, "mkvmerge.log")):
        return False

    if os.path.exists(output_file):
        shutil.copystat(input_file, output_file)

        # Optional post-mux verification
        if getattr(args, "verify", False):
            print_color("green", "Running post-mux verification (--verify)...")
            ok = verify_post_mux(input_file, output_file, measurements_file, temp_dir)
            if not ok:
                print_color("red", "Inconsistencies detected during verification.")
                return False

        print_color("green", f"✓ Success! Created: {os.path.basename(output_file)}")

        if not getattr(args, "keep_source", False):
            print_color("green", "Cleaning up source and intermediate files...")
            try:
                if os.path.exists(input_file):
                    os.remove(input_file)
                    print(f"  Deleted source: {os.path.basename(input_file)}")

                meas_to_del = measurements_file or find_measurements_file(input_file)
                if meas_to_del and os.path.exists(meas_to_del):
                    os.remove(meas_to_del)
                    print(f"  Deleted measurements: {os.path.basename(meas_to_del)}")

                details_to_del = find_details_file(input_file)
                if details_to_del and os.path.exists(details_to_del):
                    os.remove(details_to_del)
                    print(f"  Deleted details: {os.path.basename(details_to_del)}")
            except OSError as e:
                print_color("yellow", f"Warning: Failed to cleanup some files: {e}")

        return True
    else:
        print_color("red", "Muxing failed, output file not created.")
        return False
