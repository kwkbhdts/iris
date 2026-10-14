"""アップロード直前の配布物の過不足と、同梱ハッシュを確認する。"""

import hashlib
from pathlib import Path
import re
import sys


def verify(directory, executable, documents):
    """期待した通常ファイルだけを許可し、各バイト列とハッシュを照合する。"""
    root = Path(directory)
    payload = {executable, 'THIRD_PARTY_NOTICES.txt', 'RUST-LIBRARY.html', *documents}
    expected = payload | {'SHA256SUMS.txt'}
    if {p.name for p in root.iterdir()} != expected:
        raise ValueError('package file list mismatch')
    if any(not (root / name).is_file() or (root / name).is_symlink() for name in expected):
        raise ValueError('package contains a non-regular file')
    records = {}
    for line in (root / 'SHA256SUMS.txt').read_text(encoding='utf-8-sig').splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  (.+)', line)
        if not match or match[2] not in payload or match[2] in records:
            raise ValueError('invalid package hash manifest')
        records[match[2]] = match[1]
    if set(records) != payload:
        raise ValueError('incomplete package hash manifest')
    for name in sorted(payload):
        data = (root / name).read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        if not data or digest != records[name]:
            raise ValueError('package hash or empty-file check failed: ' + name)
        print(f'PASS package file: {name} bytes={len(data)} sha256={digest}')
    print(f'PASS package contents: {root.name} files={len(expected)}')


if __name__ == '__main__':
    verify(sys.argv[1], sys.argv[2], sys.argv[3:])
