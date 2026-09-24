//! Calls into native code for the numeric functions compiled by the JIT.

use grenat_codegen::Scalar;

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
        let scalars = args.pos.iter().map(|v| scalar(v.untainted())).collect::<Option<Vec<_>>>()?;
        let limit = self.max_depth.saturating_sub(self.depth);
        Some(match jit.call(def, &scalars, limit)? {
            Ok(result) => {
                let value = value(result);
                Ok(if tainted { value.taint() } else { value })
            }
            Err(trap) => {
                let (ty, message) = trap.error();
                let error = ErrorVal::new(ty, message);
                error.trace.borrow_mut().push((def.name.name.clone(), def.span));
                Err(Ctrl::Raise(Arc::new(error)))
            }
        })
    }

    /// With `--log`: which functions run as native code, and why the others do not.
    pub(crate) fn log_jit(&self) {
        let Some(jit) = &self.jit else { return };
        let report = jit.report();
        if !report.compiled.is_empty() {
            self.write_err(&format!("[jit] native: {}\n", report.compiled.join(", ")));
        }
        for (name, reason) in &report.interpreted {
            self.write_err(&format!("[jit] `{name}` stays interpreted: it {reason}\n"));
        }
    }
}

fn scalar(value: &Value) -> Option<Scalar> {
    match value {
        Value::Int(n) => Some(Scalar::Int(*n)),
        Value::Float(f) => Some(Scalar::Float(*f)),
        Value::Bool(b) => Some(Scalar::Bool(*b)),
        _ => None,
    }
}

fn value<'p>(scalar: Scalar) -> Value<'p> {
    match scalar {
        Scalar::Int(n) => Value::Int(n),
        Scalar::Float(f) => Value::Float(f),
        Scalar::Bool(b) => Value::Bool(b),
    }
}
