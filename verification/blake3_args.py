#!/usr/bin/env python3
"""Build the pinned real BLAKE3 dependency and supply its artifact to Verus."""
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent

def arguments():
    result = subprocess.run(
        ['cargo', '+1.99.1', 'build', '--manifest-path', str(ROOT / 'deps/Cargo.toml'),
         '--locked', '--message-format=json'], capture_output=True, text=True, check=True)
    artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
    files = [Path(name) for item in artifacts if item.get('reason') == 'compiler-artifact'
             and item['target']['name'] == 'blake3' for name in item['filenames'] if name.endswith('.rlib')]
    if len(files) != 1:
        raise RuntimeError('Expected exactly one pinned BLAKE3 library artifact')
    return ['--extern', 'blake3=' + str(files[0]), '-L', 'dependency=' + str(files[0].parent)]

if __name__ == '__main__':
    print('\n'.join(arguments()))
