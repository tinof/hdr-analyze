---
name: Bug Report
about: Create a report to help us improve HDR-Analyze
title: '[BUG] '
labels: ['bug']
assignees: ''
---

## Bug Description

**Describe the bug**
A clear and concise description of what the bug is.

**Expected Behavior**
A clear and concise description of what you expected to happen.

**Actual Behavior**
A clear and concise description of what actually happened.

## Steps to Reproduce

Please provide detailed steps to reproduce the behavior:

1. Go to '...'
2. Run command '...'
3. Use input file '...'
4. See error

**Command Used**
```bash
# Paste the exact command you ran here
./target/release/hdr_analyzer_mvp -i "input.mkv" -o "output.bin" --enable-optimizer
```

## System Information

**Environment Details:**
- OS: [e.g., Windows 11, macOS 13.0, Ubuntu 22.04]
- Rust Version: [output of `rustc --version`]
- FFmpeg Version: [output of `ffmpeg -version`]
- HDR-Analyze Version: [e.g., v1.0.0]

**Hardware:**
- CPU: [e.g., Intel i7-12700K, AMD Ryzen 9 5900X]
- RAM: [e.g., 32GB]
- Storage: [e.g., NVMe SSD, HDD]

## Input File Information

**Video Details:**
- File Format: [e.g., MKV, MP4, MOV]
- Resolution: [e.g., 3840x2160, 1920x1080]
- Frame Rate: [e.g., 23.976 fps, 29.97 fps]
- Duration: [e.g., 2h 15m]
- HDR Format: [e.g., HDR10, HDR10+, Dolby Vision]
- File Size: [e.g., 25GB]

**Can you share the file?**
- [ ] Yes, I can provide a sample file
- [ ] No, the file is copyrighted/private
- [ ] I can provide a short sample clip

## Error Output

**Error Messages:**
```
Paste any error messages, stack traces, or relevant output here
```

**Log Output:**
If available, please run with debug logging and include relevant output:
```bash
RUST_LOG=debug ./target/release/hdr_analyzer_mvp [your command]
```

```
Paste debug log output here
```

## Additional Context

**Screenshots**
If applicable, add screenshots to help explain your problem.

**Workarounds**
Have you found any workarounds for this issue?

**Related Issues**
Are there any related issues or discussions?

**Additional Information**
Add any other context about the problem here.

## Checklist

Before submitting, please check:

- [ ] I have searched existing issues to ensure this is not a duplicate
- [ ] I have provided all the requested system information
- [ ] I have included the exact command that caused the issue
- [ ] I have included relevant error messages or logs
- [ ] I have tested with the latest version of HDR-Analyze
- [ ] I have verified that FFmpeg is properly installed and accessible

## Priority

How critical is this issue for your workflow?

- [ ] Critical - Blocks all usage
- [ ] High - Significantly impacts usage
- [ ] Medium - Impacts some use cases
- [ ] Low - Minor inconvenience

Thank you for taking the time to report this issue! We'll investigate and respond as soon as possible.
