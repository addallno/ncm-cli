#!/bin/bash
# 多 endpoint 探测：找一个能用的加红心接口
set -u
COOKIE=$(cat ~/.config/ncm/cookie)
CSRF="c2bb9c8a27bec5af17a78c8216a38dd9"
UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36"
REF="https://music.163.com/"
HDR=(-H "Cookie: $COOKIE; __csrf=$CSRF" -H "User-Agent: $UA" -H "Referer: $REF" -H "Origin: https://music.163.com")

echo "=== 1. manipulate/tracks csrf in query ==="
curl -s --max-time 15 "https://music.163.com/api/playlist/manipulate/tracks?csrf_token=$CSRF" -X POST "${HDR[@]}" \
  -d "op=add&pid=17618659828&trackIds=21563184&imme=true&csrf_token=$CSRF"
echo
echo "=== 2. radio/like ==="
curl -s --max-time 15 "https://music.163.com/api/radio/like" -X POST "${HDR[@]}" \
  -d "alg=itembased&trackId=21563184&time=333&listenNum=1&csrf_token=$CSRF"
echo
echo "=== 3. like?like=true ==="
curl -s --max-time 15 "https://music.163.com/api/like" -X POST "${HDR[@]}" \
  -d "like=true&id=21563184&csrf_token=$CSRF"
echo
echo "=== 4. v1 playlist tracks ==="
curl -s --max-time 15 "https://music.163.com/api/v1/playlist/tracks" -X POST "${HDR[@]}" \
  -d "op=add&pid=17618659828&trackIds=21563184&csrf_token=$CSRF"
echo
echo "=== 5. old /api/playlist/tracks w/ csrf ==="
curl -s --max-time 15 "https://music.163.com/api/playlist/tracks" -X POST "${HDR[@]}" \
  -d "op=add&pid=17618659828&trackIds=21563184&csrf_token=$CSRF"
echo
echo "=== 6. ensure login: /api/nuser/account/get ==="
curl -s --max-time 15 "https://music.163.com/api/nuser/account/get" -X POST "${HDR[@]}" -d ""
echo
