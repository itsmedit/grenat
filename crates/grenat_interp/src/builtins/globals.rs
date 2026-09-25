//! Global functions: `puts`, `raise`, `spawn`, `budget`, `within`, `test`…

use crate::prelude::*;

use super::*;

pub(crate) fn budget_from_args<'p>(args: &Args<'p>) -> Result<Budget, Ctrl<'p>> {
    let mut budget = Budget::unlimited();
    for (name, value) in &args.named {
        match (name.as_str(), value.untainted()) {
            ("usd", v) if number(v).is_some() => budget.max_usd = number(v),
            ("tokens", Value::Int(n)) => budget.max_tokens = Some(*n as u64),
            ("time", Value::Duration(s)) => budget.max_seconds = Some(*s),
            ("time", Value::Int(n)) => budget.max_seconds = Some(*n as f64),
            (option, v) => {
                return raise("ArgumentError", format!("invalid budget option `{option}: {}`", v.inspect()));
            }
        }
    }
    Ok(budget)
}

pub(crate) fn puts<'p>(interp: &mut Interp<'p>, value: &Value<'p>) -> Result<(), Ctrl<'p>> {
    if let Value::Array(items) = value.untainted() {
        let items = items.borrow().clone();
        for item in &items {
            puts(interp, item)?;
        }
        return Ok(());
    }
    let text = interp.display(value)?;
    interp.write_out(&format!("{text}\n"));
    Ok(())
}

