#!/usr/bin/env python3
"""eapi 加密测试：manipulate/tracks 加歌到红心歌单"""
import json, subprocess, binascii
from Crypto.Cipher import AES

HOME = "/data/data/com.termux/files/home"
COOKIE = open(f"{HOME}/.config/ncm/cookie").read().strip()
CSRF = "c2bb9c8a27bec5af17a78c8216a38dd9"
KEY = b"e82ckenh8dichen8"


def eapi_encrypt(plaintext: str) -> str:
    pad = 16 - len(plaintext) % 16
    data = plaintext.encode() + bytes([pad]) * pad
    return binascii.hexlify(AES.new(KEY, AES.MODE_ECB).encrypt(data)).decode()


params = json.dumps({
    "op": "add",
    "pid": "17618659828",
    "trackIds": "[21563184]",
    "imme": "true",
})
body = f"params={params}&csrf_token={CSRF}"
enc = eapi_encrypt(body)

for host in ("interface.music.163.com", "interface3.music.163.com"):
    url = f"https://{host}/api/playlist/manipulate/tracks"
    print(f"=== {url} ===")
    r = subprocess.run(
        ["curl", "-s", "--max-time", "15", url, "-X", "POST",
         "-H", f"Cookie: {COOKIE}; __csrf={CSRF}",
         "-H", "User-Agent: Mozilla/5.0",
         "-H", "Referer: https://music.163.com/",
         "-H", "Content-Type: application/octet-stream",
         "--data", enc],
        capture_output=True, text=True, timeout=30,
    )
    print("resp:", r.stdout[:300], "| err:", r.stderr[:100])
