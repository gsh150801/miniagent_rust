# 知识图谱 + 链路预测 → 科学假设生成

## 核心思路

把论文抽象为知识图谱的节点和边，用链路预测算法发现"图谱中缺失但理应存在的边"——即**新的科学假设**。

```
传统科研: 读论文 → 写摘要 → 研究者灵光一现 → 假设 (依赖经验、不可复制)
KG 方法:  读论文 → 构建 KG → 链路预测 → 系统化发现缺失边 → 排序假设列表
```

## 业界关键参考文献

| 论文/系统 | 核心贡献 | 关键指标 |
|-----------|---------|---------|
| ResearchLink | 将假设生成形式化为 KG 链路预测；路径+KGE+文本+文献计量混合特征 | 78.7% P@20 (CSKG-600) |
| HypoChainer | LLM + KG 协同: 探索 → 假设链形成 → 验证排序 | IEEE VIS 2025 |
| GIVE | 可信度外推: 已知 KG 关系外推到语义相似的未见实体对 | GPT-3.5+GIVE > GPT-4; 43.5%→88.2% |
| BioScientist | VGAE 链路预测 + ADAC RL 路径发现 + LLM 因果报告 | 0.929 Accuracy, 0.424 MRR |
| REx | RL 生成可解释的药物重定位假设路径 | IJCAI 2025 |
| Agentic Deep Graph | LLM 自组织增长 KG，涌现无标度网络 | MIT 2025 |
| HGNet | 零样本分层 KG 构建，+10.76% NER, +26.2% RE | ICLR 2026 |
| EvidenceNet | 从全文构建带证据级别的 KG，98.3% 字段提取 | ArXiv Mar 2026 |

## 核心流水线

```
Phase 1: 论文 → KG 构建
  100篇论文 → DeepSeek Flash 实体/关系抽取 (并行5篇)
  → 实体规范化 (同义合并)
  → 知识图谱: 1,230 实体, 3,847 边
  ⏱ ≈5 分钟

Phase 2: KG Embedding
  TransE/RotatE 训练向量表示 (128维)
  ⏱ ≈30 秒

Phase 3: 链路预测
  对所有 (h, r, ?) 做混合评分:
    KGE得分 + 路径特征 + 文本语义 + 文献共现
  → Top-200 候选三元组
  ⏱ ≈2 分钟

Phase 4: LLM 验证 + 假设生成
  Top-200 → DeepSeek Pro 逐一验证 (thinking=16K)
  → 生成自然语言假设陈述 + 机制解释 + 实验方案
  ⏱ ≈10 分钟

Phase 5: 假设多维排序
  × 算法置信度(0.3) + LLM 合理性(0.3) + 新颖性(0.2) + 实验可行性(0.2)
  → Top-15 假设
```

## KG Schema

### 实体类型

```rust
enum EntityType {
    // 研究客体
    Gene, Protein, Pathway, Disease, Phenotype,
    CellLine, Drug, Compound,
    // 研究方法
    Method, Assay, Model,
    // 概念层
    Hypothesis, Theory, Mechanism, Biomarker,
    // 文献层
    Paper, Author, Institution,
    // 数据层
    Dataset, Metric,
}
```

### 关系类型

```rust
enum RelationType {
    // 因果/机制关系 ← 假设生成的核心
    Activates, Inhibits, Regulates, BindsTo,
    Phosphorylates, InteractsWith, Catalyzes, Transports,
    // 关联关系
    AssociatedWith, CorrelatedWith,
    // 方法/证据关系
    UsesMethod, MeasuredBy, EvidencedBy,
    // 语义关系
    IsA, PartOf, LocatedIn,
    // 文献关系
    Cites, Contradicts, Supports, Extends,
    // 假设专用
    Hypothesizes, Predicts,
}
```

## 链路预测: 混合评分策略

### 五大特征通道 (借鉴 ResearchLink + GIVE)

```rust
struct LinkPredictionScorer {
    kge_model: Box<dyn KgeModel>,     // TransE / RotatE / ComplEx / MuRE
    path_extractor: PathExtractor,    // h→t 推理路径提取
    text_encoder: TextEncoder,        // BGE-m3 / SapBERT
    llm_validator: Option<LlmValidator>, // DeepSeek Pro 语义验证
}

impl LinkPredictionScorer {
    fn composite_score(&self, h: &Entity, r: &Relation, t: &Entity,
                       paths: &[Path]) -> f64
    {
        let s_kge    = -self.kge_model.distance(h, r, t);      // 0.35
        let s_path   = paths.iter().map(|p| self.path_confidence(p))
                             .max().unwrap_or(0.0);             // 0.30
        let s_text   = self.text_encoder.similarity(h, t);      // 0.20
        let s_biblio = self.pmi(h, t);                          // 0.15

        s_kge*0.35 + s_path*0.30 + s_text*0.20 + s_biblio*0.15
    }

    /// 为预测提供可解释的推理路径
    fn explain_paths(&self, h: &Entity, t: &Entity) -> Vec<ExplanationPath> {
        // 例: BRCA1→regulates→DNA_Repair→defective_in→Breast_Cancer
        // 解释: 如果 BRCA1 通过 DNA修复通路影响乳腺癌，
        //       则任何影响 DNA修复通路的因素都可能关联乳腺癌
        self.path_extractor.find_all_paths(h, t, max_hops=3)
    }
}
```

