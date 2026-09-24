//! The structs native code can build and read, and type annotations.
//!
//! A struct is native when every field has an annotated native type other
//! than an array (structs may contain structs, but not recursively).

use std::collections::{HashMap, HashSet};

use grenat_ast::{Item, Member, Program, Type, TypeDef, TypeKind};

use crate::ty::{Elem, StructId, Ty};

pub(crate) struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
    /// Instance methods, which take precedence over fields of the same name.
    methods: HashSet<String>,
}

impl StructDef {
    pub fn field(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|(n, _)| n == name)
    }

    pub fn has_method(&self, name: &str) -> bool {
        self.methods.contains(name)
    }
}

#[derive(Default)]
pub(crate) struct Structs {
    defs: Vec<StructDef>,
    by_name: HashMap<String, StructId>,
}

impl Structs {
    pub fn from_program(program: &Program) -> Structs {
        let types: HashMap<&str, &TypeDef> = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Type(def) => Some((def.name.name.as_str(), def)),
                _ => None,
            })
            .collect();
        let mut native = HashMap::new();
        let names: Vec<&str> = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Type(def) if def.kind == TypeKind::Struct => Some(def.name.name.as_str()),
                _ => None,
            })
            .filter(|name| is_native(name, &types, &mut native, &mut Vec::new()))
            .collect();

        let mut structs = Structs::default();
        for (i, name) in names.iter().enumerate() {
            structs.by_name.insert(name.to_string(), StructId(i));
        }
        for name in names {
            let def = types[name];
            let fields = struct_fields(def)
                .map(|(field, ty)| (field.to_string(), structs.ty(ty).expect("checked by is_native")))
                .collect();
            structs.defs.push(StructDef { name: name.to_string(), fields, methods: methods(def, &types) });
        }
        structs
    }

    pub fn count(&self) -> usize {
        self.defs.len()
    }

    pub fn get(&self, id: StructId) -> &StructDef {
        &self.defs[id.0]
    }

    pub fn id(&self, name: &str) -> Option<StructId> {
        self.by_name.get(name).copied()
    }

    /// Native type of an annotation, `None` if it has none.
    pub fn ty(&self, ty: &Type) -> Option<Ty> {
        let Type::Named { path, args, .. } = ty else { return None };
        let [name] = path.as_slice() else { return None };
        match (name.name.as_str(), args.as_slice()) {
            ("Int", []) => Some(Ty::Int),
            ("Float", []) => Some(Ty::Float),
            ("Bool", []) => Some(Ty::Bool),
            ("String", []) => Some(Ty::Str),
            ("Array", [elem]) => Some(Ty::Array(self.ty(elem)?.elem()?)),
            (name, []) => self.id(name).map(Ty::Struct),
            _ => None,
        }
    }

    /// A type as written in Grenat, for messages.
    pub fn show(&self, ty: Ty) -> String {
        match ty {
            Ty::Int => "Int".into(),
            Ty::Float => "Float".into(),
            Ty::Bool => "Bool".into(),
            Ty::Str => "String".into(),
            Ty::Struct(id) => self.get(id).name.clone(),
            Ty::Array(elem) => match elem.ty() {
                Some(t) => format!("Array({})", self.show(t)),
                None => "Array".into(),
            },
        }
    }

    pub fn show_elem(&self, elem: Elem) -> String {
        elem.ty().map_or_else(|| "?".into(), |t| self.show(t))
    }
}

fn struct_fields(def: &TypeDef) -> impl Iterator<Item = (&str, &Type)> {
    def.members.iter().filter_map(|m| match m {
        Member::Field(f) if !f.is_ivar => Some((f.name.name.as_str(), f.ty.as_ref()?)),
        _ => None,
    })
}

/// Every field has a native type; `visiting` detects recursive structs.
fn is_native<'p>(
    name: &'p str,
    types: &HashMap<&'p str, &'p TypeDef>,
    memo: &mut HashMap<&'p str, bool>,
    visiting: &mut Vec<&'p str>,
) -> bool {
    if let Some(&known) = memo.get(name) {
        return known;
    }
    let Some(def) = types.get(name).filter(|d| d.kind == TypeKind::Struct) else { return false };
    if visiting.contains(&name) {
        return false;
    }
    visiting.push(name);
    let untyped = def.members.iter().any(|m| matches!(m, Member::Field(f) if !f.is_ivar && f.ty.is_none()));
    let native = !untyped
        && struct_fields(def).all(|(_, ty)| match ty {
            Type::Named { path, args, .. } if args.is_empty() && path.len() == 1 => {
                let field = path[0].name.as_str();
                matches!(field, "Int" | "Float" | "Bool" | "String") || is_native(field, types, memo, visiting)
            }
            _ => false,
        });
    visiting.pop();
    memo.insert(name, native);
    native
}

/// Instance methods, including those of included modules.
fn methods(def: &TypeDef, types: &HashMap<&str, &TypeDef>) -> HashSet<String> {
    let own = |def: &TypeDef| -> Vec<String> {
        def.members
            .iter()
            .filter_map(|m| match m {
                Member::Method(m) if !m.on_self => Some(m.name.name.clone()),
                _ => None,
            })
            .collect()
    };
    let mut names: HashSet<String> = own(def).into_iter().collect();
    for member in &def.members {
        if let Member::Include(Type::Named { path, .. }) = member
            && let Some(module) = path.last().and_then(|p| types.get(p.name.as_str()))
        {
            names.extend(own(module));
        }
    }
    names
}
