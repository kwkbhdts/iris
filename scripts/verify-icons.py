""".res または PE 内の2つのアイコン群を ICO 原本とバイト単位で照合する。"""
from pathlib import Path
import struct
import sys

ROOT = Path(__file__).resolve().parents[1] / "assets" / "icons"


def u16(data, offset):
    return struct.unpack_from("<H", data, offset)[0]


def u32(data, offset):
    return struct.unpack_from("<I", data, offset)[0]


def read_res(data):
    """生成済み .res の整数IDとデータを読む。"""
    result = {}
    pos = 0
    while pos < len(data):
        size, header = struct.unpack_from("<II", data, pos)
        assert header == 32
        assert u16(data, pos + 8) == u16(data, pos + 12) == 0xFFFF
        kind, identifier = u16(data, pos + 10), u16(data, pos + 14)
        assert pos + header + size <= len(data)
        if kind:
            assert (kind, identifier) not in result
            result[kind, identifier] = data[pos + header:pos + header + size]
        pos = (pos + header + size + 3) & ~3
    assert pos == len(data)
    return result


def read_pe(data):
    """実際にリンクされた PE のリソースディレクトリをたどる。"""
    pe = u32(data, 0x3C)
    assert data[pe:pe + 4] == b"PE\0\0"
    opt = pe + 24
    assert u16(data, opt) == 0x20B, "expected PE32+"
    rva = u32(data, opt + 112 + 16)
    sections = opt + u16(data, pe + 20)

    def offset(address):
        for n in range(u16(data, pe + 6)):
            s = sections + n * 40
            start, size, raw = u32(data, s + 12), u32(data, s + 16), u32(data, s + 20)
            if start <= address < start + size:
                return raw + address - start
        raise AssertionError("RVA outside file-backed sections")

    assert rva, "missing resource directory"
    base = offset(rva)
    result = {}

    def walk(relative, path):
        node = base + relative
        count = u16(data, node + 12) + u16(data, node + 14)
        for i in range(count):
            name, target = struct.unpack_from("<II", data, node + 16 + 8 * i)
            if name & 0x80000000:
                continue
            ids = path + (name,)
            if target & 0x80000000:
                walk(target & 0x7FFFFFFF, ids)
            elif ids[0] in (3, 14):
                assert len(ids) == 3 and ids[2] == 0
                entry = base + target
                start, size = offset(u32(data, entry)), u32(data, entry + 4)
                assert start + size <= len(data)
                result[ids[:2]] = data[start:start + size]

    walk(0, ())
    return result


def verify(path):
    """各サイズのグループ情報と PNG 本体が元 ICO に一致することを確認する。"""
    data = path.read_bytes()
    records = read_pe(data) if data[:2] == b"MZ" else read_res(data)
    seen = set()
    for identifier, name in [(101, "iris-app"), (102, "iris-tray")]:
        ico = (ROOT / (name + ".ico")).read_bytes()
        group = records[14, identifier]
        assert group[:6] == ico[:6] == struct.pack("<HHH", 0, 1, 8)
        assert len(group) == 6 + 14 * 8
        for i, size in enumerate((16, 20, 24, 32, 48, 64, 128, 256)):
            a, b = 6 + 16 * i, 6 + 14 * i
            assert ico[a:a + 12] == group[b:b + 12]
            assert ico[a] == ico[a + 1] == size % 256
            length, start = u32(ico, a + 8), u32(ico, a + 12)
            image_id = u16(group, b + 12)
            assert image_id not in seen
            seen.add(image_id)
            assert records[3, image_id] == ico[start:start + length]
    assert len(seen) == 16
    print(f"PASS {path}: app=101, tray=102; all 16 embedded images match source ICOs")


if __name__ == "__main__":
    verify(Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "iris.res")
