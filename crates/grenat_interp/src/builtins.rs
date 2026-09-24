//! Bibliothèque intégrée : fonctions globales, méthodes des types de base,
//! modules `File`, `Dir`, `Math`, `Env`, `Json`, `Runtime`, `Cli`, `Time`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::eval::{compare, error_is_a};
use crate::llm::value_to_json;
use crate::value::{Budget, ErrorVal, Value, equal};
use crate::{Args, Ctrl, Interp, R, raise};

// ── Arguments ────────────────────────────────────────────────

fn arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<Value<'p>, Ctrl<'p>> {
    match args.pos.get(i) {
        Some(v) => Ok(v.clone()),
        None => raise("ArgumentError", format!("`{method}` attend au moins {} argument(s)", i + 1)),
    }
}

fn int_arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<i64, Ctrl<'p>> {
    match arg(args, i, method)?.untainted() {
        Value::Int(n) => Ok(*n),
        other => raise("TypeError", format!("`{method}` attend un entier, reçu {}", other.type_name())),
    }
}

fn str_arg<'p>(args: &Args<'p>, i: usize, method: &str) -> Result<Rc<str>, Ctrl<'p>> {
    match arg(args, i, method)?.untainted() {
        Value::Str(s) | Value::Symbol(s) => Ok(s.clone()),
        other => raise("TypeError", format!("`{method}` attend une chaîne, reçu {}", other.type_name())),
    }
}

fn block<'p>(args: &Args<'p>, method: &str) -> Result<Value<'p>, Ctrl<'p>> {
    match &args.block {
        Some(b) => Ok(b.clone()),
        None => raise("ArgumentError", format!("`{method}` attend un bloc")),
    }
}

fn number(v: &Value) -> Option<f64> {
    match v.untainted() {
        Value::Int(n) => Some(*n as f64),
        Value::Float(f) | Value::Money(f) | Value::Duration(f) => Some(*f),
        _ => None,
    }
}

fn io_error<'p, T>(action: &str, path: &str, e: std::io::Error) -> Result<T, Ctrl<'p>> {
    raise("IoError", format!("{action} `{path}` : {e}"))
}

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

fn puts<'p>(interp: &mut Interp<'p>, value: &Value<'p>) -> Result<(), Ctrl<'p>> {
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

// ── Fonctions globales ───────────────────────────────────────

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
        "spawn" | "spawn_pool" => arg(&args, 0, name).and_then(|t| interp.spawn(&t)),
        "budget" if args.is_empty() => Ok(Value::Budget(interp.budgets.last().expect("budget global").clone())),
        "budget" => budget_from_args(&args).map(|b| Value::Budget(Rc::new(b))),
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
            let saved = interp.approver.replace(policy);
            let result = interp.call_block(&body, Vec::new());
            interp.approver = saved;
            result
        })(),
        "deny_all" | "approve_all" => Ok(Value::Symbol(name.into())),
        "test" => (|| {
            let title = arg(&args, 0, "test")?.to_display();
            interp.tests.push((title, block(&args, "test")?));
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

fn raise_value<'p>(args: Args<'p>) -> R<'p> {
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
    Err(Ctrl::Raise(Rc::new(error)))
}

// ── Méthodes des valeurs ─────────────────────────────────────

