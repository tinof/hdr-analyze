import os
import json
import re
import glob
import subprocess
import shutil
from enum import Enum, auto
from typing import List, Dict, Any, Optional

from .utils import print_color
from .external import get_mediainfo_json_cached, find_local_tool


class HdrFormat(Enum):
    """Enum for different HDR formats."""
    HDR10_PLUS = auto()
    HDR10_WITH_MEASUREMENTS = auto()
    HDR10_UNSUPPORTED = auto()
    HLG = auto()
    UNSUPPORTED = auto()


def find_measurements_file(input_file: str) -> Optional[str]:
    """Locate a madVR measurements file associated with the given input file."""
    directory = os.path.dirname(os.path.abspath(input_file))
    base_with_ext = os.path.basename(input_file)
    base_no_ext = os.path.splitext(base_with_ext)[0]

    candidates = [
        os.path.join(directory, "measurements.bin"),
        input_file + ".measurements",
        os.path.join(directory, base_with_ext + ".measurements"),
        os.path.join(directory, base_no_ext + ".measurements"),
        os.path.join(directory, base_no_ext + "_measurements.bin"),
    ]

    for candidate in candidates:
        if os.path.isfile(candidate):
            return os.path.abspath(candidate)

    try:
        pattern_matches = [
            p for p in glob.glob(os.path.join(directory, "*.measurements"))
            if os.path.basename(p).startswith(base_no_ext)
        ]
        if len(pattern_matches) == 1 and os.path.isfile(pattern_matches[0]):
            return os.path.abspath(pattern_matches[0])
    except Exception:
        pass

    try:
        pattern_matches = [
            p for p in glob.glob(os.path.join(directory, "*_measurements.bin"))
            if os.path.basename(p).startswith(base_no_ext)
        ]
        if len(pattern_matches) == 1 and os.path.isfile(pattern_matches[0]):
            return os.path.abspath(pattern_matches[0])
    except Exception:
        pass

    return None


def find_details_file(input_file: str) -> Optional[str]:
    """Locate a madVR Details.txt file associated with the given input file."""
    directory = os.path.dirname(os.path.abspath(input_file))
    base_no_ext = os.path.splitext(os.path.basename(input_file))[0]

    candidates = [
        os.path.join(directory, f"{base_no_ext}_mkv_Details.txt"),
        os.path.join(directory, f"{base_no_ext}_Details.txt"),
    ]

    for candidate in candidates:
        if os.path.isfile(candidate):
            return os.path.abspath(candidate)

    try:
        pattern_matches = [
            p
            for p in glob.glob(os.path.join(directory, "*_mkv_Details.txt"))
            if os.path.basename(p).startswith(base_no_ext)
        ]
        if len(pattern_matches) == 1 and os.path.isfile(pattern_matches[0]):
            return os.path.abspath(pattern_matches[0])
    except Exception:
        pass

    return None


def _extract_number(value_str: str) -> Optional[float]:
    """Extract the first number in a string, handling comma decimals."""
    m = re.search(r"([0-9]+[.,]?[0-9]*)", value_str)
    if not m:
        return None
    try:
        return float(m.group(1).replace(",", "."))
    except ValueError:
        return None


def parse_madvr_details_static_values(details_path: str) -> Dict[str, Any]:
    """Parse key static values from madVR Details.txt."""
    result: Dict[str, Any] = {}
    try:
        with open(details_path, "r", encoding="utf-8", errors="ignore") as f:
            lines = f.readlines()

        in_after_clipping = False
        max_cll_after: Optional[int] = None
        max_cll_100: Optional[int] = None
        max_fall: Optional[int] = None
        min_dml: Optional[float] = None
        max_dml: Optional[float] = None

        for raw in lines:
            line = raw.strip()
            if re.search(r"^Calculated values after clipping", line, re.IGNORECASE):
                in_after_clipping = True
                continue
            m_lum = re.search(r"Mastering\s+display\s+luminance:\s*([0-9.,]+)\s*/\s*([0-9.,]+)", line, re.IGNORECASE)
            if m_lum:
                min_val = _extract_number(m_lum.group(1))
                max_val = _extract_number(m_lum.group(2))
                if min_val is not None: min_dml = float(min_val)
                if max_val is not None: max_dml = float(max_val)
                continue
            m_cll_100 = re.search(r"MaxCLL\s*100%\s*:\s*([0-9.,]+)", line, re.IGNORECASE)
            if m_cll_100 and max_cll_100 is None:
                num = _extract_number(m_cll_100.group(1))
                if num is not None: max_cll_100 = int(num)
                continue
            if in_after_clipping:
                m_cll_after = re.search(r"^MaxCLL\s*:\s*([0-9.,]+)$", line, re.IGNORECASE)
                if m_cll_after:
                    num = _extract_number(m_cll_after.group(1))
                    if num is not None: max_cll_after = int(num)
                    continue
            m_fall = re.search(r"^MaxFALL\s*:\s*([0-9.,]+)", line, re.IGNORECASE)
            if m_fall and max_fall is None:
                num = _extract_number(m_fall.group(1))
                if num is not None: max_fall = int(num)
                continue

        if min_dml is not None: result["min_dml"] = min_dml
        if max_dml is not None: result["max_dml"] = int(max_dml)
        if max_fall is not None: result["max_fall"] = int(max_fall)
        if max_cll_after is not None:
            result["max_cll"] = int(max_cll_after)
        elif max_cll_100 is not None:
            result["max_cll"] = int(max_cll_100)
    except Exception:
        return {}
    return result


