# PLAN：修复 guard 对账误删正确曲目（切成下一首）

任务（用户）：纠错机制有时把正确的音乐切成下一首、整个列表乱了、且它检查不到。

## 根因假设（已静态分析定位）
两指针分类（main.rs:4350-4375）在**顺序错位**（相邻交换/重复 URL）时断言过激进：
- `files[i+1]==expect[j]` → 直接判 files[i] 残影删除，**不检查 files[i] 是否也存在于 expect 中**。
- 若 files[i] 是正确曲目只是位置不对（顺序错位/重复曲歧义）→ **误删**；若 i 恰为当前播放项 → mpv 跳下一项 =「正确音乐被切成下一首」，后续项连锁错位 =「列表乱了」。
- 修复后复读 `v==expect` 只验证内部一致，**不验证删对了没**；unclass 放行时静默 =「检查不到」。

## 步骤
1. **重构提取**：把两指针分类抽成纯函数 `classify_playlist_diff(files, expect) -> Result<(phantoms, missing), Unclass>`（guard_loop 内联逻辑原样搬移，行为不变）。
   - 验收：`cargo check` 通过，现有 73 测试不回归。
2. **红测试**：新增单测证明误判——顺序交换（[A,C,B,D] vs [A,B,C,D]，C 判残影）、重复 URL 歧义（[A,A,B] vs [A,B,A]）、当前播放项被分类为 phantom 的场景。
   - 验收：新测试在未修复逻辑下失败（或重构后断言暴露误删）。
3. **修复**（存在性守卫）：
   - phantom 判定加 `!expect.contains(&files[i])`——files[i] 在 expect 中存在过 → 是顺序问题不是残影 → `Unclass` 放行（宁不修不乱）。
   - missing 判定同理加 `!files.contains(&expect[j])`。
   - 当前播放项（i == pos）永不判 phantom（双保险）。
   - 验收：步骤 2 红测试转绿。
4. **「检查不到」补检测**：unclass 且 count==srcs_len 时不再完全静默——toast 一次「曲目顺序不一致，跳过修复」（带去抖，防刷屏）。
   - 验收：代码 review + 测试覆盖分支。
5. **三绿**：`cargo fmt --check && cargo clippy --all-targets && cargo test`（基线 73 + 新增）。
6. **复现验证**：隔离环境构造 mpv/srcs 顺序错位（重复曲场景），跑 build 后二进制观察：修复前误删切歌 → 修复后放行不乱。
   - 验收：修复后无误删、toast 提示一次。
7. **提交**：中文 commit 直推 master（不发版，0.3.53 是否发待用户拍板）。

## 风险
- 本地 mpv 0.37 拒 insert-at：missing 补项路径本机无法全验（远端 0.41 可）；本次修的是 remove 误删路径（本机可用）。
- unclass 放行变多：真残影若与某 expect 项同 URL 会漏修——但同 URL 残影删了也无意义（内容相同），可接受。