pub(crate) fn call_method<'p>(interp: &mut Interp<'p>, recv: Value<'p>, name: &str, args: Args<'p>) -> R<'p> {
    // communes à toutes les valeurs
    match name {
        "nil?" => return Ok(Value::Bool(matches!(recv, Value::Nil))),
        "to_s" => return interp.display(&recv).map(Value::str),
        "inspect" => return Ok(Value::str(recv.inspect())),
        "tainted?" => return Ok(Value::Bool(false)),
        "is_a?" => {
            let Value::Type(ty) = arg(&args, 0, name)? else {
                return raise("TypeError", "`is_a?` attend un type");
            };
            return Ok(Value::Bool(interp.is_a(&recv, &ty)));
        }
        "class" => return Ok(Value::Type(recv.type_name().into())),
        _ => {}
    }
    let result = match &recv {
        Value::Int(n) => int_method(interp, *n, name, &args),
        Value::Float(f) => float_method(*f, name, &args),
        Value::Money(m) => match name {
            "to_f" => Some(Ok(Value::Float(*m))),
            _ => None,
        },
        Value::Duration(s) => match name {
            "seconds" | "to_f" => Some(Ok(Value::Float(*s))),
            "minutes" => Some(Ok(Value::Float(s / 60.0))),
            _ => None,
        },
        Value::Str(s) => str_method(s, name, &args),
        Value::Symbol(s) => match name {
            "to_sym" => Some(Ok(recv.clone())),
            "size" | "length" => Some(Ok(Value::Int(s.chars().count() as i64))),
            _ => None,
        },
        Value::Array(items) => array_method(interp, items, name, &args),
        Value::Hash(entries) => hash_method(interp, entries, name, &args),
        Value::Range(lo, hi, inclusive) => {
            let end = if *inclusive { *hi } else { hi - 1 };
            let items = Value::array((*lo..=end).map(Value::Int).collect());
            match name {
                "include?" | "cover?" => Some(int_arg(&args, 0, name).map(|n| Value::Bool(n >= *lo && n <= end))),
                "first" if args.pos.is_empty() => Some(Ok(Value::Int(*lo))),
                "last" if args.pos.is_empty() => Some(Ok(Value::Int(end))),
                "size" | "count" if args.block.is_none() => Some(Ok(Value::Int((end - lo + 1).max(0)))),
                _ => match &items {
                    Value::Array(items) => array_method(interp, items, name, &args),
                    _ => unreachable!(),
                },
            }
        }
        Value::Record(r) => match name {
            "with" => Some((|| {
                let mut fields = r.fields.clone();
                for (field_name, value) in &args.named {
                    match fields.iter_mut().find(|(n, _)| &**n == field_name) {
                        Some((_, v)) => *v = value.clone(),
                        None => {
                            return raise("ArgumentError", format!("champ inconnu `{field_name}:` pour `{}`", r.ty));
                        }
                    }
                }
                Ok(Value::record(&r.ty, fields))
            })()),
            "to_h" => Some(Ok(Value::Hash(Rc::new(RefCell::new(
                r.fields.iter().map(|(k, v)| (Value::Symbol(k.clone()), v.clone())).collect(),
            ))))),
            _ => None,
        },
        Value::Variant(v) if &*v.enum_name == "Result" => {
            let ok = &*v.name == "Ok";
            let payload = v.fields[0].1.clone();
            match name {
                "ok?" => Some(Ok(Value::Bool(ok))),
                "err?" => Some(Ok(Value::Bool(!ok))),
                "value" | "unwrap" if ok => Some(Ok(payload)),
                "value" | "unwrap" => Some(match payload {
                    Value::Error(e) => Err(Ctrl::Raise(e)),
                    other => raise("ResultError", other.to_display()),
                }),
                "error" => Some(Ok(if ok { Value::Nil } else { payload })),
                "unwrap_or" => Some(if ok { Ok(payload) } else { arg(&args, 0, name) }),
                "or_else" if ok => Some(Ok(payload)),
                "or_else" => Some(block(&args, name).and_then(|b| interp.call_block(&b, vec![payload]))),
                _ => None,
            }
        }
        Value::Variant(v) => match name {
            "name" => Some(Ok(Value::str(&*v.name))),
            _ => None,
        },
        Value::Error(e) => match name {
            "message" => Some(Ok(Value::str(&e.message))),
            "type" => Some(Ok(Value::str(&*e.ty))),
            "full_message" => Some(Ok(Value::str(format!("{} : {}", e.ty, e.message)))),
            _ => None,
        },
        Value::Budget(b) => match name {
            "spent" => Some(Ok(Value::Money(b.spent_usd.get()))),
            "tokens" => Some(Ok(Value::Int(b.tokens.get() as i64))),
            "remaining" => {
                Some(Ok(b.max_usd.map_or(Value::Nil, |max| Value::Money((max - b.spent_usd.get()).max(0.0)))))
            }
            _ => None,
        },
        Value::Closure(_) => match name {
            "call" => Some(interp.call_block(&recv, args.pos.clone())),
            _ => None,
        },
        _ => None,
    };
    result.unwrap_or_else(|| raise("NoMethodError", format!("méthode `{name}` inconnue pour {}", recv.type_name())))
}

