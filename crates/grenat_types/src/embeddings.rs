//! Embeddings and vector search, checked: `embed(:docs, text)` names an
//! embedding model and gives an `Array(Float)` (an `Array(Array(Float))`
//! for texts), an `llm` effect no secret reaches; `Doc.nearest(:embedding,
//! vector, …)` names a `Vector(n)` field of the record and reads it.

use grenat_ast::{Arg, Diagnostic, ExprKind, ModelDecl, Span};

use crate::ty::{Ty, V};
use crate::*;

/// The options of `nearest`.
const NEAREST_OPTIONS: [&str; 3] = ["limit", "where", "max_distance"];

/// Whether a model declaration says `kind: :embedding`.
pub(crate) fn declares_embedding(model: &ModelDecl) -> bool {
    model.options.iter().any(|option| {
        matches!(option, Arg::Named { name, value: Some(value) }
            if name.name == "kind" && matches!(&value.kind, ExprKind::Symbol(k) if k == "embedding"))
    })
}

impl<'p> Checker<'p> {
    /// `embed(:docs, text)`, `embed(:docs, texts)`, `embed(text)`.
    pub(crate) fn embed_call(&mut self, cx: &mut Ctx<'p>, span: Span, argv: &[ArgV]) -> V {
        self.secrets_to_model(argv, "embed");
        cx.add_effect(Eff { path: "llm".into(), arg: None, origin: span });
        let input = match argv {
            [model, input] => {
                self.embedding_model_ref(model);
                input
            }
            [input] => {
                if self.embedding_models.is_empty() {
                    self.report(Diagnostic::new(span, "no embedding model declared").with_code(E_DECL).with_help(
                        "add `model :docs, provider: :openai, name: \"text-embedding-3-small\", kind: :embedding`",
                    ));
                }
                input
            }
            _ => {
                self.error(E_TYPE, span, "`embed` expects a model and a text: `embed(:docs, text)`");
                return V::unknown();
            }
        };
        let vector = Ty::array(Ty::Float);
        // a vector is numbers, never instructions: not untrusted, whatever its text
        match input.v.ty.base() {
            Ty::Str => V::new(vector),
            Ty::Array(item) if matches!(**item, Ty::Str | Ty::Unknown) => V::new(Ty::array(vector)),
            // a secret: reported as one
            Ty::Unknown | Ty::Secret => V::unknown(),
            other => {
                let message = format!("`embed` expects a text or an array of texts, got `{other}`");
                self.error(E_TYPE, input.span, message);
                V::unknown()
            }
        }
    }

    /// The model of `embed`: declared, and an embedding model.
    fn embedding_model_ref(&mut self, model: &ArgV) {
        let Some(name) = model.lit.as_deref().filter(|_| model.v.ty == Ty::Sym) else {
            if !model.v.ty.is_unknown() {
                self.error(E_TYPE, model.span, "`embed` expects a model first: `embed(:docs, text)`");
            }
            return;
        };
        if !self.models.contains(&name) {
            let models = self.embedding_models.clone();
            self.error_help(E_NAME, model.span, format!("model `:{name}` is not declared"), suggest(name, models));
        } else if !self.embedding_models.contains(&name) {
            self.report(
                Diagnostic::new(model.span, format!("model `:{name}` is not an embedding model"))
                    .with_code(E_TYPE)
                    .with_help("declare it with `kind: :embedding`"),
            );
        }
    }

    /// `Doc.nearest(:embedding, vector, limit: 5, where: {…}, max_distance: 0.5)`.
    pub(crate) fn nearest_call(&mut self, cx: &mut Ctx<'p>, span: Span, record: &str, argv: &[ArgV]) -> V {
        cx.add_effect(Eff { path: "db.read".into(), arg: None, origin: span });
        let positional: Vec<&ArgV> = argv.iter().filter(|a| a.name.is_none()).collect();
        match positional.first() {
            Some(field) if field.v.ty == Ty::Sym => {
                let name = field.lit.clone().unwrap_or_default();
                if !self.is_vector_field(record, &name) {
                    let message = format!("`{record}.{name}` is not a vector field (`{name}: Vector(1536)`)");
                    self.error(E_TYPE, field.span, message);
                }
            }
            Some(other) if !other.v.ty.is_unknown() => {
                self.error(E_TYPE, other.span, "`nearest` expects a vector field first: `nearest(:embedding, vector)`")
            }
            Some(_) => {}
            None => self.error(E_TYPE, span, "`nearest` expects a vector field and a vector"),
        }
        match positional.get(1) {
            Some(vector) if !self.compat(&vector.v.ty, &Ty::array(Ty::Float)) => {
                let message = format!("`nearest` compares with a vector (`Array(Float)`), got `{}`", vector.v.ty);
                self.error(E_TYPE, vector.span, message);
            }
            Some(_) => {}
            None if !positional.is_empty() => self.error(E_TYPE, span, "`nearest` expects a vector to compare with"),
            None => {}
        }
        if positional.len() > 2 {
            self.error(E_TYPE, positional[2].span, "too many arguments for `nearest` (expected 2)");
        }
        for arg in argv {
            if let Some(name) = arg.name.as_deref().filter(|n| !NEAREST_OPTIONS.contains(n)) {
                let message = format!("unknown named argument `{name}:` for `nearest`");
                self.error_help(E_TYPE, arg.span, message, suggest(name, NEAREST_OPTIONS));
            }
        }
        V::new(Ty::array(Ty::User(record.into())))
    }

    fn is_vector_field(&self, record: &str, name: &str) -> bool {
        self.types.get(record).is_some_and(|decl| {
            decl.fields.iter().any(|f| f.name.name == name && f.ty.as_ref().is_some_and(|t| type_name(t) == "Vector"))
        })
    }
}
