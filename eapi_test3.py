#!/usr/bin/env python3
"""正确格式的 eapi：url + md5 指纹 + ecb 加密"""
import hashlib, json, binascii, subprocess
from Crypto.Cipher import AES

HOME = "/data/data/com.termux/files/home"
COOKIE = open(f"{HOME}/.config/ncm/cookie").read().strip()
CSRF = "c2bb9c8a27bec5af17a78c8216a38dd9"
KEY = b"e82ckenh8dichen8"


def md5(s: str) -> str:
    return hashlib.md5(s.encode()).hexdigest()


def aes_ecb(plaintext: str) -> str:
    pad = 16 - len(plaintext) % 16
    data = plaintext.encode() + bytes([pad]) * pad
    return binascii.hexlify(AES.new(KEY, AES.MODE_ECB).encrypt(data)).decode()


def eapi_encrypt(path: str, params: dict) -> str:
    # JS JSON.stringify 风格：无空格紧凑分隔符
    json_data = json.dumps({"url": path, **params}, separators=(",", ":"), ensure_ascii=False)
    text = f"nobody{path}use{json_data}md5forencrypt"
    digest = md5(text)
    data_req = f"{path}-36cf-{json_data}-36cf-{digest}-36cf-ecommit"
    return aes_ecb(data_req)


path = "/api/playlist/manipulate/tracks"
params = {
    "op": "add",
    "pid": "17618659828",
    "trackIds": "[21563184]",
    "imme": "true",
    "csrf_token": CSRF,
}
enc = eapi_encrypt(path, params)
url = f"https://interface.music.163.com{path}"
r = subprocess.run(
    ["curl", "-s", "--max-time", "15", url, "-X", "POST",
     "-H", f"Cookie: {COOKIE}; __csrf={CSRF}",
     "-H", "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
     "-H", "Referer: https://music.163.com/",
     "-H", "Content-Type: application/octet-stream",
     "--data", enc],
    capture_output=True, text=True, timeout=30,
)
print("resp:", r.stdout[:400])
