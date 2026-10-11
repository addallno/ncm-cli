#!/usr/bin/env python3
"""weapi 完整 headers 重测"""
import base64, json, random, subprocess
from Crypto.Cipher import AES

HOME = "/data/data/com.termux/files/home"
COOKIE = open(f"{HOME}/.config/ncm/cookie").read().strip()
CSRF = "c2bb9c8a27bec5af17a78c8216a38dd9"
MODULUS = "e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7"
EXPONENT = "010001"


def rsa_encrypt(sec_key: bytes) -> str:
    n, e = int(MODULUS, 16), int(EXPONENT, 16)
    # NCM 约定：secKey hex 字符串反转后按 hex 解析为大整数
    m = int(sec_key[::-1].decode(), 16)
    return format(pow(m, e, n), "x").zfill(256)


def aes_cbc(data: bytes, key: bytes) -> bytes:
    iv = b"0CoJUm6Qyw8W8jud"
    pad = 16 - len(data) % 16
    data += bytes([pad]) * pad
    return AES.new(key, AES.MODE_CBC, iv).encrypt(data)


def weapi(params: dict) -> dict:
    text = json.dumps(params)
    iv = b"0CoJUm6Qyw8W8jud"
    sec_key = "".join(random.choices("0123456789abcdef", k=16)).encode()
    first = aes_cbc(text.encode(), iv)
    second = aes_cbc(first, sec_key)
    return {
        "params": base64.b64encode(second).decode(),
        "encSecKey": rsa_encrypt(sec_key),
    }


# 测1：读接口 weapi/v6/playlist/detail
body = weapi({"id": "17618659828", "n": "100000", "s": "8"})
r = subprocess.run(
    ["curl", "-sv", "--max-time", "20",
     "https://music.163.com/weapi/v6/playlist/detail", "-X", "POST",
     "-H", f"Cookie: {COOKIE}; __csrf={CSRF}",
     "-H", "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
     "-H", "Referer: https://music.163.com/",
     "-H", "Origin: https://music.163.com",
     "-H", "Accept: */*",
     "-H", "Accept-Language: zh-CN,zh;q=0.9",
     "--data-urlencode", f"params={body['params']}",
     "--data-urlencode", f"encSecKey={body['encSecKey']}"],
    capture_output=True, text=True, timeout=40,
)
print("STDOUT:", r.stdout[:400])
# stderr 里挑关键行
for line in r.stderr.splitlines():
    if any(k in line for k in ("< HTTP", "< content", "< x-", "error", "left intact", "closed")):
        print("STDERR:", line)
