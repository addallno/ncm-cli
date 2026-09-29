//! 自定义 DNS 解析：系统解析优先，失败时回退手写 UDP A 查询
//!
//! 背景：musl 静态二进制在 Termux/Android 下无 `/etc/resolv.conf`，
//! 系统 `getaddrinfo` 报 `Try again`。回退路径的 nameserver 来源（优先级从高到低）：
//! `NCM_DNS` 环境变量（`ip`、`ip:port`，逗号分隔）→ Termux `$PREFIX/etc/resolv.conf`
//! → `/etc/resolv.conf` → 内置公共 DNS。
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

const CACHE_TTL: Duration = Duration::from_secs(60);
const SERVER_TIMEOUT: Duration = Duration::from_millis(1500);
const DEFAULT_PORT: u16 = 53;
const BUILTIN_SERVERS: [(&str, u16); 3] =
    [("223.5.5.5", 53), ("114.114.114.114", 53), ("1.1.1.1", 53)];
const MAX_CNAME_HOPS: usize = 5;

static QUERY_ID: AtomicU16 = AtomicU16::new(0x1234);

type CacheEntry = (Instant, Vec<Ipv4Addr>);
type DnsCache = Arc<Mutex<HashMap<String, CacheEntry>>>;

/// 挂到 reqwest Client 的 resolver：系统成功即短路，失败走 UDP 回退（带 60s 缓存）
pub struct FallbackDns {
    cache: DnsCache,
}

impl Default for FallbackDns {
    fn default() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Resolve for FallbackDns {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().trim_end_matches('.').to_string();
        let cache = self.cache.clone();
        Box::pin(async move {
            // 1) 系统解析（绝大多数平台在此成功，零额外开销）
            if let Ok(addrs) = tokio::net::lookup_host((host.as_str(), 0u16)).await {
                let v: Vec<SocketAddr> = addrs.collect();
                if !v.is_empty() {
                    return Ok(Box::new(v.into_iter()) as Addrs);
                }
            }
            // 2) 缓存
            if let Ok(guard) = cache.lock() {
                if let Some((at, ips)) = guard.get(&host) {
                    if at.elapsed() < CACHE_TTL && !ips.is_empty() {
                        return Ok(Box::new(ips_to_addrs(ips)) as Addrs);
                    }
                }
            }
            // 3) UDP 回退
            let ips = query_host(&host)
                .await
                .map_err(|e| std::io::Error::other(format!("DNS fallback 失败 ({host}): {e}")))?;
            if let Ok(mut guard) = cache.lock() {
                guard.insert(host, (Instant::now(), ips.clone()));
            }
            Ok(Box::new(ips_to_addrs(&ips)) as Addrs)
        })
    }
}

fn ips_to_addrs(ips: &[Ipv4Addr]) -> Addrs {
    let v: Vec<SocketAddr> = ips
        .iter()
        .map(|ip| SocketAddr::new(IpAddr::V4(*ip), 0))
        .collect();
    Box::new(v.into_iter())
}

/// 依次询问可用 nameserver，全部失败返回聚合错误
async fn query_host(host: &str) -> Result<Vec<Ipv4Addr>, String> {
    let servers = dns_servers();
    if servers.is_empty() {
        return Err("没有可用的 nameserver".into());
    }
    let mut errs: Vec<String> = Vec::new();
    for server in servers {
        match query_server(server, host).await {
            Ok(v) if !v.is_empty() => return Ok(v),
            Ok(_) => errs.push(format!("{server}: 应答为空")),
            Err(e) => errs.push(format!("{server}: {e}")),
        }
    }
    Err(errs.join("；"))
}

