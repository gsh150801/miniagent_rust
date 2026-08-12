//! Knowledge Graph & Hypothesis 工具封装。
//!
//! 把原本仅在 CLI `research_pipeline` 中命令式调用的 KG/Hypothesis 能力
//! 暴露为标准 `Tool`，让 Agent 主循环可以主动查询/扩展知识图谱、生成假设。
//!
//! # 状态共享
//! `KgHandle` 用 `Arc<tokio::sync::Mutex<KnowledgeGraph>>` 在同一 Agent session 内累积。
//! 三个工具共享同一个 handle：
//! - [`KgQueryTool`]（只读）：查实体/关系/邻居/路径
//! - [`KgAddTool`]（mutating）：注入新实体/关系
//! - [`HypothesisSuggestTool`]：基于 link prediction + LLM 生成假设

use std::sync::Arc;

use async_trait::async_trait;
use miniagent_core::error::AgentError;
use miniagent_hypothesis::{HypothesisGenerator, HypothesisRanker};
use miniagent_kg::link_prediction::LinkPredictionScorer;
use miniagent_kg::schema::{Entity, EntityId, EntityType, Relation, RelationId, RelationType};
use miniagent_kg::KnowledgeGraph;
use miniagent_provider::traits::{
    CompletionRequest, CompletionResponse, LlmProvider, StreamResponse,
};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::traits::{Tool, ToolClass, ToolContext, ToolOutput};

/// 适配器：把 `Arc<dyn LlmProvider>` 包装成可传给 `HypothesisGenerator::with_provider`
/// 的 `Box<dyn LlmProvider>`。`HypothesisGenerator` 按值持有 provider，
/// 而我们想跨多次候选生成复用同一个底层 provider，故通过 Arc 共享。
struct ArcProvider {
    inner: Arc<dyn LlmProvider>,
}

#[async_trait]
impl LlmProvider for ArcProvider {
    async fn complete(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<CompletionResponse, AgentError> {
        self.inner.complete(request, cancel).await
    }

    async fn stream(
        &self,
        request: &CompletionRequest,
        cancel: CancellationToken,
    ) -> Result<StreamResponse, AgentError> {
        self.inner.stream(request, cancel).await
    }
}

/// 进程内共享的 KG 句柄。所有 KG 相关工具通过 `clone()` 获得同一份图谱。
///
/// 使用 `tokio::sync::RwLock`：KG 查询（query/neighborhood/paths/suggest）是只读的，
/// 用读锁——多个并发查询不互斥。只有 `kg_add` 写图谱时用写锁。
///
/// 关键修复：原用 `Mutex`，`HypothesisSuggestTool` 持锁期间 `await` LLM（数秒级），
/// 串行化所有 KG 访问。改 RwLock 后，多个假设生成可并发进行（共享读锁）。
#[derive(Clone)]
pub struct KgHandle {
    graph: Arc<RwLock<KnowledgeGraph>>,
}

impl KgHandle {
    pub fn new() -> Self {
        Self {
            graph: Arc::new(RwLock::new(KnowledgeGraph::new())),
        }
    }

    /// 用预填充的 KG 构造（如 research pipeline 已抽取后注入给 Agent）。
    pub fn from_graph(graph: KnowledgeGraph) -> Self {
        Self {
            graph: Arc::new(RwLock::new(graph)),
        }
    }

    pub fn graph(&self) -> &Arc<RwLock<KnowledgeGraph>> {
        &self.graph
    }
}

impl Default for KgHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// 获取 KG 读锁。用于 query/neighborhood/paths/suggest 等只读操作。
/// 读锁之间不互斥——多个并发查询可同时持有。返回 RwLockReadGuard（Send，可跨 await）。
async fn read_graph(
    handle: &KgHandle,
) -> Result<tokio::sync::RwLockReadGuard<'_, KnowledgeGraph>, AgentError> {
    Ok(handle.graph.read().await)
}

/// 获取 KG 写锁。仅用于 kg_add 等修改图谱的操作。
/// 写锁与所有读锁互斥——写操作会等待所有读者释放。
async fn write_graph(
    handle: &KgHandle,
) -> Result<tokio::sync::RwLockWriteGuard<'_, KnowledgeGraph>, AgentError> {
    Ok(handle.graph.write().await)
}

