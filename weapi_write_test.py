#!/usr/bin/env python3
"""weapi 加密 POST manipulate/tracks + radio/like"""
import base64, json, random, subprocess
from urllib.parse import urlencode
from Crypto.Cipher import AES

HOME = "/data/data/com.termux/files/home"
COOKIE = open(f"{HOME}/.config/ncm/cookie").read().strip()
CSRF = "c2bb9c8a27bec5af17a78c8216a38dd9"
MODULUS = "e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7"
EXPONENT = "010001"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"


def rsa_encrypt(sec_key: bytes) -> str:
    n, e = int(MODULUS, 16), int(EXPONENT, 16)
    m = int(sec_key[::-1].decode(), 16)
    return format(pow(m, e, n), "x").zfill(256)


def aes_cbc(data: bytes, key: bytes) -> bytes:
    iv = b"0CoJUm6Qyw8W8jud"
    pad = 16 - len(data) % 16
    data += bytes([pad]) * pad
    return AES.new(key, AES.MODE_CBC, iv).encrypt(data)


def weapi(params: dict) -> dict:
    text = json.dumps(params, separators=(",", ":"))
    iv = b"0CoJUm6Qyw8W8jud"
    sec_key = "".join(random.choices("0123456789abcdef", k=16)).encode()
    first = aes_cbc(text.encode(), iv)
    second = aes_cbc(first, sec_key)
    return {
        "params": base64.b64encode(second).decode(),
        "encSecKey": rsa_encrypt(sec_key),
    }


def post_weapi(path: str, params: dict) -> str:
    body = weapi(params)
    r = subprocess.run(
        ["curl", "-s", "--max-time", "20",
         f"https://music.163.com{path}", "-X", "POST",
         "-H", f"Cookie: {COOKIE}; __csrf={CSRF}",
         "-H", f"User-Agent: {UA}",
         "-H", "Referer: https://music.163.com/",
         "-H", "Origin: https://music.163.com",
         "-H", "Accept: */*",
         "--data-urlencode", f"params={body['params']}",
         "--data-urlencode", f"encSecKey={body['encSecKey']}"],
        capture_output=True, text=True, timeout=40,
    )
    return r.stdout[:400]


# 1. weapi manipulate/tracks add 单曲测试
print("=== weapi/manipulate/tracks ===")
print(post_weapi("/weapi/playlist/manipulate/tracks", {
    "op": "add",
    "pid": "17618659828",
    "trackIds": json.dumps([27536479]),
    "imme": "true",
    "csrf_token": CSRF,
}))

# 2. weapi radio/like
print("=== weapi/radio/like ===")
print(post_weapi("/weapi/radio/like", {
    "alg": "itembased",
    "trackId": "27536479",
    "time": "99",
    "listenNum": "1",
    "csrf_token": CSRF,
}))

# 3. weapi playlist/tracks（旧路径也试）
print("=== weapi/playlist/tracks ===")
print(post_weapi("/weapi/playlist/tracks", {
    "op": "add",
    "pid": "17618659828",
    "trackIds": json.dumps([27536479]),
    "imme": "true",
    "csrf_token": CSRF,
}))
