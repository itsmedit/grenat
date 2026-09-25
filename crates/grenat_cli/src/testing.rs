//! `grenat test` and `grenat eval`: correctness, then quality.

use std::process::ExitCode;

use grenat_driver::{load, options_for, render_runtime_error};
use grenat_interp::EvalReport;

/// Runs the `test` blocks of each file; a test never reaches a real model.
pub fn test(paths: &[String]) -> ExitCode {
    let (mut passed, mut failed) = (0, 0);
    for path in paths {
        let Some((src, program)) = load(path, false) else {
            failed += 1;
            continue;
        };
        match grenat_interp::run_tests(&program, options_for(path)) {
            Ok(outcomes) => {
                for outcome in outcomes {
                    match outcome.error {
                        None => {
                            passed += 1;
                            eprintln!("✓ {}", outcome.name);
                        }
                        Some(error) => {
                            failed += 1;
                            eprintln!("✗ {}", outcome.name);
                            render_runtime_error(path, &src, &error);
                        }
                    }
                }
            }
            Err(error) => {
                failed += 1;
                render_runtime_error(path, &src, &error);
            }
        }
    }
    eprintln!("\n{passed} passed, {failed} failed");
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// `grenat eval <file> [name]`: runs the evals (whose name contains `name`)
/// and reports each one; fails if one is under its threshold.
pub fn eval(args: &[String]) -> ExitCode {
    let (path, filter) = match args {
        [path] => (path, None),
        [path, filter] => (path, Some(filter.as_str())),
        _ => {
            eprintln!("usage: grenat eval <file.grn> [name]");
            return ExitCode::from(2);
        }
    };
    let Some((src, program)) = load(path, false) else { return ExitCode::FAILURE };
    let reports = match grenat_interp::run_evals(&program, options_for(path), filter) {
        Ok(reports) => reports,
        Err(error) => {
            render_runtime_error(path, &src, &error);
            return ExitCode::FAILURE;
        }
    };
    if reports.is_empty() {
        eprintln!("no eval {}", filter.map_or("in this file".into(), |f| format!("matches `{f}`")));
        return ExitCode::FAILURE;
    }
    for report in &reports {
        eprint!("{}", describe(report));
    }
    let failed = reports.iter().filter(|r| !r.passed()).count();
    eprintln!("\n{} passed, {failed} failed", reports.len() - failed);
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// One eval: its verdict, score, cost and duration, then the rows that failed.
fn describe(report: &EvalReport) -> String {
    let mark = if report.passed() { "✓" } else { "✗" };
    if let Some(error) = &report.error {
        return format!("{mark} {}: {}: {}\n", report.name, error.ty, error.message);
    }
    let errors = report.rows.iter().filter(|r| r.error.is_some()).count();
    let mut text = format!(
        "{mark} {} · score {:.2} (threshold {:.2}) · {} row(s){} · ${:.4} · {:.1}s\n",
        report.name,
        report.mean(),
        report.threshold,
        report.rows.len(),
        if errors > 0 { format!(", {errors} failed") } else { String::new() },
        report.cost_usd(),
        report.seconds,
    );
    for (i, row) in report.rows.iter().enumerate() {
        if let Some(error) = &row.error {
            text.push_str(&format!("    row {}: {}: {}\n", i + 1, error.ty, error.message));
        }
    }
    text
}
