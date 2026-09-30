# CLAUDE.md

本文件为 Claude Code 提供此项目的上下文指引。

## 构建与测试

```bash
cargo build                              # 编译
cargo test                               # 全部测试（110 单测 + 22 DSL 差分 + 6 集成）
cargo test --test integration            # 集成测试
cargo run -- full --rounds 5             # 一键下载+预检+5轮测试（~5-15 分钟）
cargo run -- full --limit 10 --rounds 3  # 快速测试前 10 个
cargo run -- full --sample 10 --rounds 3 # 随机抽样 10 个（代表性测试，与 --limit 互斥）
cargo run -- --json status               # 查看管道进度（JSON 格式）
cargo run -- status                      # 查看管道进度（人类可读）
cargo run -- --json full --limit 5       # 一键运行+JSON 输出（AI 友好）
```

## 管道流程

0. `自动更新` — 从 legado.aoaostar.com 获取最新全量书源 JSON（本地缓存，增量更新）
1. `preflight` — ~3911 个源 → ~2957 个文字+启用源。排除非文字/禁用、URL 无效、无搜索能力的源。
2. `test` — 每源最多试 3 个关键词（checkKeyWord → 通用关键词 → config.toml 书）。50 并发请求。结果缓存在 SQLite 中，支持断点续测。

## 阅读链路验证（--verify-content，默认开启）

`passed` 现在意味着**完整阅读链路可读**，而不仅是搜索有结果：

```
搜索(ruleSearch) → 详情页(ruleBookInfo) → 目录页(ruleToc) → 第一章正文(ruleContent)
```

- 实现：`src/content_check.rs`，任一环节失败即 `failed` 并记录具体 stage
- **张冠李戴一致性校验**：`verify_chain` 拿到搜索结果的书名后，从详情页
  `ruleBookInfo.name` 提取实际书名做相似度比对（包含关系或 ≥50% bigram 重合），
  完全不匹配判 `mismatch` —— 链路"能走通"但读的是另一本书的源不再误判 passed
- **static 探测**：链路验证通过后用配置的"冷门"关键词（`[test] static_probe_keywords`，
  默认 `zzzzz`）再搜一次；结果与主关键词 Jaccard 重合 ≥90% 说明搜索写死。
  命中源仍保留 `passed`（链路确实可读），但证据里标 `static_probe: true` 并写入
  `output/static.json` 供人工复核
- **证据记录**：每次终态判定把 `keywords_tried` / `chains_ok` / `verified_keyword` /
  `book_name` / `content_len` / `static_probe` 写入 `test_results.evidence`（JSON 列），
  并追加 `test_history` 表（历史状态序列）。`cache_ttl_days`（默认 0=关）可让
  passed 源超期自动重测
- **per-domain 限速**：尊重书源声明的 `concurrentRate`（毫秒字符串 `"1000"` 或
  `{"rate","interval","timeUnit"}` 对象），同域取最保守间隔，`HttpClient.fetch`
  按域名串行限速（`with_rate_limits`）；未声明域名不受影响
- **Legado 语义差分测试**：`tests/dsl_diff.rs` 以官方规则语义为基准，用真实书源
  规则模式 + fixture 锁定提取行为（定位器/位置索引 `!n` 排除/提取器/`||` 回退/
  `##` 替换/JSONPath/类型不匹配短路），已知差距（负索引、区间、`&&`、children、
  ownText/all）在 `known_gaps` 文档化
- JS 规则（`@js:`/`<js>`）在 `ruleToc`/`ruleContent`/`ruleBookInfo.tocUrl` 中通过
  `src/js_polyfill/runner.rs::execute_rule_batch_async` 求值（node 批量）
- **java.ajax / java.post 交互桥**：`ruleContent`/`ruleToc`/`@js:` 搜索 URL 里的
  `java.ajax(url)` 不再返回空串，而是通过持久 node 进程的 stdin/stdout 双向协议回调 Rust 侧
  真实发 HTTP 请求（GET/POST、带 source.header + Referer），把响应体喂回 JS 继续求值。
  实现：`runner.rs::execute_rule_batch_async` / `execute_js_batch_async` 的 `run_rule_bridge`
  循环 + `polyfill.js` 的同步 `__readLineSync`/`__writeLine`
- 桥的 HTTP 请求**不跟随重定向**（`no_redirect` client），`java.post(...).header("Location")`
  之类的规则能读到 302 的响应头；返回值为 String 包装对象，既当字符串用又带 `.header(name)`
- 请求自动携带 `source.header` 解析出的头 + `Referer`
- 失败原因细分：`toc_parse` / `content_short` / `bookinfo_fetch` / `toc_fetch` /
  `chapter_fetch` / `blocked`(403) / `not_found`(404) / `rate_limited`(429)
- `--no-verify-content` 或 `--verify-content=false` 回退到仅搜索验证
- 后几轮只重试瞬时原因（network_error / *_fetch / rate_limited），确定性失败跳过