/// 对单个 nameserver 查询 A 记录，自动跟随 CNAME 链
async fn query_server(server: SocketAddr, host: &str) -> Result<Vec<Ipv4Addr>, String> {
    let sock = UdpSocket::bind(if server.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .await
    .map_err(|e| format!("bind: {e}"))?;
    sock.connect(server)
        .await
        .map_err(|e| format!("connect: {e}"))?;

    let mut current = host.to_string();
    let mut visited = vec![current.clone()];
    let mut all: Vec<Ipv4Addr> = Vec::new();
    for _ in 0..MAX_CNAME_HOPS {
        let id = next_id();
        let query = build_query(id, &current);
        sock.send(&query).await.map_err(|e| format!("send: {e}"))?;

        let mut buf = [0u8; 1500];
        let resp = tokio::time::timeout(SERVER_TIMEOUT, async {
            loop {
                let n = sock
                    .recv(&mut buf)
                    .await
                    .map_err(|e| format!("recv: {e}"))?;
                let buf = &buf[..n];
                if buf.len() >= 2 && u16::from_be_bytes([buf[0], buf[1]]) == id {
                    return parse_response(buf, id);
                }
                // 非本查询 ID 的包直接忽略继续等
            }
        })
        .await
        .map_err(|_| format!("超时 {SERVER_TIMEOUT:?}"))?
        .map_err(|e| e.to_string())?;

        all.extend(resp.addrs);
        if !all.is_empty() {
            break;
        }
        // 本轮无 A：若应答给了 CNAME 则追链（防环），否则结束
        match resp.cname {
            Some(target) if !visited.contains(&target) => {
                visited.push(target.clone());
                current = target;
            }
            _ => break,
        }
    }
    Ok(all)
}

/// 可用 nameserver 列表（去重，保持顺序）
fn dns_servers() -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();

    // 1) NCM_DNS 显式指定（给了就只用它）
    if let Ok(v) = std::env::var("NCM_DNS") {
        for p in v.split([',', ';', ' ']) {
            push_server(&mut out, p);
        }
        if !out.is_empty() {
            return out;
        }
    }
    // 2) Termux $PREFIX/etc/resolv.conf
    if let Ok(prefix) = std::env::var("PREFIX") {
        if let Ok(txt) = std::fs::read_to_string(format!("{prefix}/etc/resolv.conf")) {
            extract_nameservers(&txt, &mut out);
        }
    }
    // 3) 系统 /etc/resolv.conf（存在但系统解析仍失败的场景）
    if let Ok(txt) = std::fs::read_to_string("/etc/resolv.conf") {
        extract_nameservers(&txt, &mut out);
    }
    // 4) 内置兜底
    for (ip, port) in BUILTIN_SERVERS {
        push_server(&mut out, &format!("{ip}:{port}"));
    }
    out
}

/// 解析 `ip` 或 `ip:port` 并去重入列
fn push_server(out: &mut Vec<SocketAddr>, s: &str) {
    let s = s.trim();
    if s.is_empty() {
        return;
    }
    let addr = match s.rsplit_once(':') {
        Some((ip, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => ip
            .parse::<IpAddr>()
            .ok()
            .zip(port.parse::<u16>().ok())
            .map(|(ip, port)| SocketAddr::new(ip, port)),
        _ => s
            .parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, DEFAULT_PORT)),
    };
    if let Some(a) = addr {
        if !out.contains(&a) {
            out.push(a);
        }
    }
}

fn extract_nameservers(txt: &str, out: &mut Vec<SocketAddr>) {
    for line in txt.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("nameserver") {
            let v = rest.trim();
            if !v.is_empty() {
                push_server(out, v);
            }
        }
    }
}

fn next_id() -> u16 {
    let t = Instant::now().elapsed().as_nanos() as u16;
    QUERY_ID.fetch_add(1, Ordering::Relaxed) ^ t
}

