//! Built-in modules: `File`, `Dir`, `Math`, `Env`, `Json`, `Runtime`, `Cli`, `Time`
//! (and `Http`, in its own module).

use crate::prelude::*;

use crate::llm::value_to_json;

use super::*;

pub(crate) fn call_static<'p>(interp: &mut Interp<'p>, ty: &str, name: &str, args: Args<'p>) -> R<'p> {
    let unknown = || raise("NoMethodError", format!("unknown method `{ty}.{name}`"));
    match (ty, name) {
        ("Http", _) => call_http(interp, name, args),
        ("File", "read") => {
            let path = str_arg(&args, 0, name)?;
            interp.check_fs("fs.read", &path)?;
            std::fs::read_to_string(&*path).map(Value::str).or_else(|e| io_error("reading", &path, e))
        }
        ("File", "lines") => {
            let path = str_arg(&args, 0, name)?;
            interp.check_fs("fs.read", &path)?;
            let text = std::fs::read_to_string(&*path).or_else(|e| io_error("reading", &path, e))?;
            Ok(Value::array(text.lines().map(Value::str).collect()))
        }
        ("File", "directory?" | "exist?") => {
            let path = str_arg(&args, 0, name)?;
            interp.check_fs("fs.read", &path)?;
            let p = std::path::Path::new(&*path);
            Ok(Value::Bool(if name == "exist?" { p.exists() } else { p.is_dir() }))
        }
        ("File", "write") => {
            let (path, content) = (str_arg(&args, 0, name)?, arg(&args, 1, name)?);
            interp.check_fs("fs.write", &path)?;
            if content.contains_taint() {
                return raise(
                    "TaintError",
                    "an untrusted value reaches `File.write` (effect `fs.write`) without validation",
                );
            }
            std::fs::write(&*path, content.to_display()).or_else(|e| io_error("writing", &path, e))?;
            Ok(Value::Nil)
        }
        ("Dir", "list") => {
            let path = str_arg(&args, 0, name)?;
            interp.check_fs("fs.read", &path)?;
            let entries = std::fs::read_dir(&*path).or_else(|e| io_error("reading directory", &path, e))?;
            let mut names: Vec<String> =
                entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            Ok(Value::array(names.into_iter().map(Value::str).collect()))
        }
        ("Math", "pi") => Ok(Value::Float(std::f64::consts::PI)),
        ("Math", "sqrt" | "log" | "sin" | "cos" | "exp") => {
            let x = number(&arg(&args, 0, name)?).map_or_else(|| raise("TypeError", "expected a number"), Ok)?;
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
                (Err(_), None) => raise("KeyError", format!("missing environment variable `{key}`")),
            }
        }
        ("Json", "dump" | "generate") => Ok(Value::str(value_to_json(&arg(&args, 0, name)?).to_string())),
        ("Json", "parse") => {
            // what untrusted text holds is untrusted
            let tainted = arg(&args, 0, name)?.contains_taint();
            let text = str_arg(&args, 0, name)?;
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) if tainted => Ok(json_to_untyped(&json).taint()),
                Ok(json) => Ok(json_to_untyped(&json)),
                Err(e) => raise("ParseError", format!("invalid JSON: {e}")),
            }
        }
        ("Runtime", "on_approval") => {
            *interp.approver.borrow_mut() = Some(block(&args, name)?);
            Ok(Value::Nil)
        }
        ("Cli", "confirm") => {
            let message = arg(&args, 0, name)?.to_display();
            interp.write_err(&format!("{message} (y/N) "));
            let answer = interp.read_line()?.unwrap_or_default();
            Ok(Value::Bool(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")))
        }
        ("Cli", "ask") => {
            let message = arg(&args, 0, name)?.to_display();
            interp.write_err(&format!("{message} "));
            Ok(interp.read_line()?.map_or(Value::Nil, Value::str))
        }
        ("Time", "now") => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            Ok(Value::Float(now.as_secs_f64()))
        }
        _ => unknown(),
    }
}

pub(crate) fn json_to_untyped<'p>(json: &serde_json::Value) -> Value<'p> {
    use serde_json::Value as J;
    match json {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => n.as_i64().map_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0)), Value::Int),
        J::String(s) => Value::str(s),
        J::Array(items) => Value::array(items.iter().map(json_to_untyped).collect()),
        J::Object(o) => {
            Value::Hash(Arc::new(Mutex::new(o.iter().map(|(k, v)| (Value::str(k), json_to_untyped(v))).collect())))
        }
    }
}
