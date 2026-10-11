#!/bin/bash
# 升级 mp3→lossless flac：按文件名搜索下载，出现新 flac 才删旧 mp3
set -u
cd /sdcard/Music/NCMusic || exit 1
count=0
for f in *.mp3; do
  [ -f "$f" ] || continue
  name="${f%.mp3}"
  before=$(ls *.flac 2>/dev/null | sort)
  echo "=== $name ==="
  ncm download "$name" -n 1 -d 1 --quality lossless -q 2>&1 | tail -2
  after=$(ls *.flac 2>/dev/null | sort)
  new=$(comm -13 <(echo "$before") <(echo "$after"))
  if [ -n "$new" ]; then
    echo "NEW_FLAC: $new"
    rm -f "$f" && echo "DELETED: $f"
    count=$((count+1))
  else
    echo "NO_NEW_FLAC keep: $f"
  fi
done
echo "=== DONE upgraded=$count ==="
