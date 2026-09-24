//! Affectation et indexation.

use crate::prelude::*;

impl<'p> Interp<'p> {
    // ── Affectation ──────────────────────────────────────────

    pub(crate) fn assign(&mut self, target: &'p Expr, value: Value<'p>) -> Result<(), Ctrl<'p>> {
        match &target.kind {
            ExprKind::Var(name) => {
                scope_set(self.scope(), name, value);
                Ok(())
            }
            ExprKind::IVar(name) => {
                let obj = self.self_object(name)?;
                set_field(&mut obj.fields.borrow_mut(), name, value);
                Ok(())
            }
            ExprKind::Index { recv, args } => {
                let target = self.eval(recv)?;
                let index = args.iter().map(|a| self.eval(a)).collect::<Result<Vec<_>, _>>()?;
                self.index_set(target, index, value)
            }
            ExprKind::Call { recv: Some(recv), name, .. } => match self.eval(recv)? {
                Value::Object(obj) => {
                    set_field(&mut obj.fields.borrow_mut(), &name.name, value);
                    Ok(())
                }
                Value::Record(r) => raise(
                    "TypeError",
                    format!("`{}` est une struct immuable : créez une copie avec `.with({}: …)`", r.ty, name.name),
                ),
                other => raise("TypeError", format!("impossible d'affecter `{}` sur {}", name.name, other.type_name())),
            },
            _ => raise("TypeError", "cible d'affectation invalide"),
        }
    }

    pub(crate) fn op_assign(&mut self, op: BinOp, target: &'p Expr, value: &'p Expr) -> R<'p> {
        let current = match &target.kind {
            ExprKind::Var(name) => scope_get(self.scope(), name).unwrap_or(Value::Nil),
            ExprKind::IVar(name) => {
                let obj = self.self_object(name)?;
                let fields = obj.fields.borrow();
                field(&fields, name).cloned().unwrap_or(Value::Nil)
            }
            _ => self.eval(target)?,
        };
        let new = match op {
            BinOp::Or if current.truthy() => return Ok(current),
            BinOp::And if !current.truthy() => return Ok(current),
            BinOp::Or | BinOp::And => self.eval(value)?,
            op => {
                let rhs = self.eval(value)?;
                self.binop(op, current, rhs)?
            }
        };
        self.assign(target, new.clone())?;
        Ok(new)
    }

    // ── Indexation ───────────────────────────────────────────

    pub(crate) fn index(&mut self, target: Value<'p>, index: Vec<Value<'p>>) -> R<'p> {
        if let Value::Tainted(inner) = &target {
            return Ok(self.index((**inner).clone(), index)?.taint());
        }
        let [key] = index.as_slice() else {
            return raise("ArgumentError", "un seul indice attendu");
        };
        match (&target, key.untainted()) {
            (Value::Array(items), Value::Int(i)) => {
                let items = items.borrow();
                Ok(resolve_index(*i, items.len()).and_then(|i| items.get(i)).cloned().unwrap_or(Value::Nil))
            }
            (Value::Array(items), Value::Range(lo, hi, inclusive)) => {
                let items = items.borrow();
                let len = items.len() as i64;
                let start = if *lo < 0 { lo + len } else { *lo }.clamp(0, len) as usize;
                let end = if *hi < 0 { hi + len } else { *hi } + i64::from(*inclusive);
                let end = end.clamp(start as i64, len) as usize;
                Ok(Value::array(items[start..end].to_vec()))
            }
            (Value::Str(s), Value::Int(i)) => {
                let chars: Vec<char> = s.chars().collect();
                Ok(resolve_index(*i, chars.len())
                    .and_then(|i| chars.get(i))
                    .map_or(Value::Nil, |c| Value::str(c.to_string())))
            }
            (Value::Hash(entries), key) => {
                Ok(entries.borrow().iter().find(|(k, _)| equal(k, key)).map_or(Value::Nil, |(_, v)| v.clone()))
            }
            (Value::Type(sup), Value::Type(agent)) => self.supervisor_child(sup, agent),
            (target, key) => {
                raise("TypeError", format!("{} ne peut pas être indexé par {}", target.type_name(), key.type_name()))
            }
        }
    }

    pub(crate) fn index_set(
        &mut self,
        target: Value<'p>,
        index: Vec<Value<'p>>,
        value: Value<'p>,
    ) -> Result<(), Ctrl<'p>> {
        let [key] = index.as_slice() else {
            return raise("ArgumentError", "un seul indice attendu");
        };
        match (&target, key.untainted()) {
            (Value::Array(items), Value::Int(i)) => {
                let mut items = items.borrow_mut();
                let len = items.len();
                let Some(i) = resolve_index(*i, len.max(*i as usize + 1)) else {
                    return raise("IndexError", format!("indice {i} hors du tableau"));
                };
                if i >= items.len() {
                    items.resize(i + 1, Value::Nil);
                }
                items[i] = value;
                Ok(())
            }
            (Value::Hash(entries), key) => {
                let mut entries = entries.borrow_mut();
                match entries.iter_mut().find(|(k, _)| equal(k, key)) {
                    Some((_, v)) => *v = value,
                    None => entries.push((key.clone(), value)),
                }
                Ok(())
            }
            (target, _) => raise("TypeError", format!("impossible d'affecter un indice sur {}", target.type_name())),
        }
    }
}

pub(crate) fn set_field<'p>(fields: &mut Fields<'p>, name: &str, value: Value<'p>) {
    match fields.iter_mut().find(|(n, _)| &**n == name) {
        Some((_, v)) => *v = value,
        None => fields.push((name.into(), value)),
    }
}

fn resolve_index(i: i64, len: usize) -> Option<usize> {
    let i = if i < 0 { i + len as i64 } else { i };
    (0..len as i64).contains(&i).then_some(i as usize)
}
