#!/usr/bin/env python3
# 每日推荐前 40 首，去掉红心歌单已有的，ncm pl add 批量加入
import json, subprocess, pathlib

home = pathlib.Path("/data/data/com.termux/files/home")
ncm = str(home / "ncm")
pid = "17618659828"
liked = set(map(int, (home / "like_ids.txt").read_text().split()))

# 每日推荐前 40
r = subprocess.run(
    [ncm, "daily", "--json", "-n", "40", "--no-color"],
    capture_output=True, text=True, timeout=120,
)
songs = json.loads(r.stdout)
daily_ids = [s["id"] for s in songs]
missing = [i for i in daily_ids if i not in liked]
print(f"daily={len(daily_ids)} already_liked={len(daily_ids)-len(missing)} to_add={len(missing)}")

# 分批 add（每批 50，这里 40 一批就够）
if missing:
    r2 = subprocess.run(
        [ncm, "pl", "add", *map(str, missing), "-p", pid, "--no-color"],
        capture_output=True, text=True, timeout=120,
    )
    print(r2.stdout[-500:])
    print(r2.stderr[-300:], flush=True)
    print("rc=", r2.returncode)
    # 列出加了哪些
    for s in songs:
        if s["id"] in missing:
            arts = "/".join(s.get("artists", []))
            print(f"+ {arts} - {s['name']} ({s['id']})")
