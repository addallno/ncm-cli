#!/usr/bin/env python3
"""明文 manipulate/tracks：trackIds 用 JSON 数组字符串格式"""
import json, subprocess
from urllib.parse import urlencode

HOME = "/data/data/com.termux/files/home"
COOKIE_RAW = open(f"{HOME}/.config/ncm/cookie").read().strip()
JAR = f"{HOME}/ncm_cookies2.txt"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"


def curl(args):
    cmd = ["curl", "-s", "--max-time", "20", "-b", JAR, "-c", JAR,
           "-H", f"User-Agent: {UA}", *args]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=40)
    return r.stdout


# 1. 首页落 cookie 拿新鲜 csrf
curl(["-H", f"Cookie: {COOKIE_RAW}", "https://music.163.com/"])
csrf = ""
with open(JAR) as f:
    for line in f:
        if "__csrf" in line:
            csrf = line.strip().split("\t")[-1]
print("csrf:", csrf)

# 2. trackIds 作为 JSON 数组字符串
form = urlencode({
    "op": "add",
    "pid": "17618659828",
    "trackIds": json.dumps([27536479]),
    "imme": "true",
    "csrf_token": csrf,
})
print("form:", form)
out = curl([
    "https://music.163.com/api/playlist/manipulate/tracks",
    "-X", "POST",
    "-H", "Referer: https://music.163.com/",
    "-H", "Origin: https://music.163.com",
    "-H", "Content-Type: application/x-www-form-urlencoded",
    "--data", form,
])
print("result:", out[:400])

# 3. 若仍301，试加 timestamp/nonce 空参数变体
if "301" in out or not out:
    form2 = urlencode({
        "op": "add",
        "pid": "17618659828",
        "trackIds": json.dumps([27536479]),
        "imme": "true",
        "csrf_token": csrf,
        "timestamp": "1",
    })
    out2 = curl([
        "https://music.163.com/api/playlist/manipulate/tracks",
        "-X", "POST",
        "-H", "Referer: https://music.163.com/",
        "-H", "Origin: https://music.163.com",
        "-H", "Content-Type: application/x-www-form-urlencoded",
        "--data", form2,
    ])
    print("retry+timestamp:", out2[:400])