fn entity_to_json(e: &Entity) -> Value {
    json!({
        "id": e.id.0.to_string(),
        "name": e.name,
        "type": format!("{:?}", e.entity_type),
        "aliases": e.aliases,
        "metadata": e.metadata,
    })
}

// ── kg_query ────────────────────────────────────────────────────

pub struct KgQueryTool {
    handle: KgHandle,
}

impl KgQueryTool {
    pub fn new(handle: KgHandle) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl Tool for KgQueryTool {
    fn name(&self) -> &str {
        "kg_query"
    }
    fn description(&self) -> &str {
        "Query the shared Knowledge Graph. Look up an entity by name, list its 1-hop \
         neighborhood, find paths between two entities, or dump graph stats. \
         Returns JSON with entities/relations. Use this to ground claims in prior extracted \
         knowledge instead of guessing."
    }
    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["lookup", "neighborhood", "paths", "stats"],
                    "description": "lookup=按名称查实体; neighborhood=某实体的一跳邻居; \
                                    paths=两实体间的路径; stats=图谱规模统计"
                },
                "entity": {"type": "string", "description": "实体名称 (lookup/neighborhood 必填)"},
                "from": {"type": "string", "description": "起点实体名 (paths 必填)"},
                "to": {"type": "string", "description": "终点实体名 (paths 必填)"},
                "max_depth": {"type": "integer", "description": "路径搜索最大深度 (默认 3)"}
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let action = input["action"]
            .as_str()
            .ok_or_else(|| AgentError::tool("kg_query", "missing 'action'"))?;

        let kg = read_graph(&self.handle).await?;

        match action {
            "lookup" => {
                let name = input["entity"]
                    .as_str()
                    .ok_or_else(|| AgentError::tool("kg_query", "missing 'entity' for lookup"))?;
                match kg.find_entity_by_name(name) {
                    Some(e) => {
                        let body = entity_to_json(e);
                        Ok(ToolOutput {
                            content: serde_json::to_string_pretty(&body)
                                .map_err(|e| AgentError::tool("kg_query", e.to_string()))?,
                            metadata: None,
                        })
                    }
                    None => Ok(ToolOutput {
                        content: format!("(no entity named '{name}')"),
                        metadata: None,
                    }),
                }
            }
            "neighborhood" => {
                let name = input["entity"].as_str().ok_or_else(|| {
                    AgentError::tool("kg_query", "missing 'entity' for neighborhood")
                })?;
                let entity = kg.find_entity_by_name(name).ok_or_else(|| {
                    AgentError::tool("kg_query", format!("entity '{name}' not found"))
                })?;
                let neighbors: Vec<Value> = kg
                    .neighborhood(&entity.id)
                    .into_iter()
                    .map(|(rt, target_id, conf)| {
                        let target_name = kg
                            .get_entity(&target_id)
                            .map(|e| e.name.clone())
                            .unwrap_or_else(|| target_id.0.to_string());
                        json!({
                            "relation": format!("{:?}", rt),
                            "target": target_name,
                            "confidence": conf,
                        })
                    })
                    .collect();
                let body = json!({ "entity": entity.name, "neighbors": neighbors });
                Ok(ToolOutput {
                    content: serde_json::to_string_pretty(&body)
                        .map_err(|e| AgentError::tool("kg_query", e.to_string()))?,
                    metadata: None,
                })
            }
            "paths" => {
                let from_name = input["from"]
                    .as_str()
                    .ok_or_else(|| AgentError::tool("kg_query", "missing 'from' for paths"))?;
                let to_name = input["to"]
                    .as_str()
                    .ok_or_else(|| AgentError::tool("kg_query", "missing 'to' for paths"))?;
                let max_depth = input["max_depth"].as_u64().unwrap_or(3) as usize;
                let from = kg.find_entity_by_name(from_name).ok_or_else(|| {
                    AgentError::tool("kg_query", format!("entity '{from_name}' not found"))
                })?;
                let to = kg.find_entity_by_name(to_name).ok_or_else(|| {
                    AgentError::tool("kg_query", format!("entity '{to_name}' not found"))
                })?;
                let paths = kg.find_paths(&from.id, &to.id, max_depth);
                let rendered: Vec<Value> = paths
                    .into_iter()
                    .map(|path| {
                        let steps: Vec<Value> = path
                            .into_iter()
                            .map(|(h, rt, t)| {
                                let h_name = kg
                                    .get_entity(&h)
                                    .map(|e| e.name.clone())
                                    .unwrap_or_else(|| h.0.to_string());
                                let t_name = kg
                                    .get_entity(&t)
                                    .map(|e| e.name.clone())
                                    .unwrap_or_else(|| t.0.to_string());
                                json!({"from": h_name, "relation": format!("{:?}", rt), "to": t_name})
                            })
                            .collect();
                        json!({ "steps": steps })
                    })
                    .collect();
                let body = json!({ "from": from_name, "to": to_name, "paths": rendered });
                Ok(ToolOutput {
                    content: serde_json::to_string_pretty(&body)
                        .map_err(|e| AgentError::tool("kg_query", e.to_string()))?,
                    metadata: None,
                })
            }
            "stats" => {
                let body = json!({
                    "entities": kg.entity_count(),
                    "relations": kg.relation_count(),
                });
                Ok(ToolOutput {
                    content: serde_json::to_string_pretty(&body)
                        .map_err(|e| AgentError::tool("kg_query", e.to_string()))?,
                    metadata: None,
                })
            }
            other => Err(AgentError::tool(
                "kg_query",
                format!("unknown action '{other}'"),
            )),
        }
    }
}

