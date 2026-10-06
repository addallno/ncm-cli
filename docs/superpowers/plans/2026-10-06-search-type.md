# v0.3.13 search/recommend --type 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 ncm-rs 增加 `search -t` 多类型搜索、`recommend -t` 推荐位枚举，严格化 subcommand 语义并把 `dl` 正名为 `download` 别名。

**Architecture:** 新增纯逻辑模块 `src/searchtype.rs`（type 别名解析/配额/行提取），`api.rs` 增加按 type 搜索与每日推荐歌单接口，`main.rs` 接线两个命令并删除参数自动注入，最后文档与版本收尾。

**Tech Stack:** Rust + clap（derive/value_parser/ValueEnum）+ serde_json + reqwest；测试用 `cargo test` 内置单元测试。

## Global Constraints

- spec：`docs/superpowers/specs/2026-10-06-search-type-design.md`（本计划覆盖其全部需求）
- type 全放开（无白名单）；`-t` 支持数字+英文别名，逗号分隔
- `-n` 语义 = 输出总条数；多类配额 `quota_i = n/k + (i < n%k ? 1 : 0)`；`n=0` = 该类不限（请求上限常量 100）
- `recommend -t` 默认 `daily`，值域 `daily/toplist/fm/radar`
- 严格 subcommand：未知首参由 clap 报错；`download` 正式名、`dl` 别名
- 中文注释/文案；`cargo clippy -- -D warnings && cargo fmt --check && cargo test` 全绿才算任务完成
- 每个任务结束 commit（小步提交）；全部任务后统一 push 触发 CI

---

### Task 1: searchtype 纯逻辑模块

**Files:**
- Create: `src/searchtype.rs`
- Modify: `src/main.rs`（`mod` 声明处，与 `mod api;` 等并列）

**Interfaces:**
- Produces: `parse_type_token(&str) -> Result<u32, String>`、`dedup_types(Vec<u32>) -> Vec<u32>`、`quotas(u32, usize) -> Vec<u32>`、`type_label(u32) -> &'static str`、`const TYPE_HELP: &str`、`struct RenderRow { name, artist, meta, id: String, label: &'static str }`、`extract_row(u32, &Value) -> RenderRow`

