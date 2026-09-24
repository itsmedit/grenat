//! Construction of structs, classes, variants, errors and messages.

use crate::prelude::*;

impl<'p> Interp<'p> {
    // ── Construction ─────────────────────────────────────────

    pub(crate) fn construct(&mut self, name: &str, args: Args<'p>) -> R<'p> {
        match name {
            "Ok" => return Ok(Value::ok(args.pos.into_iter().next().unwrap_or(Value::Nil))),
            "Err" => return Ok(Value::err(args.pos.into_iter().next().unwrap_or(Value::Nil))),
            _ => {}
        }
        if let Some(info) = self.types.get(name) {
            let (kind, fields) = (info.def.kind, info.fields.clone());
            let name: &'p str = info.def.name.name.as_str();
            return match kind {
                TypeKind::Struct => {
                    let fields = self.build_fields(name, &fields, args)?;
                    Ok(Value::record(name, fields))
                }
                TypeKind::Class => self.new_object(name, args, false),
                TypeKind::Agent => raise("TypeError", format!("an agent is started with `spawn {name}`")),
                TypeKind::Enum => raise("TypeError", format!("`{name}` is an enum: build one of its variants")),
                TypeKind::Module | TypeKind::Supervisor => {
                    raise("TypeError", format!("`{name}` cannot be instantiated"))
                }
            };
        }
        if let Some(enum_name) = self.variants.get(name).copied() {
            let variant =
                self.types[enum_name].variants.iter().find(|v| v.name.name == name).copied().expect("variante");
            let defs: Vec<_> = variant.fields.iter().collect();
            let fields = self.build_fields(name, &defs, args)?;
            return Ok(Value::Variant(Arc::new(Variant { enum_name: enum_name.into(), name: name.into(), fields })));
        }
        if is_error_name(name) {
            let mut error = ErrorVal::new(name, name);
            let mut pos = args.pos.into_iter();
            if let Some(message) = pos.next() {
                error.message = message.to_display();
            }
            error.fields = args.named.into_iter().map(|(n, v)| (n.into(), v)).collect();
            return Ok(Value::Error(Arc::new(error)));
        }
        if self.messages.contains(name) {
            // agent message: positional fields `_0`, `_1`… then named ones
            let mut fields: Fields<'p> =
                args.pos.into_iter().enumerate().map(|(i, v)| (format!("_{i}").into(), v)).collect();
            fields.extend(args.named.into_iter().map(|(n, v)| (n.into(), v)));
            return Ok(Value::record(name, fields));
        }
        raise("NameError", format!("unknown type `{name}`"))
    }

    pub(crate) fn build_fields(
        &mut self,
        owner: &str,
        defs: &[&'p grenat_ast::Field],
        args: Args<'p>,
    ) -> Result<Fields<'p>, Ctrl<'p>> {
        let mut pos = args.pos.into_iter();
        let mut named = args.named;
        let mut fields = Vec::with_capacity(defs.len());
        for def in defs {
            let value = if let Some(i) = named.iter().position(|(n, _)| *n == def.name.name) {
                named.remove(i).1
            } else if let Some(v) = pos.next() {
                v
            } else if let Some(default) = &def.default {
                self.eval(default)?
            } else if matches!(def.ty, Some(grenat_ast::Type::Optional(..))) {
                Value::Nil
            } else {
                return raise("ArgumentError", format!("missing field `{}` for `{owner}`", def.name.name));
            };
            fields.push((def.name.name.as_str().into(), value));
        }
        if pos.next().is_some() {
            return raise("ArgumentError", format!("too many values for `{owner}` ({} fields)", defs.len()));
        }
        if let Some((name, _)) = named.first() {
            return raise("ArgumentError", format!("unknown field `{name}:` for `{owner}`"));
        }
        Ok(fields)
    }

    /// Class or agent instance: `@…` state initialized, then `initialize`.
    pub(crate) fn new_object(&mut self, ty: &'p str, args: Args<'p>, is_agent: bool) -> R<'p> {
        let obj = Arc::new(Object { ty: ty.into(), fields: Mutex::new(Vec::new()), is_agent });
        let value = Value::Object(obj.clone());
        let info_fields = self.types[ty].fields.clone();
        self.push_frame(Some(value.clone()), new_scope(None))?;
        let init = (|| {
            for f in info_fields.iter().filter(|f| f.is_ivar) {
                let v = match &f.default {
                    Some(d) => self.eval(d)?,
                    None => Value::Nil,
                };
                set_field(&mut obj.fields.borrow_mut(), &f.name.name, v);
            }
            Ok(())
        })();
        self.pop_frame();
        init?;

        if let Some(initialize) = self.types[ty].methods.get("initialize").copied() {
            self.call_fn(initialize, args, Some(value.clone()))?;
        } else {
            if let Some(v) = args.pos.first() {
                return raise(
                    "ArgumentError",
                    format!("`{ty}` without `initialize` only accepts named arguments (got {})", v.inspect()),
                );
            }
            for (name, v) in args.named {
                if !info_fields.iter().any(|f| f.name.name == name) {
                    return raise("ArgumentError", format!("unknown state `@{name}` for `{ty}`"));
                }
                set_field(&mut obj.fields.borrow_mut(), &name, v);
            }
        }
        Ok(value)
    }
}
