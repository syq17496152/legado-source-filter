# AGENTS.md

本项目上下文与架构说明见 [`CLAUDE.md`](./CLAUDE.md)（构建/测试命令、管道流程、架构图、DSL 说明均在那里维护）。

快速上手：

```bash
cargo build                    # 编译
cargo test                     # 全部测试（110 单测 + 22 DSL 差分 + 6 集成）
cargo run -- full --rounds 5   # 一键预检 + 5 轮测试
cargo run -- status            # 查看进度
```

关键约定：书源从 legado.aoaostar.com 自动获取；`passed` 表示完整阅读链路（搜索→详情→目录→正文）可读；结果缓存于 SQLite（`output/test_cache.db`），默认断点续测。