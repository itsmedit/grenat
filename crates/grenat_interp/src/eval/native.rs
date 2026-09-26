//! Calls into native code (compiled by the JIT, or linked into the executable).
//!
//! Values are copied to native code and back. Arrays being references, the
//! content of every array argument is written back after the call, and an
//! array passed twice (or returned) stays one and the same array.
//!
//! When native code cannot finish a call identically (a result it cannot
//! represent, or an error after it modified an array argument), the call is
//! interpreted from the start instead: native code worked on copies, so
//! nothing it did is visible.

use grenat_codegen::{Data, Failure, Trap};

use crate::prelude::*;

impl<'p> Interp<'p> {
    /// Runs `def` natively when it is compiled and the arguments have exactly
    /// its parameter types; `None` means "interpret this call". Taint flows
    /// through: a tainted argument gives a tainted result, as when interpreted.
    pub(crate) fn call_native(&mut self, def: &'p FnDef, args: &Args<'p>) -> Option<R<'p>> {
        let shared = self.shared.clone();
        let jit = shared.jit.as_ref()?;
        if !args.named.is_empty() || args.block.is_some() || !jit.is_compiled(def) {
            return None;
        }
        let tainted = args.pos.iter().any(Value::is_tainted);
        let mut arrays: Vec<(usize, Arc<Mutex<Vec<Value<'p>>>>)> = Vec::new();
        let mut data = Vec::with_capacity(args.pos.len());
        for (i, arg) in args.pos.iter().enumerate() {
            let arg = arg.untainted();
            if let Value::Array(items) = arg {
                if let Some((j, _)) = arrays.iter().find(|(_, a)| Arc::ptr_eq(a, items)) {
                    data.push(Data::Alias(*j));
                    continue;
                }
                arrays.push((i, items.clone()));
            }
            data.push(to_data(arg)?);
        }
        let limit = self.max_depth.saturating_sub(self.depth);
        // asked at native checkpoints: a native loop lets the other tasks run,
        // and stops when its task is cancelled
        let flags = self.cancel.clone();
        let cancelled = move || {
            grenat_green::yield_now();
            flags.iter().any(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
        };
        let result = match jit.call(def, &data, limit, &cancelled)? {
            Ok(returned) => {
                for (i, items) in &arrays {
                    if let Some(content) = &returned.arrays[*i] {
                        *items.borrow_mut() = content.iter().map(value).collect();
                    }
                }
                let result = match returned.value {
                    Data::Alias(i) => args.pos[i].untainted().clone(),
                    other => value(&other),
                };
                Ok(if tainted { result.taint() } else { result })
            }
            Err(Failure::Deopt) => return None,
            // the interpreter would have left the arrays half modified (a
            // cancellation is timing-dependent anyway: it is reported as is)
            Err(Failure::Trap(trap)) if !arrays.is_empty() && trap != Trap::Cancelled => return None,
            Err(Failure::Trap(trap)) => {
                let (ty, message) = trap.error();
                let error = ErrorVal::new(ty, message);
                error.trace.borrow_mut().push((def.name.name.clone(), def.span));
                Err(Ctrl::Raise(Arc::new(error)))
            }
        };
        Some(result)
    }

    /// With `--log`: which functions run as native code, and why the others do not.
    /// `[jit]` for code compiled at load time, `[aot]` for code linked into
    /// the executable.
    pub(crate) fn log_jit(&self, linked: bool) {
        let Some(jit) = &self.jit else { return };
        let tag = if linked { "aot" } else { "jit" };
        let report = jit.report();
        if !report.compiled.is_empty() {
            self.write_err(&format!("[{tag}] native: {}\n", report.compiled.join(", ")));
        }
        for (name, reason) in &report.interpreted {
            self.write_err(&format!("[{tag}] `{name}` stays interpreted: it {reason}\n"));
        }
    }
}

/// A copy of `value` for native code; `None` for what it does not handle
/// (including anything tainted inside an array or a struct).
fn to_data(value: &Value) -> Option<Data> {
    Some(match value {
        Value::Int(n) => Data::Int(*n),
        Value::Float(f) => Data::Float(*f),
        Value::Bool(b) => Data::Bool(*b),
        Value::Str(s) => Data::Str(s.to_string()),
        Value::Array(items) => Data::Array(items.borrow().iter().map(to_data).collect::<Option<_>>()?),
        Value::Record(r) => Data::Record {
            ty: r.ty.to_string(),
            fields: r.fields.iter().map(|(n, v)| Some((n.to_string(), to_data(v)?))).collect::<Option<_>>()?,
        },
        _ => return None,
    })
}

fn value<'p>(data: &Data) -> Value<'p> {
    match data {
        Data::Int(n) => Value::Int(*n),
        Data::Float(f) => Value::Float(*f),
        Data::Bool(b) => Value::Bool(*b),
        Data::Str(s) => Value::str(s),
        Data::Array(items) => Value::array(items.iter().map(value).collect()),
        Data::Record { ty, fields } => {
            Value::record(ty, fields.iter().map(|(n, v)| (n.as_str().into(), value(v))).collect())
        }
        Data::Alias(_) => unreachable!("only a whole result is an alias"),
    }
}
