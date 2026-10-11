#!/usr/bin/env python3
"""网易云 weapi 加密：测试 manipulate/tracks 是否可通过"""
import base64, json, os, random, subprocess, sys, pathlib

try:
    from Crypto.Cipher import AES
except ImportError:
    subprocess.run([sys.executable, "-m", "pip", "install", "--quiet", "pycryptodome"], check=True)
    from Crypto.Cipher import AES

# NCM weapi 固定公钥 (n/e hex)
MODULUS = "e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7"
EXPONENT = "010001"


def rsa_encrypt(sec_key: bytes) -> str:
    """NCM 自定义 RSA：数字反序后按小端拼成大整数再幂模"""
    n = int(MODULUS, 16)
    e = int(EXPONENT, 16)
    num = int.from_bytes(sec_key[::-1], "big")
    enc = pow(num, e, n)
    return format(enc, "x").zfill(256)


def aes_cbc(data: bytes, key: bytes, iv: bytes) -> bytes:
    pad = 16 - len(data) % 16
    data += bytes([pad]) * pad
    return AES.new(key, AES.MODE_CBC, iv).encrypt(data)


def weapi(params: dict) -> dict:
    text = json.dumps(params)
    iv = b"0CoJUm6Qyw8W8jud"
    sec_key = "".join(random.choices("0123456789abcdef", k=16)).encode()
    first = aes_cbc(text.encode(), iv, iv)
    second = aes_cbc(first, sec_key, iv)
    return {
        "params": base64.b64encode(second).decode(),
        "encSecKey": rsa_encrypt(sec_key),
    }


def main():
    home = pathlib.Path("/data/data/com.termux/files/home")
    cookie = (home / ".config/ncm/cookie").read_text().strip()
    csrf = "c2bb9c8a27bec5af17a78c8216a38dd9"
    print("csrf:", csrf)

    # 先测能读：playlist/detail 通过 weapi
    body = weapi({"id": "17618659828", "n": 100000, "s": 8})
    r2 = subprocess.run(
        ["curl", "-s", "https://music.163.com/weapi/v6/playlist/detail",
         "-X", "POST",
         "-H", f"Cookie: {cookie}; __csrf={csrf}",
         "-H", "User-Agent: Mozilla/5.0",
         "-H", "Referer: https://music.163.com/",
         "--data-urlencode", f"params={body['params']}",
         "--data-urlencode", f"encSecKey={body['encSecKey']}"],
        capture_output=True, text=True, timeout=30,
    )
    try:
        d = json.loads(r2.stdout)
        pl = d.get("playlist", {})
        print("detail code:", d.get("code"), "trackIds:", len(pl.get("trackIds", [])))
    except Exception:
        print("detail raw:", r2.stdout[:200], "err:", r2.stderr[:200])

    # 测 add 单曲
    body2 = weapi({
        "op": "add",
        "pid": "17618659828",
        "trackIds": json.dumps([21563184]),
        "imme": "true",
    })
    r3 = subprocess.run(
        ["curl", "-s", "https://music.163.com/weapi/playlist/manipulate/tracks",
         "-X", "POST",
         "-H", f"Cookie: {cookie}; __csrf={csrf}",
         "-H", "User-Agent: Mozilla/5.0",
         "-H", "Referer: https://music.163.com/",
         "--data-urlencode", f"params={body2['params']}",
         "--data-urlencode", f"encSecKey={body2['encSecKey']}"],
        capture_output=True, text=True, timeout=30,
    )
    print("add resp:", r3.stdout[:300], "err:", r3.stderr[:200])


if __name__ == "__main__":
    main()