// ── kg_add ──────────────────────────────────────────────────────

pub struct KgAddTool {
    handle: KgHandle,
}

impl KgAddTool {
    pub fn new(handle: KgHandle) -> Self {
        Self { handle }
    }
}

fn parse_entity_type(s: &str) -> EntityType {
    // 宽松解析：未知类型归入 Concept，避免 LLM 提供无效变体时报错。
    // 与 EntityType enum 变体名（PascalCase）显式映射。
    match s.to_lowercase().as_str() {
        "gene" => EntityType::Gene,
        "protein" => EntityType::Protein,
        "pathway" => EntityType::Pathway,
        "disease" => EntityType::Disease,
        "phenotype" => EntityType::Phenotype,
        "cellline" | "cell_line" => EntityType::CellLine,
        "drug" => EntityType::Drug,
        "compound" | "chemical" => EntityType::Compound,
        "method" | "technique" => EntityType::Method,
        "assay" => EntityType::Assay,
        "model" => EntityType::Model,
        "hypothesis" => EntityType::Hypothesis,
        "theory" => EntityType::Theory,
        "mechanism" => EntityType::Mechanism,
        "biomarker" => EntityType::Biomarker,
        "paper" => EntityType::Paper,
        "author" => EntityType::Author,
        "institution" => EntityType::Institution,
        "dataset" => EntityType::Dataset,
        "metric" => EntityType::Metric,
        _ => EntityType::Concept,
    }
}

