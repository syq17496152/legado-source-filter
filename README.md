# Legado 书源筛选工具

自动从 [legado.aoaostar.com](https://legado.aoaostar.com) 获取最新书源列表，通过预检过滤和并发搜索测试，筛选出**当前可用的文字书源**，直接导入 [Legado](https://github.com/gedoor/legado) 阅读 APP。

## 快速开始

```bash
# 一行命令：预检 + 5 轮重试测试（全自动，约 5-15 分钟）
cargo run -- full --rounds 5

# 查看当前管道进度
cargo run -- status
```

## 命令详解

### `full` — 一键完成

从下载书源 → 预检 → 多轮测试 → 输出结果，一站式执行。推荐使用。

```bash
# 基本用法
cargo run -- full --rounds 5

# 快速测试前 10 个源
cargo run -- full --limit 10 --rounds 3

# 强制从头重测（清除缓存；默认会断点续测，已测源自动跳过）
cargo run -- full --force --rounds 5
```

### `status` — 查看进度

显示预检和测试的当前完成情况。

```bash
cargo run -- status
```

## 全部参数

| 参数 | 适用命令 | 说明 |
|------|---------|------|
| `-c`, `--concurrency N` | full | 并发数，默认 50 |
| `-t`, `--timeout N` | full | 单请求超时（秒），默认 15 |
| `--rounds N` | full | 测试轮数，失败源每轮重试，默认 1 |
| `--limit N` | full | 只测前 N 个源 |
| `--force` | full | 清除缓存，全部从头重新测试；默认保留缓存断点续测 |
| `--config PATH` | full | 指定 config.toml 路径（可选） |
| `--verify-content[=false]` | full | 完整阅读链路验证（默认开启）；传 `=false` 关闭 |
| `--no-verify-content` | full | 关闭阅读链路验证，仅验证搜索能力 |
| `--json` | 全局 | 输出 JSON Lines 到 stdout，适合 AI 解析 |
| `-o`, `--output DIR` | 全局 | 输出目录（默认 XDG_CACHE_HOME） |

## 配置文件

`--config PATH` 可指定自定义 TOML 配置文件。未指定时，工具按以下顺序查找：

1. 项目根目录 `data/config.toml`
2. 内置默认配置

配置文件主要用于自定义搜索测试的关键词，格式如下：

```toml
[[search]]
name = "玄鉴仙族"
author = "季越人"
# 起点中文网 - 2026年5月畅销榜第1

[[search]]
name = "修真界第一营销咖"
author = "云霄桂月"
# 晋江文学城 - 2026年十大热门小说第1

[[search]]
name = "无敌天命"
author = "青鸾峰上"
# 纵横中文网 - 2026年4月月票榜第1

[[search]]
name = "罪恶之城"
author = "烟雨江南"
# 17k小说网 - 推荐榜第1（1099万推荐）

[[search]]
name = "我不是戏神"
author = "三九音域"
# 番茄小说 - 2026年巅峰榜第1

[[search]]
name = "三国帝皇之万界征战"
author = "无量功德"
# 飞卢中文网 - 霸占天榜两年以上

[[search]]
name = "我真不是邪神走狗"
author = "万劫火"
# 刺猬猫 - 均订5.4万，全站原创首订记录

[[search]]
name = "大荒吞天诀"
author = "铁马飞桥"
# 七猫免费小说 - 310万在读，玄幻顶流
```

每个 `[[search]]` 条目指定一本书名和作者，测试时会被优先用作搜索关键词，覆盖通用的默认关键词。`#` 为注释行，仅用于说明，不影响运行。

## 输出文件

输出目录默认位于 `~/.cache/legado-source-filter/`（可通过 `--output` 或 `XDG_CACHE_HOME` 环境变量修改）。

| 文件 | 说明 |
|------|------|
| `filtered.json` | **完整阅读链路验证通过**的可用书源，直接导入 Legado |
| `explore_only.json` | 仅支持「发现」功能、不支持搜索的书源 |
| `test_cache.db` | SQLite 缓存 + 运行时数据（eligible、report） |

## 验证深度：搜索 vs 完整阅读链路

默认（`--verify-content` 默认开启）下，书源必须通过**完整阅读链路验证**才会进入 `filtered.json`：

```
搜索(ruleSearch) → 书籍详情页(ruleBookInfo) → 目录页(ruleToc) → 第一章正文(ruleContent)
```

每个环节都会真实发请求并解析，任何一环失败（如目录规则失效、正文为空、站点 403/404/反爬）都记为 `failed`，带具体原因：

| 失败原因 | 含义 |
|----------|------|
| `bookinfo_fetch` / `toc_fetch` / `chapter_fetch` | 对应页面请求失败（瞬时，后续轮次会重试） |
| `toc_parse` | 目录规则提取不到章节链接（规则已失效） |
| `content_short` | 正文过短（<50 字符），疑似解析错误 |
| `blocked` | HTTP 403/401，站点反爬 |
| `not_found` | HTTP 404 |
| `rate_limited` | HTTP 408/429 限流 |
| `no_results` | 搜索无结果（可能站内无此书或规则失效） |

JS 规则里的 `java.ajax(url)` / `java.post(url)` 会通过 **node↔Rust 交互桥**真实发
HTTP 请求（携带 source.header + Referer），响应体回传 JS 继续求值——正文需要二次请求
的源（解密、取 token、分页接口等）也能被正确验证。桥的请求不跟随重定向，支持
`java.post(...).header("Location")` 读取 302 响应头。

JS 规则支持 Legado 的多种惯用写法，均能正确构建请求：
- **裸表达式结尾**：`@js:...\n"https://api/"+path` 之类的规则，以最后一条表达式的值作为结果
- **POST 约定**：`url,{"key":...}` 或 JS 里的 `java.post(url, body, headers)` 会真实发 POST，
  body 原样发送（不做 url-encoded）
- **JSON-object body**：`{"keyword":{{key}}}` 形式的 POST body 能正确提取
- **`{{$.field}}` 占位符**：详情页返回的字段（含 `_id`/`total_num` 等非固定字段）会回填到
  目录 URL 模板中

`--no-verify-content` 可回退到旧的「仅验证搜索」模式。

## 技术栈

- **语言**: Rust（tokio + reqwest）
- **HTML 解析**: scraper（Servo CSS 引擎）
- **编码探测**: chardetng + encoding_rs
- **JS 执行**: Node.js polyfill（可选依赖）；`java.ajax` 经 stdin/stdout 桥回调 Rust 真实发 HTTP
- **缓存**: rusqlite（SQLite，支持断点续测）
- **阅读链路验证**: content_check 模块，真实走 搜索→详情→目录→正文 四跳
