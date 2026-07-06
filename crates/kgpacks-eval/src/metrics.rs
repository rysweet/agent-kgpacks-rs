//! Pure metric aggregation for eval reports.
//!
//! Ports `packages/eval/src/metrics.ts`: IO-free, total and deterministic
//! functions that turn per-question verdicts into the per-arm and head-to-head
//! numbers headlining an [`EvalReport`](crate::EvalReport). Empty inputs yield
//! zeros, never `NaN`.

/// A single grade. `correct` drives accuracy; `score` supports finer aggregation.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgeVerdict {
    /// The pass/fail decision; accuracy is the mean of this across an arm.
    pub correct: bool,
    /// Graded quality; aggregation clamps it into `[0, 1]` (`NaN` → `0`).
    pub score: f64,
    /// The judge's free-text rationale (untrusted model output — do not execute).
    pub reasoning: String,
}

/// Per-arm aggregate metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmReport {
    /// The arm's name (e.g. `with-pack` / `training-only`).
    pub name: String,
    /// Mean of `verdict.correct` over the arm's questions (the headline accuracy).
    pub accuracy: f64,
    /// Mean of `verdict.score` (0–1) — a finer-grained aggregate.
    pub mean_score: f64,
    /// Number of questions scored for this arm.
    pub count: usize,
}

/// with-pack vs training-only head-to-head comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonReport {
    /// `with_pack.accuracy − training_only.accuracy` — the pack's lift (can be negative).
    pub delta_accuracy: f64,
    /// with-pack correct ∧ training-only incorrect.
    pub wins: usize,
    /// with-pack incorrect ∧ training-only correct (a regression).
    pub losses: usize,
    /// both correct or both incorrect.
    pub ties: usize,
    /// `wins / (wins + losses)`; `0` when there are no decisive questions.
    pub win_rate: f64,
}

/// Mean of `verdict.correct` over the verdicts; `0` for an empty list.
fn accuracy_of(verdicts: &[JudgeVerdict]) -> f64 {
    if verdicts.is_empty() {
        return 0.0;
    }
    let correct = verdicts.iter().filter(|v| v.correct).count();
    correct as f64 / verdicts.len() as f64
}

/// Compute one arm's `accuracy` (mean of `correct`) and `mean_score` (mean of
/// `score`, each clamped into `[0, 1]` with `NaN` treated as `0`). An empty input
/// yields `accuracy: 0, mean_score: 0, count: 0` (never `NaN`).
pub fn aggregate_arm(name: impl Into<String>, results: &[JudgeVerdict]) -> ArmReport {
    let count = results.len();
    if count == 0 {
        return ArmReport {
            name: name.into(),
            accuracy: 0.0,
            mean_score: 0.0,
            count: 0,
        };
    }
    let score_sum: f64 = results.iter().map(|v| clamp_unit(v.score)).sum();
    ArmReport {
        name: name.into(),
        accuracy: accuracy_of(results),
        mean_score: score_sum / count as f64,
        count,
    }
}

/// Clamp a judge score into `[0, 1]`, mapping `NaN` to `0` so aggregates are
/// always well-formed even if a judge returns an out-of-range or `NaN` score
/// (the `JudgeVerdict.score` contract is a clamped `[0, 1]` quality).
fn clamp_unit(score: f64) -> f64 {
    if score.is_nan() {
        0.0
    } else {
        score.clamp(0.0, 1.0)
    }
}

/// Compute the head-to-head comparison of two index-aligned verdict lists:
/// `delta_accuracy`, per-question `wins` / `losses` / `ties`, and
/// `win_rate = wins / (wins + losses)` (which is `0` — never `NaN` — when no
/// question is decisive). The two slices MUST be aligned by question; the runner
/// guarantees this.
pub fn compare_arms(
    with_pack: &[JudgeVerdict],
    training_only: &[JudgeVerdict],
) -> ComparisonReport {
    let delta_accuracy = accuracy_of(with_pack) - accuracy_of(training_only);

    let paired = with_pack.len().min(training_only.len());
    let mut wins = 0;
    let mut losses = 0;
    let mut ties = 0;
    for i in 0..paired {
        let w = with_pack[i].correct;
        let t = training_only[i].correct;
        if w && !t {
            wins += 1;
        } else if !w && t {
            losses += 1;
        } else {
            ties += 1;
        }
    }

    let decisive = wins + losses;
    let win_rate = if decisive == 0 {
        0.0
    } else {
        wins as f64 / decisive as f64
    };

    ComparisonReport {
        delta_accuracy,
        wins,
        losses,
        ties,
        win_rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(correct: bool, score: f64) -> JudgeVerdict {
        JudgeVerdict {
            correct,
            score,
            reasoning: String::new(),
        }
    }

    #[test]
    fn empty_arm_is_all_zeros() {
        let report = aggregate_arm("with-pack", &[]);
        assert_eq!(report.accuracy, 0.0);
        assert_eq!(report.mean_score, 0.0);
        assert_eq!(report.count, 0);
    }

    #[test]
    fn aggregates_accuracy_and_mean_score() {
        let report = aggregate_arm(
            "with-pack",
            &[verdict(true, 1.0), verdict(false, 0.5), verdict(true, 0.75)],
        );
        assert_eq!(report.count, 3);
        assert!((report.accuracy - 2.0 / 3.0).abs() < 1e-9);
        assert!((report.mean_score - 0.75).abs() < 1e-9);
    }

    #[test]
    fn compares_arms_with_wins_losses_ties() {
        let with_pack = [verdict(true, 1.0), verdict(false, 0.0), verdict(true, 1.0)];
        let training = [verdict(false, 0.0), verdict(true, 1.0), verdict(true, 1.0)];
        let cmp = compare_arms(&with_pack, &training);
        assert_eq!(cmp.wins, 1);
        assert_eq!(cmp.losses, 1);
        assert_eq!(cmp.ties, 1);
        assert!((cmp.delta_accuracy - 0.0).abs() < 1e-9);
        assert!((cmp.win_rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn win_rate_is_zero_when_no_decisive_questions() {
        let with_pack = [verdict(true, 1.0), verdict(false, 0.0)];
        let training = [verdict(true, 1.0), verdict(false, 0.0)];
        let cmp = compare_arms(&with_pack, &training);
        assert_eq!(cmp.wins, 0);
        assert_eq!(cmp.losses, 0);
        assert_eq!(cmp.ties, 2);
        assert_eq!(cmp.win_rate, 0.0);
    }

    #[test]
    fn mean_score_clamps_out_of_range_and_nan_scores() {
        let report = aggregate_arm(
            "with-pack",
            &[
                verdict(true, f64::NAN),
                verdict(true, 2.5),
                verdict(false, -1.0),
            ],
        );
        // NaN -> 0, 2.5 -> 1, -1 -> 0 => mean 1/3; never NaN or out of [0, 1].
        assert!((report.mean_score - 1.0 / 3.0).abs() < 1e-9);
        assert!((0.0..=1.0).contains(&report.mean_score));
    }
}