#[async_trait]
impl Tool for KgAddTool {
    fn name(&self) -> &str {
        "kg_add"
    }
    fn description(&self) -> &str {
        "Add entities and relations to the shared Knowledge Graph. Use after extracting \
         facts from a paper or reasoning so later queries (kg_query / hypothesis_suggest) \
         can ground in them. Idempotent by entity name."
    }
    fn class(&self) -> ToolClass {
        ToolClass::Mutating
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "entities": {
                    "type": "array",
                    "description": "要新增的实体列表",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "type": {"type": "string", "description": "gene/protein/disease/drug/chemical/pathway/concept 等"},
                            "aliases": {"type": "array", "items": {"type": "string"}}
                        },
                        "required": ["name"]
                    }
                },
                "relations": {
                    "type": "array",
                    "description": "要新增的关系列表 (实体名引用)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "from": {"type": "string", "description": "起点实体名"},
                            "relation": {"type": "string", "description": "关系类型，如 treats, regulates, interacts_with"},
                            "to": {"type": "string", "description": "终点实体名"},
                            "confidence": {"type": "number", "description": "0.0-1.0，默认 1.0"},
                            "evidence": {"type": "string", "description": "支持证据/来源"}
                        },
                        "required": ["from", "relation", "to"]
                    }
                }
            }
        })
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let mut kg = write_graph(&self.handle).await?;

        let mut added_entities = 0usize;
        let mut skipped_existing = 0usize;
        let mut name_to_id: std::collections::HashMap<String, EntityId> =
            std::collections::HashMap::new();

        if let Some(entities) = input["entities"].as_array() {
            for e in entities {
                let name = e["name"]
                    .as_str()
                    .ok_or_else(|| AgentError::tool("kg_add", "entity missing 'name'"))?;
                // 幂等：同名实体跳过（不覆盖已有）
                if let Some(existing) = kg.find_entity_by_name(name) {
                    name_to_id.insert(name.to_lowercase(), existing.id);
                    skipped_existing += 1;
                    continue;
                }
                let etype = e["type"]
                    .as_str()
                    .map(parse_entity_type)
                    .unwrap_or(EntityType::Concept);
                let aliases = e["aliases"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let entity = Entity {
                    id: EntityId::new(),
                    name: name.to_string(),
                    entity_type: etype,
                    aliases,
                    metadata: json!({}),
                };
                name_to_id.insert(name.to_lowercase(), entity.id);
                kg.add_entity(entity);
                added_entities += 1;
            }
        }

        let mut added_relations = 0usize;
        if let Some(relations) = input["relations"].as_array() {
            for r in relations {
                let from_name = r["from"]
                    .as_str()
                    .ok_or_else(|| AgentError::tool("kg_add", "relation missing 'from'"))?;
                let to_name = r["to"]
                    .as_str()
                    .ok_or_else(|| AgentError::tool("kg_add", "relation missing 'to'"))?;
                let rel_str = r["relation"].as_str().ok_or_else(|| {
                    AgentError::tool("kg_add", "relation missing 'relation'")
                })?;

                let from_id = match kg.find_entity_by_name(from_name) {
                    Some(e) => e.id,
                    None => match name_to_id.get(&from_name.to_lowercase()) {
                        Some(id) => *id,
                        None => {
                            return Err(AgentError::tool(
                                "kg_add",
                                format!("relation references unknown entity '{from_name}'"),
                            ))
                        }
                    },
                };
                let to_id = match kg.find_entity_by_name(to_name) {
                    Some(e) => e.id,
                    None => match name_to_id.get(&to_name.to_lowercase()) {
                        Some(id) => *id,
                        None => {
                            return Err(AgentError::tool(
                                "kg_add",
                                format!("relation references unknown entity '{to_name}'"),
                            ))
                        }
                    },
                };
                let rel_type = RelationType::parse(rel_str).ok_or_else(|| {
                    AgentError::tool("kg_add", format!("unknown relation type '{rel_str}'"))
                })?;
                let confidence = r["confidence"].as_f64().unwrap_or(1.0);
                let evidence = r["evidence"].as_str().unwrap_or("").to_string();

                kg.add_relation(Relation {
                    id: RelationId::new(),
                    from_id,
                    to_id,
                    relation_type: rel_type,
                    confidence,
                    evidence,
                    source_paper_id: None,
                });
                added_relations += 1;
            }
        }

        let body = json!({
            "added_entities": added_entities,
            "skipped_existing_entities": skipped_existing,
            "added_relations": added_relations,
            "total_entities": kg.entity_count(),
            "total_relations": kg.relation_count(),
        });
        Ok(ToolOutput {
            content: serde_json::to_string_pretty(&body)
                .map_err(|e| AgentError::tool("kg_add", e.to_string()))?,
            metadata: None,
        })
    }
}

// ── hypothesis_suggest ──────────────────────────────────────────

pub struct HypothesisSuggestTool {
    handle: KgHandle,
    provider: Option<Arc<dyn LlmProvider>>,
}

impl HypothesisSuggestTool {
    /// 不带 provider：仅做 link prediction，不调用 LLM 生成完整假设。
    pub fn new(handle: KgHandle) -> Self {
        Self {
            handle,
            provider: None,
        }
    }

    /// 带 provider：link prediction 后用 LLM 生成可读假设与实验设计。
    pub fn with_provider(handle: KgHandle, provider: Arc<dyn LlmProvider>) -> Self {
        Self {
            handle,
            provider: Some(provider),
        }
    }
}

