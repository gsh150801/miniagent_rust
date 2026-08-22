#!/usr/bin/env python3
"""Deterministic OpenAI-compatible mock LLM for end-to-end pipeline tests.

All paid providers being out of quota (or in CI), this stub lets the FULL
research pipeline run end-to-end: PubMed search / efetch, KG construction,
TransE link prediction, debate orchestration, plan grounding, notebook
generation + Jupyter execution, provenance and report writing all exercise
real code — only the LLM inference is replaced by canned, prompt-routed
responses.

Usage:
    python3 scripts/mock_llm_server.py [port]        # default 8765

Then register a custom model profile (models.json):
    {
      "active_id": "custom-mock-llm",
      "custom": [{
        "id": "custom-mock-llm",
        "display_name": "Local Mock LLM (e2e)",
        "kind": "openai_compatible",
        "base_url": "http://127.0.0.1:8765/v1",
        "model_name": "mock-flash",
        "pro_model_name": "mock-pro",
        "api_key": "mock-key"
      }],
      "debate": {}
    }
"""

import hashlib
import json
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# ── canned content pools ──────────────────────────────────────────────

GENE_POOL = [
    ("SOD1", "Gene"), ("TARDBP", "Gene"), ("C9orf72", "Gene"), ("FUS", "Gene"),
    ("TDP-43", "Protein"), ("SOD1 protein", "Protein"),
    ("glutamate excitotoxicity", "Pathway"),
    ("neuroinflammation", "Pathway"),
    ("oxidative stress", "Pathway"),
    ("muscle weakness", "Phenotype"), ("riluzole", "Drug"),
    ("motor neuron", "Concept"),
]

MECHS = [
    "protein misfolding and aggregation",
    "impaired axonal transport",
    "mitochondrial dysfunction",
    "RNA processing defects",
    "glial-mediated neuroinflammation",
]


def _h(s: str) -> int:
    return int(hashlib.sha256(s.encode()).hexdigest(), 16)


def kg_extraction(pmid: str) -> str:
    """Two-to-four plausible ALS entities per paper, varied by PMID hash."""
    h = _h(pmid)
    picks = [GENE_POOL[(h >> (4 * i)) % len(GENE_POOL)] for i in range(4)]
    seen, pairs = set(), []
    for name, typ in picks:
        if name.lower() not in seen:
            seen.add(name.lower())
            pairs.append((name, typ))
    pairs.append(("amyotrophic lateral sclerosis", "Disease"))
    ents = [{"name": n, "type": t, "aliases": []} for n, t in pairs]
    rels = []
    for name, typ in pairs:
        if name == "amyotrophic lateral sclerosis":
            continue
        rel = "associated_with" if typ in ("Gene", "Protein", "Pathway", "Phenotype") else "interacts_with"
        rels.append({
            "from": name, "to": "amyotrophic lateral sclerosis", "type": rel,
            "evidence": "mock evidence sentence from abstract",
        })
    if len(pairs) > 2:
        rels.append({
            "from": pairs[0][0], "to": pairs[1][0], "type": "regulates",
            "evidence": "mock regulatory link",
        })
    return json.dumps({"entities": ents, "relations": rels})


CAND_RE = re.compile(
    r"-\s*(.+?)\s*\((\w+)\)\s*--\[(.+?)\]-->\s*(.+?)\s*\((\w+)\)"
)
HYP_UNDER_VALIDATION_RE = re.compile(
    r"\*\*Hypothesis under validation:\*\*\n(.+?)(?:\n\n|\n\*\*)", re.S
)
ROSTER_ID_RE = re.compile(r"\[id=([0-9a-f-]{36})\]")
REFINE_ID_RE = re.compile(r"id=([0-9a-f-]{36})")
INPUT_PATH_RE = re.compile(r'INPUT_DATA_PATH\s*=\s*"([^"]+)"')
OUTPUT_DIR_RE = re.compile(r'OUTPUT_DIR\s*=\s*"?([^"\s]+)"?')
SEED_RE = re.compile(r"SEED\s*=\s*(\d+)")


