# Improving Performance: A Two-Pronged Approach

**Phase 1: The Low-Hanging Fruit (Consolidate without GPU)**

This gives a solid performance boost while remaining 100% portable.

1.  **Keep `get_video_info`:** Continue to run `ffprobe` once at the beginning to get the resolution and frame count. This is the correct approach.
2.  **Combine Scene Detection and Frame Analysis:** Modify the `analyze_frames` function to do everything. Spawn a single `ffmpeg` process that:
    *   Pipes raw RGB24 data to `stdout` (as it does now).
    *   Simultaneously runs the `scdet` filter and prints the metadata to `stderr`.
3.  **Adapt Your Rust Code:**
    *   Your Rust application will now need to read from `stdout` (for frames) and `stderr` (for scene cuts) from the *same child process* at the same time. This requires using non-blocking reads or dedicated threads for each stream.

**Result:** You reduce the number of processes from four to two (`ffprobe` + `ffmpeg`) and eliminate one full read of the video file. This is a significant, easy, and portable win.