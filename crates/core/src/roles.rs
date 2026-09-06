//! 自定义智能体角色（Agent Role Profile）档案。
//!
//! 一条 `AgentRoleProfile` 描述一个可被规划器（Plan 阶段）指派、可被调度器
//! （Dispatch 阶段）执行的智能体：角色名、角色 Persona（system prompt）、
//! 配套的工具白名单、配套的技能列表。
//!
//! 持久化：工作区根目录 `agents.json`（与 `models.json` 同层，已 gitignore）。
//! 读取方有三处，全部按需 load 文件（文件很小，无热点）：
//! - server `/api/agents` CRUD；
//! - loop-pipeline Plan 阶段（把角色目录注入规划提示，LLM 可自主选择把
//!   子任务 `assigned_role` 设为某个自定义角色）；
//! - loop-pipeline Dispatch 阶段（按角色档案构建 system prompt / 工具白名单 /
//!   注入配套技能正文）。
//!
//! 内置角色（researcher/executor/...）不落盘：它们的工具表与提示词由
//! `loop_pipeline::prompts` 的静态表提供；本模块的 [`BUILTIN_ROLE_KEYS`]
//! 只用于防止自定义角色覆盖内置角色键。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 内置角色键（与 `loop_pipeline::prompts::tools_for_role` 的静态表对齐）。
/// 自定义角色不得使用这些键（防止覆盖内置语义）。
pub const BUILTIN_ROLE_KEYS: &[&str] = &[
    "researcher",
    "explorer",
    "executor",
    "writer",
    "critic",
    "synthesizer",
    "analyst",
];

/// 一条自定义智能体角色档案。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRoleProfile {
    /// 稳定 id（uuid 简写）；编辑/删除按此定位。
    pub id: String,
    /// 展示名（如「气象数据专员」）。
    pub name: String,
    /// 角色键：Plan 阶段 `assigned_role` 填的值。slug 化，唯一，不得撞内置键。
    pub role_key: String,
    /// 一句话职责描述（供规划器判断何时指派该角色）。
    #[serde(default)]
    pub description: String,
    /// 角色 Persona：该角色执行子任务时的 system prompt 附加段。
    #[serde(default)]
    pub system_prompt: String,
    /// 工具白名单（空 = 全部内置工具，与 RunContext 语义一致）。
    #[serde(default)]
    pub tools: Vec<String>,
    /// 配套技能名（SKILL.md 的 name 字段）；执行时注入完整技能正文。
    #[serde(default)]
    pub skills: Vec<String>,
    /// 图标（前端展示用）。
    #[serde(default = "default_icon")]
    pub icon: String,
    #[serde(default)]
    pub created_at: String,
}

fn default_icon() -> String {
    "🤖".into()
}

impl AgentRoleProfile {
    /// 校验档案必填字段。返回 Err(人类可读原因)。
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("角色名不能为空".into());
        }
        if self.role_key.trim().is_empty() {
            return Err("角色键（role_key）不能为空".into());
        }
        if !self.role_key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!(
                "角色键只能包含字母、数字、-、_：{}",
                self.role_key
            ));
        }
        if BUILTIN_ROLE_KEYS.contains(&self.role_key.as_str()) {
            return Err(format!(
                "角色键 {} 与内置角色冲突，请换一个",
                self.role_key
            ));
        }
        Ok(())
    }
}

/// slug 化角色键：非 [A-Za-z0-9-_] 的字符折叠为 '-'，小写，去首尾 '-'。
/// 中文角色名会被整体折叠——此时回退到 "agent" 前缀（调用方可再拼 id）。
pub fn slugify_role_key(input: &str) -> String {
    let mut out = String::new();
    for c in input.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        format!("agent-{}", &uuid::Uuid::new_v4().simple().to_string()[..6])
    } else {
        trimmed
    }
}

/// 自定义角色档案存取（agents.json，非锁文件读写——CRUD 频率极低）。
#[derive(Debug, Default, Serialize, Deserialize)]
struct RoleFile {
    #[serde(default)]
    roles: Vec<AgentRoleProfile>,
}

#[derive(Debug, Default)]
pub struct AgentRoleStore {
    path: PathBuf,
    roles: Vec<AgentRoleProfile>,
}