fn int_method<'p>(interp: &mut Interp<'p>, n: i64, name: &str, args: &Args<'p>) -> Option<R<'p>> {
    let duration = |unit: f64| Some(Ok(Value::Duration(n as f64 * unit)));
    Some(match name {
        "times" => (|| {
            let body = block(args, name)?;
            for i in 0..n {
                interp.call_block(&body, vec![Value::Int(i)])?;
            }
            Ok(Value::Int(n))
        })(),
        "upto" => (|| {
            let (end, body) = (int_arg(args, 0, name)?, block(args, name)?);
            for i in n..=end {
                interp.call_block(&body, vec![Value::Int(i)])?;
            }
            Ok(Value::Int(n))
        })(),
        "to_i" | "round" | "floor" | "ceil" => Ok(Value::Int(n)),
        "to_f" => Ok(Value::Float(n as f64)),
        "abs" => Ok(Value::Int(n.abs())),
        "zero?" => Ok(Value::Bool(n == 0)),
        "even?" => Ok(Value::Bool(n % 2 == 0)),
        "odd?" => Ok(Value::Bool(n % 2 != 0)),
        "succ" => Ok(Value::Int(n + 1)),
        "pred" => Ok(Value::Int(n - 1)),
        "between?" => (|| {
            let (lo, hi) = (arg(args, 0, name)?, arg(args, 1, name)?);
            let v = Value::Int(n);
            Ok(Value::Bool(compare(&v, &lo).is_some_and(|o| o.is_ge()) && compare(&v, &hi).is_some_and(|o| o.is_le())))
        })(),
        "s" | "sec" | "second" | "seconds" => return duration(1.0),
        "min" | "minute" | "minutes" => return duration(60.0),
        "h" | "hour" | "hours" => return duration(3600.0),
        "day" | "days" => return duration(86_400.0),
        _ => return None,
    })
}

fn float_method<'p>(f: f64, name: &str, args: &Args<'p>) -> Option<R<'p>> {
    Some(match name {
        "round" => match args.pos.first() {
            Some(Value::Int(digits)) => {
                let factor = 10f64.powi(*digits as i32);
                Ok(Value::Float((f * factor).round() / factor))
            }
            _ => Ok(Value::Int(f.round() as i64)),
        },
        "floor" => Ok(Value::Int(f.floor() as i64)),
        "ceil" => Ok(Value::Int(f.ceil() as i64)),
        "to_i" => Ok(Value::Int(f as i64)),
        "to_f" => Ok(Value::Float(f)),
        "abs" => Ok(Value::Float(f.abs())),
        "zero?" => Ok(Value::Bool(f == 0.0)),
        "between?" => (|| {
            let (lo, hi) = (arg(args, 0, name)?, arg(args, 1, name)?);
            let v = Value::Float(f);
            Ok(Value::Bool(compare(&v, &lo).is_some_and(|o| o.is_ge()) && compare(&v, &hi).is_some_and(|o| o.is_le())))
        })(),
        _ => return None,
    })
}

