use crate::{HistogramSummary, LatencyMetric, Telemetry};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScenarioMetric {
    ColdStart,
    WarmStart,
    FirstResult,
    FullResult,
    DirectoryOpen,
    VisibleThumbnailCompletion,
}

impl ScenarioMetric {
    pub const ALL: [Self; 6] = [
        Self::ColdStart,
        Self::WarmStart,
        Self::FirstResult,
        Self::FullResult,
        Self::DirectoryOpen,
        Self::VisibleThumbnailCompletion,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ColdStart => "cold_start",
            Self::WarmStart => "warm_start",
            Self::FirstResult => "first_result",
            Self::FullResult => "full_result",
            Self::DirectoryOpen => "directory_open",
            Self::VisibleThumbnailCompletion => "visible_thumbnail_completion",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyBudget {
    pub p50: Duration,
    pub p95: Duration,
    pub p99: Duration,
}

impl LatencyBudget {
    pub const fn new(p50: Duration, p95: Duration, p99: Duration) -> Self {
        Self { p50, p95, p99 }
    }

    pub fn validate(self) -> Result<(), BudgetViolation> {
        if self.p50 > self.p95 || self.p95 > self.p99 {
            return Err(BudgetViolation::InvalidBudget {
                metric: "latency",
                reason: "p50 must be <= p95 and p95 must be <= p99",
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioBudget {
    pub max: Duration,
}

impl ScenarioBudget {
    pub const fn new(max: Duration) -> Self {
        Self { max }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceBudgets {
    latency: BTreeMap<LatencyMetric, LatencyBudget>,
    scenarios: BTreeMap<ScenarioMetric, ScenarioBudget>,
}

impl Default for PerformanceBudgets {
    fn default() -> Self {
        let mut latency = BTreeMap::new();
        latency.insert(
            LatencyMetric::Navigation,
            LatencyBudget::new(ms(4), ms(8), ms(16)),
        );
        latency.insert(
            LatencyMetric::Selection,
            LatencyBudget::new(ms(2), ms(4), ms(8)),
        );
        latency.insert(
            LatencyMetric::Rename,
            LatencyBudget::new(ms(8), ms(16), ms(33)),
        );
        latency.insert(
            LatencyMetric::SearchKeystroke,
            LatencyBudget::new(ms(8), ms(16), ms(33)),
        );
        latency.insert(
            LatencyMetric::ResultStreaming,
            LatencyBudget::new(ms(12), ms(25), ms(50)),
        );
        latency.insert(
            LatencyMetric::ThumbnailDisplay,
            LatencyBudget::new(ms(16), ms(50), ms(100)),
        );
        latency.insert(
            LatencyMetric::PreviewOpen,
            LatencyBudget::new(ms(25), ms(75), ms(150)),
        );
        latency.insert(
            LatencyMetric::CopyStart,
            LatencyBudget::new(ms(25), ms(75), ms(150)),
        );
        latency.insert(
            LatencyMetric::Cancel,
            LatencyBudget::new(ms(8), ms(16), ms(33)),
        );
        latency.insert(
            LatencyMetric::WindowRender,
            LatencyBudget::new(ms(8), ms(16), ms(25)),
        );

        let mut scenarios = BTreeMap::new();
        scenarios.insert(ScenarioMetric::ColdStart, ScenarioBudget::new(ms(800)));
        scenarios.insert(ScenarioMetric::WarmStart, ScenarioBudget::new(ms(180)));
        scenarios.insert(ScenarioMetric::FirstResult, ScenarioBudget::new(ms(25)));
        scenarios.insert(ScenarioMetric::FullResult, ScenarioBudget::new(ms(250)));
        scenarios.insert(ScenarioMetric::DirectoryOpen, ScenarioBudget::new(ms(50)));
        scenarios.insert(
            ScenarioMetric::VisibleThumbnailCompletion,
            ScenarioBudget::new(ms(500)),
        );

        Self { latency, scenarios }
    }
}

impl PerformanceBudgets {
    pub fn latency(&self, metric: LatencyMetric) -> Option<LatencyBudget> {
        self.latency.get(&metric).copied()
    }

    pub fn scenario(&self, metric: ScenarioMetric) -> Option<ScenarioBudget> {
        self.scenarios.get(&metric).copied()
    }

    pub fn set_latency(
        &mut self,
        metric: LatencyMetric,
        budget: LatencyBudget,
    ) -> Result<(), BudgetViolation> {
        budget.validate()?;
        self.latency.insert(metric, budget);
        Ok(())
    }

    pub fn set_scenario(&mut self, metric: ScenarioMetric, budget: ScenarioBudget) {
        self.scenarios.insert(metric, budget);
    }

    pub fn validate(&self) -> Result<(), BudgetViolation> {
        for metric in LatencyMetric::ALL {
            let Some(budget) = self.latency.get(&metric).copied() else {
                return Err(BudgetViolation::MissingLatencyBudget { metric });
            };
            budget.validate()?;
        }
        for metric in ScenarioMetric::ALL {
            if !self.scenarios.contains_key(&metric) {
                return Err(BudgetViolation::MissingScenarioBudget { metric });
            }
        }
        Ok(())
    }

    pub fn evaluate_telemetry(&self, telemetry: &Telemetry) -> BudgetEvaluation {
        let mut violations = Vec::new();
        for metric in LatencyMetric::ALL {
            let summary = telemetry.latency(metric);
            if summary.count == 0 {
                continue;
            }
            if let Some(budget) = self.latency(metric) {
                push_latency_violations(&mut violations, metric, summary, budget);
            } else {
                violations.push(BudgetViolation::MissingLatencyBudget { metric });
            }
        }
        BudgetEvaluation { violations }
    }

    pub fn evaluate_scenarios(
        &self,
        observations: &BTreeMap<ScenarioMetric, Duration>,
    ) -> BudgetEvaluation {
        let mut violations = Vec::new();
        for (metric, observed) in observations {
            match self.scenario(*metric) {
                Some(budget) if *observed > budget.max => {
                    violations.push(BudgetViolation::ScenarioExceeded {
                        metric: *metric,
                        observed: *observed,
                        budget: budget.max,
                    });
                }
                Some(_) => {}
                None => violations.push(BudgetViolation::MissingScenarioBudget { metric: *metric }),
            }
        }
        BudgetEvaluation { violations }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetEvaluation {
    pub violations: Vec<BudgetViolation>,
}

impl BudgetEvaluation {
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetViolation {
    MissingLatencyBudget {
        metric: LatencyMetric,
    },
    MissingScenarioBudget {
        metric: ScenarioMetric,
    },
    InvalidBudget {
        metric: &'static str,
        reason: &'static str,
    },
    LatencyPercentileExceeded {
        metric: LatencyMetric,
        percentile: Percentile,
        observed: Duration,
        budget: Duration,
    },
    ScenarioExceeded {
        metric: ScenarioMetric,
        observed: Duration,
        budget: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Percentile {
    P50,
    P95,
    P99,
}

fn push_latency_violations(
    violations: &mut Vec<BudgetViolation>,
    metric: LatencyMetric,
    summary: HistogramSummary,
    budget: LatencyBudget,
) {
    if let Some(observed) = summary.p50 {
        if observed > budget.p50 {
            violations.push(BudgetViolation::LatencyPercentileExceeded {
                metric,
                percentile: Percentile::P50,
                observed,
                budget: budget.p50,
            });
        }
    }
    if let Some(observed) = summary.p95 {
        if observed > budget.p95 {
            violations.push(BudgetViolation::LatencyPercentileExceeded {
                metric,
                percentile: Percentile::P95,
                observed,
                budget: budget.p95,
            });
        }
    }
    if let Some(observed) = summary.p99 {
        if observed > budget.p99 {
            violations.push(BudgetViolation::LatencyPercentileExceeded {
                metric,
                percentile: Percentile::P99,
                observed,
                budget: budget.p99,
            });
        }
    }
}

const fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budgets_cover_all_required_metrics() {
        let budgets = PerformanceBudgets::default();

        budgets.validate().unwrap();
        for metric in LatencyMetric::ALL {
            let budget = budgets.latency(metric).unwrap();
            assert!(budget.p50 <= budget.p95);
            assert!(budget.p95 <= budget.p99);
        }
        for metric in ScenarioMetric::ALL {
            assert!(budgets.scenario(metric).unwrap().max > Duration::ZERO);
        }
    }

    #[test]
    fn evaluates_latency_budget_violations() {
        let mut budgets = PerformanceBudgets::default();
        budgets
            .set_latency(
                LatencyMetric::SearchKeystroke,
                LatencyBudget::new(ms(1), ms(2), ms(4)),
            )
            .unwrap();
        let mut telemetry = Telemetry::default();
        telemetry.observe_latency(LatencyMetric::SearchKeystroke, ms(8));

        let evaluation = budgets.evaluate_telemetry(&telemetry);

        assert!(!evaluation.passed());
        assert!(evaluation.violations.iter().any(|violation| {
            matches!(
                violation,
                BudgetViolation::LatencyPercentileExceeded {
                    metric: LatencyMetric::SearchKeystroke,
                    percentile: Percentile::P99,
                    ..
                }
            )
        }));
    }

    #[test]
    fn evaluates_scenario_budget_violations() {
        let budgets = PerformanceBudgets::default();
        let mut observations = BTreeMap::new();
        observations.insert(ScenarioMetric::ColdStart, Duration::from_secs(2));
        observations.insert(ScenarioMetric::FirstResult, ms(5));

        let evaluation = budgets.evaluate_scenarios(&observations);

        assert!(!evaluation.passed());
        assert_eq!(evaluation.violations.len(), 1);
        assert!(matches!(
            evaluation.violations[0],
            BudgetViolation::ScenarioExceeded {
                metric: ScenarioMetric::ColdStart,
                ..
            }
        ));
    }

    #[test]
    fn rejects_invalid_latency_budget_ordering() {
        let mut budgets = PerformanceBudgets::default();

        let err = budgets
            .set_latency(
                LatencyMetric::Navigation,
                LatencyBudget::new(ms(10), ms(5), ms(20)),
            )
            .unwrap_err();

        assert!(matches!(err, BudgetViolation::InvalidBudget { .. }));
    }
}