#[async_trait]
impl Tool for HypothesisSuggestTool {
    fn name(&self) -> &str {
        "hypothesis_suggest"
    }
    fn description(&self) -> &str {
        "Suggest scientific hypotheses by predicting missing links in the Knowledge Graph. \
         Given a head entity and a relation type, ranks candidate tails by structural \
         evidence. If a Pro provider is configured, also generates a natural-language \
         hypothesis with mechanism and experimental design. Returns ranked candidates \
         as JSON."
    }
    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "head": {"type": "string", "description": "起点实体名"},
                "relation": {"type": "string", "description": "关系类型，如 treats, regulates"},
                "max_results": {"type": "integer", "description": "返回候选数 (默认 5, 上限 10)"}
            },
            "required": ["head", "relation"]
        })
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, AgentError> {
        let head_name = input["head"]
            .as_str()
            .ok_or_else(|| AgentError::tool("hypothesis_suggest", "missing 'head'"))?;
        let rel_str = input["relation"]
            .as_str()
            .ok_or_else(|| AgentError::tool("hypothesis_suggest", "missing 'relation'"))?;
        let max_results = input["max_results"].as_u64().unwrap_or(5).min(10) as usize;
        let rel_type = RelationType::parse(rel_str).ok_or_else(|| {
            AgentError::tool(
                "hypothesis_suggest",
                format!("unknown relation type '{rel_str}'"),
            )
        })?;

        // 1. link prediction（无 KGE 模型，仅 path-based scoring，速度快）
        let scorer = LinkPredictionScorer::new();
        let candidates = {
            let kg = read_graph(&self.handle).await?;
            let head = kg.find_entity_by_name(head_name).ok_or_else(|| {
                AgentError::tool(
                    "hypothesis_suggest",
                    format!("head entity '{head_name}' not found in KG"),
                )
            })?;
            scorer.predict_tails(&head.id, &rel_type, &kg, max_results)
        };

        if candidates.is_empty() {
            return Ok(ToolOutput {
                content: json!({
                    "head": head_name,
                    "relation": rel_str,
                    "candidates": [],
                    "note": "no structurally-supported candidates found; \
                             consider adding more entities/relations via kg_add"
                })
                .to_string(),
                metadata: None,
            });
        }

        // 2. 若配置了 provider，用 LLM 为每个候选生成完整假设
        let mut ranked_out: Vec<Value> = Vec::new();
        if let Some(provider) = &self.provider {
            let mut hypotheses = Vec::new();
            for c in &candidates {
                let kg = read_graph(&self.handle).await?;
                let generator = HypothesisGenerator::new()
                    .with_provider(Box::new(ArcProvider { inner: provider.clone() }));
                match generator.generate(c, &kg, cancel.clone()).await {
                    Ok(h) => hypotheses.push(h),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            head = head_name,
                            "hypothesis generation failed for a candidate; skipping"
                        );
                    }
                }
                if hypotheses.len() >= max_results {
                    break;
                }
            }
            let ranked = HypothesisRanker::rank(&hypotheses);
            for rh in ranked {
                let h = &rh.hypothesis;
                let head_n = {
                    let kg = read_graph(&self.handle).await?;
                    kg.get_entity(&h.source_candidate.head)
                        .map(|e| e.name.clone())
                        .unwrap_or_default()
                };
                let tail_n = {
                    let kg = read_graph(&self.handle).await?;
                    kg.get_entity(&h.source_candidate.tail)
                        .map(|e| e.name.clone())
                        .unwrap_or_default()
                };
                ranked_out.push(json!({
                    "head": head_n,
                    "tail": tail_n,
                    "statement": h.statement,
                    "mechanism": h.mechanism,
                    "novelty": format!("{:?}", h.novelty),
                    "confidence": h.confidence,
                    "supporting_evidence": h.supporting_evidence,
                    "counter_evidence": h.counter_evidence,
                    "experimental_design": h.experimental_design.as_ref().map(|d| json!({
                        "approach": d.approach,
                        "methods": d.methods,
                        "expected_outcomes": d.expected_outcomes,
                        "controls": d.controls,
                        "feasibility": d.feasibility,
                    })),
                    "composite_score": rh.composite_score,
                    "score_breakdown": {
                        "kge": rh.breakdown.kge,
                        "llm_confidence": rh.breakdown.llm_confidence,
                        "novelty": rh.breakdown.novelty,
                        "feasibility": rh.breakdown.feasibility,
                    },
                }));
            }
        } else {
            // 无 provider：仅返回结构化候选
            for c in &candidates {
                let (head_n, tail_n) = {
                    let kg = read_graph(&self.handle).await?;
                    let h = kg
                        .get_entity(&c.head)
                        .map(|e| e.name.clone())
                        .unwrap_or_default();
                    let t = kg
                        .get_entity(&c.tail)
                        .map(|e| e.name.clone())
                        .unwrap_or_default();
                    (h, t)
                };
                ranked_out.push(json!({
                    "head": head_n,
                    "tail": tail_n,
                    "relation": rel_str,
                    "score": c.score,
                    "kge_score": c.evidence.kge_score,
                    "path_score": c.evidence.path_score,
                    "novelty": format!("{:?}", c.evidence.novelty),
                    "note": "LLM provider not configured; only structural candidate returned"
                }));
            }
        }

        let body = json!({
            "head": head_name,
            "relation": rel_str,
            "candidates": ranked_out,
        });
        Ok(ToolOutput {
            content: serde_json::to_string_pretty(&body)
                .map_err(|e| AgentError::tool("hypothesis_suggest", e.to_string()))?,
            metadata: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_handle_with_data() -> KgHandle {
        // 先在普通 KG 上同步填充数据，再 wrap 进 KgHandle，
        // 避免 tokio Mutex 在 runtime 内部 blocking_lock（会 panic）。
        let mut kg = KnowledgeGraph::new();
        kg.add_entity(Entity {
            id: EntityId::new(),
            name: "BRCA1".into(),
            entity_type: EntityType::Gene,
            aliases: vec![],
            metadata: json!({}),
        });
        kg.add_entity(Entity {
            id: EntityId::new(),
            name: "breast cancer".into(),
            entity_type: EntityType::Disease,
            aliases: vec![],
            metadata: json!({}),
        });
        KgHandle::from_graph(kg)
    }

    #[tokio::test]
    async fn kg_query_stats_and_lookup() {
        let tool = KgQueryTool::new(make_handle_with_data());
        let ctx = ToolContext::new(".".to_string(), "test".to_string());

        let out = tool
            .execute(json!({"action": "stats"}), &ctx, CancellationToken::new())
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["entities"], 2);

        let out = tool
            .execute(
                json!({"action": "lookup", "entity": "BRCA1"}),
                &ctx,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["name"], "BRCA1");
    }

    #[tokio::test]
    async fn kg_add_is_idempotent_by_name() {
        let tool = KgAddTool::new(make_handle_with_data());
        let ctx = ToolContext::new(".".to_string(), "test".to_string());

        // BRCA1 已存在，应被跳过；TP53 应新增
        let out = tool
            .execute(
                json!({
                    "entities": [
                        {"name": "BRCA1", "type": "gene"},
                        {"name": "TP53", "type": "gene"}
                    ]
                }),
                &ctx,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["added_entities"], 1);
        assert_eq!(parsed["skipped_existing_entities"], 1);
        assert_eq!(parsed["total_entities"], 3);
    }

    #[tokio::test]
    async fn kg_add_relation_resolves_names() {
        let tool = KgAddTool::new(make_handle_with_data());
        let ctx = ToolContext::new(".".to_string(), "test".to_string());

        let out = tool
            .execute(
                json!({
                    "relations": [{
                        "from": "BRCA1",
                        "relation": "associated_with",
                        "to": "breast cancer",
                        "confidence": 0.95,
                        "evidence": "well-established link"
                    }]
                }),
                &ctx,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["added_relations"], 1);
        assert_eq!(parsed["total_relations"], 1);
    }

    #[tokio::test]
    async fn kg_query_neighborhood() {
        let handle = make_handle_with_data();
        // 先加一条关系
        {
            let tool = KgAddTool::new(handle.clone());
            let ctx = ToolContext::new(".".to_string(), "test".to_string());
            tool.execute(
                json!({"relations": [{"from": "BRCA1", "relation": "associated_with", "to": "breast cancer"}]}),
                &ctx,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        }
        let tool = KgQueryTool::new(handle);
        let ctx = ToolContext::new(".".to_string(), "test".to_string());
        let out = tool
            .execute(
                json!({"action": "neighborhood", "entity": "BRCA1"}),
                &ctx,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&out.content).unwrap();
        let neighbors = parsed["neighbors"].as_array().unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0]["target"], "breast cancer");
    }

    #[tokio::test]
    async fn kg_query_unknown_action_errors() {
        let tool = KgQueryTool::new(KgHandle::new());
        let ctx = ToolContext::new(".".to_string(), "test".to_string());
        let res = tool
            .execute(json!({"action": "bogus"}), &ctx, CancellationToken::new())
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn kg_query_missing_entity_errors() {
        let tool = KgQueryTool::new(KgHandle::new());
        let ctx = ToolContext::new(".".to_string(), "test".to_string());
        let res = tool
            .execute(
                json!({"action": "neighborhood", "entity": "nonexistent"}),
                &ctx,
                CancellationToken::new(),
            )
            .await;
        assert!(res.is_err());
    }
}