### JS 搜索结果提取（ruleSearch 含 JS）

约 73 个源的 `ruleSearch.bookList` 含 `<js>`（解密/变换 payload），另有大量源的
`bookUrl`/`name` 等字段是 `<js>`/`@js:`。tester 和 `chain_check` 现在通过
`content_check::extract_results_js` 处理：

- 先对 `<js>...</js>` bookList 段用 `execute_rule_batch_async` 求值，得到变换后的
  文本（常是解密后的 JSON/HTML），再用 `</js>` 后剩余 DSL 规则提取每项
- **字段级 JS**：`bookUrl`/`name`/`coverUrl` 若含 `<js>`/`@js:`，对每个 item 的
  内容（`result`）单独求值（如 `java.getString('$.book_id')` 生成详情 URL）
- **`{{$.a||$.b}}` 模板回退**：`apply_field_extract` 用 `expand_template` 展开
  双花括号占位符，`||` 分段取第一个非空 JSONPath 值（如长佩文学 bookUrl）
- `extract_items` 会把 bookList 提取出的整个 JSON 数组展开为逐元素 item

调试工具：
- `cargo run --example chain_check` — 对真实源跑完整链路并打印每环结论（搜索提取同样走 JS 预处理）
- `cargo run --example ajax_bridge_check` — 专门扫描 ruleContent 含 java.ajax 的源并跑链路
- `cargo run --example ajax_live_check` — 对真实公网 API 验证 java.ajax 桥

## 自动续跑

全量测试（2957 源 × 5 轮）约需 5-15 分钟，第 1 轮为主，后 4 轮只重试极少量 network_error 源。SQLite 缓存支持断点续测：

```bash
# 首次启动（无缓存时自动从头跑；已有缓存则直接续测）
cargo run -- full --rounds 5

# 如果超时或中断：查进度，然后续跑
cargo run -- --json status              # 看已完成/剩余多少
cargo run -- full --rounds 5            # 续跑——已有缓存的源直接跳过

# 强制从头重测（清除 test_cache.db 和 filtered.json）
cargo run -- full --force --rounds 5
```

关键原则：
- `full` 命令**默认保留缓存续测**：已 `passed` / `dead_domain` / `js_api` 的源直接跳过，只重测未测项和 `failed` 源
- 仅当书源 JSON 更新或显式加 `--force` 时才会清除缓存、从头重测
- SQLite 缓存（`output/test_cache.db`）保存每源最终状态，重复执行只测未完成项
- `--rounds N` 的后几轮只重试瞬时失败（network_error / bookinfo_fetch / toc_fetch / chapter_fetch / rate_limited / server_error），不重试确定性失败（no_results / toc_parse / blocked / not_found / content_short / passed / dead_domain）
- 用 `--json` flag 输出结构化结果，方便 AI 解析进度和状态

## 已知的正文解析盲区（改进方向）

当前阅读链路验证仍无法覆盖以下场景，命中时不会误判 passed，但也无法自动验证：

1. ~~**JS 正文需要二次 HTTP**~~ ✅ 已实现（java.ajax/java.post 交互桥）。仍无法覆盖的变体：
   依赖 `java.getCookie`/验证码/`setVariable` 跨请求状态的源（如来看文学），桥只能做单次请求。
2. **强加密正文** — 大部分已实现：
   `aesBase64DecodeToString` 支持 AES/DESede；`SecretKeySpec`/`IvParameterSpec`/
   `Base64.decode`/`Cipher`/`Arrays.copyOfRange` Java 类 polyfill 覆盖
   `Cipher.getInstance(...)` 风格的 DESede/AES 解密（含七猫 iv 内嵌模式）。
   仍无法覆盖：RSA/SHA256WithRSA（约 20 源，需 PKCS8 密钥解析）、
   DES-CBC（Node OpenSSL 3 禁用）、自定义 key 派生。
3. ~~**正文分页**~~ ✅ 已实现 — `nextContentUrl` 翻页正文已支持：`verify_chain` 提取
   当前页正文后按 `nextContentUrl` 循环翻页累计（上限 30 页、自引用防死循环）；
   提取不到下一页时回退为单页判定，不影响普通章节。数据中约 865 源声明了该规则。
4. **正文依赖登录/cookie** — `enabledCookieJar` 源（1690 个）需先访问站点种 cookie，
   当前 HttpClient 未开 cookie_store，命中会判 `blocked` 或 `no_results`。
5. **图片型正文（漫画）** — ruleContent 提取纯图片无文字，`content_short` 阈值会误杀；
   对 `bookSourceType != 0` 的漫画源当前不入 eligible，影响有限。

## 架构

