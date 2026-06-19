use serde::{Deserialize, Serialize};

/// Pre-defined hypothesis profiles for Alzheimer's disease research.
/// Each profile seeds a Proposer agent with domain-specific context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlzheimersHypothesisProfile {
    pub id: String,
    pub mechanism: String,
    pub seed_prompt: String,
    pub key_genes: Vec<String>,
    pub key_pathways: Vec<String>,
    pub supporting_evidence: Vec<String>,
    pub opposing_evidence: Vec<String>,
    pub priority: i32,
}

pub fn default_profiles() -> Vec<AlzheimersHypothesisProfile> {
    vec![
        AlzheimersHypothesisProfile {
            id: "amyloid_cascade".into(),
            mechanism: "Amyloid Cascade Hypothesis".into(),
            seed_prompt: "Propose a refined amyloid cascade hypothesis for AD pathogenesis, \
                incorporating recent evidence about APP processing, Aβ42 oligomers, and plaque formation timing. \
                Address why amyloid-targeting therapies have had limited clinical success."
                .into(),
            key_genes: vec!["APP".into(), "PSEN1".into(), "PSEN2".into(), "APOE4".into()],
            key_pathways: vec!["amyloidogenesis".into(), "secretase cleavage".into(), "microglial activation".into()],
            supporting_evidence: vec![
                "Down syndrome (trisomy 21) patients develop AD pathology early due to extra APP copy".into(),
                "PSEN1/2 mutations cause early-onset familial AD with increased Aβ42/40 ratio".into(),
                "Aducanumab shows dose-dependent amyloid plaque reduction in EMERGE trial".into(),
            ],
            opposing_evidence: vec![
                "Many individuals with high amyloid burden show no cognitive decline".into(),
                "Multiple anti-amyloid trials failed to show clinical benefit (bapineuzumab, solanezumab)".into(),
                "Tau pathology correlates better with cognitive decline than amyloid burden".into(),
            ],
            priority: 10,
        },
        AlzheimersHypothesisProfile {
            id: "tau_propagation".into(),
            mechanism: "Tau Propagation Hypothesis".into(),
            seed_prompt: "Propose a tau-centered hypothesis for AD, focusing on tau hyperphosphorylation, \
                spread via prion-like seeding, and the relationship between tau pathology and neurodegeneration. \
                Consider Braak staging and the disconnect between amyloid and symptom progression."
                .into(),
            key_genes: vec!["MAPT".into(), "GSK3B".into(), "CDK5".into()],
            key_pathways: vec!["tau phosphorylation".into(), "microtubule destabilization".into(), "prion-like propagation".into()],
            supporting_evidence: vec![
                "Braak staging shows predictable tau spread from entorhinal cortex".into(),
                "Tau PET correlates with cognitive decline better than amyloid PET".into(),
                "In vitro and in vivo evidence of trans-synaptic tau spread".into(),
            ],
            opposing_evidence: vec![
                "Primary tauopathies (PSP, CBD) have different clinical presentations".into(),
                "Tau pathology alone insufficient to explain full AD phenotype".into(),
            ],
            priority: 9,
        },
        AlzheimersHypothesisProfile {
            id: "neuroinflammation".into(),
            mechanism: "Neuroinflammatory Hypothesis".into(),
            seed_prompt: "Propose a neuroinflammation-driven hypothesis for AD, focusing on microglial activation, \
                the inflammasome (NLRP3), cytokine cascades, and how chronic inflammation creates a \
                self-reinforcing cycle of neurodegeneration."
                .into(),
            key_genes: vec!["TREM2".into(), "NLRP3".into(), "IL1B".into(), "TNF".into(), "CD33".into()],
            key_pathways: vec!["NLRP3 inflammasome".into(), "complement cascade".into(), "microglial phenotype switching".into()],
            supporting_evidence: vec![
                "TREM2 variants are major AD risk factors (R47H)".into(),
                "GWAS hits heavily enriched in microglial/inflammatory pathways".into(),
                "NLRP3 inhibition reduces pathology in AD mouse models".into(),
            ],
            opposing_evidence: vec![
                "Anti-inflammatory trials (NSAIDs) failed to prevent or slow AD".into(),
                "Inflammation may be protective early (debris clearance) before becoming harmful".into(),
            ],
            priority: 8,
        },
        AlzheimersHypothesisProfile {
            id: "vascular_dysfunction".into(),
            mechanism: "Vascular Contribution Hypothesis".into(),
            seed_prompt: "Propose a vascular hypothesis for AD, examining blood-brain barrier breakdown, \
                cerebral amyloid angiopathy, reduced cerebral blood flow, and how vascular insufficiency \
                may initiate or accelerate neurodegeneration."
                .into(),
            key_genes: vec!["APOE4".into(), "NOTCH3".into(), "COL4A1".into()],
            key_pathways: vec!["BBB transport".into(), "pericyte function".into(), "cerebral blood flow regulation".into()],
            supporting_evidence: vec![
                "BBB breakdown observed in cognitively normal APOE4 carriers before amyloid accumulation".into(),
                "White matter hyperintensities independently predict cognitive decline".into(),
                "Cerebral amyloid angiopathy present in >80% of AD patients".into(),
            ],
            opposing_evidence: vec![
                "Vascular pathology alone does not produce full AD phenotype".into(),
                "Pure vascular dementia has distinct clinical and pathological features".into(),
            ],
            priority: 7,
        },
        AlzheimersHypothesisProfile {
            id: "mitochondrial_cascade".into(),
            mechanism: "Mitochondrial Cascade Hypothesis".into(),
            seed_prompt: "Propose a mitochondrial cascade hypothesis for AD, focusing on mitochondrial dysfunction \
                as an upstream event: oxidative stress, impaired mitophagy, electron transport chain deficits, \
                and how APOE4 genotype may drive mitochondrial vulnerability."
                .into(),
            key_genes: vec!["APOE4".into(), "TOMM40".into(), "PINK1".into(), "PARKIN".into()],
            key_pathways: vec!["mitophagy".into(), "oxidative phosphorylation".into(), "ROS production".into()],
            supporting_evidence: vec![
                "Mitochondrial dysfunction observed in AD brains before plaque formation".into(),
                "APOE4 carriers show reduced cerebral glucose metabolism decades before symptoms".into(),
                "Swarms of mitochondrial DNA deletions accumulate in AD hippocampal neurons".into(),
            ],
            opposing_evidence: vec![
                "Mitochondrial dysfunction is also present in normal aging".into(),
                "Direct causal evidence linking mitochondria to AD-specific pathology is limited".into(),
            ],
            priority: 6,
        },
        AlzheimersHypothesisProfile {
            id: "synaptic_failure".into(),
            mechanism: "Synaptic Dysfunction Hypothesis".into(),
            seed_prompt: "Propose a synaptic failure hypothesis for AD, examining how soluble Aβ oligomers \
                impair synaptic plasticity, the role of tau in dendritic spine loss, and why synaptic loss \
                (not cell death) best correlates with cognitive decline."
                .into(),
            key_genes: vec!["ARC".into(), "C1Q".into(), "NRXN1".into()],
            key_pathways: vec!["LTP/LTD imbalance".into(), "complement-mediated pruning".into(), "glutamate excitotoxicity".into()],
            supporting_evidence: vec![
                "Synaptic density (SV2A PET) correlates with cognitive decline in early AD".into(),
                "Aβ oligomers inhibit LTP and enhance LTD at picomolar concentrations".into(),
                "Complement-mediated synaptic pruning by microglia is excessive in AD models".into(),
            ],
            opposing_evidence: vec![
                "Synaptic loss is a common endpoint in many neurodegenerative diseases, not AD-specific".into(),
                "The initial trigger of synaptic dysfunction remains unclear".into(),
            ],
            priority: 5,
        },
    ]
}