def parse_madvr_details_for_trims(details_path: str) -> Optional[List[int]]:
    """Extract suggested trim target nits from Details.txt."""
    try:
        with open(details_path, "r", encoding="utf-8", errors="ignore") as f:
            text = f.read()

        real_display_peak = None
        max_target_nits = None

        m_peak = re.search(r"Real\s+display\s+peak\s+nits:\s*([0-9.,]+)", text, re.IGNORECASE)
        if m_peak:
            v = _extract_number(m_peak.group(1))
            if v is not None and v > 0: real_display_peak = int(round(v))

        m_max = re.search(r"Maximum\s+Target\s+Nits:\s*([0-9.,]+)", text, re.IGNORECASE)
        if m_max:
            v = _extract_number(m_max.group(1))
            if v is not None and v > 0: max_target_nits = int(round(v))

        targets: List[int] = [100]
        if real_display_peak: targets.append(real_display_peak)
        if max_target_nits: targets.append(max_target_nits)

        targets = sorted({t for t in targets if 80 <= t <= 10000})
        if len(targets) >= 2:
            return targets
        return None
    except Exception:
        return None


def get_static_metadata(input_file: str) -> Dict[str, Any]:
    """Consolidates logic to extract static HDR metadata from a video file."""
    metadata = {}
    defaults = {"max_dml": 1000, "min_dml": 0.0050, "max_cll": 1000, "max_fall": 400}

    try:
        mi_json = get_mediainfo_json_cached(input_file) or {}
        video_track = next((t for t in mi_json.get("media", {}).get("track", []) if t.get("@type") == "Video"), None)

        if video_track:
            mdl = video_track.get("MasteringDisplay_Luminance")
            if mdl:
                max_dml_match = re.search(r"max: ([0-9.]+)", mdl)
                min_dml_match = re.search(r"min: ([0-9.]+)", mdl)
                if max_dml_match: metadata["max_dml"] = int(float(max_dml_match.group(1)))
                if min_dml_match: metadata["min_dml"] = float(min_dml_match.group(1))

            max_cll_str = video_track.get("MaxCLL", "0")
            max_fall_str = video_track.get("MaxFALL", "0")
            max_cll_match = re.search(r"([0-9.]+)", str(max_cll_str))
            max_fall_match = re.search(r"([0-9.]+)", str(max_fall_str))
            metadata["max_cll"] = int(float(max_cll_match.group(1))) if max_cll_match else 0
            metadata["max_fall"] = int(float(max_fall_match.group(1))) if max_fall_match else 0
    except (subprocess.SubprocessError, json.JSONDecodeError, FileNotFoundError) as e:
        print_color("yellow", f"Warning: Could not get metadata from mediainfo: {e}")

    details_path = find_details_file(input_file)
    if details_path:
        details_values = parse_madvr_details_static_values(details_path)
        if details_values:
            metadata.update({k: v for k, v in details_values.items() if v is not None})
            print(f"Supplemented static metadata from Details.txt: { {k: metadata[k] for k in ['max_dml','min_dml','max_cll','max_fall'] if k in metadata} }")

    final_metadata = defaults.copy()
    missing_keys = []
    for key in defaults:
        if metadata.get(key):
            final_metadata[key] = metadata[key]
        else:
            missing_keys.append(key.upper())

    if missing_keys:
        print_color("yellow", f"Warning: Missing metadata for: {', '.join(missing_keys)}. Using defaults.")
        print_color("yellow", f"Default values used: MaxDML={defaults['max_dml']}, MinDML={defaults['min_dml']}, MaxCLL={defaults['max_cll']}, MaxFALL={defaults['max_fall']}")

    return final_metadata


