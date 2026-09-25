//! Program loading: functions, types, variants, messages and models.

use std::collections::HashMap;

use grenat_ast::{Arg, Directive, Field, FnDef, Handler, Item, Member, TypeDef, TypeKind, Variant};
use grenat_llm::ModelConfig;

use crate::value::{Locked, Value};
use crate::*;

pub(crate) struct TypeInfo<'p> {
    pub def: &'p TypeDef,
    pub fields: Vec<&'p Field>,
    pub methods: HashMap<&'p str, &'p FnDef>,
    pub statics: HashMap<&'p str, &'p FnDef>,
    pub variants: Vec<&'p Variant>,
    pub handlers: HashMap<&'p str, &'p Handler>,
    pub directives: Vec<&'p Directive>,
}

impl<'p> Interp<'p> {
    pub(crate) fn load_models(&mut self) -> Result<(), Ctrl<'p>> {
        let program = self.program;
        for item in &program.items {
            if let Item::Model(decl) = item {
                let config = self.model_config(decl)?;
                self.models.borrow_mut().push((decl.name.name.clone(), config));
            }
        }
        Ok(())
    }

    pub(crate) fn model_config(&mut self, decl: &'p grenat_ast::ModelDecl) -> Result<ModelConfig, Ctrl<'p>> {
        let mut config = ModelConfig::new("anthropic", "");
        let mut fallbacks = None;
        for option in &decl.options {
            let Arg::Named { name, value: Some(expr) } = option else {
                return raise("ArgumentError", format!("invalid option for model `:{}`", decl.name.name));
            };
            let value = self.eval(expr)?;
            match (name.name.as_str(), value) {
                ("provider", Value::Symbol(s) | Value::Str(s)) => config.provider = s.to_string(),
                ("name", Value::Str(s)) => config.name = s.to_string(),
                ("temperature", Value::Float(f)) => config.temperature = Some(f),
                ("temperature", Value::Int(n)) => config.temperature = Some(n as f64),
                ("max_tokens", Value::Int(n)) if n > 0 => config.max_tokens = n as u32,
                ("effort", Value::Symbol(s) | Value::Str(s)) => config.effort = Some(s.to_string()),
                ("fallbacks", Value::Bool(b)) => fallbacks = Some(b),
                (option, value) => {
                    return raise(
                        "ArgumentError",
                        format!("invalid option `{option}: {}` for model `:{}`", value.inspect(), decl.name.name),
                    );
                }
            }
        }
        if config.name.is_empty() {
            return raise("ArgumentError", format!("model `:{}` has no `name:`", decl.name.name));
        }
        config.fallbacks = fallbacks.unwrap_or_else(|| ModelConfig::new("", config.name.as_str()).fallbacks);
        Ok(config)
    }

    pub(crate) fn run_script(&mut self) -> Result<(), Ctrl<'p>> {
        for item in &self.program.items {
            if let Item::Stmt(expr) = item {
                self.eval(expr)?;
            }
        }
        Ok(())
    }
}

impl<'p> Shared<'p> {
    /// Registers functions, types and models before anything runs.
    pub(crate) fn load(&mut self) -> Result<(), Ctrl<'p>> {
        let program = self.program;
        for item in &program.items {
            match item {
                Item::Fn(def) => {
                    if self.fns.insert(&def.name.name, def).is_some() {
                        return raise("NameError", format!("function `{}` defined twice", def.name.name));
                    }
                }
                Item::Type(def) => {
                    let info = TypeInfo::new(def);
                    self.messages.extend(info.handlers.keys().copied());
                    if self.types.insert(&def.name.name, info).is_some() {
                        return raise("NameError", format!("type `{}` defined twice", def.name.name));
                    }
                }
                // macros are expanded before running (see `grenat_macros`)
                Item::Model(_) | Item::Macro(_) | Item::Stmt(_) => {}
            }
        }

        // `include Module`: copy the methods that are not overridden
        let includes: Vec<(&'p str, &'p str)> = self
            .types
            .values()
            .flat_map(|info| {
                info.def.members.iter().filter_map(move |m| match m {
                    Member::Include(ty) => Some((info.def.name.name.as_str(), llm::type_name(ty))),
                    _ => None,
                })
            })
            .collect();
        for (owner, module) in includes {
            let Some(methods) = self.types.get(module).map(|m| m.methods.clone()) else {
                return raise("NameError", format!("unknown module `{module}` (included by `{owner}`)"));
            };
            let info = self.types.get_mut(owner).expect("loaded type");
            for (name, def) in methods {
                info.methods.entry(name).or_insert(def);
            }
        }

        for info in self.types.values() {
            for variant in &info.variants {
                self.variants.insert(&variant.name.name, &info.def.name.name);
            }
        }
        Ok(())
    }
}

impl<'p> TypeInfo<'p> {
    pub(crate) fn new(def: &'p TypeDef) -> Self {
        let mut info = TypeInfo {
            def,
            fields: Vec::new(),
            methods: HashMap::new(),
            statics: HashMap::new(),
            variants: Vec::new(),
            handlers: HashMap::new(),
            directives: Vec::new(),
        };
        for member in &def.members {
            match member {
                Member::Field(f) => info.fields.push(f),
                Member::Method(m) if m.on_self => {
                    info.statics.insert(&m.name.name, m);
                }
                Member::Method(m) => {
                    info.methods.insert(&m.name.name, m);
                }
                Member::Variant(v) => info.variants.push(v),
                Member::Handler(h) => {
                    info.handlers.insert(&h.message.name, h);
                }
                Member::Directive(d) => info.directives.push(d),
                Member::Include(_) => {}
            }
        }
        info
    }

    pub fn is(&self, kind: TypeKind) -> bool {
        self.def.kind == kind
    }
}