impl AgentRoleStore {
    /// 从工作区根 `agents.json` 加载；文件缺失/损坏时返回空 store
    /// （损坏打 error 日志，不中断——角色目录是增强能力，不是硬依赖）。
    pub fn load() -> Self {
        let path = crate::paths::agents_file();
        let roles = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| match serde_json::from_str::<RoleFile>(&s) {
                Ok(f) => Some(f.roles),
                Err(e) => {
                    tracing::error!(
                        path = %path.display(),
                        error = %e,
                        "agents.json failed to parse — custom agent roles ignored; fix the file to restore them"
                    );
                    None
                }
            })
            .unwrap_or_default();
        Self { path, roles }
    }

    pub fn roles(&self) -> &[AgentRoleProfile] {
        &self.roles
    }

    pub fn get(&self, id: &str) -> Option<&AgentRoleProfile> {
        self.roles.iter().find(|r| r.id == id)
    }

    /// 按角色键查档案（Plan/Dispatch 用 assigned_role 命中）。
    pub fn get_by_key(&self, role_key: &str) -> Option<&AgentRoleProfile> {
        self.roles.iter().find(|r| r.role_key == role_key)
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = RoleFile {
            roles: self.roles.clone(),
        };
        std::fs::write(&self.path, serde_json::to_string_pretty(&file).unwrap_or_default())
    }

    /// 新增角色。role_key 冲突（内置或已有自定义）时自动加后缀去重。
    pub fn add(&mut self, mut profile: AgentRoleProfile) -> AgentRoleProfile {
        if profile.id.trim().is_empty() {
            profile.id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        }
        if profile.created_at.is_empty() {
            profile.created_at = chrono::Utc::now().to_rfc3339();
        }
        // role_key 唯一化：内置键冲突直接拒绝（validate 已挡）；自定义冲突加 -2/-3 后缀。
        let base = profile.role_key.clone();
        let mut candidate = base.clone();
        let mut n = 2;
        while self.roles.iter().any(|r| r.role_key == candidate) {
            candidate = format!("{base}-{n}");
            n += 1;
        }
        profile.role_key = candidate;
        self.roles.push(profile.clone());
        let _ = self.save();
        profile
    }

    /// 全量替换更新（按 id 定位）。
    pub fn update(&mut self, id: &str, patch: AgentRoleProfile) -> Result<(), String> {
        if !self.roles.iter().any(|r| r.id == id) {
            return Err(format!("角色 {id} 不存在"));
        }
        // role_key 不允许改撞其它角色。
        if self
            .roles
            .iter()
            .any(|r| r.id != id && r.role_key == patch.role_key)
        {
            return Err(format!("角色键 {} 已被其它角色使用", patch.role_key));
        }
        let slot = self.roles.iter_mut().find(|r| r.id == id).expect("checked above");
        *slot = patch;
        self.save().map_err(|e| e.to_string())
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let before = self.roles.len();
        self.roles.retain(|r| r.id != id);
        if self.roles.len() == before {
            return Err(format!("角色 {id} 不存在"));
        }
        self.save().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(key: &str) -> AgentRoleProfile {
        AgentRoleProfile {
            id: String::new(),
            name: format!("角色 {key}"),
            role_key: key.into(),
            description: "测试角色".into(),
            system_prompt: "你是测试专员。".into(),
            tools: vec!["read".into(), "bash".into()],
            skills: vec![],
            icon: "🧪".into(),
            created_at: String::new(),
        }
    }

    #[test]
    fn slugify_ascii() {
        assert_eq!(slugify_role_key("Data Analyst"), "data-analyst");
        assert_eq!(slugify_role_key("Weather_API-2"), "weather_api-2");
    }

    #[test]
    fn slugify_cjk_falls_back() {
        let slug = slugify_role_key("气象专员");
        assert!(slug.starts_with("agent-"), "got {slug}");
    }

    #[test]
    fn validate_rejects_builtin_key() {
        let mut p = profile("researcher");
        assert!(p.validate().is_err());
        p.role_key = "my-researcher".into();
        assert!(p.validate().is_ok());
    }

    #[test]
    fn store_add_dedupes_role_key() {
        let mut store = AgentRoleStore::default();
        let a = store.add(profile("analyst-pro"));
        let b = store.add(profile("analyst-pro"));
        assert_eq!(a.role_key, "analyst-pro");
        assert_eq!(b.role_key, "analyst-pro-2");
    }

    #[test]
    fn store_get_by_key() {
        let mut store = AgentRoleStore::default();
        store.add(profile("weather-agent"));
        assert!(store.get_by_key("weather-agent").is_some());
        assert!(store.get_by_key("nope").is_none());
    }
}
