#!/usr/bin/env python3
"""Generate fixtures/test-click-12_4s.wav — 12.4s mono WAV with a click at 2.5s."""

from __future__ import annotations

import math
import struct
import wave
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "fixtures" / "test-click-12_4s.wav"


def main() -> None:
    OUT.parent.mkdir(parents=True, exist_ok=True)
    sr = 48000
    dur = 12.4
    click_at = 2.5
    n = int(sr * dur)
    click_samp = int(click_at * sr)
    with wave.open(str(OUT), "w") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        frames = bytearray()
        for i in range(n):
            t = i / sr
            sample = 0.02 * math.sin(2 * math.pi * 220 * t)
            if abs(i - click_samp) < int(0.01 * sr):
                env = 1.0 - abs(i - click_samp) / (0.01 * sr)
                sample = 0.95 * math.sin(2 * math.pi * 2000 * t) * env
            frames += struct.pack("<h", max(-32767, min(32767, int(sample * 32767))))
        w.writeframes(frames)
    print(f"wrote {OUT} duration={dur}s click@{click_at}s samples={n}")


if __name__ == "__main__":
    main()
EOF