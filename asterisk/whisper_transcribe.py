#!/usr/bin/env python3
"""
Transcribe a call recording using local OpenAI Whisper (tiny model).
Called by Asterisk extensions.conf after a call ends:
  TrySystem(/usr/local/bin/whisper_transcribe.py '/logs/recordings/file.wav' &)
Output is appended to /logs/otp_voice_to_text.txt
"""
import sys
import os
import time
from datetime import datetime

if len(sys.argv) < 2:
    print("Usage: whisper_transcribe.py <audio_file>", file=sys.stderr)
    sys.exit(1)

audio_file = sys.argv[1]

# Wait for file to be fully written (MixMonitor flushes on hangup)
for _ in range(20):
    if os.path.exists(audio_file) and os.path.getsize(audio_file) > 1024:
        break
    time.sleep(0.5)
else:
    print(f"File not ready or empty: {audio_file}", file=sys.stderr)
    sys.exit(1)

import glob as _glob
from faster_whisper import WhisperModel  # import after file check to avoid delay on failure

# Use local snapshot path to bypass HuggingFace Hub network check (hangs without internet)
_model_dirs = _glob.glob('/root/.cache/huggingface/hub/models--Systran--faster-whisper-tiny/snapshots/*')
_model_path = _model_dirs[0] if _model_dirs else 'tiny'

# float32 required: int8 quantization hangs on CPU ISA detection in this environment
model = WhisperModel(_model_path, device="cpu", compute_type="float32")
segments, _ = model.transcribe(audio_file, language="en")
text = " ".join(seg.text.strip() for seg in segments)

# Extract caller ID from filename: YYYYMMDD_HHMMSS_CALLERID.wav
basename = os.path.basename(audio_file)
caller = basename.rsplit("_", 1)[-1].replace(".wav", "") if "_" in basename else "unknown"

output_file = "/logs/otp_voice_to_text.txt"
timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
entry = (
    f"[{timestamp}] From: {caller}\n"
    f"Recording: {basename}\n"
    f"Text: {text}\n"
    f"{'=' * 50}\n\n"
)

with open(output_file, "a") as f:
    f.write(entry)

print(entry, end="")