```
src/
├── main.rs          — 入口：解析 CLI、分发 status/full（薄层）
├── lib.rs           — 库导出，供集成测试使用
├── cli.rs           — clap 参数定义 + --json 事件输出
├── paths.rs         — 输入/输出路径解析（CWD 优先，XDG 兜底）
├── config.rs        — config.toml 加载 + TestConfig 构造（[[search]] 支持 domain 域名命中、
│                      [test] generic_keywords 可配置）
├── fetcher.rs       — 书源下载（aoaostar 索引发现 + 原子替换）
├── pipeline.rs      — 多轮测试编排、dead_domain 标记、汇总、输出
├── types.rs         — BookSource、规则结构体、枚举
├── status.rs        — 强类型状态模型：TestStatus（passed/failed/dead_domain/js_api/
│                      skipped/unverified）+ FailReason（含 is_transient 分类），
│                      DB 字符串兼容（历史缓存可直接读）
├── preflight.rs     — 5 步预检分类管道
├── url_fixer.rs     — 自动修复残缺 bookSourceUrl + normalized_domain/domain_matches
│                      （url crate 归一化，www. 剥离；dead_domain 按域聚合）
├── search_url.rs    — 从 searchUrl 构造 HTTP 请求（{{key}}、POST、JS L1）
├── http_client.rs   — reqwest 封装（chardetng 编码探测 + encoding_rs 转码）
├── tester.rs        — 异步测试调度（tokio Semaphore + 抖动重试）
├── content_check.rs — 完整阅读链路验证（搜索→详情→目录→正文）+ JS 搜索结果提取
│                     （extract_results_js / rule_has_js / expand_template / extract_items）
├── reporter.rs      — JSON 报告输出
├── db.rs            — SQLite 缓存层（单连接 Mutex + WAL，断点续测）
├── rule_dsl/        — Legado 自定义规则 DSL 解析器
│   ├── parser.rs         — 词法分析器（裸字母 token → TagName 选择器；"." 保留整页兜底；
│   │                       `.n` 取 / `!n` 排除拆分为 Index / ExcludeIndex；split_segments 分段）
│   ├── html_extractor.rs — HTML 规则求值（scraper；JSONPath 等类型不匹配规则返回空；
│   │                       extract 按 `||` 分段回退 + 段内 `##`；textNodes/@html 取直接
│   │                       子节点而非 html 包装层）
│   ├── json_extractor.rs — JSONPath 求值（同样按 `||` 分段回退）
│   └── combinator.rs     — `##` 正则替换（按段应用；Index 由 extractor 消费，不再二次处理）
└── js_polyfill/     — JS 规则求值引擎（searchUrl + ruleToc/ruleContent）
    ├── polyfill.js       — 模拟 java.* API（getString/getElements/加解密，node crypto；
    │                       java.ajax/post 经 stdin/stdout 桥回调 Rust 侧真实 HTTP）
    └── runner.rs         — node 批量求值（execute_js_batch/_async 搜索URL /
                           execute_rule_batch_async 规则+java.ajax 交互桥；JS 规则支持
                           「裸表达式结尾」，eval 完成值作为结果；搜索结果同样走桥）
```

## 关键数据

- 输入：从 legado.aoaostar.com 自动获取最新全量书源（~3911 个，~21MB），本地 `data/` 目录缓存
- 文字+启用：3680 → 预检 → 2957 个待测（683 跳过、46 仅发现）
- searchUrl 类型：2785 模板 + 129 @js + 21 `<js>` + 22 纯 URL
- 输出：`output/filtered.json`（约 640 个可用源，导入 Legado） + `output/explore_only.json`
  + `output/unverified.json`（搜索无 bookUrl，链路不可验证）+ `output/static.json`
  （搜索疑似写死的 passed 源，供人工复核）
- `passed` 语义：搜索有结果且**能提取到书籍 URL 并走通阅读链路**才算 passed；
  搜索结果无 bookUrl 的源记为 `unverified`（写 `output/unverified.json`，不入 filtered）。
- 测试状态为强类型（`status.rs`），DB 存字符串兼容旧缓存；`reason` 的瞬时性判定
  （`is_transient`）决定后续轮次是否重试。

## CLI 参数

| 参数 | 说明 |
|------|------|
| `full` | 一键 preflight + test（推荐） |
| `status` | 查看当前管道进度（支持 `--json`） |
| `--json` | 全局 flag，输出 JSON Lines 到 stdout（AI 友好） |
| `--force` | 忽略缓存，全部重新测试 |
| `--limit N` | 只测前 N 个源 |
| `--sample N` | 随机抽样 N 个源（全池代表性测试，与 `--limit` 互斥） |
| `--rounds N` | 测试轮数，失败源每轮重试（默认 1） |
| `--concurrency N` | 并发数，默认 50 |
| `--verify-content` | 完整阅读链路验证（默认开启）；`--verify-content=false` 或 `--no-verify-content` 关闭 |

## Legado DSL

规则字符串是以 `@` 分隔的管道：`class.item@tag.li.0@text` 表示 "查找 .item，在其中找 li，取第 1 个，提取文本"。详见 `rule_dsl/parser.rs:RuleToken`。
