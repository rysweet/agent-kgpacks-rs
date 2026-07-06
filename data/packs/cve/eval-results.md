# CVE pack — eval results

The committed eval artifact for the CVE pack. It records what was measured for the
full committed eval set in [`eval_questions.json`](./eval_questions.json) (14
questions; 12 real, recent 2024/2025 CVEs with reference answers, plus 2 concept
questions). The machine-readable copy is [`eval-results.json`](./eval-results.json).

## Environment status: pinned LLM judge unavailable → retrieval-recall reported

The full eval (`kgpacks-eval`'s `run_eval`) grades both arms — the pack-augmented
`with-pack` arm and the closed-book `training-only` arm — with a **pinned judge
model**, and compares them via `accuracy` / `mean_score` / `delta_accuracy`. That
path requires a Copilot synthesis + judge transport.

In this build/CI environment **no pinned judge/synthesis transport is available**
(the automated eval tests are offline by design so CI stays green and hermetic). The
LLM-judged numbers therefore cannot be produced here without re-baselining on a
different judge, which the fixed-model contract forbids. Per issue #16's fallback,
the reported number is the **deterministic, LLM-free retrieval-recall** metric.

## Deterministic retrieval-recall (embedding hit@k)

For each CVE-specific question, the question is embedded with the BGE query encoder
(the deterministic-hash parity embedder) and matched (cosine) against a corpus of
one record per CVE question, keyed by its committed reference answer. recall@k is
the fraction of questions whose target record appears in the top-k. Measured over
the **full** question set (no sampling), across the 12 CVE-specific questions:

| Metric    | Value |
| --------- | ----- |
| recall@1  | 0.750 |
| recall@3  | 0.917 |
| recall@5  | 0.917 |

This is a deterministic, self-contained **reference-answer embedding** sanity
check: it confirms each CVE question is discriminable against the other CVE records
by embedding similarity, a lower-bound proxy for the retrieval half of the
with-pack arm. It deliberately does not stand in for real pack ingestion +
`kgpacks-query` retrieval or the LLM-judged with-pack-vs-training comparison — those
are the blocked path above. It is measured on a small, deliberately homogeneous
corpus (every record is a CVE, so distractors are close); the full 343k-record CVE
pack has more diverse content.

The numbers are produced — and CI-guarded against drift — by
`cargo test -p kgpacks-eval --test full_pack_eval`, which recomputes recall from the
committed questions and asserts it equals the values in
[`eval-results.json`](./eval-results.json). It is fully deterministic (the embedder
yields stable unit-norm vectors), so the artifact is reproducible.

### Benchmark-only status (no live self-metric)

These recall@k are **fixed benchmark numbers** over the committed 12-CVE corpus, not
a live production self-metric trended over time. That is by necessity: the trended
metric is the LLM-judged with-pack-vs-training `accuracy` / `mean_score` /
`delta_accuracy`, which requires a pinned judge + synthesis transport that is
unavailable in this offline/CI environment (see the status section above). When a
transport is available, `run_eval` produces those live numbers (see "Reproducing the
full LLM-judged eval" below); until then this deterministic benchmark is the
CI-guarded stand-in.

## Reproducing the full LLM-judged eval

Where a pinned judge + synthesis transport is available, wire the real `with-pack`
(retrieve + synthesize) and `training-only` (closed-book) arms plus the pinned
[`Judge`] into `kgpacks_eval::run_eval` with `SampleMode::Full`, and record
`arms.with_pack.accuracy`, `arms.training_only.accuracy` and
`comparison.delta_accuracy` alongside the recall numbers above. The expanded
question set is designed to maximize `comparison.delta_accuracy` — it exercises
knowledge the base model is least likely to already hold, so the pack's lift shows.