### GIVE 可信度外推算法

```rust
/// 核心: 利用已知 KG 关系的语义相似性，外推到未见过的实体对
async fn veracity_extrapolation(
    &self, h: &Entity, r: &Relation, kg: &KnowledgeGraph,
) -> Vec<HypothesisCandidate>
{
    // Step 1: Observe — 找到所有与 h 有 r 关系的已知尾部
    let known_tails = kg.query(h, r);

    // Step 2: Reflect — 用已知尾部语义向量，在嵌入空间找相似未知实体
    let known_emb = self.aggregate_embeddings(&known_tails);
    let candidates = self.vector_store.similar(known_emb, top_k=100);

    // Step 3: 路径过滤 — 只保留有推理路径连接的候选
    let mut hypotheses = Vec::new();
    for c in candidates {
        if kg.contains_edge(h, r, &c) { continue; } // 已知关系跳过
        let paths = self.path_extractor.find_all_paths(h, &c, 3);
        if paths.is_empty() { continue; }            // 无路径不可解释

        hypotheses.push(HypothesisCandidate {
            head: h, relation: r, tail: c,
            score: self.composite_score(h, r, &c, &paths),
            evidence_paths: paths,
        });
    }

    // Step 4: Speak — LLM 验证 + 生成自然语言假设
    self.generate_statements(&self.rank_and_filter(hypotheses)).await
}
```

## LLM 假设生成

```rust
impl HypothesisGenerator {
    async fn generate(
        &self, candidate: &HypothesisCandidate,
        kg: &KnowledgeGraph,
    ) -> Hypothesis
    {
        let prompt = format!(
            "你是科研假设评审专家。KG 链路预测发现了一个潜在的未知关系:\n\
             ({head}) --[{relation}]--> ({tail})\n\
             算法置信度: {score:.3}\n\
             图谱证据链:\n{evidence_paths}\n\
             图谱上下文 (已知关系):\n{kg_neighborhood}\n\n\
             请完成:\n\
             1. 判断生物学/科学合理性\n\
             2. 转化为完整可验证的科学假设\n\
             3. 提出理论依据 (基于文献)\n\
             4. 提出实验验证方案 (具体方法 + 预期结果)\n\
             5. 评估新颖性 (Novel/Incremental/Trivial)\n\
             6. 指出潜在反驳证据或替代解释",
            head = candidate.head.name,
            relation = candidate.relation.name,
            tail = candidate.tail.name,
            score = candidate.score,
            evidence_paths = self.format_paths(&candidate.evidence_paths),
            kg_neighborhood = self.format_neighborhood(&candidate.head, &candidate.tail, kg),
        );

        self.pro.with_thinking(16000).complete(&prompt).await? // DeepSeek Pro 深度推理
    }
}
```

## 与记忆系统的集成

```
L0 (工作记忆):
  └── 当前任务实体焦点集 + 候选假设预览

L1 (情景记忆 / SQLite):
  └── 结构化论文摘要 (每条对应 KG 子图)
  └── HypothesisCandidate 表 (链路预测中间结果)
  └── Hypothesis 表 (LLM 验证后的完整假设 + 实验方案)

L2 (语义记忆 / LanceDB):
  └── 实体向量 (128-1024维)
  └── 论文全文 chunk embeddings
  └── 知识图谱完整存储 (用于图遍历和路径发现)

L3 (技能记忆):
  └── 论文→KG 的抽取模板 (按学科分类)
  └── 链路预测权重配置 (最佳实践)
  └── 假设评估 checklist

Sleeptime Consolidation:
  └── 积累新论文后重新训练 KG Embedding
  └── 发现新跨论文关系 (社区检测)
  └── 退役被证伪假设 / 标记已验证假设
  └── 更新 Meta-Skill: 从成功/失败假设中学到更好的评估策略
```

## 关键挑战与应对

| 挑战 | 应对 |
|------|------|
| LLM 实体抽取不一致 | 实体规范化: 同义词词典 + SapBERT 语义匹配 + LLM 投票合并 |
| KG 稀疏 → 预测不可靠 | 引入路径特征和文本 embedding 作为补充信号 (ResearchLink 混合策略) |
| 错误假设混入 | 多层过滤: KGE 得分 → 路径验证 → LLM 批判性审查 → 文献反查 |
| 新颖性不足 | LLM 提示词明确区分已知/未知；Experience Graph 中的已知模式去重 |
| 领域本体缺失 | 集成公开本体 (GO, MeSH, ChEBI) + LLM 自动发现新概念 |

## 与人类科研的类比

| 人类科研 | KG + 链路预测 |
|---------|-------------|
| 读论文 | KG 构建 (文本 → 结构化三元组) |
| 发现规律 | 链路预测 (数学空间中发现缺失的因果边) |
| 提出假说 | LLM 语义化 (数值评分 → 可理解的科学陈述) |
| 设计实验 | LLM 验证输出 (机制 → 检验方案) |