- [ ] **Step 1: 写失败测试**（`src/searchtype.rs` 尾部 `#[cfg(test)] mod tests`）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_numbers_and_aliases() {
        assert_eq!(parse_type_token("1").unwrap(), 1);
        assert_eq!(parse_type_token("song").unwrap(), 1);
        assert_eq!(parse_type_token("PLAYLIST").unwrap(), 1000);
        assert!(parse_type_token("2000").is_ok()); // 全放开
        assert!(parse_type_token("nonsense").is_err());
        assert!(parse_type_token("").is_err());
    }

    #[test]
    fn quotas_split_total() {
        assert_eq!(quotas(10, 2), vec![5, 5]);
        assert_eq!(quotas(10, 3), vec![4, 3, 3]);
        assert_eq!(quotas(1, 2), vec![1, 0]);
        assert_eq!(quotas(0, 2), vec![0, 0]); // 0 = 不限
    }

    #[test]
    fn labels() {
        assert_eq!(type_label(1), "单曲");
        assert_eq!(type_label(1000), "歌单");
        assert_eq!(type_label(9999), "类型9999");
    }

    #[test]
    fn extract_song_and_playlist_rows() {
        let song = json!({"id": 5, "name": "晴天", "ar": [{"name": "周杰伦"}], "dt": 269000});
        let r = extract_row(1, &song);
        assert_eq!((r.name.as_str(), r.artist.as_str(), r.meta.as_str(), r.label),
                   ("晴天", "周杰伦", "04:29", "单曲"));
        let pl = json!({"id": 9, "name": "雷达", "creator": {"nickname": "甲"}, "trackCount": 30});
        let r = extract_row(1000, &pl);
        assert_eq!((r.artist.as_str(), r.meta.as_str(), r.label), ("甲", "30 首", "歌单"));
        // 字段缺失降级不 panic
        let r = extract_row(1004, &json!({"name": "mv"}));
        assert_eq!((r.artist.as_str(), r.meta.as_str()), ("-", "-"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test searchtype`
Expected: FAIL（模块不存在 / 编译错误）

- [ ] **Step 3: 实现模块**

```rust
//! 搜索/推荐 type 解析、配额与统一表行提取
use serde_json::Value;

/// type 英文别名表（大小写不敏感；数字直接透传，不做白名单）
const TYPE_ALIASES: &[(&str, u32)] = &[
    ("song", 1), ("album", 10), ("artist", 100), ("playlist", 1000),
    ("user", 1002), ("mv", 1004), ("lyric", 1006), ("radio", 1009),
    ("video", 1014), ("cloud", 1018), ("sound", 2000),
];

/// `search -t` 的 long_help（clap 静态文案）
pub const TYPE_HELP: &str = "\
搜索类型，逗号分隔，可混用数字与别名（如 1,1000 或 song,playlist）：
  1   song     单曲（默认）
  10  album    专辑
  100 artist   歌手
  1000 playlist 歌单
  1002 user    用户（仅列 ID）
  1004 mv      MV（仅列 ID）
  1006 lyric   歌词（仅列）
  1009 radio   主播电台（仅列 ID）
  1014 video   视频（仅列 ID）
  1018 cloud   综合（服务端不分页，limit 可能无效）
  2000 sound   声音（仅列）
未收录的数字也接受，按通用降级提取显示。";

/// 解析单个 type 标记（数字全放开；英文别名查表；其余报错）
pub fn parse_type_token(tok: &str) -> Result<u32, String> {
    let t = tok.trim();
    if t.is_empty() {
        return Err("type 不能为空".into());
    }
    if let Ok(v) = t.parse::<u32>() {
        return Ok(v);
    }
    for (a, v) in TYPE_ALIASES {
        if t.eq_ignore_ascii_case(a) {
            return Ok(*v);
        }
    }
    let all: Vec<&str> = TYPE_ALIASES.iter().map(|(a, _)| *a).collect();
    Err(format!("未知 type “{t}”（数字或别名：{}）", all.join("/")))
}

/// 保序去重
pub fn dedup_types(types: Vec<u32>) -> Vec<u32> {
    let mut out = Vec::new();
    for t in types {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// 总数 n 平分 k 类；余数依次给前几类；n=0 表示不限（配额 0）
pub fn quotas(n: u32, k: usize) -> Vec<u32> {
    if k == 0 {
        return vec![];
    }
    let base = n / k as u32;
    let rem = (n % k as u32) as usize;
    (0..k).map(|i| base + u32::from(i < rem)).collect()
}

/// 类型列中文文案；未收录 → `类型N`
pub fn type_label(ty: u32) -> String {
    match ty {
        1 => "单曲".into(), 10 => "专辑".into(), 100 => "歌手".into(), 1000 => "歌单".into(),
        1002 => "用户".into(), 1004 => "MV".into(), 1006 => "歌词".into(), 1009 => "电台".into(),
        1014 => "视频".into(), 1018 => "综合".into(), 2000 => "声音".into(),
        _ => format!("类型{ty}"),
    }
}

pub struct RenderRow {
    pub name: String,
    pub artist: String,
    pub meta: String,
    pub id: String,
    pub label: &'static str,
}

fn str_of(v: Option<&Value>) -> String {
    v.and_then(|x| x.as_str()).unwrap_or("-").to_string()
}

/// 从结果条目提取统一表行；字段缺失显示 `-`，不 panic
pub fn extract_row(ty: u32, it: &Value) -> RenderRow {
    let id = it["id"].as_u64()
        .or_else(|| it["userId"].as_u64())
        .map(|x| x.to_string())
        .unwrap_or_else(|| "-".into());
    let name = str_of(it["name"].as_ref().or(it["nickname"].as_ref()).or(it["title"].as_ref()));
    match ty {
        1 | 10 => {
            let artist = str_of(
                it["ar"][0]["name"].as_ref().or(it["artists"][0]["name"].as_ref()),
            );
            let meta = if ty == 1 {
                let ms = it["dt"].as_u64().or_else(|| it["duration"].as_u64()).unwrap_or(0);
                crate::model::hms(ms)
            } else {
                format!("{} 首", it["size"].as_u64().unwrap_or(0))
            };
            RenderRow { name, artist, meta, id,
                label: if ty == 1 { "单曲" } else { "专辑" } }
        }
        100 => RenderRow {
            meta: format!("{} 首", it["musicSize"].as_u64().unwrap_or(0)),
            name, artist: "-".into(), id, label: "歌手",
        },
        1000 => RenderRow {
            artist: str_of(it["creator"]["nickname"].as_ref()),
            meta: format!("{} 首", it["trackCount"].as_u64().unwrap_or(0)),
            name, id, label: "歌单",
        },
        1002 => RenderRow { name, artist: "-".into(), meta: "-".into(), id, label: "用户" },
        1004 => RenderRow {
            artist: str_of(it["artists"][0]["name"].as_ref()),
            meta: crate::model::hms(it["duration"].as_u64().unwrap_or(0)),
            name, id, label: "MV",
        },
        1006 => RenderRow {
            name,
            artist: str_of(it["ar"][0]["name"].as_ref()),
            meta: "-".into(),
            id,
            label: "歌词",
        },
        1009 => RenderRow {
            artist: str_of(it["dj"]["nickname"].as_ref()),
            name, meta: "-".into(), id, label: "电台",
        },
        1014 => RenderRow {
            artist: str_of(it["creator"][0]["userName"].as_ref()),
            meta: crate::model::hms(it["duration"].as_u64().unwrap_or(0)),
            name, id, label: "视频",
        },
        2000 => RenderRow {
            artist: str_of(it["creator"]["nickname"].as_ref()),
            name, meta: "-".into(), id, label: "声音",
        },
        // 未收录类型：通用降级（能取到什么显示什么；label 每行一个小字符串，量级 ≤ limit）
        _ => RenderRow {
            name,
            artist: "-".into(),
            meta: "-".into(),
            id,
            label: Box::leak(format!("类型{ty}").into_boxed_str()),
        },
    }
}
```

（`label` 用 `&'static str` + `Box::leak` 仅为与既有 `RenderRow` 测试写法一致；若 clippy 抱怨可整体改 `String`，测试断言同步用 `String` 比较。）

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test searchtype`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/searchtype.rs src/main.rs
git commit -m "feat: searchtype type 解析/配额/行提取模块"
```

---

### Task 2: api 按 type 搜索 + 每日推荐歌单

**Files:**
- Modify: `src/api.rs`

**Interfaces:**
- Consumes: Task 1 无
- Produces:
  - `pub async fn search_typed(&self, keyword: &str, ty: u32, limit: u32) -> Result<Vec<(u32, Value)>>`（返回条目带**最终** type；type=1018 展开为子类；条目保序）
  - `pub async fn daily_playlists(&self) -> Result<Vec<Value>>`
  - 现有 `search()` 改为委托 `search_typed(keyword, 1, limit)` 后取 `(ty, item)` 的 item（保持外部签名 `Result<Vec<Value>>` 不变）

- [ ] **Step 1: 实现 `search_typed`**

在 `api.rs` 的 `search` 函数位置改写：

```rust
/// 按 type 搜索；返回 (最终type, 条目)。type=1018 综合：按 result.order 展开各子类
pub async fn search_typed(&self, keyword: &str, ty: u32, limit: u32) -> Result<Vec<(u32, Value)>> {
    let data = self
        .get_json(
            "/api/search/get/web",
            &[
                ("type", ty.to_string()),
                ("s", keyword.into()),
                ("limit", limit.to_string()),
                ("offset", "0".into()),
            ],
        )
        .await?;
    let result = &data["result"];
    let mut out: Vec<(u32, Value)> = Vec::new();
    if ty == 1018 {
        // 综合：order 声明子类顺序；key → 子 type 映射
        const CLOUD_KEYS: &[(&str, u32)] = &[
            ("songs", 1), ("albums", 10), ("artists", 100), ("playlists", 1000),
            ("mvs", 1004), ("djRadios", 1009), ("videos", 1014),
        ];
        if let Some(order) = result["order"].as_array() {
            for k in order.iter().filter_map(|x| x.as_str()) {
                if let Some((_, sub)) = CLOUD_KEYS.iter().find(|(key, _)| key == k) {
                    if let Some(arr) = result[k].as_array() {
                        out.extend(arr.iter().map(|x| (*sub, x.clone())));
                    }
                }
            }
        }
        return Ok(out);
    }
    // 单类型：取 result 内该 type 的数组键；未收录键 → 找第一个数组
    let key = match ty {
        1 | 1006 => "songs",
        10 => "albums",
        100 => "artists",
        1000 => "playlists",
        1002 => "userprofiles",
        1004 => "mvs",
        1009 => "djRadios",
        1014 => "videos",
        2000 => "sounds",
        _ => "",
    };
    let arr = if !key.is_empty() {
        result[key].as_array()
    } else {
        None
    };
    let arr = arr.or_else(|| {
        result.as_object().and_then(|m| m.values().find_map(|v| v.as_array()))
    });
    if let Some(arr) = arr {
        out.extend(arr.iter().map(|x| (ty, x.clone())));
    }
    if ty == 1 {
        // 单曲保持既有相关度排序
        let mut songs: Vec<Value> = out.iter().map(|(_, v)| v.clone()).collect();
        rank_songs(&mut songs, keyword);
        out = songs.into_iter().map(|v| (1, v)).collect();
    }
    Ok(out)
}

/// 兼容旧签名：仅搜单曲
pub async fn search(&self, keyword: &str, limit: u32) -> Result<Vec<Value>> {
    Ok(self
        .search_typed(keyword, 1, limit)
        .await?
        .into_iter()
        .map(|(_, v)| v)
        .collect())
}
```

- [ ] **Step 2: 实现 `daily_playlists`**（放在 `daily_songs` 旁边）

```rust
/// 每日推荐歌单列表（需登录）；私人雷达在其中按名称筛出
pub async fn daily_playlists(&self) -> Result<Vec<Value>> {
    let data = self
        .get_json("/api/v1/discovery/recommend/resource", &[])
        .await?;
    Ok(data["recommend"].as_array().cloned().unwrap_or_default())
}
```

- [ ] **Step 3: 编译与既有测试**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: PASS（`search` 兼容层保证旧调用点不破；若 `rank_songs` 可见性/借用问题按报错修）

- [ ] **Step 4: 手测接口连通性**（网络请求，本地跑一次）

Run: `cargo run -- search -n 1`（应仍只搜单曲）与下条 Task 接线后测 `daily_playlists`
Expected: 正常表格输出

- [ ] **Step 5: Commit**

```bash
git add src/api.rs
git commit -m "feat: api search_typed 按 type 搜索与 daily_playlists"
```

---

### Task 3: 统一表渲染 print_render_table

**Files:**
- Modify: `src/main.rs`（`print_search_table` 附近）

**Interfaces:**
- Consumes: `searchtype::RenderRow`（Task 1）
- Produces: `fn print_render_table(rows: &[RenderRow])`（表头 `# / 名称 / 艺术家·创建者 / 时长·曲数 / 类型 / ID`）

- [ ] **Step 1: 实现渲染函数**（放在 `print_search_table` 后）

```rust
/// 多类型统一表：通用列 + 类型列（单组 type=1 仍走 print_search_table 保持原状）
fn print_render_table(rows: &[searchtype::RenderRow]) {
    let body: Vec<Vec<String>> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            vec![
                (i + 1).to_string(),
                r.name.clone(),
                r.artist.clone(),
                r.meta.clone(),
                r.label.to_string(),
                r.id.clone(),
            ]
        })
        .collect();
    ui::table(&["#", "名称", "艺术家·创建者", "时长·曲数", "类型", "ID"], &body);
}
```

- [ ] **Step 2: 编译**

Run: `cargo build`
Expected: PASS（若提示未使用会在 Task 4 接线后消失，加 `#[allow(dead_code)]` 临时挂起——Task 4 删除该属性；**不要**留到最终提交）

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: 多类型统一表渲染"
```

---

### Task 4: search 命令接线 `-t/--type`

**Files:**
- Modify: `src/main.rs`
  - `Commands::Search` 定义（约 :53-62）
  - `run()` 的 `Search` 分支（约 :1272-1291）
  - 测试 `cli_tests`（尾部）

**Interfaces:**
- Consumes: `searchtype::{parse_type_token, dedup_types, quotas, TYPE_HELP, extract_row}`、`Ncm::search_typed`、`print_render_table`
- Produces: `Commands::Search { keyword, types: Vec<u32>, limit, opts }`

- [ ] **Step 1: 写失败测试**（`cli_tests` 内追加）

```rust
/// -t 支持数字、别名、逗号分隔，解析为去重 Vec<u32>
#[test]
fn search_type_flag_parses() {
    let cli = Cli::try_parse_from(["ncm", "search", "-t", "1,1000", "moon"]).unwrap();
    match cli.cmd {
        Some(Commands::Search { types, .. }) => assert_eq!(types, vec![1, 1000]),
        _ => panic!("应为 Search"),
    }
    let cli = Cli::try_parse_from(["ncm", "search", "-t", "song,playlist,SONG", "x"]).unwrap();
    match cli.cmd {
        // value_parser 逐 token 解析；去重在 handler 做，此处断言解析值
        Some(Commands::Search { types, .. }) => assert_eq!(types, vec![1, 1000, 1]),
        _ => panic!("应为 Search"),
    }
    // 未知别名 → clap 解析错误
    assert!(Cli::try_parse_from(["ncm", "search", "-t", "bogus", "x"]).is_err());
    // 不给 -t 默认 [1]
    let cli = Cli::try_parse_from(["ncm", "search", "moon"]).unwrap();
    match cli.cmd {
        Some(Commands::Search { types, .. }) => assert_eq!(types, vec![1]),
        _ => panic!("应为 Search"),
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test search_type_flag`
Expected: FAIL（字段不存在）

- [ ] **Step 3: 改 `Commands::Search` 定义**

```rust
    /// 搜索并列出表格（-t 选类型如 1,1000；下载用 ncm download <关键词>）
    Search {
        /// 搜索关键词（可多个词，自动合并）
        #[arg(required = true)]
        keyword: Vec<String>,
        /// 搜索类型，逗号分隔（数字或别名，多类型合并为一张表）
        #[arg(
            short = 't',
            long = "type",
            value_delimiter = ',',
            value_parser = searchtype::parse_type_token,
            default_value = "1",
            long_help = searchtype::TYPE_HELP
        )]
        types: Vec<u32>,
        /// 输出总条数（多类型时平分）
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
```

并在 `main.rs` 的 `mod` 区加 `mod searchtype;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test search_type_flag`
Expected: PASS

- [ ] **Step 5: 改 `run()` 的 Search 分支**（多类型请求 + 配额 + 渲染 + JSON 分流）

```rust
        Some(Commands::Search {
            keyword,
            types,
            limit,
            opts,
        }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let kw = keyword.join(" ");
            let types = searchtype::dedup_types(types);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            let qs = searchtype::quotas(limit, types.len());
            let mut groups: Vec<(u32, Vec<(u32, Value)>)> = Vec::new();
            for (&ty, &q) in types.iter().zip(&qs) {
                // 配额 0 = 不限，请求上限 100
                let items = ncm.search_typed(&kw, ty, if q == 0 { 100 } else { q }).await?;
                groups.push((ty, items));
            }
            let total: usize = groups.iter().map(|(_, v)| v.len()).sum();
            if opts.json {
                if groups.len() == 1 {
                    let items: Vec<Value> = groups[0].1.iter().map(|(_, v)| v.clone()).collect();
                    print_search_json(&items)?;
                } else {
                    // 多类型：{ "1": [...], "1000": [...] }，键序同输入
                    let mut obj = serde_json::Map::new();
                    for (ty, items) in &groups {
                        obj.insert(
                            ty.to_string(),
                            Value::Array(items.iter().map(|(_, v)| v.clone()).collect()),
                        );
                    }
                    println!("{}", serde_json::to_string_pretty(&Value::Object(obj))?);
                }
            } else if total == 0 {
                ui::info(&format!("未搜索到结果: {kw}"));
            } else if groups.len() == 1 && groups[0].0 == 1 {
                // 单组单曲：保持原输出不变
                let items: Vec<Value> = groups[0].1.iter().map(|(_, v)| v.clone()).collect();
                print_search_table(&items);
                ui::info(&format!("下载: ncm download \"{kw}\" -d 编号"));
            } else {
                let mut rows: Vec<searchtype::RenderRow> = Vec::new();
                for (ty, items) in &groups {
                    rows.extend(items.iter().map(|(t, v)| searchtype::extract_row(*t, v)));
                }
                print_render_table(&rows);
                ui::info(&format!("下载: ncm download \"{kw}\" -d 编号  ·  浏览歌单: ncm list <ID>"));
            }
            Ok(0)
        }
```

删除 Step 3 中为 `print_render_table` 临时加的 `#[allow(dead_code)]`（若有）。

- [ ] **Step 6: 全量测试 + 手测**

Run: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: 全绿
手测: `cargo run -- search -t song,playlist -n 10 周杰伦` → 一张表、类型列、总行数 ≤10 且两类都有（配额 5/5）

- [ ] **Step 7: Commit**

```bash
git add src/main.rs
git commit -m "feat: search -t 多类型搜索与统一表"
```

---

### Task 5: recommend 命令重构 `-t daily,toplist,fm,radar`

**Files:**
- Modify: `src/main.rs`（`Commands::Recommend` 定义约 :97-102、`run()` 分支约 :1342-1393、`cli_tests`）

**Interfaces:**
- Consumes: `Ncm::{daily_songs, toplist, radio_songs, daily_playlists}`、`model::resolve_list`、`searchtype::{quotas, extract_row}`、`print_render_table`
- Produces: `Commands::Recommend { types: Vec<RecKind>, limit, opts }`；`enum RecKind { Daily, Toplist, Fm, Radar }`（clap `ValueEnum`）

- [ ] **Step 1: 写失败测试**（`cli_tests` 追加）

```rust
/// recommend -t 值域与默认
#[test]
fn recommend_type_flag_parses() {
    use crate::RecKind;
    // 默认 daily
    let cli = Cli::try_parse_from(["ncm", "recommend"]).unwrap();
    match cli.cmd {
        Some(Commands::Recommend { types, .. }) => {
            assert_eq!(types, vec![RecKind::Daily])
        }
        _ => panic!("应为 Recommend"),
    }
    // 逗号多值
    let cli = Cli::try_parse_from(["ncm", "recommend", "-t", "fm,radar"]).unwrap();
    match cli.cmd {
        Some(Commands::Recommend { types, .. }) => {
            assert_eq!(types, vec![RecKind::Fm, RecKind::Radar])
        }
        _ => panic!("应为 Recommend"),
    }
    // 未知值报错
    assert!(Cli::try_parse_from(["ncm", "recommend", "-t", "nope"]).is_err());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test recommend_type_flag`
Expected: FAIL

- [ ] **Step 3: 定义 RecKind 与改命令**

```rust
/// recommend 的推荐位枚举（clap ValueEnum 自动列出可能值）
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum RecKind {
    /// 每日推荐歌曲（需登录）
    Daily,
    /// 官方榜单
    Toplist,
    /// 私人FM 首批曲目
    Fm,
    /// 私人雷达歌单
    Radar,
}
```

```rust
    /// 首页推荐（-t daily,toplist,fm,radar，默认 daily 只列每日推荐歌曲）
    Recommend {
        /// 推荐位，逗号分隔：daily 每日推荐 / toplist 榜单 / fm 私人FM / radar 私人雷达
        #[arg(
            short = 't',
            long = "type",
            value_delimiter = ',',
            default_value = "daily"
        )]
        types: Vec<crate::RecKind>,
        /// 输出总条数（多推荐位平分；0 = 全部）
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: u32,
        #[command(flatten)]
        opts: ViewArgs,
    },
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test recommend_type_flag`
Expected: PASS

- [ ] **Step 5: 重写 `run()` 的 Recommend 分支**

```rust
        Some(Commands::Recommend {
            types,
            limit,
            opts,
        }) => {
            ui::set_json(opts.json);
            ui::set_quiet(opts.quiet);
            let ncm = Ncm::with_cookie(auth.as_deref())?;
            // 去重保序
            let mut kinds: Vec<RecKind> = Vec::new();
            for &k in &types {
                if !kinds.contains(&k) {
                    kinds.push(k);
                }
            }
            let qs = searchtype::quotas(limit, kinds.len());
            // 各源取曲目：单曲源 Vec<Value>；榜单是歌单列表单独处理
            let mut song_rows: Vec<searchtype::RenderRow> = Vec::new();
            let mut topo_rows: Vec<searchtype::RenderRow> = Vec::new();
            let mut json_out = serde_json::Map::new();
            let mut errors: Vec<String> = Vec::new();
            for (&k, &q) in kinds.iter().zip(&qs) {
                let cap = if q == 0 { usize::MAX } else { q as usize };
                let key = match k {
                    RecKind::Daily => "daily",
                    RecKind::Toplist => "toplist",
                    RecKind::Fm => "fm",
                    RecKind::Radar => "radar",
                };
                match k {
                    RecKind::Daily => {
                        match ncm.daily_songs().await {
                            Ok(mut s) => {
                                if s.len() > cap {
                                    s.truncate(cap);
                            }
                                json_out.insert(key.into(), Value::Array(s.clone()));
                                song_rows.extend(s.iter().map(|v| searchtype::extract_row(1, v)));
                            }
                            Err(e) => errors.push(format!("每日推荐失败: {e:#}")),
                        }
                    }
                    RecKind::Fm => match ncm.radio_songs().await {
                        Ok(mut s) => {
                            if s.len() > cap {
                                s.truncate(cap);
                            }
                            json_out.insert(key.into(), Value::Array(s.clone()));
                            song_rows.extend(s.iter().map(|v| searchtype::extract_row(1, v)));
                        }
                        Err(e) => errors.push(format!("私人FM失败: {e:#}")),
                    },
                    RecKind::Toplist => match ncm.toplist().await {
                        Ok(mut t) => {
                            if t.len() > cap {
                                t.truncate(cap);
                            }
                            json_out.insert(key.into(), Value::Array(t.clone()));
                            topo_rows.extend(t.iter().map(|it| searchtype::RenderRow {
                                name: it["name"].as_str().unwrap_or("-").into(),
                                artist: it["updateFrequency"].as_str().unwrap_or("-").into(),
                                meta: format!("{} 首", it["trackCount"].as_u64().unwrap_or(0)),
                                id: it["id"].as_u64().map(|x| x.to_string()).unwrap_or_else(|| "-".into()),
                                label: "榜单",
                            }));
                        }
                        Err(e) => errors.push(format!("榜单获取失败: {e:#}")),
                    },
                    RecKind::Radar => {
                        // 每日推荐歌单中筛「私人雷达」；接口失败或未命中给统一兜底
                        let found = ncm
                            .daily_playlists()
                            .await
                            .ok()
                            .and_then(|pls| {
                                pls.iter()
                                    .find(|p| p["name"].as_str().is_some_and(|n| n.contains("私人雷达")))
                                    .and_then(|p| p["id"].as_u64())
                            });
                        match found {
                            Some(id) => match resolve_list(&ncm, &id.to_string()).await {
                                Ok((mut songs, desc)) => {
                                    if songs.len() > cap {
                                        songs.truncate(cap);
                                    }
                                    json_out.insert(
                                        key.into(),
                                        json!({ "playlist_id": id, "desc": desc, "songs": songs }),
                                    );
                                    song_rows.extend(
                                        songs.iter().map(|v| searchtype::extract_row(1, v)),
                                    );
                                }
                                Err(e) => errors.push(format!("私人雷达歌单解析失败: {e:#}")),
                            },
                            None => errors.push(
                                "未找到私人雷达歌单（可 ncm list <私人雷达链接> 手动浏览）".into(),
                            ),
                        }
                    }
                }
            }
            if opts.json {
                if kinds.len() == 1 && kinds[0] != RecKind::Toplist {
                    // 单非榜单源：直接输出数组（保持旧 daily JSON 形状）
                    let v = json_out.values().next().cloned().unwrap_or(Value::Array(vec![]));
                    println!("{}", serde_json::to_string_pretty(&v)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&Value::Object(json_out))?);
                }
                if !errors.is_empty() {
                    bail!("{}", errors.join("；"));
                }
                return Ok(0);
            }
            let rows: Vec<searchtype::RenderRow> =
                song_rows.into_iter().chain(topo_rows).collect();
            if rows.is_empty() {
                bail!(
                    "{}（每日推荐/雷达需登录: ncm login）",
                    if errors.is_empty() { "推荐内容为空".to_string() } else { errors.join("；") }
                );
            }
            print_render_table(&rows);
            for e in &errors {
                ui::info(e);
            }
            ui::info("下载: ncm download -d 编号  ·  浏览: ncm list <ID>");
            Ok(0)
        }
```

- [ ] **Step 6: 全量测试 + 手测**

Run: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: 全绿
手测:
- `cargo run -- recommend` → 只列每日推荐（不再有榜单节）
- `cargo run -- recommend -t toplist -n 5` → 榜单表 5 行、类型列「榜单」
- `cargo run -- recommend -t fm` → 私人FM 曲目
- `cargo run -- recommend -t radar` → 雷达歌单曲目或明确兜底文案

- [ ] **Step 7: Commit**

```bash
git add src/main.rs
git commit -m "feat: recommend -t 推荐位枚举 daily/toplist/fm/radar"
```

---

### Task 6: 严格 subcommand（删自动注入）

**Files:**
- Modify: `src/main.rs`（`SUBCOMMANDS` :1200 附近、`inject_dl_subcommand` :1214-1225、`main()` :1227、`cli_tests`）

**Interfaces:**
- Consumes: 无
- Produces: 未知首参 → clap `unrecognized subcommand` 错误

- [ ] **Step 1: 写失败测试**（`cli_tests` 追加）

```rust
/// 未知首参不再注入 dl，由 clap 报 unrecognized subcommand
#[test]
fn unknown_first_arg_is_not_search_or_dl() {
    assert!(Cli::try_parse_from(["ncm", "259138"]).is_err());
    assert!(Cli::try_parse_from(["ncm", "moon"]).is_err());
    // 显式子命令不受影响
    assert!(Cli::try_parse_from(["ncm", "dl", "259138"]).is_ok());
    assert!(Cli::try_parse_from(["ncm", "download", "259138"]).is_ok());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test unknown_first_arg`
Expected: FAIL（`259138` 现被注入 dl 而解析成功）

- [ ] **Step 3: 删除注入机制**

- 删除 `SUBCOMMANDS` 常量与 `inject_dl_subcommand()` 整段。
- `main()` 改为 `let cli = Cli::parse();`（`Parser` 已在 use 中）。
- 删除测试 `subcommand_whitelist_covers_all`（依赖 SUBCOMMANDS）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test unknown_first_arg && cargo test`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat!: 严格 subcommand，移除未知参数自动注入 dl"
```

---

### Task 7: `download` 正式名 + `dl` 别名

**Files:**
- Modify: `src/main.rs`（`Commands::Dl` 定义 :63-69、`section_of` :1242、`run()` 分支 :1292、`run_batch`/其他 `Commands::Dl` 引用、`cli_tests`）

**Interfaces:**
- Consumes: 无
- Produces: `Commands::Download`（clap name=`download`、visible_alias=`dl`）；`section_of` 对其返回 `"download"`

- [ ] **Step 1: 写失败测试**（`cli_tests` 追加；已有 `section_of_commands` 中 `(vec!["ncm","dl","1"],"download")` 继续作别名回归）

```rust
/// download 为正式名，dl 为别名，同一 variant
#[test]
fn download_alias_parses() {
    let a = Cli::try_parse_from(["ncm", "download", "1"]).unwrap();
    let b = Cli::try_parse_from(["ncm", "dl", "1"]).unwrap();
    assert!(matches!(a.cmd, Some(Commands::Download { .. })));
    assert!(matches!(b.cmd, Some(Commands::Download { .. })));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test download_alias`
Expected: FAIL（variant 还叫 `Dl`）

- [ ] **Step 3: 重命名与别名**

- `Commands::Dl` → `Commands::Download`，加属性：`#[command(name = "download", visible_alias = "dl")]`，doc 注释改为 `/// 下载（唯一下载入口，别名 dl）：歌曲 ID / 歌单·专辑链接 / 关键词 / daily（每日推荐）`。
- 全部 `Commands::Dl` 引用改 `Commands::Download`（`section_of`、`run()` 分支、测试等，用 `grep -n "Commands::Dl" src/main.rs` 找全）。
- `run()` 内 bail 文案 `ncm help dl` → `ncm help download`。
- 其余提示文案中 `ncm dl ` → `ncm download `（grep `ncm dl` 逐处改；`run_batch` 内部逻辑不动）。

- [ ] **Step 4: 跑测试 + 手测**

Run: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: 全绿
手测: `cargo run -- help` 显示 `download [dl]`；`cargo run -- dl --help` 可用

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: download 正式命令名，dl 保留为别名"
```

---

### Task 8: 文档、版本与交付

**Files:**
- Modify: `README.md`、`Description.md`、`Cargo.toml`（version）、`Cargo.lock`（version 同步）

**Interfaces:** 无

- [ ] **Step 1: README.md**
  - `search` 节：加 `-t/--type` 用法与含义表（复制 spec §1 表）、`-n` 总条数语义、多类型统一表示例输出。
  - `recommend` 节：默认只列每日推荐；`-t daily,toplist,fm,radar` 说明与各源一句话描述。
  - 删除"未知参数自动当下载"快捷方式描述（若有）；明确 `ncm <未知命令>` 会报错。
  - 命令示例统一 `ncm download`（注明别名 `ncm dl`）。
  - `-n` 语义统一小节：所有命令 = 输出总条数，0 = 全部（适用处）。

- [ ] **Step 2: Description.md**：同步上述变更点（命令表、特性列表）。

- [ ] **Step 3: 版本 bump**：`Cargo.toml` `version = "0.3.13"`；`cargo build` 后 `Cargo.lock` 自动同步。

- [ ] **Step 4: 全量验证**

```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo run -- --version
```
Expected: 全绿，`ncm 0.3.13`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: v0.3.13 search/recommend --type 与 download 别名文档"
```

- [ ] **Step 6: push 触发 CI 并等结果**

```bash
git push origin master
gh run list --limit 3
gh run watch   # 至 success
```

- [ ] **Step 7: 打 tag 触发 Release**

```bash
git tag v0.3.13 && git push origin v0.3.13
gh run list --limit 3   # tag workflow success
gh release view v0.3.13 --json assets --jq '.assets[].name'   # 确认 aarch64 产物
```

- [ ] **Step 8: 远端部署验证**

```bash
# 从 Release 下载 aarch64 产物到远端 ~/ncm（沿用 v0.3.12 部署方式）
ssh -p 18022 root@43.136.171.164 '~/ncm --version'          # ncm 0.3.13
ssh -p 18022 root@43.136.171.164 '~/ncm search -t song,playlist -n 10 周杰伦'
ssh -p 18022 root@43.136.171.164 '~/ncm recommend -t fm | head'
ssh -p 18022 root@43.136.171.164 '~/ncm 123'                 # 应报 unrecognized subcommand
ssh -p 18022 root@43.136.171.164 '~/ncm dl --help | head'    # 别名可用
```
Expected: 逐条符合预期

---

## Self-Review 记录

1. **Spec 覆盖**：§1 search -t（Task 1/2/4）、§2 recommend -t+radar（Task 5）、§3 统一表（Task 1/3/4/5）、§4 严格 subcommand（Task 6）、§5 download 别名（Task 7）、§6 -n 归一（Task 1 quotas + 各接线 + Task 8 文档）、§7 错误处理（value_parser/兜底文案散于 Task 4/5）、§8 测试（各任务 step 1）、§9 非目标未引入、§10 交付（Task 8）。无缺口。
2. **占位扫描**：Task 1 Step 3 的 `returnBox` 为**已注明的临时占位**，Step 3a 给出修正（改为 `String` 返回）——执行时按 3a 落地，不留占位。
3. **类型一致性**：`search_typed -> Vec<(u32, Value)>` 与 Task 4/5 消费一致；`extract_row(u32, &Value) -> RenderRow` 一致；`quotas(u32, usize) -> Vec<u32>` 一致；`RecKind` 在定义与测试/分支一致。
