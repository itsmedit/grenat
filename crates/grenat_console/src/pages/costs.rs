//! Costs: what the models cost, by agent, workflow, model and day.

use grenat_ops::calls::{self, By, Total};

use super::{Ctx, Result, page};
use crate::html::{escape, money, table};

/// The periods offered, in days.
const PERIODS: [u32; 3] = [1, 7, 30];

pub(crate) fn show(ctx: &mut Ctx) -> Result {
    let days = ctx.param("days").and_then(|d| d.parse().ok()).filter(|d| PERIODS.contains(d)).unwrap_or(7);
    let recorded = calls::since(ctx.db, ctx.now - f64::from(days) * 86_400.0)?;
    let tabs: String = PERIODS
        .iter()
        .map(|p| {
            let current = if *p == days { " aria-current=\"page\"" } else { "" };
            format!("<a href=\"/costs?days={p}\"{current}>{}</a>", if *p == 1 { "24 h".into() } else { format!("{p} days") })
        })
        .collect();
    let total: f64 = recorded.iter().map(|c| c.cost_usd).sum();
    let tokens: i64 = recorded.iter().map(|c| c.input_tokens + c.output_tokens).sum();
    let section = |title: &str, by: By, outside: &str| {
        let totals = calls::totals(&recorded, by);
        let most = totals.iter().map(|t| t.cost_usd).fold(0.0, f64::max);
        format!("<h2>{title}</h2>{}", table(&[title, "Calls", "Tokens in", "Tokens out", "Cost", ""], &rows(&totals, most, outside), "No model call."))
    };
    let body = format!(
        "<div class=\"tabs\">{tabs}</div><div class=\"cards\"><div class=\"card\"><b>{}</b><span>spent</span></div>\
         <div class=\"card\"><b>{}</b><span>model calls</span></div><div class=\"card\"><b>{tokens}</b><span>tokens</span></div></div>\
         {}{}{}{}",
        money(total),
        recorded.len(),
        section("Agents", By::Agent, "outside agents"),
        section("Workflows", By::Workflow, "outside workflows"),
        section("Models", By::Model, ""),
        section("Days", By::Day, ""),
    );
    page("Costs", "/costs", body)
}

fn rows(totals: &[Total], most: f64, outside: &str) -> Vec<Vec<String>> {
    totals
        .iter()
        .map(|t| {
            let key = match &t.key {
                Some(key) => escape(key),
                None => format!("<span class=\"muted\">{}</span>", escape(outside)),
            };
            let width = if most > 0.0 { (t.cost_usd / most * 100.0).round() } else { 0.0 };
            vec![
                key,
                format!("<span class=\"num\">{}</span>", t.calls),
                format!("<span class=\"num\">{}</span>", t.input_tokens),
                format!("<span class=\"num\">{}</span>", t.output_tokens),
                money(t.cost_usd),
                format!("<div class=\"bar\" style=\"width:{width}%\"></div>"),
            ]
        })
        .collect()
}