fn str_method<'p>(s: &Rc<str>, name: &str, args: &Args<'p>) -> Option<R<'p>> {
    let text = |t: String| Some(Ok(Value::str(t)));
    let strings = |items: Vec<&str>| Some(Ok(Value::array(items.into_iter().map(Value::str).collect())));
    match name {
        "size" | "length" => Some(Ok(Value::Int(s.chars().count() as i64))),
        "upcase" => text(s.to_uppercase()),
        "downcase" => text(s.to_lowercase()),
        "capitalize" => {
            let mut chars = s.chars();
            text(
                chars
                    .next()
                    .map_or(String::new(), |c| c.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect()),
            )
        }
        "strip" => text(s.trim().to_string()),
        "lstrip" => text(s.trim_start().to_string()),
        "rstrip" => text(s.trim_end().to_string()),
        "reverse" => text(s.chars().rev().collect()),
        "chars" => Some(Ok(Value::array(s.chars().map(|c| Value::str(c.to_string())).collect()))),
        "lines" => strings(s.lines().collect()),
        "split" => match args.pos.first().map(Value::untainted) {
            Some(Value::Str(sep)) => strings(s.split(&**sep).collect()),
            _ => strings(s.split_whitespace().collect()),
        },
        "empty?" => Some(Ok(Value::Bool(s.is_empty()))),
        "include?" => Some(str_arg(args, 0, name).map(|x| Value::Bool(s.contains(&*x)))),
        "start_with?" => Some(str_arg(args, 0, name).map(|x| Value::Bool(s.starts_with(&*x)))),
        "end_with?" => Some(str_arg(args, 0, name).map(|x| Value::Bool(s.ends_with(&*x)))),
        "index" => Some(
            str_arg(args, 0, name)
                .map(|x| s.find(&*x).map_or(Value::Nil, |byte| Value::Int(s[..byte].chars().count() as i64))),
        ),
        "sub" | "gsub" => Some((|| {
            let (from, to) = (str_arg(args, 0, name)?, str_arg(args, 1, name)?);
            Ok(Value::str(if name == "sub" { s.replacen(&*from, &to, 1) } else { s.replace(&*from, &to) }))
        })()),
        "truncate" => Some(int_arg(args, 0, name).map(|n| {
            let n = n.max(1) as usize;
            if s.chars().count() <= n {
                Value::Str(s.clone())
            } else {
                Value::str(format!("{}…", s.chars().take(n - 1).collect::<String>()))
            }
        })),
        "ljust" | "rjust" => Some(int_arg(args, 0, name).map(|n| {
            let pad = " ".repeat((n.max(0) as usize).saturating_sub(s.chars().count()));
            Value::str(if name == "ljust" { format!("{s}{pad}") } else { format!("{pad}{s}") })
        })),
        "to_i" => Some(Ok(Value::Int(s.trim().parse().unwrap_or(0)))),
        "to_f" => Some(Ok(Value::Float(s.trim().parse().unwrap_or(0.0)))),
        "to_sym" => Some(Ok(Value::Symbol(s.clone()))),
        _ => None,
    }
}

