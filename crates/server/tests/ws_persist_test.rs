//! 断连续跑集成测试：WS 断开后运行中的任务继续执行并完成。
//!
//! 用 MockProvider（固定回复让 agent 调用 write 工具）驱动 workflow，
//! 起任务后立即断开 WS 连接，通过 REST 轮询确认任务仍跑到 completed
//! 且交付物落盘。

use std::sync::Arc;

// 通过直接构造 AppState + Router 的内部路径测试（server crate 需要导出）
// —— 简化：起真 server（随机端口）+ 真 WS 客户端
