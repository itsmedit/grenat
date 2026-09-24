//! Fonctions globales : `puts`, `raise`, `spawn`, `budget`, `within`, `test`…

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
                return raise("ArgumentError", format!("option de budget invalide `{option}: {}`", v.inspect()));
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

/// `None` si `name` n'est pas une fonction intégrée.
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
                    return raise("TypeError", format!("`size:` attend un entier, reçu {}", other.inspect()));
                }
            };
            interp.spawn_pool(&arg(&args, 0, name)?, size)
        })(),
        "budget" if args.is_empty() => Ok(Value::Budget(interp.budgets.last().expect("budget global").clone())),
        "budget" => budget_from_args(&args).map(|b| Value::Budget(Arc::new(b))),
        "within" => (|| {
            let Value::Budget(budget) = arg(&args, 0, "within")? else {
                return raise("TypeError", "`within` attend un budget : `within budget(usd: 1.00) do … end`");
            };
            interp.within(budget, &block(&args, "within")?)
        })(),
        // Phase 1 : pas encore de journal durable, le bloc est exécuté directement.
        "step" => block(&args, "step").and_then(|b| interp.call_block(&b, Vec::new())),
        "approve!" => (|| {
            let message = arg(&args, 0, "approve!")?.to_display();
            if interp.ask_human(&message)? {
                Ok(Value::Nil)
            } else {
                raise("ApprovalDenied", format!("refusé par l'humain : {message}"))
            }
        })(),
        "with_human" => (|| {
            let policy = arg(&args, 0, "with_human")?;
            let body = block(&args, "with_human")?;
            let saved = interp.approver.borrow_mut().replace(policy);
            let result = interp.call_block(&body, Vec::new());
            *interp.approver.borrow_mut() = saved;
            result
        })(),
        "deny_all" | "approve_all" => Ok(Value::Symbol(name.into())),
        "test" => (|| {
            let title = arg(&args, 0, "test")?.to_display();
            let body = block(&args, "test")?;
            interp.tests.borrow_mut().push((title, body));
            Ok(Value::Nil)
        })(),
        "assert" => (|| {
            if arg(&args, 0, "assert")?.truthy() {
                return Ok(Value::Nil);
            }
            let message = args.pos.get(1).map_or("assertion échouée".into(), Value::to_display);
            raise("AssertionError", message)
        })(),
        "assert_equal" => (|| {
            let (expected, actual) = (arg(&args, 0, name)?, arg(&args, 1, name)?);
            if equal(&expected, &actual) {
                Ok(Value::Nil)
            } else {
                raise("AssertionError", format!("attendu {}, obtenu {}", expected.inspect(), actual.inspect()))
            }
        })(),
        "assert_raises" => (|| {
            let Value::Type(expected) = arg(&args, 0, name)? else {
                return raise("TypeError", "`assert_raises` attend un type d'erreur");
            };
            match interp.call_block(&block(&args, name)?, Vec::new()) {
                Err(Ctrl::Raise(e)) if error_is_a(&e.ty, &expected) => Ok(Value::Error(e)),
                Err(other) => Err(other),
                Ok(_) => raise("AssertionError", format!("`{expected}` attendue, aucune erreur levée")),
            }
        })(),
        "loop" => (|| {
            let body = block(&args, "loop")?;
            loop {
                interp.call_block(&body, Vec::new())?;
            }
        })(),
        "sleep" => (|| {
            let seconds = number(&arg(&args, 0, "sleep")?).unwrap_or(0.0);
            std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
            Ok(Value::Nil)
        })(),
        "exit" => Err(Ctrl::Exit(args.pos.first().and_then(|v| number(v)).unwrap_or(0.0) as i32)),
        _ => return None,
    })
}

pub(crate) fn raise_value<'p>(args: Args<'p>) -> R<'p> {
    let mut pos = args.pos.into_iter();
    let error = match pos.next() {
        None => ErrorVal::new("RuntimeError", "erreur"),
        Some(Value::Error(e)) => return Err(Ctrl::Raise(e)),
        Some(Value::Type(ty)) => {
            let message = pos.next().map_or_else(|| ty.to_string(), |m| m.to_display());
            ErrorVal::new(&ty, message)
        }
        Some(other) => ErrorVal::new("RuntimeError", other.to_display()),
    };
    Err(Ctrl::Raise(Arc::new(error)))
}