def hypothesis_eval(prompt: str) -> str:
    m = CAND_RE.search(prompt)
    head, rel, tail = (m.group(1), m.group(3), m.group(4)) if m else ("X", "relates_to", "ALS")
    h = _h(head + tail) % len(MECHS)
    conf = 0.55 + (_h(head) % 40) / 100.0
    return json.dumps({
        "plausible": True,
        "statement": f"{head} drives ALS pathogenesis via {MECHS[h]}, linking {rel} "
                     f"between {head} and {tail} to motor neuron degeneration",
        "mechanism": f"{head} perturbs {tail} homeostasis, inducing {MECHS[h]} "
                     "that cascades into upper and lower motor neuron loss",
        "novelty": "Incremental",
        "confidence": round(conf, 2),
        "supporting_evidence": [
            "Familial ALS linkage studies implicate this pathway",
            "Transgenic models reproduce the downstream pathology",
        ],
        "counter_evidence": [
            "Some cohort studies failed to replicate the association",
        ],
        "experiment": {
            "approach": "compare mutant vs wild-type model systems",
            "methods": ["qPCR", "western blot", "behavioral scoring"],
            "expected_outcomes": ["progressive motor deficit in mutants"],
            "controls": ["wild-type littermates", "vehicle treatment"],
            "feasibility": 0.7,
        },
    }, ensure_ascii=False)


def validation_plan(prompt: str) -> str:
    m = HYP_UNDER_VALIDATION_RE.search(prompt)
    stmt = (m.group(1).strip() if m else "the hypothesis")[:100]
    return json.dumps({
        "rationale": f"Computational re-analysis of public expression data plus a "
                     f"targeted bench assay directly test: {stmt}…",
        "data_analysis_tasks": [{
            "id": "DA-1",
            "objective": f"Test whether the {stmt[:60]}… signature separates cases "
                         "from controls in a bulk-RNA cohort",
            "dataset_source": {"kind": "local", "value": "supplied-cohort.csv"},
            "dataset_accession": "",
            "cohort_definition": "ALS cases vs neurologically healthy controls",
            "variables": {
                "independent": ["group"],
                "dependent": ["biomarker"],
                "covariates": ["age", "sex"],
            },
            "statistical_method": "two-sample comparison of group means",
            "expected_outcome": "case group mean differs from controls",
            "deliverable": "summary table (CSV) + group-mean statistics (JSON)",
            "priority": 0.9,
        }],
        "wet_lab_protocols": [{
            "id": "WL-1",
            "objective": f"Validate the {stmt[:60]}… effect in a cellular model",
            "reagents": ["motor neuron progenitor line", "standard culture media"],
            "steps": [
                "differentiate progenitors into motor neurons",
                "apply perturbation vs vehicle for 72h",
                "quantify viability and marker expression",
            ],
            "controls": ["untreated wells", "positive-control toxin"],
            "expected_outcome": "perturbed wells show reduced viability and marker loss",
            "timeline_days": 21,
            "feasibility": 0.65,
        }],
    }, ensure_ascii=False)


def analysis_script(prompt: str) -> str:
    inp = INPUT_PATH_RE.search(prompt)
    out = OUTPUT_DIR_RE.search(prompt)
    seed = SEED_RE.search(prompt)
    inp_path = inp.group(1) if inp else "None"
    out_dir = out.group(1) if out else "."
    seed_v = seed.group(1) if seed else "42"
    # NOTE: braces are escaped with {{}} because this template is inserted
    # verbatim (no .format) — they are literal in the emitted script.
    return f'''import csv
import json
import os
import random

random.seed({seed_v})

OUTPUT_DIR = {out_dir!r}
INPUT_DATA_PATH = {inp_path!r}


def load_rows(path):
    try:
        import pandas as pd
        return pd.read_csv(path).to_dict("records")
    except Exception:
        with open(path, newline="") as f:
            return list(csv.DictReader(f))


rows = []
if INPUT_DATA_PATH and os.path.exists(INPUT_DATA_PATH):
    rows = load_rows(INPUT_DATA_PATH)

groups = {{}}
for r in rows:
    g = str(r.get("group", "unknown"))
    v = r.get("biomarker")
    if v is None:
        continue
    try:
        v = float(v)
    except (TypeError, ValueError):
        continue
    groups.setdefault(g, []).append(v)

summary = {{
    g: {{"n": len(vs), "mean": round(sum(vs) / len(vs), 4)}}
    for g, vs in groups.items()
    if vs
}}

result = {{
    "task": "group-mean-biomarker-comparison",
    "input": INPUT_DATA_PATH,
    "rows": len(rows),
    "group_summary": summary,
    "seed": {seed_v},
}}

os.makedirs(OUTPUT_DIR, exist_ok=True)
with open(os.path.join(OUTPUT_DIR, "summary.json"), "w") as f:
    json.dump(result, f, indent=2)

with open(os.path.join(OUTPUT_DIR, "summary.csv"), "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["group", "n", "mean_biomarker"])
    for g, s in summary.items():
        w.writerow([g, s["n"], s["mean"]])

print("ANALYSIS-OK " + json.dumps(result))
'''