fn array_method<'p>(
    interp: &mut Interp<'p>,
    items: &Rc<RefCell<Vec<Value<'p>>>>,
    name: &str,
    args: &Args<'p>,
) -> Option<R<'p>> {
    let snapshot = || items.borrow().clone();
    let each = |interp: &mut Interp<'p>, f: &mut dyn FnMut(Value<'p>, Value<'p>) -> Result<bool, Ctrl<'p>>| {
        let body = block(args, name)?;
        for item in snapshot() {
            let result = interp.call_block(&body, vec![item.clone()])?;
            if !f(item, result)? {
                break;
            }
        }
        Ok(())
    };
    Some(match name {
        "size" | "length" if args.block.is_none() => Ok(Value::Int(items.borrow().len() as i64)),
        "count" => match &args.block {
            None => Ok(Value::Int(items.borrow().len() as i64)),
            Some(_) => {
                let mut n = 0;
                each(interp, &mut |_, r| {
                    n += i64::from(r.truthy());
                    Ok(true)
                })
                .map(|()| Value::Int(n))
            }
        },
        "empty?" => Ok(Value::Bool(items.borrow().is_empty())),
        "first" => match args.pos.first() {
            Some(Value::Int(n)) => Ok(Value::array(snapshot().into_iter().take(*n as usize).collect())),
            _ => Ok(items.borrow().first().cloned().unwrap_or(Value::Nil)),
        },
        "last" => match args.pos.first() {
            Some(Value::Int(n)) => {
                let all = snapshot();
                Ok(Value::array(all[all.len().saturating_sub(*n as usize)..].to_vec()))
            }
            _ => Ok(items.borrow().last().cloned().unwrap_or(Value::Nil)),
        },
        "each" => each(interp, &mut |_, _| Ok(true)).map(|()| Value::Array(items.clone())),
        "each_with_index" => (|| {
            let body = block(args, name)?;
            for (i, item) in snapshot().into_iter().enumerate() {
                interp.call_block(&body, vec![item, Value::Int(i as i64)])?;
            }
            Ok(Value::Array(items.clone()))
        })(),
        // Phase 1 : `parallel_map` est séquentiel ; la concurrence arrive en phase 3.
        "map" | "collect" | "parallel_map" => {
            let mut out = Vec::new();
            each(interp, &mut |_, r| {
                out.push(r);
                Ok(true)
            })
            .map(|()| Value::array(out))
        }
        "flat_map" => {
            let mut out = Vec::new();
            each(interp, &mut |_, r| {
                match r.untainted() {
                    Value::Array(inner) => out.extend(inner.borrow().iter().cloned()),
                    _ => out.push(r),
                }
                Ok(true)
            })
            .map(|()| Value::array(out))
        }
        "select" | "filter" | "reject" => {
            let keep = name != "reject";
            let mut out = Vec::new();
            each(interp, &mut |item, r| {
                if r.truthy() == keep {
                    out.push(item);
                }
                Ok(true)
            })
            .map(|()| Value::array(out))
        }
        "partition" => {
            let (mut yes, mut no) = (Vec::new(), Vec::new());
            each(interp, &mut |item, r| {
                if r.truthy() {
                    yes.push(item)
                } else {
                    no.push(item)
                }
                Ok(true)
            })
            .map(|()| Value::array(vec![Value::array(yes), Value::array(no)]))
        }
        "find" | "detect" => {
            let mut found = Value::Nil;
            each(interp, &mut |item, r| {
                if r.truthy() {
                    found = item;
                    return Ok(false);
                }
                Ok(true)
            })
            .map(|()| found)
        }
        "any?" | "all?" | "none?" => {
            let predicate = args.block.is_some();
            let results: Vec<bool> = if predicate {
                let mut out = Vec::new();
                if let Err(e) = each(interp, &mut |_, r| {
                    out.push(r.truthy());
                    Ok(true)
                }) {
                    return Some(Err(e));
                }
                out
            } else {
                snapshot().iter().map(Value::truthy).collect()
            };
            Ok(Value::Bool(match name {
                "any?" => results.iter().any(|b| *b),
                "all?" => results.iter().all(|b| *b),
                _ => !results.iter().any(|b| *b),
            }))
        }
        "include?" => arg(args, 0, name).map(|x| Value::Bool(items.borrow().iter().any(|i| equal(i, &x)))),
        "index" | "find_index" => arg(args, 0, name)
            .map(|x| items.borrow().iter().position(|i| equal(i, &x)).map_or(Value::Nil, |i| Value::Int(i as i64))),
        "sum" => (|| {
            let values = match &args.block {
                Some(body) => {
                    let body = body.clone();
                    snapshot().into_iter().map(|i| interp.call_block(&body, vec![i])).collect::<Result<Vec<_>, _>>()?
                }
                None => snapshot(),
            };
            let mut total = Value::Int(0);
            for v in values {
                total = interp.binop(grenat_ast::BinOp::Add, total, v)?;
            }
            Ok(total)
        })(),
        "min" | "max" => {
            let all = snapshot();
            let mut best: Option<Value<'p>> = None;
            for v in all {
                let better = match &best {
                    None => true,
                    Some(b) => match compare(&v, b) {
                        Some(o) => {
                            if name == "min" {
                                o.is_lt()
                            } else {
                                o.is_gt()
                            }
                        }
                        None => return Some(raise("TypeError", format!("`{name}` : valeurs non comparables"))),
                    },
                };
                if better {
                    best = Some(v);
                }
            }
            Ok(best.unwrap_or(Value::Nil))
        }
        "sort" | "sort_by" | "min_by" | "max_by" => (|| {
            let all = snapshot();
            let keys = match &args.block {
                Some(body) => {
                    let body = body.clone();
                    all.iter().map(|i| interp.call_block(&body, vec![i.clone()])).collect::<Result<Vec<_>, _>>()?
                }
                None => all.clone(),
            };
            let mut indexed: Vec<usize> = (0..all.len()).collect();
            let mut incomparable = false;
            indexed.sort_by(|&a, &b| {
                compare(&keys[a], &keys[b]).unwrap_or_else(|| {
                    incomparable = true;
                    std::cmp::Ordering::Equal
                })
            });
            if incomparable {
                return raise("TypeError", format!("`{name}` : valeurs non comparables"));
            }
            Ok(match name {
                "min_by" => indexed.first().map_or(Value::Nil, |&i| all[i].clone()),
                "max_by" => indexed.last().map_or(Value::Nil, |&i| all[i].clone()),
                _ => Value::array(indexed.into_iter().map(|i| all[i].clone()).collect()),
            })
        })(),
        "reverse" => Ok(Value::array(snapshot().into_iter().rev().collect())),
        "join" => (|| {
            let sep = match args.pos.first() {
                Some(v) => v.to_display(),
                None => String::new(),
            };
            let parts = snapshot().iter().map(|v| interp.display(v)).collect::<Result<Vec<_>, _>>()?;
            Ok(Value::str(parts.join(&sep)))
        })(),
        "push" | "append" => {
            items.borrow_mut().extend(args.pos.iter().cloned());
            Ok(Value::Array(items.clone()))
        }
        "pop" => Ok(items.borrow_mut().pop().unwrap_or(Value::Nil)),
        "shift" => {
            let mut items = items.borrow_mut();
            Ok(if items.is_empty() { Value::Nil } else { items.remove(0) })
        }
        "unshift" => {
            let mut all = args.pos.clone();
            all.extend(snapshot());
            *items.borrow_mut() = all;
            Ok(Value::Array(items.clone()))
        }
        "delete" => arg(args, 0, name).inspect(|x| {
            items.borrow_mut().retain(|i| !equal(i, x));
        }),
        "uniq" => {
            let mut out: Vec<Value<'p>> = Vec::new();
            for v in snapshot() {
                if !out.iter().any(|o| equal(o, &v)) {
                    out.push(v);
                }
            }
            Ok(Value::array(out))
        }
        "compact" => Ok(Value::array(snapshot().into_iter().filter(|v| !matches!(v, Value::Nil)).collect())),
        "take" => {
            int_arg(args, 0, name).map(|n| Value::array(snapshot().into_iter().take(n.max(0) as usize).collect()))
        }
        "drop" => {
            int_arg(args, 0, name).map(|n| Value::array(snapshot().into_iter().skip(n.max(0) as usize).collect()))
        }
        "zip" => (|| {
            let Value::Array(other) = arg(args, 0, name)? else {
                return raise("TypeError", "`zip` attend un tableau");
            };
            let other = other.borrow();
            Ok(Value::array(
                snapshot()
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| Value::array(vec![v, other.get(i).cloned().unwrap_or(Value::Nil)]))
                    .collect(),
            ))
        })(),
        "reduce" | "inject" => (|| {
            let body = block(args, name)?;
            let mut all = snapshot().into_iter();
            let mut acc = match args.pos.first() {
                Some(init) => init.clone(),
                None => all.next().unwrap_or(Value::Nil),
            };
            for v in all {
                acc = interp.call_block(&body, vec![acc, v])?;
            }
            Ok(acc)
        })(),
        "group_by" | "tally" => (|| {
            let mut groups: Vec<(Value<'p>, Value<'p>)> = Vec::new();
            for item in snapshot() {
                let key = match &args.block {
                    Some(body) if name == "group_by" => interp.call_block(&body.clone(), vec![item.clone()])?,
                    _ => item.clone(),
                };
                let slot = groups.iter_mut().find(|(k, _)| equal(k, &key));
                match (name, slot) {
                    ("tally", Some((_, Value::Int(n)))) => *n += 1,
                    ("tally", _) => groups.push((key, Value::Int(1))),
                    (_, Some((_, Value::Array(list)))) => list.borrow_mut().push(item),
                    _ => groups.push((key, Value::array(vec![item]))),
                }
            }
            Ok(Value::Hash(Rc::new(RefCell::new(groups))))
        })(),
        "to_a" | "dup" => Ok(Value::array(snapshot())),
        _ => return None,
    })
}

