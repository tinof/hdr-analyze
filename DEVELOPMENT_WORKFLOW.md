# Development Workflow

This document provides practical instructions for building, running, and verifying the `hdr-analyze` project and its components.

---

## 1. Prerequisites

Before you begin, ensure the following are installed and available in your system's `PATH`:

-   **Rust Toolchain**: Build the workspace with `cargo build --release --workspace`.
-   **External Tools**:
    -   `ffmpeg`
    -   `mkvmerge`
    -   `mediainfo`
    -   `dovi_tool`
    -   `hdr10plus_tool`
-   **Analyzer in PATH**: For convenience, add the compiled binary to your path:
    ```bash
    export PATH="$(pwd)/target/release:$PATH"
    ```

---

## 2. Running the `mkvdolby` Script

The `mkvdolby` script provides an end-to-end workflow for processing video files.

-   **Standard Run**:
    ```bash
    PYTHONPATH="mkvdolby/src" python -m mkvdolby.cli "<input_video>"
    ```
-   **Run with Verification**: The `--verify` flag runs a series of checks after processing to ensure the output is valid.
    ```bash
    PYTHONPATH="mkvdolby/src" python -m mkvdolby.cli "<input_video>" --verify
    ```

### Workflow Notes:

-   **HDR10 Input**: If a `_measurements.bin` file already exists for the input, `mkvdolby` will use it. Otherwise, it will run `hdr-analyze` to generate one.
-   **HLG Input**: `mkvdolby` uses the native HLG analysis path in `hdr-analyze`. It also performs a single HLG-to-PQ encode to create the required Dolby Vision Profile 8.1 base layer.

---

## 3. Running the Analyzer Directly

You can also run the `hdr_analyzer_mvp` tool directly for more granular control.

-   **PQ/HDR10 Content**:
    ```bash
    cargo run -p hdr_analyzer_mvp --release -- "video.mkv" -o "video_measurements.bin"
    ```
-   **HLG Content (Native Path)**:
    ```bash
    cargo run -p hdr_analyzer_mvp --release -- "video_hlg.mkv" -o "video_hlg_measurements.bin" --hlg-peak-nits 1000
    ```

---

## 4. Verification Steps

### 4.1. Verifying Measurement Files (`.bin`)

Use the `verifier` tool to inspect and validate the generated measurement files.

```bash
target/release/verifier "path/to/measurements.bin"
```
Expect to see a summary of the file's properties, including version, scene/frame counts, and optimizer status, followed by a validation result.

### 4.2. Verifying Dolby Vision Output

To inspect the final Dolby Vision MKV file:

1.  **Check MediaInfo**:
    ```bash
    mediainfo "output.DV.mkv"
    ```
    Expect to see Dolby Vision Profile 8.1, PQ transfer function, and BT.2020 primaries.

2.  **Inspect RPU with `dovi_tool`**:
    ```bash
    dovi_tool extract-rpu -i "output.DV.mkv" -o RPU.bin
    dovi_tool info -i RPU.bin --summary
    ```
    Expect the frame count to match the measurements and the profile to be reported as Profile 8, CM v4.0.

---

## 5. Baseline Comparison Harness

To guard against regressions, use the `compare_baseline` tool.

1.  **Build the tool**:
    ```bash
    cargo build --release -p compare_baseline
    ```
2.  **Run comparison**:
    ```bash
    target/release/compare_baseline --baseline ./path/to/baseline_bins --current ./path/to/current_bins
    ```
    The tool will report deltas for key metrics like scene count, MaxCLL/MaxFALL, and `target_nits`.
