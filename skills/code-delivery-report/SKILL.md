---
name: code-delivery-report
description: >
  Code/script delivery documentation format: purpose, usage, API, inputs/
  outputs, dependencies, verification results. Use for 代码脚本交付说明/
  script documentation. Pairs with executed .py files in the task dir.
triggers:
  - 代码说明
  - script documentation
  - 代码交付
  - README for script
follow_ups:
  - bioinf-verify-report
tools_needed: []
version: "1.0.0"
priority: 8
---

# 代码/脚本交付文档格式

## 结构（固定章节）

1. **用途**：一段话说清脚本解决什么问题、何时使用。
2. **运行方式**：精确命令行（解释器版本、依赖、参数），
   如 `python3 fib.py --n 40`（依赖：numpy≥1.24）。
3. **输入/输出**：输入文件/参数表；输出文件与格式（含样例行）。
4. **验证结果**：实际运行的输出摘录（真实粘贴，不编造），
   含关键数值与退出码。
5. **实现要点**：核心函数/算法一句话说明（带函数名定位）。
6. **已知限制**：边界条件、性能上限、未处理输入。

## 硬性规则

- 运行方式与验证结果必须来自真实执行（bash 工具回显），禁止想象输出。
- 文件名/函数名/参数名必须与实际代码一致（核验时会 grep 代码比对）。
- 依赖列出实际 import 的第三方包与最低版本。