fn hash_method<'p>(
    interp: &mut Interp<'p>,
    entries: &Rc<RefCell<Vec<(Value<'p>, Value<'p>)>>>,
    name: &str,
    args: &Args<'p>,
) -> Option<R<'p>> {
    let snapshot = || entries.borrow().clone();
    let get = |key: &Value<'p>| entries.borrow().iter().find(|(k, _)| equal(k, key)).map(|(_, v)| v.clone());
    let pairs = || Value::array(snapshot().into_iter().map(|(k, v)| Value::array(vec![k, v])).collect());
    Some(match name {
        "size" | "length" | "count" => Ok(Value::Int(entries.borrow().len() as i64)),
        "empty?" => Ok(Value::Bool(entries.borrow().is_empty())),
        "keys" => Ok(Value::array(snapshot().into_iter().map(|(k, _)| k).collect())),
        "values" => Ok(Value::array(snapshot().into_iter().map(|(_, v)| v).collect())),
        "key?" | "has_key?" | "include?" => arg(args, 0, name).map(|k| Value::Bool(get(&k).is_some())),
        "fetch" => (|| {
            let key = arg(args, 0, name)?;
            match (get(&key), args.pos.get(1)) {
                (Some(v), _) => Ok(v),
                (None, Some(default)) => Ok(default.clone()),
                (None, None) => raise("KeyError", format!("clé absente : {}", key.inspect())),
            }
        })(),
        "delete" => arg(args, 0, name).map(|k| {
            let found = get(&k);
            entries.borrow_mut().retain(|(key, _)| !equal(key, &k));
            found.unwrap_or(Value::Nil)
        }),
        "merge" => (|| {
            let Value::Hash(other) = arg(args, 0, name)? else {
                return raise("TypeError", "`merge` attend un Hash");
            };
            let mut merged = snapshot();
            for (k, v) in other.borrow().iter() {
                match merged.iter_mut().find(|(key, _)| equal(key, k)) {
                    Some((_, slot)) => *slot = v.clone(),
                    None => merged.push((k.clone(), v.clone())),
                }
            }
            Ok(Value::Hash(Rc::new(RefCell::new(merged))))
        })(),
        "to_a" => Ok(pairs()),
        "each" | "map" | "select" | "filter" | "reject" | "any?" | "all?" | "find" | "sum" | "sort_by" | "min_by"
        | "max_by" => {
            let Value::Array(list) = pairs() else { unreachable!() };
            let result = array_method(interp, &list, name, args)?;
            // `select`/`reject` sur un Hash renvoient un Hash
            Ok(match (name, result) {
                ("select" | "filter" | "reject", Ok(Value::Array(kept))) => {
                    let kept = kept
                        .borrow()
                        .iter()
                        .filter_map(|pair| match pair {
                            Value::Array(kv) => {
                                let kv = kv.borrow();
                                Some((kv[0].clone(), kv[1].clone()))
                            }
                            _ => None,
                        })
                        .collect();
                    Value::Hash(Rc::new(RefCell::new(kept)))
                }
                ("each", Ok(_)) => Value::Hash(entries.clone()),
                (_, result) => return Some(result),
            })
        }
        _ => return None,
    })
}

