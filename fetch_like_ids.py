#!/usr/bin/env python3
# 查询红心歌单 trackIds 并写入文件
import json, subprocess, pathlib

home = pathlib.Path("/data/data/com.termux/files/home")
cookie = (home / ".config/ncm/cookie").read_text().split(";")[0]
uid = "17265863458"
pid = "17618659828"

# 用 curl 拉歌单详情（trackIds 全量）
r = subprocess.run(
    ["curl", "-s", f"https://music.163.com/api/v6/playlist/detail?id={pid}&n=100000",
     "-H", f"Cookie: {cookie}",
     "-H", "User-Agent: Mozilla/5.0"],
    capture_output=True, text=True, timeout=60,
)
d = json.loads(r.stdout)
pl = d["playlist"]
ids = [x["id"] for x in pl.get("trackIds", [])]
(home / "like_ids.txt").write_text(" ".join(map(str, ids)))
print(f"track_count={pl.get('trackCount')} got={len(ids)}")
