#!/usr/bin/env python3
"""eapi 参数格式变体测试"""
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


variants = [
    # 1. json 内 trackIds 为逗号字符串
    'params={"op":"add","pid":"17618659828","trackIds":"21563184","imme":"true"}&csrf_token=' + CSRF,
    # 2. 无 imme
    'params={"op":"add","pid":"17618659828","trackIds":"[21563184]"}&csrf_token=' + CSRF,
    # 3. eapi 风格字段小写 + csrf
    'params={"op":"add","pid":17618659828,"trackIds":[21563184],"imme":true,"csrf_token":"' + CSRF + '"}',
    # 4. cloudsearch style
    'params=' + json.dumps({"op": "add", "pid": "17618659828", "trackIds": "[21563184]", "imme": "true", "csrf_token": CSRF}) + '&csrf_token=' + CSRF,
]

url = "https://interface.music.163.com/api/playlist/manipulate/tracks"
for i, plain in enumerate(variants, 1):
    enc = eapi_encrypt(plain)
    r = subprocess.run(
        ["curl", "-s", "--max-time", "15", url, "-X", "POST",
         "-H", f"Cookie: {COOKIE}; __csrf={CSRF}",
         "-H", "User-Agent: Mozilla/5.0",
         "-H", "Referer: https://music.163.com/",
         "-H", "Content-Type: application/octet-stream",
         "--data", enc],
        capture_output=True, text=True, timeout=30,
    )
    print(f"[{i}] {r.stdout[:180]}")