def validate_static_metadata(metadata: Dict[str, Any], source_desc: str = "input") -> bool:
    """Validate static HDR metadata for sanity and conformance."""
    issues = []
    warnings = []
    max_dml = metadata.get("max_dml", 0)
    min_dml = metadata.get("min_dml", 0)
    max_cll = metadata.get("max_cll", 0)
    max_fall = metadata.get("max_fall", 0)

    if max_dml < 100: warnings.append(f"MaxDML={max_dml} is unusually low (<100 nits)")
    elif max_dml > 10000: issues.append(f"MaxDML={max_dml} exceeds ST.2084 range (10000 nits)")
    if min_dml > 0.05: warnings.append(f"MinDML={min_dml} is unusually high (>0.05 nits, typical OLED ~0.005)")
    elif min_dml <= 0: issues.append(f"MinDML={min_dml} must be positive")
    if max_cll <= 0: issues.append(f"MaxCLL={max_cll} must be positive")
    elif max_cll > 10000: warnings.append(f"MaxCLL={max_cll} exceeds ST.2084 range (10000 nits)")
    if max_fall <= 0: issues.append(f"MaxFALL={max_fall} must be positive")
    elif max_fall > 10000: warnings.append(f"MaxFALL={max_fall} exceeds ST.2084 range (10000 nits)")
    if max_cll > 0 and max_fall > 0 and max_cll < max_fall: warnings.append(f"MaxCLL={max_cll} < MaxFALL={max_fall} (unusual: peak should >= average)")

    if issues:
        print_color("red", f"Metadata validation errors for {source_desc}:")
        for issue in issues: print_color("red", f"  • {issue}")
        return False
    if warnings:
        print_color("yellow", f"Metadata validation warnings for {source_desc}:")
        for warning in warnings: print_color("yellow", f"  • {warning}")
    return True


def generate_extra_json(output_file: str, metadata: Dict[str, Any], trim_targets: List[int]):
    """Generates the extra.json file for dovi_tool."""
    try:
        min_display_luminance = int(float(metadata["min_dml"]) * 10000)
        json_content = {
            "target_nits": trim_targets,
            "level6": {
                "max_display_mastering_luminance": int(metadata["max_dml"]),
                "min_display_mastering_luminance": min_display_luminance,
                "max_content_light_level": int(metadata["max_cll"]),
                "max_frame_average_light_level": int(metadata["max_fall"]),
            },
        }
        with open(output_file, "w") as f:
            json.dump(json_content, f, indent=2)
        print("Generated extra.json content:")
        print(json.dumps(json_content, indent=2))
        return True
    except (IOError, TypeError) as e:
        print_color("red", f"Error: Failed to generate or write extra.json: {e}")
        return False


