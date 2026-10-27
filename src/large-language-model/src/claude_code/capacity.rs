use std::time::{Duration, Instant};

const FIVE_HOURS: Duration = Duration::from_secs(5 * 60 * 60);
const SEVEN_DAYS: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Rolling spend windows matching the subscription's five-hour and weekly limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BudgetWindow {
    FiveHours,
    SevenDays,
}

impl BudgetWindow {
    const fn duration(self) -> Duration {
        match self {
            Self::FiveHours => FIVE_HOURS,
            Self::SevenDays => SEVEN_DAYS,
        }
    }
}

impl std::fmt::Display for BudgetWindow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::FiveHours => "five-hour",
            Self::SevenDays => "seven-day",
        })
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub(super) enum CapacityRefusal {
    #[error("Claude Code is paused until the reported usage limit resets")]
    Paused { retry_after: Duration },
    #[error("Claude Code spent {spent_usd:.4} USD of its {budget_usd} USD {window} budget")]
    BudgetReached {
        window: BudgetWindow,
        spent_usd: f64,
        budget_usd: f64,
        retry_after: Duration,
    },
}

impl CapacityRefusal {
    pub(super) const fn retry_after(&self) -> Duration {
        match self {
            Self::Paused { retry_after } | Self::BudgetReached { retry_after, .. } => *retry_after,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Spend {
    at: Instant,
    cost_usd: f64,
}

/// Admission control for one Claude Code seat: soft budgets over the reported
/// `total_cost_usd` and a pause until a reported usage limit resets.
///
/// Budgets are soft: concurrent calls admitted before a spend is recorded can
/// overshoot them by the cost of those calls.
#[derive(Debug)]
pub(super) struct CapacityGate {
    budgets: [(BudgetWindow, Option<f64>); 2],
    spend: Vec<Spend>,
    paused_until: Option<Instant>,
}

impl CapacityGate {
    pub(super) fn new(window_budget_usd: Option<f64>, weekly_budget_usd: Option<f64>) -> Self {
        Self {
            budgets: [
                (BudgetWindow::FiveHours, window_budget_usd),
                (BudgetWindow::SevenDays, weekly_budget_usd),
            ],
            spend: Vec::new(),
            paused_until: None,
        }
    }

    /// Refuse a call while paused or while a rolling window has used its budget.
    /// A budget refusal lasts until the oldest spend in that window leaves it.
    pub(super) fn admit(&mut self, now: Instant) -> Result<(), CapacityRefusal> {
        self.spend
            .retain(|spend| now.saturating_duration_since(spend.at) < SEVEN_DAYS);
        if let Some(until) = self.paused_until {
            if until > now {
                return Err(CapacityRefusal::Paused {
                    retry_after: until - now,
                });
            }
            self.paused_until = None;
        }
        for (window, budget_usd) in self.budgets {
            let Some(budget_usd) = budget_usd else {
                continue;
            };
            let in_window = || {
                self.spend
                    .iter()
                    .filter(move |spend| now.saturating_duration_since(spend.at) < window.duration())
            };
            let spent_usd = in_window().map(|spend| spend.cost_usd).sum::<f64>();
            if spent_usd < budget_usd {
                continue;
            }
            let oldest_age = in_window()
                .map(|spend| now.saturating_duration_since(spend.at))
                .max()
                .unwrap_or_default();
            return Err(CapacityRefusal::BudgetReached {
                window,
                spent_usd,
                budget_usd,
                retry_after: window.duration().saturating_sub(oldest_age),
            });
        }
        Ok(())
    }

    /// Record a call's reported list-price cost. Costs that are not finite and
    /// positive cannot be summed meaningfully and are ignored.
    pub(super) fn record_spend(&mut self, now: Instant, cost_usd: f64) {
        if cost_usd.is_finite() && cost_usd > 0.0 {
            self.spend.push(Spend { at: now, cost_usd });
        }
    }

    /// Refuse calls until `until`; an existing longer pause is kept.
    pub(super) fn pause_until(&mut self, until: Instant) {
        self.paused_until = Some(self.paused_until.map_or(until, |current| current.max(until)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn should_admit_any_spend_without_budgets() {
        let start = Instant::now();
        let mut gate = CapacityGate::new(None, None);
        gate.record_spend(start, 1_000.0);
        assert_eq!(gate.admit(start + MINUTE), Ok(()));
    }

    #[test]
    fn should_refuse_until_oldest_window_spend_expires_when_window_budget_is_reached() {
        let start = Instant::now();
        let mut gate = CapacityGate::new(Some(1.0), None);
        gate.record_spend(start, 0.4);
        assert_eq!(gate.admit(start + MINUTE), Ok(()));
        gate.record_spend(start + 2 * MINUTE, 0.6);

        let refusal = gate.admit(start + 3 * MINUTE).unwrap_err();
        assert!(matches!(
            refusal,
            CapacityRefusal::BudgetReached {
                window: BudgetWindow::FiveHours,
                budget_usd: 1.0,
                ..
            }
        ));
        assert_eq!(refusal.retry_after(), FIVE_HOURS - 3 * MINUTE);
        assert_eq!(gate.admit(start + FIVE_HOURS), Ok(()));
    }

    #[test]
    fn should_count_spend_older_than_five_hours_against_weekly_budget() {
        let start = Instant::now();
        let mut gate = CapacityGate::new(Some(1.0), Some(1.5));
        gate.record_spend(start, 0.9);
        gate.record_spend(start + FIVE_HOURS, 0.9);
        let now = start + FIVE_HOURS + MINUTE;

        let refusal = gate.admit(now).unwrap_err();
        assert!(matches!(
            refusal,
            CapacityRefusal::BudgetReached {
                window: BudgetWindow::SevenDays,
                ..
            }
        ));
        assert_eq!(refusal.retry_after(), SEVEN_DAYS - FIVE_HOURS - MINUTE);
        assert_eq!(gate.admit(start + SEVEN_DAYS), Ok(()));
        assert_eq!(gate.spend.len(), 1, "expired spend should be forgotten");
    }

    #[test]
    fn should_refuse_until_pause_ends_and_keep_longer_pause() {
        let start = Instant::now();
        let mut gate = CapacityGate::new(None, None);
        gate.pause_until(start + 10 * MINUTE);
        gate.pause_until(start + 5 * MINUTE);

        assert_eq!(
            gate.admit(start + MINUTE),
            Err(CapacityRefusal::Paused {
                retry_after: 9 * MINUTE
            })
        );
        assert_eq!(gate.admit(start + 10 * MINUTE), Ok(()));
        assert_eq!(gate.paused_until, None);
    }

    #[test]
    fn should_ignore_costs_that_are_not_finite_and_positive() {
        let start = Instant::now();
        let mut gate = CapacityGate::new(Some(0.5), None);
        for cost in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
            gate.record_spend(start, cost);
        }
        assert_eq!(gate.admit(start), Ok(()));
        assert!(gate.spend.is_empty());
    }
}