// ── Modules intégrés ─────────────────────────────────────────

pub(crate) fn call_static<'p>(interp: &mut Interp<'p>, ty: &str, name: &str, args: Args<'p>) -> R<'p> {
    let unknown = || raise("NoMethodError", format!("méthode `{ty}.{name}` inconnue"));
    match (ty, name) {
        ("File", "read") => {
            let path = str_arg(&args, 0, name)?;
            std::fs::read_to_string(&*path).map(Value::str).or_else(|e| io_error("lecture de", &path, e))
        }
        ("File", "lines") => {
            let path = str_arg(&args, 0, name)?;
            let text = std::fs::read_to_string(&*path).or_else(|e| io_error("lecture de", &path, e))?;
            Ok(Value::array(text.lines().map(Value::str).collect()))
        }
        ("File", "directory?") => Ok(Value::Bool(std::path::Path::new(&*str_arg(&args, 0, name)?).is_dir())),
        ("File", "exist?") => Ok(Value::Bool(std::path::Path::new(&*str_arg(&args, 0, name)?).exists())),
        ("File", "write") => {
            let (path, content) = (str_arg(&args, 0, name)?, arg(&args, 1, name)?);
            if content.contains_taint() {
                return raise(
                    "TaintError",
                    "une valeur produite par un LLM atteint `File.write` (effet `fs.write`) sans validation",
                );
            }
            std::fs::write(&*path, content.to_display()).or_else(|e| io_error("écriture de", &path, e))?;
            Ok(Value::Nil)
        }
        ("Dir", "list") => {
            let path = str_arg(&args, 0, name)?;
            let entries = std::fs::read_dir(&*path).or_else(|e| io_error("lecture du dossier", &path, e))?;
            let mut names: Vec<String> =
                entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            Ok(Value::array(names.into_iter().map(Value::str).collect()))
        }
        ("Math", "pi") => Ok(Value::Float(std::f64::consts::PI)),
        ("Math", "sqrt" | "log" | "sin" | "cos" | "exp") => {
            let x = number(&arg(&args, 0, name)?).map_or_else(|| raise("TypeError", "nombre attendu"), Ok)?;
            Ok(Value::Float(match name {
                "sqrt" => x.sqrt(),
                "log" => x.ln(),
                "sin" => x.sin(),
                "cos" => x.cos(),
                _ => x.exp(),
            }))
        }
        ("Env", "get") => Ok(std::env::var(&*str_arg(&args, 0, name)?).map_or(Value::Nil, Value::str)),
        ("Env", "fetch") => {
            let key = str_arg(&args, 0, name)?;
            match (std::env::var(&*key), args.pos.get(1)) {
                (Ok(v), _) => Ok(Value::str(v)),
                (Err(_), Some(default)) => Ok(default.clone()),
                (Err(_), None) => raise("KeyError", format!("variable d'environnement `{key}` absente")),
            }
        }
        ("Json", "dump" | "generate") => Ok(Value::str(value_to_json(&arg(&args, 0, name)?).to_string())),
        ("Json", "parse") => {
            let text = str_arg(&args, 0, name)?;
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) => Ok(json_to_untyped(&json)),
                Err(e) => raise("ParseError", format!("JSON invalide : {e}")),
            }
        }
        ("Runtime", "on_approval") => {
            interp.approver = Some(block(&args, name)?);
            Ok(Value::Nil)
        }
        ("Cli", "confirm") => {
            let message = arg(&args, 0, name)?.to_display();
            interp.write_err(&format!("{message} (o/N) "));
            let answer = interp.read_line().unwrap_or_default();
            Ok(Value::Bool(matches!(answer.trim().to_lowercase().as_str(), "o" | "oui" | "y" | "yes")))
        }
        ("Cli", "ask") => {
            let message = arg(&args, 0, name)?.to_display();
            interp.write_err(&format!("{message} "));
            Ok(interp.read_line().map_or(Value::Nil, Value::str))
        }
        ("Time", "now") => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            Ok(Value::Float(now.as_secs_f64()))
        }
        _ => unknown(),
    }
}

fn json_to_untyped<'p>(json: &serde_json::Value) -> Value<'p> {
    use serde_json::Value as J;
    match json {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => n.as_i64().map_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0)), Value::Int),
        J::String(s) => Value::str(s),
        J::Array(items) => Value::array(items.iter().map(json_to_untyped).collect()),
        J::Object(o) => {
            Value::Hash(Rc::new(RefCell::new(o.iter().map(|(k, v)| (Value::str(k), json_to_untyped(v))).collect())))
        }
    }
}