def check_hdr_format(input_file: str) -> HdrFormat:
    """Checks the HDR format of the input file using mediainfo with robust fallbacks."""

    def infer_hdr_from_mi_json(video_track: Dict[str, Any]) -> Optional[HdrFormat]:
        # Gather candidates from explicit HDR fields
        hdr_fmt = str(video_track.get("HDR_Format", "")).upper()
        hdr_compat = str(video_track.get("HDR_Format_Compatibility", "")).upper()
        combined = f"{hdr_fmt} {hdr_compat}"
        if "2094" in combined or "HDR10+" in combined or "HDR10 PLUS" in combined:
            return HdrFormat.HDR10_PLUS
        if "HLG" in combined:
            return HdrFormat.HLG
        if "HDR10" in combined:
            return HdrFormat.HDR10_UNSUPPORTED

        # Scan other fields for transfer function hints
        for key, value in video_track.items():
            if not isinstance(value, str):
                continue
            val = value.upper()
            if "ARIB" in val and "B67" in val:
                return HdrFormat.HLG
            if "HLG" in val:
                return HdrFormat.HLG
            if "SMPTE" in val and "2084" in val:
                # PQ detected -> treat as HDR10 (non-plus) unless plus metadata found elsewhere
                return HdrFormat.HDR10_UNSUPPORTED
        return None

    def ffprobe_color_transfer(input_path: str) -> Optional[str]:
        try:
            if not shutil.which("ffprobe"):
                return None
            out = subprocess.check_output(
                [
                    "ffprobe",
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=color_transfer",
                    "-of",
                    "default=nokey=1:noprint_wrappers=1",
                    input_path,
                ],
                text=True,
            ).strip()
            return out or None
        except subprocess.CalledProcessError:
            return None

    try:
        result = subprocess.check_output(
            [
                "mediainfo",
                "--Inform=Video;%HDR_Format%/%HDR_Format_Compatibility%",
                input_file,
            ],
            text=True,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        format_info = result.strip()

        measurements_file = find_measurements_file(input_file)

        # Primary, simple string-based detection
        if "SMPTE ST 2094 App 4" in format_info or "HDR10+" in format_info:
            print("Detected: HDR10+")
            return HdrFormat.HDR10_PLUS
        if "HLG" in format_info:
            print("Detected: HLG")
            return HdrFormat.HLG
        if "HDR10" in format_info or "PQ" in format_info or "ST 2084" in format_info:
            if measurements_file:
                print("Detected: HDR10 with madVR measurements file")
                return HdrFormat.HDR10_WITH_MEASUREMENTS
            print("Detected: HDR10 (no measurements)")
            return HdrFormat.HDR10_UNSUPPORTED

        # Fallback: use mediainfo JSON to infer
        mi_json = get_mediainfo_json_cached(input_file) or {}
        video_track = next(
            (
                t
                for t in mi_json.get("media", {}).get("track", [])
                if t.get("@type") == "Video"
            ),
            None,
        )
        if video_track:
            inferred = infer_hdr_from_mi_json(video_track)
            if inferred:
                if inferred == HdrFormat.HDR10_UNSUPPORTED and measurements_file:
                    print("Detected: HDR10 with madVR measurements file (inferred)")
                    return HdrFormat.HDR10_WITH_MEASUREMENTS
                print(f"Detected (inferred): {inferred.name}")
                return inferred

        # Fallback 2: ffprobe color_transfer
        color_trc = ffprobe_color_transfer(input_file)
        if color_trc:
            trc = color_trc.strip().lower()
            if "arib-std-b67" in trc or "hlg" in trc:
                print("Detected (ffprobe): HLG")
                return HdrFormat.HLG
            if "smpte2084" in trc or "pq" in trc:
                if measurements_file:
                    print("Detected (ffprobe): HDR10 with madVR measurements file")
                    return HdrFormat.HDR10_WITH_MEASUREMENTS
                print("Detected (ffprobe): HDR10 (no measurements)")
                return HdrFormat.HDR10_UNSUPPORTED

        # If still unknown, report unsupported with raw info string
        fmt_display = format_info if format_info else "/"
        print(f"Detected: Unsupported format ({fmt_display})")
        return HdrFormat.UNSUPPORTED
    except subprocess.CalledProcessError as e:
        print_color("red", f"Error checking HDR format with mediainfo: {e.stderr}")
        return HdrFormat.UNSUPPORTED


def get_frame_count_from_mediainfo(input_file: str) -> Optional[int]:
    """Try to obtain frame count from mediainfo JSON, with fallbacks."""
    mi_json = get_mediainfo_json_cached(input_file)
    if not mi_json: return None
    video_track = next((t for t in mi_json.get("media", {}).get("track", []) if t.get("@type") == "Video"), None)
    if not video_track: return None
    frame_count_str = video_track.get("FrameCount")
    if frame_count_str and str(frame_count_str).isdigit():
        return int(frame_count_str)
    try:
        duration_ms = float(video_track.get("Duration", 0))
        frame_rate = float(video_track.get("FrameRate", 0))
        if duration_ms > 0 and frame_rate > 0:
            return int(round((duration_ms / 1000.0) * frame_rate))
    except Exception:
        pass
    return None


def get_duration_from_mediainfo(input_file: str) -> Optional[float]:
    """Get video duration in seconds from mediainfo."""
    mi_json = get_mediainfo_json_cached(input_file)
    if not mi_json: return None
    video_track = next((t for t in mi_json.get("media", {}).get("track", []) if t.get("@type") == "Video"), None)
    if not video_track: return None
    try:
        duration_ms = float(video_track.get("Duration", 0))
        if duration_ms > 0:
            return duration_ms / 1000.0
    except (ValueError, TypeError):
        pass
    return None