def cross_comparison(prompt: str) -> str:
    ids = ROSTER_ID_RE.findall(prompt)
    contra = []
    if len(ids) >= 2:
        contra.append({
            "a": ids[0], "b": ids[1],
            "reason": "the two mechanisms predict opposite directions for the "
                      "same downstream marker",
        })
    return json.dumps({
        "contradictions_between": contra,
        "ranking_rationale": "ranked by mechanistic specificity and supporting "
                             "evidence breadth from the debate",
        "strongest_id": ids[0] if ids else None,
        "merge_suggestions": [
            "merge overlapping mechanism variants into a single cascading model"
        ] if len(ids) >= 2 else [],
    })


def refinement(prompt: str) -> str:
    ids = REFINE_ID_RE.findall(prompt)
    return json.dumps({
        "refined": [
            {
                "id": i,
                "statement": f"(refined) Narrowed claim for {i[:8]}: the mechanism "
                              "operates in a defined neuronal population under "
                              "stated covariate adjustments",
                "mechanism": "cell-type-specific perturbation cascade with "
                             "covariate-adjusted effect estimates",
                "supporting_evidence": [
                    "strengthened: replicated association in independent cohorts",
                ],
                "counter_evidence": [
                    "residual confounding cannot be fully excluded",
                ],
                "confidence": 0.71,
            }
            for i in ids
        ]
    })


def route(prompt: str) -> str:
    p = prompt
    if "Convert this research question into a PubMed search query" in p \
            or "Corrected PubMed query" in p:
        return "amyotrophic lateral sclerosis AND (pathogenesis OR mechanism)"
    if "Is this paper on-topic" in p:
        if "conflict of interest" in p.lower():
            return json.dumps({"score": 1, "reason": "editorial statement, not research"})
        return json.dumps({"score": 8, "reason": "on-topic for the query"})
    if "Extract key entities and their relationships" in p:
        m = re.search(r"PMID:(\d+)", p)
        return kg_extraction(m.group(1) if m else "0")
    if "scientific hypothesis evaluator" in p:
        return hypothesis_eval(p)
    if "PROPOSER" in p:
        return json.dumps({
            "supporting_points": [
                "convergent evidence from familial ALS genetics",
                "replicated expression signatures in independent cohorts",
            ],
            "proposer_confidence": 0.75,
        })
    if "OPPONENT" in p:
        return json.dumps({
            "contradicting_points": [
                "association does not establish causation",
                "effect sizes shrink after covariate adjustment",
            ],
            "opponent_recommendation": "revise",
        })
    if "comparing competing hypotheses" in p:
        return cross_comparison(p)
    if "refining hypotheses in light of a debate" in p:
        return refinement(p)
    if "JUDGE" in p:
        return json.dumps({
            "verdict": "revise",
            "confidence_after": 0.68,
            "refinement_notes": "narrow the claim to a defined cell type and "
                                "add a covariate-adjusted replication cohort",
        })
    if "senior biomedical researcher designing a validation plan" in p:
        return validation_plan(p)
    if "bioinformatics engineer" in p:
        return analysis_script(p)
    return "OK"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        sys.stderr.write("[mock-llm] " + (fmt % args) + "\n")

    def do_POST(self):
        if not self.path.endswith("/chat/completions"):
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length) or b"{}")
        messages = body.get("messages", [])
        prompt = ""
        for m in reversed(messages):
            c = m.get("content", "")
            if isinstance(c, str) and c.strip():
                prompt = c
                break
            if isinstance(c, list):
                texts = [p.get("text", "") for p in c if isinstance(p, dict)]
                if any(t.strip() for t in texts):
                    prompt = "\n".join(texts)
                    break
        text = route(prompt)
        resp = {
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "created": 0,
            "model": body.get("model", "mock"),
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }],
            "usage": {
                "prompt_tokens": max(1, len(prompt) // 4),
                "completion_tokens": max(1, len(text) // 4),
                "total_tokens": max(2, (len(prompt) + len(text)) // 4),
            },
        }
        payload = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
    srv = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    print(f"[mock-llm] listening on http://127.0.0.1:{port}/v1/chat/completions")
    srv.serve_forever()
