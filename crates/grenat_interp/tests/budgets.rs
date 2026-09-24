//! Spending budgets.

mod common;

use common::*;

#[test]
fn budget_exceeded_is_raised_and_rescuable() {
    let src = format!(
        "{SUMMARY}within budget(usd: 0.001) do\n  summarize(\"x\")\n  puts \"not reached\"\nrescue BudgetExceeded => e\n  puts \"stopped at #{{e.spent}}\"\nend\n"
    );
    // 1,000 input tokens + 1,000 output tokens on Haiku 4.5 = $0.006
    let r = run_with(&src, vec![summary_reply().with_usage(1000, 1000)], &[]);
    let summary = r.result.unwrap();
    assert_eq!(r.output, "stopped at $0.0060\n");
    assert_eq!(summary.llm_calls, 1);
    assert!((summary.cost_usd - 0.006).abs() < 1e-9);
}
