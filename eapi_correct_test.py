#!/usr/bin/env python3
"""eapi 正确格式：内层 url=/api/...，POST 目标=/eapi/...，hex 大写，需 os=pc cookie"""
import hashlib, json, random, subprocess, uuid, time
from Crypto.Cipher import AES

HOME = "/data/data/com.termux/files/home"
MUSIC_U = open(f"{HOME}/.config/ncm/cookie").read().strip()  # MUSIC_U=xxx
EAPI_KEY = b"e82ckenh8dichen8"
UA = "NeteaseMusic/9.0.0.5 g07040 android12 okhttp4.9.0"

# 模拟安卓 App cookie（eapi 常需要）
DEVICE_ID = uuid.uuid4().hex[:32]
BUILDVER = str(int(time.time()))
COOKIE = (
    f"{MUSIC_U}; os=android; appver=9.0.0.5; versioncode=9000005; "
    f"mobilename=TestPhone; buildver={BUILDVER}; deviceId={DEVICE_ID}; "
    f"resolution=1080x2340; __csrf=c2bb9c8a27bec5af17a78c8216a38dd9"
)


def aes_ecb(data: str) -> str:
    pad = 16 - len(data) % 16
    data += chr(pad) * pad
    return AES.new(EAPI_KEY, AES.MODE_ECB).encrypt(data.encode()).hex().upper()


def eapi_encrypt(url: str, obj: dict) -> str:
    js = json.dumps(obj, separators=(",", ":"))
    s1 = f"nobody{url}use{js}md5forencrypt"
    md5 = hashlib.md5(s1.encode()).hexdigest()
    s2 = f"nobody{url}use{js}md5forencrypt{md5}"
    enc = aes_ecb(s2)
    return f"{url}-36cf-{enc}-36cf-{md5}-36cf-ecommit"


def post_eapi(api_path: str, obj: dict) -> str:
    # api_path 如 /api/playlist/manipulate/tracks
    body = eapi_encrypt(api_path, obj)
    eapi_url = api_path.replace("/api/", "/eapi/", 1)
    r = subprocess.run(
        ["curl", "-s", "--max-time", "20",
         f"https://interface.music.163.com{eapi_url}", "-X", "POST",
         "-H", f"Cookie: {COOKIE}",
         "-H", f"User-Agent: {UA}",
         "-H", "Content-Type: application/octet-stream",
         "-H", "Referer: https://music.163.com/",
         "--data-binary", body],
        capture_output=True, text=True, timeout=40,
    )
    return r.stdout[:500]


# 验证读接口（应能通）
print("=== eapi playlist/detail (read) ===")
print(post_eapi("/api/v6/playlist/detail", {
    "id": "17618659828", "n": "100", "s": "0",
}))

# 写接口 manipulate/tracks add
print("=== eapi manipulate/tracks add ===")
print(post_eapi("/api/playlist/manipulate/tracks", {
    "op": "add",
    "pid": "17618659828",
    "trackIds": json.dumps([27536479]),
    "imme": "true",
    "csrf_token": "c2bb9c8a27bec5af17a78c8216a38dd9",
}))

# radio/like
print("=== eapi radio/like ===")
print(post_eapi("/api/radio/like", {
    "alg": "itembased",
    "trackId": "27536479",
    "time": "99",
    "listenNum": "1",
    "csrf_token": "c2bb9c8a27bec5af17a78c8216a38dd9",
}))
