#!/usr/bin/env python3
# Pack an .iconset directory of PNGs into an .icns, storing the PNG bytes
# as-is (so pre-optimized/zopfli'd PNGs stay small — unlike iconutil, which
# re-encodes them). Usage: pack_icns.py <in.iconset> <out.icns>
import struct, sys, pathlib

# iconset filename -> icns OSType (PNG-data chunk types)
TYPES = {
    "icon_16x16.png": b"icp4",
    "icon_16x16@2x.png": b"ic11",
    "icon_32x32.png": b"icp5",
    "icon_32x32@2x.png": b"ic12",
    "icon_128x128.png": b"ic07",
    "icon_128x128@2x.png": b"ic13",
    "icon_256x256.png": b"ic08",
    "icon_256x256@2x.png": b"ic14",
    "icon_512x512.png": b"ic09",
    "icon_512x512@2x.png": b"ic10",
}

src = pathlib.Path(sys.argv[1])
out = pathlib.Path(sys.argv[2])

chunks = b""
for name, ostype in TYPES.items():
    data = (src / name).read_bytes()
    chunks += ostype + struct.pack(">I", len(data) + 8) + data

icns = b"icns" + struct.pack(">I", len(chunks) + 8) + chunks
out.write_bytes(icns)
print(f"{out}: {len(icns)} bytes ({len(TYPES)} reps)")
