#!/usr/bin/env python3
"""诊断：eapi 连接层 + 明文接口 /api/song/detail 读测试对照"""
import hashlib, json, subprocess, uuid, time
from Crypto.Cipher import AES

HOME = "/data/data/com.termux/files/home"
MUSIC_U = open(f"{HOME}/.config/ncm/cookie").read().strip()
EAPI_KEY = b"e82ckenh8dichen8"
DEVICE_ID = uuid.uuid4().hex[:32]
BUILDVER = str(int(time.time()))
CSRF = "c2bb9c8a27bec5af17a78c8216a38dd9"
COOKIE = (
    f"{MUSIC_U}; os=android; appver=9.0.0.5; versioncode=9000005; "
    f"mobilename=TestPhone; buildver={BUILDVER}; deviceId={DEVICE_ID}; "
    f"resolution=1080x2340; __csrf={CSRF}"
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


# 1. 对照组：明文读接口（已知能通）
print("=== plain GET playlist/detail ===")
r = subprocess.run(
    ["curl", "-s", "--max-time", "20", "-w", "\nHTTP=%{http_code}",
     f"https://music.163.com/api/v6/playlist/detail?id=17618659828&n=1",
     "-H", f"Cookie: {MUSIC_U}", "-H", "User-Agent: Mozilla/5.0"],
    capture_output=True, text=True, timeout=30,
)
print(r.stdout[:200])

# 2. eapi 诊断：显示 HTTP 状态码 + stderr 关键行
print("=== eapi interface host diag ===")
body = eapi_encrypt("/api/v6/playlist/detail", {"id": "17618659828", "n": "1", "s": "0"})
r = subprocess.run(
    ["curl", "-sv", "--max-time", "20", "-w", "\nHTTP=%{http_code} SIZE=%{size_download}",
     "https://interface.music.163.com/eapi/v6/playlist/detail", "-X", "POST",
     "-H", f"Cookie: {COOKIE}",
     "-H", "User-Agent: NeteaseMusic/9.0.0.5 android12 okhttp4.9.0",
     "-H", "Content-Type: application/octet-stream",
     "--data-binary", body],
    capture_output=True, text=True, timeout=30,
)
print("STDOUT:", r.stdout[:300])
for line in r.stderr.splitlines():
    if any(k in line for k in ("< HTTP", "< content", "< x-", "error", "Could not", "refused", "timed", "SSL")):
        print("STDERR:", line)

# 3. 用 music.163.com/eapi 前缀（同域）
print("=== eapi on music.163.com ===")
r = subprocess.run(
    ["curl", "-s", "--max-time", "20", "-w", "\nHTTP=%{http_code}",
     "https://music.163.com/eapi/v6/playlist/detail", "-X", "POST",
     "-H", f"Cookie: {COOKIE}",
     "-H", "User-Agent: NeteaseMusic/9.0.0.5 android12 okhttp4.9.0",
     "-H", "Content-Type: application/octet-stream",
     "--data-binary", body],
    capture_output=True, text=True, timeout=30,
)
print(r.stdout[:300])
