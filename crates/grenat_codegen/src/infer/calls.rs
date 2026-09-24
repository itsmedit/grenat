//! Calls: compiled functions, struct constructors, methods.

use grenat_ast::{Arg, Block, Expr};

use super::methods::constant;
use super::{Construct, Flow, Infer, Method, Reject, Target, positional};
use crate::ty::{Elem, Ty};

impl Infer<'_, '_> {
    pub(super) fn call(
        &mut self,
        call: &Expr,
        recv: Option<&Expr>,
        name: &str,
        args: &[Arg],
        block: Option<&Block>,
    ) -> Result<Flow, Reject> {
        match (recv, block) {
            (Some(recv), _) => match (constant(recv), name) {
                (Some("Math"), "sqrt") => self.sqrt(call, recv, args),
                (Some(ty), "new") if block.is_none() => {
                    self.typed.types.insert(recv as *const Expr, Flow::Unit);
                    self.construct(call, ty, args)
                }
                (Some(ty), _) => Err(format!("calls `{ty}.{name}`")),
                (None, _) => self.method(call, recv, name, args, block),
            },
            (None, Some(_)) => Err(format!("calls `{name}` with a block")),
            (None, None) if name.starts_with(|c: char| c.is_uppercase()) => self.construct(call, name, args),
            (None, None) if self.is_builtin(name) => self.builtin(call, name, args),
            (None, None) => self.function(name, args),
        }
    }

    /// A compiled function, called with arguments of exactly its parameter
    /// types (the interpreter would compute with the argument's own type).
    fn function(&mut self, name: &str, args: &[Arg]) -> Result<Flow, Reject> {
        let Some(sig) = self.sigs.get(name).cloned() else {
            return Err(format!("calls `{name}`, which is not compiled"));
        };
        self.typed.calls.insert(name.to_string());
        if args.len() != sig.params.len() {
            return Err(format!("calls `{name}` with default or missing arguments"));
        }
        for (arg, expected) in positional(args, name)?.into_iter().zip(&sig.params) {
            let t = self.value(arg, Some(*expected))?;
            self.expect(arg, t, *expected, "passes")
                .map_err(|_| format!("passes `{}` where `{name}` expects `{}`", self.show(t), self.show(*expected)))?;
        }
        Ok(sig.ret.map_or(Flow::Unit, Flow::Value))
    }

    /// `puts`, `print`, `p`, `exit` of a standalone program (the interpreter's
    /// own otherwise), unless the program defines its own.
    pub(super) fn is_builtin(&self, name: &str) -> bool {
        self.target == Target::Standalone
            && matches!(name, "puts" | "print" | "p" | "exit")
            && !self.sigs.contains_key(name)
            && !self.typed.vars.contains_key(name)
    }

    /// `puts`, `print`, `p`, `exit` of a standalone program.
    pub(super) fn builtin(&mut self, call: &Expr, name: &str, args: &[Arg]) -> Result<Flow, Reject> {
        let args = positional(args, name)?;
        let method = match name {
            "puts" => Method::Puts,
            "print" => Method::Print,
            "p" => Method::Inspect,
            "exit" => {
                match args.as_slice() {
                    [] => {}
                    [code] => {
                        if self.value(code, None)? != Ty::Int {
                            return Err("calls `exit` with a non-`Int` code".into());
                        }
                    }
                    _ => return Err("calls `exit` with several codes".into()),
                }
                self.typed.methods.insert(call as *const Expr, Method::Exit);
                return Ok(Flow::Never);
            }
            _ => return Err(format!("calls `{name}`, which is not compiled")),
        };
        for arg in args {
            let t = self.value(arg, None)?;
            self.printable(t, name)?;
        }
        self.typed.methods.insert(call as *const Expr, method);
        Ok(Flow::Unit)
    }

    /// Printed as the interpreter prints it: no struct with its own `to_s`.
    fn printable(&self, t: Ty, name: &str) -> Result<(), Reject> {
        match t {
            Ty::Struct(id) if self.structs.get(id).has_method("to_s") => {
                Err(format!("calls `{name}` on a `{}`, which defines `to_s`", self.show(t)))
            }
            Ty::Struct(id) => {
                self.structs.get(id).fields.iter().try_for_each(|(_, f)| self.printable(*f, name))
            }
            Ty::Array(Elem::Unknown) => Err(format!("calls `{name}` on `[]`")),
            Ty::Array(elem) => self.printable(elem.ty().expect("known"), name),
            _ => Ok(()),
        }
    }

    /// `Point(x: 1.0, y: 2.0)` or `Point.new(…)`: fields are filled as the
    /// interpreter fills them (named first, then positional in order), and
    /// every field must be given (defaults are not evaluated natively).
    fn construct(&mut self, call: &Expr, name: &str, args: &[Arg]) -> Result<Flow, Reject> {
        let Some(id) = self.structs.id(name) else {
            return Err(format!("builds a `{name}`, which is not a native struct"));
        };
        let fields = self.structs.get(id).fields.clone();
        let named: Vec<&str> = args
            .iter()
            .filter_map(|a| match a {
                Arg::Named { name, .. } => Some(name.name.as_str()),
                _ => None,
            })
            .collect();
        let mut free = fields.iter().enumerate().filter(|(_, (n, _))| !named.contains(&n.as_str())).map(|(i, _)| i);
        let mut slots = Vec::new();
        let mut given = vec![false; fields.len()];
        for arg in args {
            let (index, value) = match arg {
                Arg::Named { name: field, value: Some(value) } => {
                    let index = fields
                        .iter()
                        .position(|(n, _)| *n == field.name)
                        .ok_or_else(|| format!("gives `{name}` an unknown field `{}`", field.name))?;
                    (index, value)
                }
                Arg::Named { name: field, value: None } => {
                    return Err(format!("uses the shorthand `{}:` for a field", field.name));
                }
                Arg::Pos(value) => (free.next().ok_or_else(|| format!("gives `{name}` too many values"))?, value),
                Arg::BlockPass(_) => return Err(format!("passes a block to `{name}`")),
            };
            if std::mem::replace(&mut given[index], true) {
                return Err(format!("gives field `{}` twice", fields[index].0));
            }
            let expected = fields[index].1;
            let t = self.value(value, Some(expected))?;
            if t != expected {
                return Err(format!(
                    "gives `{}` to field `{}` of type `{}`",
                    self.show(t),
                    fields[index].0,
                    self.show(expected)
                ));
            }
            slots.push(index);
        }
        if let Some(missing) = given.iter().position(|g| !g) {
            return Err(format!("leaves field `{}` of `{name}` to its default", fields[missing].0));
        }
        self.typed.constructs.insert(call as *const Expr, Construct { id, fields: slots });
        Ok(Flow::Value(Ty::Struct(id)))
    }
}
