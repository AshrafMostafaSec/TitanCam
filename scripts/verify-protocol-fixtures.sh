#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh
cargo test --locked -p titan-protocol header_golden_fixture
python3 - <<'PY'
import struct,pathlib,json
actual=pathlib.Path('protocol/media-header-v1.bin').read_bytes()
expected=struct.pack('>4sBBH16sIIQQIIHHI',b'TCAM',1,1,1,bytes([7])*16,1,1,1,123,16666667,6,0,2,0)
assert actual==expected
for name in ('control-v1','stream-config-v1'):json.loads(pathlib.Path(f'protocol/{name}.schema.json').read_text())
print('Shared 64-byte media fixture and schemas verified.')
PY
