#!/usr/bin/env python3
"""模拟浏览器 session：先访问拿全量 cookie，再操作"""
import json, subprocess, re
from urllib.parse import urlencode

HOME = "/data/data/com.termux/files/home"
MUSIC_U = open(f"{HOME}/.config/ncm/cookie").read().strip()  # MUSIC_U=...
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"


JAR = f"{HOME}/ncm_cookies.txt"


def curl(args, jar=JAR):
    cmd = ["curl", "-s", "--max-time", "20", "-b", jar, "-c", jar,
           "-H", f"User-Agent: {UA}",
           "-H", "Accept: */*",
           "-H", "Accept-Language: zh-CN,zh;q=0.9,en;q=0.8",
           *args]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=40)
    return r.stdout


# 1. 初始带 MUSIC_U 访问首页，落盘全部 cookie
curl(["-H", f"Cookie: {MUSIC_U}", "https://music.163.com/"])
# 2. 再访问一次确保 csrf 刷新
curl(["https://music.163.com/"])

# 读 jar 拿 csrf
csrf = ""
with open(JAR) as f:
    for line in f:
        if "__csrf" in line:
            csrf = line.strip().split("\t")[-1]
print("csrf from jar:", csrf)

# 3. 加红心（老接口带完整 cookie jar）
form = urlencode({
    "op": "add",
    "pid": "17618659828",
    "trackIds": "21563184",
    "imme": "true",
    "csrf_token": csrf,
})
out = curl([
    "https://music.163.com/api/playlist/manipulate/tracks",
    "-X", "POST",
    "-H", f"Referer: https://music.163.com/",
    "-H", "Origin: https://music.163.com",
    "-H", "Content-Type: application/x-www-form-urlencoded",
    "--data", form,
])
print("manipulate:", out[:300])

# 4. 备选：radio/like
form2 = urlencode({
    "alg": "itembased",
    "trackId": "21563184",
    "time": "99",
    "listenNum": "1",
    "csrf_token": csrf,
})
out2 = curl([
    "https://music.163.com/api/radio/like",
    "-X", "POST",
    "-H", f"Referer: https://music.163.com/",
    "-H", "Origin: https://music.163.com",
    "--data", form2,
])
print("radio/like:", out2[:300])