/// 构造标准 A/IN 递归查询报文
fn build_query(id: u16, host: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(host.len() + 20);
    v.extend(id.to_be_bytes());
    v.extend([0x01, 0x00]); // RD
    v.extend([0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // QD=1 其余 0
    for label in host.trim_end_matches('.').split('.') {
        let b = label.as_bytes();
        v.push(b.len() as u8);
        v.extend(b);
    }
    v.push(0);
    v.extend([0x00, 0x01, 0x00, 0x01]); // QTYPE=A QCLASS=IN
    v
}

struct ParsedResponse {
    addrs: Vec<Ipv4Addr>,
    cname: Option<String>,
}

/// 解析应答：收集 A 记录与 CNAME 目标，支持压缩指针与多段 question
fn parse_response(buf: &[u8], expect_id: u16) -> Result<ParsedResponse, String> {
    if buf.len() < 12 {
        return Err("报文过短".into());
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    if id != expect_id {
        return Err(format!("ID 不匹配 {id}≠{expect_id}"));
    }
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    if flags & 0x8000 == 0 {
        return Err("不是应答报文".into());
    }
    let rcode = flags & 0x000F;
    if rcode != 0 {
        return Err(format!("rcode={rcode}"));
    }
    let qd = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let an = u16::from_be_bytes([buf[6], buf[7]]) as usize;
    let mut pos = 12;
    for _ in 0..qd {
        let (_, p) = read_name(buf, pos)?;
        pos = p + 4;
        if pos > buf.len() {
            return Err("question 段越界".into());
        }
    }
    let mut out = ParsedResponse {
        addrs: Vec::new(),
        cname: None,
    };
    for _ in 0..an {
        let (_, p) = read_name(buf, pos)?;
        pos = p;
        if pos + 10 > buf.len() {
            return Err("answer 段越界".into());
        }
        let rtype = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let rclass = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]);
        let rdlen = u16::from_be_bytes([buf[pos + 8], buf[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > buf.len() {
            return Err("rdata 越界".into());
        }
        if rclass == 1 {
            match rtype {
                1 if rdlen == 4 => {
                    out.addrs.push(Ipv4Addr::new(
                        buf[pos],
                        buf[pos + 1],
                        buf[pos + 2],
                        buf[pos + 3],
                    ));
                }
                5 => {
                    let (target, _) = read_name(buf, pos)?;
                    if out.cname.is_none() {
                        out.cname = Some(target);
                    }
                }
                _ => {}
            }
        }
        pos += rdlen;
    }
    Ok(out)
}

/// 读域名（支持 0xC0 压缩指针，防环），返回 (解码名, 消耗字节数)
fn read_name(buf: &[u8], start: usize) -> Result<(String, usize), String> {
    let mut labels: Vec<String> = Vec::new();
    let mut pos = start;
    let mut end: Option<usize> = None;
    let mut jumps = 0;
    loop {
        if pos >= buf.len() {
            return Err("name 越界".into());
        }
        let len = buf[pos];
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            if pos + 1 >= buf.len() {
                return Err("指针越界".into());
            }
            let off = (((len as usize) & 0x3F) << 8) | buf[pos + 1] as usize;
            if end.is_none() {
                end = Some(pos + 2);
            }
            pos = off;
            jumps += 1;
            if jumps > 20 {
                return Err("指针跳转过多".into());
            }
        } else {
            let l = len as usize;
            if pos + 1 + l > buf.len() {
                return Err("label 越界".into());
            }
            labels.push(String::from_utf8_lossy(&buf[pos + 1..pos + 1 + l]).into_owned());
            pos += 1 + l;
        }
    }
    Ok((labels.join("."), end.unwrap_or(pos)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    #[test]
    fn query_bytes_layout() {
        let q = build_query(0xABCD, "music.163.com");
        assert_eq!(&q[0..2], &[0xAB, 0xCD]);
        assert_eq!(&q[2..4], &[0x01, 0x00]); // RD
        assert_eq!(u16::from_be_bytes([q[4], q[5]]), 1); // QDCOUNT
                                                         // QNAME: 5 music 3 163 3 com 0（偏移 12 起）
        assert_eq!(&q[12..18], b"\x05music");
        assert_eq!(&q[18..22], b"\x03163");
        assert_eq!(&q[22..26], b"\x03com");
        assert_eq!(q[26], 0);
        assert_eq!(&q[27..31], &[0x00, 0x01, 0x00, 0x01]); // A IN
    }

    #[test]
    fn read_name_with_pointer() {
        // [header 12B] + 03www 07example 03com 00 | C0 0C（指回 offset 12）
        let mut buf = vec![0u8; 12];
        buf.extend_from_slice(b"\x03www\x07example\x03com\x00");
        let ptr_at = buf.len();
        buf.extend_from_slice(&[0xC0, 12]);
        let (n1, p1) = read_name(&buf, 12).unwrap();
        assert_eq!(n1, "www.example.com");
        assert_eq!(p1, ptr_at); // 消耗到指针前
        let (n2, p2) = read_name(&buf, ptr_at).unwrap();
        assert_eq!(n2, "www.example.com"); // 指针解出全名
        assert_eq!(p2, ptr_at + 2);
    }

    /// CNAME 应答：answer 段为 CNAME 指向 t.example.com，无 A
    #[test]
    fn parse_cname_only() {
        let id: u16 = 0x4242;
        let mut buf = Vec::new();
        buf.extend(id.to_be_bytes());
        buf.extend([0x81, 0x80]);
        buf.extend([0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);
        for l in ["a", "example", "com"] {
            buf.push(l.len() as u8);
            buf.extend(l.as_bytes());
        }
        buf.push(0);
        buf.extend([0x00, 0x01, 0x00, 0x01]);
        // answer: CNAME → t.example.com
        buf.extend([0xC0, 0x0C]);
        buf.extend([0x00, 0x05, 0x00, 0x01]);
        buf.extend([0x00, 0x00, 0x00, 0x3C]);
        let rd: &[u8] = b"\x01t\x07example\x03com\x00";
        buf.extend([(rd.len() as u16 >> 8) as u8, rd.len() as u8]);
        buf.extend(rd);
        let r = parse_response(&buf, id).unwrap();
        assert!(r.addrs.is_empty());
        assert_eq!(r.cname.as_deref(), Some("t.example.com"));
    }

    /// 干净的 A 记录应答解析
    #[test]
    fn parse_single_a() {
        let id: u16 = 0x7777;
        let mut buf = Vec::new();
        buf.extend(id.to_be_bytes());
        buf.extend([0x81, 0x80]);
        buf.extend([0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);
        // question: a.example.com
        for l in ["a", "example", "com"] {
            buf.push(l.len() as u8);
            buf.extend(l.as_bytes());
        }
        buf.push(0);
        buf.extend([0x00, 0x01, 0x00, 0x01]);
        // answer: 指向 offset12 的 name + A 1.2.3.4
        buf.extend([0xC0, 0x0C]);
        buf.extend([0x00, 0x01, 0x00, 0x01]);
        buf.extend([0x00, 0x00, 0x00, 0x3C]); // TTL
        buf.extend([0x00, 0x04, 1, 2, 3, 4]);
        let r = parse_response(&buf, id).unwrap();
        assert_eq!(r.addrs, vec![Ipv4Addr::new(1, 2, 3, 4)]);
        assert!(r.cname.is_none());
    }

    /// 端到端：mock UDP DNS server 应答 A 记录
    #[tokio::test]
    async fn e2e_mock_server() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        // server 任务：收查询，回 A 记录应答
        tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            let (n, from) = server.recv_from(&mut buf).await.unwrap();
            let q = &buf[..n];
            let mut resp = Vec::from(&q[..2]); // 回用 ID
            resp.extend([0x81, 0x80]);
            resp.extend([q[4], q[5], 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]); // QD=1 AN=1
            resp.extend(&q[12..]); // 原样回 question
            resp.extend([0xC0, 0x0C]); // name 指针
            resp.extend([0x00, 0x01, 0x00, 0x01]);
            resp.extend([0x00, 0x00, 0x00, 0x3C]);
            resp.extend([0x00, 0x04, 9, 9, 9, 9]);
            server.send_to(&resp, from).await.unwrap();
        });
        let ips = query_server(addr, "mock.example.com").await.unwrap();
        assert_eq!(ips, vec![Ipv4Addr::new(9, 9, 9, 9)]);
    }

    #[test]
    fn servers_parse_nameserver_lines() {
        let mut out: Vec<SocketAddr> = Vec::new();
        extract_nameservers(
            "search x\nnameserver 8.8.8.8\n# nameserver 1.1.1.1\nnameserver 9.9.9.9:5353\nnameserver bad\n",
            &mut out,
        );
        assert_eq!(
            out,
            vec![
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)), 5353),
            ]
        );
        // push_server 去重 + ip:port 解析
        push_server(&mut out, "8.8.8.8:53");
        assert_eq!(out.len(), 2);
        push_server(&mut out, "127.0.0.1:1053");
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].port(), 1053);
    }
}
