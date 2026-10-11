#!/bin/bash
# 排查 weapi 端点连通性与响应
set -u
COOKIE=$(cat ~/.config/ncm/cookie)
UA="Mozilla/5.0"
echo "=== A: plain POST api/playlist/manipulate/tracks ==="
curl -s --max-time 15 "https://music.163.com/api/playlist/manipulate/tracks" -X POST \
  -H "Cookie: $COOKIE; __csrf=c2bb9c8a27bec5af17a78c8216a38dd9" \
  -H "User-Agent: $UA" -H "Referer: https://music.163.com/" \
  --data "op=add&pid=17618659828&trackIds=21563184&imme=true&csrf_token=c2bb9c8a27bec5af17a78c8216a38dd9"
echo
echo "=== B: weapi dummy POST ==="
curl -s --max-time 15 -w "HTTP=%{http_code} SIZE=%{size_download}\n" \
  "https://music.163.com/weapi/playlist/manipulate/tracks" -X POST \
  -H "User-Agent: $UA" --data "params=x&encSecKey=y" -o /tmp/weapi_b.txt
head -c 200 /tmp/weapi_b.txt 2>/dev/null; echo
echo "=== C: interface host api ==="
curl -s --max-time 15 "https://interface.music.163.com/api/playlist/manipulate/tracks" -X POST \
  -H "Cookie: $COOKIE; __csrf=c2bb9c8a27bec5af17a78c8216a38dd9" \
  -H "User-Agent: $UA" -H "Referer: https://music.163.com/" \
  --data "op=add&pid=17618659828&trackIds=21563184&imme=true&csrf_token=c2bb9c8a27bec5af17a78c8216a38dd9"
echo
