import os
import re
from typing import Optional

from .utils import print_color
from .external import run_command, find_local_tool, get_mediainfo_json_cached
from .metadata import get_frame_count_from_mediainfo


def _parse_frame_count_from_verifier_log(log_path: str) -> Optional[int]:
    try:
        with open(log_path, "r", encoding="utf-8", errors="ignore") as f:
            for line in f:
                m = re.search(r"^Frame count:\s*(\d+)", line.strip(), re.IGNORECASE)
                if m:
                    return int(m.group(1))
    except Exception:
        pass
    return None


def verify_post_mux(
    input_file: str,
    dv_output_file: str,
    measurements_file: Optional[str],
    temp_dir: str,
) -> bool:
    """
    Run post-mux verification:
      - Verify measurements with our Rust verifier (if provided)
      - Extract RPU and run dovi_tool info --summary
      - Cross-check DV container and RPU against mediainfo frame count

    Returns True on success, False on inconsistency or tool failure.
    """
    ok = True

    # 1) Verify measurements if present
    if measurements_file and os.path.isfile(measurements_file):
        verifier = find_local_tool("verifier") or "verifier"
        v_log = os.path.join(temp_dir, "verifier.log")
        print_color("green", "Running verifier on measurements...")
        if not run_command([verifier, measurements_file], v_log):
            print_color("red", "Measurements verification failed. See verifier.log.")
            return False
        meas_frames = _parse_frame_count_from_verifier_log(v_log)
    else:
        print_color("yellow", "Measurements not found; skipping measurements verification.")
        meas_frames = None

    # 2) Extract RPU and run info --summary
    rpu_bin = os.path.join(temp_dir, "RPU_verify.bin")
    dovi = find_local_tool("dovi_tool") or "dovi_tool"
    dovi_extract_log = os.path.join(temp_dir, "dovi_extract_verify.log")
    dovi_info_log = os.path.join(temp_dir, "dovi_info_verify.log")

    print_color("green", "Extracting RPU from DV output...")
    if not run_command([dovi, "extract-rpu", "-i", dv_output_file, "-o", rpu_bin], dovi_extract_log):
        print_color("red", "Failed to extract RPU from DV output. See logs.")
        return False

    print_color("green", "Inspecting RPU summary (dovi_tool info)...")
    if not run_command([dovi, "info", "-i", rpu_bin, "--summary"], dovi_info_log):
        print_color("red", "Failed to read RPU info. See logs.")
        return False

    # 3) Cross-check frame counts using mediainfo on the DV output
    dv_frames = get_frame_count_from_mediainfo(dv_output_file)
    if dv_frames is None:
        print_color("yellow", "Could not obtain DV frame count from mediainfo; skipping frame count check.")
    else:
        print(f"DV output frame count (mediainfo): {dv_frames}")
        if meas_frames is not None and dv_frames != meas_frames:
            print_color(
                "red",
                f"Frame count mismatch: measurements={meas_frames}, DV container={dv_frames}",
            )
            ok = False

    # 4) Parse dovi_tool info summary for frames + profile
    try:
        frames_in_rpu = None
        profile_str = None
        with open(dovi_info_log, "r", encoding="utf-8", errors="ignore") as f:
            for line in f:
                s = line.strip()
                m_frames = re.search(r"^Frames?:\s*(\d+)$", s, re.IGNORECASE)
                if m_frames:
                    frames_in_rpu = int(m_frames.group(1))
                m_prof = re.search(r"Profile\s+([0-9.]+)", s, re.IGNORECASE)
                if m_prof:
                    profile_str = m_prof.group(1)
        if frames_in_rpu is not None and dv_frames is not None and frames_in_rpu != dv_frames:
            print_color("red", f"RPU frames ({frames_in_rpu}) != DV container frames ({dv_frames})")
            ok = False
        if meas_frames is not None and frames_in_rpu is not None and frames_in_rpu != meas_frames:
            print_color("red", f"RPU frames ({frames_in_rpu}) != measurements frames ({meas_frames})")
            ok = False
        if profile_str and not profile_str.startswith("8"):
            print_color("yellow", f"Warning: dovi_tool reported Profile {profile_str}, expected 8.x for P8.1")
    except Exception:
        pass

    # 5) DV presence via mediainfo JSON
    mi_json = get_mediainfo_json_cached(dv_output_file)
    if mi_json:
        video_track = next((t for t in mi_json.get("media", {}).get("track", []) if t.get("@type") == "Video"), None)
        if video_track:
            hdr_fmt = str(video_track.get("HDR_Format", ""))
            hdr_compat = str(video_track.get("HDR_Format_Compatibility", ""))
            combined = f"{hdr_fmt} {hdr_compat}".lower()
            if "dolby vision" not in combined:
                print_color("yellow", "Warning: mediainfo did not report Dolby Vision in HDR format fields.")

    if ok:
        print_color("green", "Verification passed: measurements and DV checks look consistent.")
    else:
        print_color("red", "Verification failed.")
    return ok
