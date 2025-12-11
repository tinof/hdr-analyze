import struct
import sys


def read_header(file_path):
    with open(file_path, "rb") as f:
        data = f.read(28)  # Read enough for MaxCLL

    magic = data[0:4]
    if magic != b"mvr+":
        print(f"Invalid magic: {magic}")
        return

    version = struct.unpack("<I", data[4:8])[0]
    header_size = struct.unpack("<I", data[8:12])[0]
    scene_count = struct.unpack("<I", data[12:16])[0]
    frame_count = struct.unpack("<I", data[16:20])[0]
    flags = struct.unpack("<I", data[20:24])[0]
    maxcll = struct.unpack("<I", data[24:28])[0]

    print(f"File: {file_path}")
    print(f"Version: {version}")
    print(f"MaxCLL: {maxcll}")
    print(f"Flags: {flags}")


read_header("s01e10.measurements")