/// `None` if `name` is not a built-in function.
pub(crate) fn call_global<'p>(interp: &mut Interp<'p>, name: &str, args: Args<'p>) -> Option<R<'p>> {
    Some(match name {
        "puts" => (|| {
            if args.pos.is_empty() {
                interp.write_out("\n");
            }
            for v in &args.pos {
                puts(interp, v)?;
            }
            Ok(Value::Nil)
        })(),
        "print" => (|| {
            for v in &args.pos {
                let text = interp.display(v)?;
                interp.write_out(&text);
            }
            Ok(Value::Nil)
        })(),
        "p" => {
            for v in &args.pos {
                interp.write_out(&format!("{}\n", v.inspect()));
            }
            Ok(args.pos.into_iter().next().unwrap_or(Value::Nil))
        }
        "warn" => {
            let text: Vec<String> = args.pos.iter().map(Value::to_display).collect();
            interp.write_err(&format!("{}\n", text.join(" ")));
            Ok(Value::Nil)
        }
        "raise" => raise_value(args),
        "system" | "user" | "assistant" if !interp.prompts.is_empty() => {
            let role = match name {
                "system" => "system",
                "user" => "user",
                _ => "assistant",
            };
            let text = args.pos.iter().map(Value::to_display).collect::<Vec<_>>().join("\n");
            interp.prompt_message(role, text);
            Ok(Value::Nil)
        }
        "spawn" => arg(&args, 0, name).and_then(|t| interp.spawn(&t)),
        "spawn_pool" => (|| {
            let size = match args.named.iter().find(|(n, _)| n == "size").map(|(_, v)| v.untainted().clone()) {
                Some(Value::Int(n)) => n,
                None => 1,
                Some(other) => {
                    return raise("TypeError", format!("`size:` expects an integer, got {}", other.inspect()));
                }
            };
            interp.spawn_pool(&arg(&args, 0, name)?, size)
        })(),
        "budget" if args.is_empty() => Ok(Value::Budget(interp.budgets.last().expect("global budget").clone())),
        "budget" => budget_from_args(&args).map(|b| Value::Budget(Arc::new(b))),
        "within" => (|| {
            let Value::Budget(budget) = arg(&args, 0, "within")? else {
                return raise("TypeError", "`within` expects a budget: `within budget(usd: 1.00) do … end`");
            };
            interp.within(budget, &block(&args, "within")?)
        })(),
        // journaled inside a workflow (see `eval::workflow`)
        "step" => (|| {
            let name = match args.pos.first().map(Value::untainted) {
                Some(Value::Symbol(s) | Value::Str(s)) => s.to_string(),
                _ => return raise("ArgumentError", "`step` expects a name: `step(:research) { … }`"),
            };
            interp.step(&name, &block(&args, "step")?)
        })(),
        "approve!" => (|| {
            let message = arg(&args, 0, "approve!")?.to_display();
            if interp.ask_human(&message)? {
                Ok(Value::Nil)
            } else {
                raise("ApprovalDenied", format!("rejected by the human: {message}"))
            }
        })(),
        "with_human" => (|| {
            let policy = arg(&args, 0, "with_human")?;
            let body = block(&args, "with_human")?;
            let saved = interp.human_double.borrow_mut().replace(policy);
            let result = interp.call_block(&body, Vec::new());
            *interp.human_double.borrow_mut() = saved;
            result
        })(),
        "deny_all" | "approve_all" => Ok(Value::Symbol(name.into())),
        // test doubles (see `eval::doubles`)
        "mock" => (|| {
            let model = match args.pos.first().map(Value::untainted) {
                None => None,
                Some(Value::Symbol(s)) => Some(s.to_string()),
                Some(other) => {
                    return raise("TypeError", format!("`mock` expects a model (`:fast`), got {}", other.inspect()));
                }
            };
            let Some((_, replies)) = args.named.iter().find(|(n, _)| n == "replies") else {
                return raise("ArgumentError", "`mock` expects `replies: [...]`");
            };
            interp.mock(model.as_deref(), replies)
        })(),
        "mcp" => interp.declare_mcp(&args),
        "mock_mcp" => interp.mock_mcp(&args),
        "mock_shell" => (|| {
            let pattern = arg(&args, 0, name)?.to_display();
            interp.mock_shell(&pattern, &args)
        })(),
        "mock_http" => (|| {
            let target = arg(&args, 0, name)?.to_display();
            interp.mock_http(&target, &args)
        })(),
        "cassette" => (|| {
            let name = arg(&args, 0, name)?.to_display();
            interp.cassette(&name, &block(&args, "cassette")?)
        })(),
        "fixture" => (|| {
            let path = arg(&args, 0, name)?.to_display();
            interp.fixture(&path)
        })(),
        "call" => match args.pos.first().map(Value::untainted) {
            Some(Value::Symbol(tool)) => {
                let tool = tool.to_string();
                interp.tool_call_reply(&tool, &args)
            }
            _ => raise("ArgumentError", "`call` expects a tool: `call(:search, query: \"…\")`"),
        },
        "test" => (|| {
            let title = arg(&args, 0, "test")?.to_display();
            let body = block(&args, "test")?;
            interp.tests.borrow_mut().push((title, body));
            Ok(Value::Nil)
        })(),
        // quality measured on a dataset (see `evals`)
        "eval" => (|| {
            let title = arg(&args, 0, name)?.to_display();
            let body = block(&args, name)?;
            let mut eval = crate::evals::EvalDef {
                name: title,
                dataset: String::new(),
                threshold: 1.0,
                concurrency: crate::evals::DEFAULT_CONCURRENCY,
                block: body,
            };
            for (option, value) in &args.named {
                match (option.as_str(), value.untainted()) {
                    ("dataset", Value::Str(path)) => eval.dataset = path.to_string(),
                    ("threshold", v) if number(v).is_some_and(|t| (0.0..=1.0).contains(&t)) => {
                        eval.threshold = number(v).expect("checked");
                    }
                    ("concurrency", Value::Int(n)) if *n > 0 => eval.concurrency = *n as usize,
                    (option, v) => {
                        return raise("ArgumentError", format!("invalid eval option `{option}: {}`", v.inspect()));
                    }
                }
            }
            if eval.dataset.is_empty() {
                return raise("ArgumentError", "`eval` expects a dataset: `eval \"name\", dataset: \"rows.jsonl\" do |row| … end`");
            }
            interp.evals.borrow_mut().push(eval);
            Ok(Value::Nil)
        })(),
        "judge" => (|| {
            let mut pos = args.pos.iter().map(Value::untainted).peekable();
            let model = match pos.peek() {
                Some(Value::Symbol(s)) => {
                    let s = s.to_string();
                    pos.next();
                    Some(s)
                }
                _ => None,
            };
            let Some(question) = pos.next().map(Value::to_display) else {
                return raise("ArgumentError", "`judge` expects a question: `judge(:smart, \"Is it faithful?\", text)`");
            };
            let context: Vec<Value> = pos.cloned().collect();
            interp.judge(model.as_deref(), &question, &context)
        })(),
        "assert" => (|| {
            if arg(&args, 0, "assert")?.truthy() {
                return Ok(Value::Nil);
            }
            let message = args.pos.get(1).map_or("assertion failed".into(), Value::to_display);
            raise("AssertionError", message)
        })(),
        "assert_equal" => (|| {
            let (expected, actual) = (arg(&args, 0, name)?, arg(&args, 1, name)?);
            if equal(&expected, &actual) {
                Ok(Value::Nil)
            } else {
                raise("AssertionError", format!("expected {}, got {}", expected.inspect(), actual.inspect()))
            }
        })(),
        "assert_raises" => (|| {
            let Value::Type(expected) = arg(&args, 0, name)? else {
                return raise("TypeError", "`assert_raises` expects an error type");
            };
            match interp.call_block(&block(&args, name)?, Vec::new()) {
                Err(Ctrl::Raise(e)) if error_is_a(&e.ty, &expected) => Ok(Value::Error(e)),
                Err(other) => Err(other),
                Ok(_) => raise("AssertionError", format!("expected `{expected}`, but nothing was raised")),
            }
        })(),
        "loop" => (|| {
            let body = block(&args, "loop")?;
            loop {
                interp.call_block(&body, Vec::new())?;
            }
        })(),
        "sleep" => (|| {
            let seconds = number(&arg(&args, 0, "sleep")?).unwrap_or(0.0).max(0.0);
            // in slices, so that a cancelled task (`race`, `parallel_map`) stops quickly
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds);
            while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
                interp.check_cancel()?;
                grenat_green::sleep(left.min(std::time::Duration::from_millis(20)));
            }
            interp.check_cancel()?;
            Ok(Value::Nil)
        })(),
        "exit" => Err(Ctrl::Exit(args.pos.first().and_then(|v| number(v)).unwrap_or(0.0) as i32)),
        _ => return None,
    })
}

pub(crate) fn raise_value<'p>(args: Args<'p>) -> R<'p> {
    let mut pos = args.pos.into_iter();
    let error = match pos.next() {
        None => ErrorVal::new("RuntimeError", "error"),
        Some(Value::Error(e)) => return Err(Ctrl::Raise(e)),
        Some(Value::Type(ty)) => {
            let message = pos.next().map_or_else(|| ty.to_string(), |m| m.to_display());
            ErrorVal::new(&ty, message)
        }
        Some(other) => ErrorVal::new("RuntimeError", other.to_display()),
    };
    Err(Ctrl::Raise(Arc::new(error)))
}
