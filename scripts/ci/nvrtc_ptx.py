"""Compile a CUDA source to PTX with one specific NVRTC library, through ctypes.

Used by scripts/gen-kernels-ptx.sh. The library is loaded from an absolute path, never by its
plain name, so the PTX cannot silently come from another NVRTC version on the host. The output
is NVRTC's PTX byte for byte (trailing NUL removed): no header or stamp is added.

Usage: python3 -I nvrtc_ptx.py <libnvrtc path> <expected major.minor> <source.cu> <out.ptx> [option...]

Exit 1: the kernel does not compile. Exit 2: the environment is wrong (arguments, library path or
version, an NVRTC call that fails), which no edit to the kernel fixes.
"""

import ctypes
import os
import sys


def fail(message: str, code: int = 2) -> None:
    sys.stderr.write(f"nvrtc_ptx: {message}\n")
    sys.exit(code)


def main() -> None:
    if len(sys.argv) < 5:
        fail("usage: nvrtc_ptx.py <libnvrtc> <major.minor> <source.cu> <out.ptx> [option...]")
    lib_path, expected, src_path, out_path = sys.argv[1:5]
    options = [option.encode() for option in sys.argv[5:]]
    if not os.path.isabs(lib_path):
        fail(f"the NVRTC library must be an absolute path, got {lib_path!r}")
    if not os.path.isfile(lib_path):
        fail(f"NVRTC library not found: {lib_path}")

    nvrtc = ctypes.CDLL(lib_path)
    major, minor = ctypes.c_int(), ctypes.c_int()
    if nvrtc.nvrtcVersion(ctypes.byref(major), ctypes.byref(minor)) != 0:
        fail("nvrtcVersion failed")
    found = f"{major.value}.{minor.value}"
    if found != expected:
        fail(f"{lib_path} is NVRTC {found}; the committed PTX is pinned to NVRTC {expected}")

    with open(src_path, "rb") as source:
        src = source.read()
    program = ctypes.c_void_p()
    if nvrtc.nvrtcCreateProgram(ctypes.byref(program), src, None, 0, None, None) != 0:
        fail("nvrtcCreateProgram failed")
    option_array = (ctypes.c_char_p * len(options))(*options)
    status = nvrtc.nvrtcCompileProgram(program, len(options), option_array)

    size = ctypes.c_size_t()
    _ = nvrtc.nvrtcGetProgramLogSize(program, ctypes.byref(size))
    log = ctypes.create_string_buffer(size.value)
    _ = nvrtc.nvrtcGetProgramLog(program, log)
    log_text = log.value.decode(errors="replace").strip()
    if status != 0:
        fail(f"compile failed ({status}):\n{log_text}", code=1)
    if log_text:
        sys.stderr.write(f"nvrtc_ptx: compiler log:\n{log_text}\n")

    if nvrtc.nvrtcGetPTXSize(program, ctypes.byref(size)) != 0:
        fail("nvrtcGetPTXSize failed")
    ptx = ctypes.create_string_buffer(size.value)
    if nvrtc.nvrtcGetPTX(program, ptx) != 0:
        fail("nvrtcGetPTX failed")
    _ = nvrtc.nvrtcDestroyProgram(ctypes.byref(program))
    with open(out_path, "wb") as out:
        _ = out.write(ptx.raw.rstrip(b"\0"))


if __name__ == "__main__":
    main()
