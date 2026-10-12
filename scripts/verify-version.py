"""PE/.res の版を独立に読み、Cargo・Git・CIの出所と照合する（標準ライブラリのみ）。"""
from pathlib import Path
import importlib.util
import os
import re
import struct
import subprocess
import sys

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("icons", Path(__file__).with_name("verify-icons.py"))
icons = importlib.util.module_from_spec(spec)
spec.loader.exec_module(icons)
ROOT = Path(__file__).resolve().parents[1]


def align(n):
    return (n + 3) & ~3


def block(data, start=0):
    """長さと文字列・バイナリの単位を確認しながら VERSIONINFO を読む。"""
    length, value_length, kind = struct.unpack_from("<HHH", data, start)
    end = start + length
    assert 6 <= length and end <= len(data)
    pos = start + 6
    key_start = pos
    while icons.u16(data, pos):
        pos += 2
        assert pos < end
    key = data[key_start:pos].decode("utf-16-le")
    pos = align(pos + 2)
    size = value_length * (2 if kind == 1 else 1)
    value = data[pos:pos + size]
    assert len(value) == size and pos + size <= end
    pos = align(pos + size)
    children = {}
    while pos < end:
        name, child_value, nested, child_end = block(data, pos)
        assert name not in children
        children[name] = (child_value, nested)
        pos = align(child_end)
    return key, value, children, end


def verify(path):
    """コンパイル結果を実行せず、識別情報と固定数値版を照合する。"""
    data = path.read_bytes()
    records = icons.read_pe(data) if data[:2] == b"MZ" else icons.read_res(data)
    root, fixed, children, end = block(records[16, 1])
    assert root == "VS_VERSION_INFO" and end == len(records[16, 1])
    assert len(fixed) == 52
    fields = struct.unpack("<13I", fixed)
    assert fields[:2] == (0xFEEF04BD, 0x10000)
    table = children["StringFileInfo"][1]["040904B0"][1]
    strings = {k: v[0].decode("utf-16-le").removesuffix("\0") for k, v in table.items()}
    assert children["VarFileInfo"][1]["Translation"][0] == struct.pack("<HH", 0x409, 1200)
    product = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M)[1]
    major, minor, patch = map(int, re.split(r"[-+]", product)[0].split("."))
    github = os.environ.get("GITHUB_ACTIONS") == "true"
    number = int(os.environ["GITHUB_RUN_NUMBER"]) if github else 0
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    tree = "dirty" if subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=normal"], cwd=ROOT).strip() else "clean"
    origin = "github" if github else "local"
    run = os.environ["GITHUB_RUN_ID"] if github else "none"
    attempt = os.environ["GITHUB_RUN_ATTEMPT"] if github else "none"
    file_version = f"{major}.{minor}.{patch}.{number}"
    label = f"product={product} file={file_version} origin={origin} run={run} attempt={attempt} commit={commit} tree={tree}"
    expected = {"FileVersion": file_version, "ProductVersion": product,
                "OriginalFilename": path.stem + ".exe", "BuildOrigin": origin,
                "BuildRunId": run, "BuildAttempt": attempt, "Commit": commit,
                "SourceTree": tree, "Comments": label}
    for key, value in expected.items():
        assert strings[key] == value, (key, strings[key], value)
    assert fields[2:6] == (major << 16 | minor, patch << 16 | number, major << 16 | minor, patch << 16)
    assert fields[7] == (0 if github else 8)
    assert strings["PrivateBuild"] == ("" if github else label)
    print(f"PASS {path}: VERSIONINFO matches Cargo/Git/build origin; {label}")


if __name__ == "__main__":
    for argument in sys.argv[1:]:
        verify(Path(argument))
    if len(sys.argv) < 2:
        raise SystemExit("usage: verify-version.py FILE.exe [FILE.exe ...]")
