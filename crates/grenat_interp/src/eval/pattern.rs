//! `case` : filtrage par motifs (`in`) et par valeurs (`when`).

use crate::prelude::*;

impl<'p> Interp<'p> {
    // ── case ─────────────────────────────────────────────────

    pub(crate) fn case(
        &mut self,
        subject: Option<&'p Expr>,
        arms: &'p [grenat_ast::CaseArm],
        else_: Option<&'p [Expr]>,
    ) -> R<'p> {
        let subject = subject.map(|s| self.eval(s)).transpose()?;
        for arm in arms {
            let matched = match &arm.test {
                ArmTest::In(pattern) => match &subject {
                    Some(s) => self.match_pattern(pattern, s)?,
                    None => return raise("SyntaxError", "`case … in` exige une valeur à filtrer"),
                },
                ArmTest::When(values) => {
                    let mut any = false;
                    for v in values {
                        let candidate = self.eval(v)?;
                        let hit = match &subject {
                            Some(s) => self.case_eq(&candidate, s),
                            None => candidate.truthy(),
                        };
                        if hit {
                            any = true;
                            break;
                        }
                    }
                    any
                }
            };
            let guard = match (&arm.guard, matched) {
                (Some(g), true) => self.eval(g)?.truthy(),
                (None, m) => m,
                (_, false) => false,
            };
            if guard {
                return self.eval_stmts(&arm.body);
            }
        }
        match else_ {
            Some(stmts) => self.eval_stmts(stmts),
            None if arms.iter().any(|a| matches!(a.test, ArmTest::In(_))) => raise(
                "NoMatchingPattern",
                format!("aucun motif ne correspond à {}", subject.map_or("nil".into(), |s| s.inspect())),
            ),
            None => Ok(Value::Nil),
        }
    }

    pub(crate) fn case_eq(&self, candidate: &Value<'p>, subject: &Value<'p>) -> bool {
        match candidate {
            Value::Type(name) => self.is_a(subject, name),
            Value::Range(lo, hi, inclusive) => match subject.untainted() {
                Value::Int(n) => *n >= *lo && if *inclusive { n <= hi } else { n < hi },
                _ => false,
            },
            _ => equal(candidate, subject),
        }
    }

    pub(crate) fn is_a(&self, value: &Value<'p>, name: &str) -> bool {
        match value.untainted() {
            Value::Record(r) => &*r.ty == name,
            Value::Object(o) => &*o.ty == name,
            Value::Variant(v) => &*v.enum_name == name || &*v.name == name,
            Value::Error(e) => error_is_a(&e.ty, name),
            Value::Int(_) | Value::Float(_) if name == "Numeric" => true,
            other => other.type_name() == name,
        }
    }

    pub(crate) fn match_pattern(&mut self, pattern: &'p Pattern, value: &Value<'p>) -> Result<bool, Ctrl<'p>> {
        let tainted = value.is_tainted();
        let inner = value.untainted().clone();
        let wrap = |v: Value<'p>| if tainted { v.taint() } else { v };
        match &pattern.kind {
            PatternKind::Wildcard => Ok(true),
            PatternKind::Bind(name) => {
                scope_set(self.scope(), name, value.clone());
                Ok(true)
            }
            PatternKind::Lit(e) => {
                let lit = self.eval(e)?;
                Ok(equal(&lit, &inner))
            }
            PatternKind::Const { path, fields } => {
                let name = path.last().expect("chemin non vide").name.as_str();
                let (type_ok, value_fields): (bool, Option<Fields<'p>>) = match &inner {
                    Value::Record(r) => (&*r.ty == name, Some(r.fields.clone())),
                    Value::Variant(v) => {
                        (&*v.name == name || (&*v.enum_name == name && fields.is_none()), Some(v.fields.clone()))
                    }
                    Value::Error(e) => (error_is_a(&e.ty, name), Some(e.fields.clone())),
                    other => (self.is_a(other, name), None),
                };
                if !type_ok {
                    return Ok(false);
                }
                let Some(pattern_fields) = fields else { return Ok(true) };
                let value_fields = value_fields.unwrap_or_default();
                for (i, pf) in pattern_fields.iter().enumerate() {
                    let fv = match &pf.name {
                        Some(n) => field(&value_fields, &n.name).cloned(),
                        None => value_fields.get(i).map(|(_, v)| v.clone()),
                    };
                    let Some(fv) = fv else { return Ok(false) };
                    if !self.match_pattern(&pf.pattern, &wrap(fv))? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            PatternKind::Array(patterns) => {
                let Value::Array(items) = &inner else { return Ok(false) };
                let items = items.borrow().clone();
                if items.len() != patterns.len() {
                    return Ok(false);
                }
                for (p, item) in patterns.iter().zip(items) {
                    if !self.match_pattern(p, &wrap(item))? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            PatternKind::Or(alts) => {
                for alt in alts {
                    if self.match_pattern(alt, value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}
