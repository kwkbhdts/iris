"""SVG から ICO と Win32 .res を再生成する（Pillow と Inkscape が必要）。"""
from pathlib import Path
import io
import struct
import subprocess
import tempfile

from PIL import Image

ROOT = Path(__file__).resolve().parents[1] / "assets" / "icons"
SIZES = (16, 20, 24, 32, 48, 64, 128, 256)


def resource(kind, identifier, data):
    """整数ID・言語中立の RESOURCEHEADER と4バイト境界のデータを作る。"""
    header = struct.pack("<IIHHHHIHHII", len(data), 32, 0xFFFF, kind,
                         0xFFFF, identifier, 0, 0x1030 if kind else 0, 0, 0, 0)
    return header + data + bytes((-len(data)) % 4)


def generate():
    """原本を高解像度で描画し、各サイズと対応するアイコン群を保存する。"""
    res = resource(0, 0, b"")
    image_id = 1
    with tempfile.TemporaryDirectory() as temp:
        for group_id, name in [(101, "iris-app"), (102, "iris-tray")]:
            png = Path(temp) / (name + ".png")
            subprocess.run(["inkscape", str(ROOT / (name + ".svg")),
                            "--export-type=png", "--export-width=1024",
                            "--export-filename=" + str(png)], check=True)
            with Image.open(png) as source:
                master = source.convert("RGBA")
            chunks = []
            for size in SIZES:
                output = io.BytesIO()
                master.resize((size, size), Image.Resampling.LANCZOS).save(output, "PNG")
                chunks.append(output.getvalue())

            ico = struct.pack("<HHH", 0, 1, len(SIZES))
            group = ico
            offset = 6 + 16 * len(SIZES)
            for size, chunk in zip(SIZES, chunks):
                entry = struct.pack("<BBBBHHI", size % 256, size % 256, 0, 0, 1, 32, len(chunk))
                ico += entry + struct.pack("<I", offset)
                group += entry + struct.pack("<H", image_id)
                res += resource(3, image_id, chunk)
                offset += len(chunk)
                image_id += 1
            (ROOT / (name + ".ico")).write_bytes(ico + b"".join(chunks))
            res += resource(14, group_id, group)
    (ROOT / "iris.res").write_bytes(res)


if __name__ == "__main__":
    generate()
