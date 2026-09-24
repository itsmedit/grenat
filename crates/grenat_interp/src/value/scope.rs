//! Portées de variables, partagées par les fermetures.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::*;

pub type Scope<'p> = Arc<Mutex<ScopeData<'p>>>;

#[derive(Default)]
pub struct ScopeData<'p> {
    pub vars: HashMap<String, Value<'p>>,
    pub parent: Option<Scope<'p>>,
}

pub fn new_scope<'p>(parent: Option<Scope<'p>>) -> Scope<'p> {
    Arc::new(Mutex::new(ScopeData { vars: HashMap::new(), parent }))
}

pub fn scope_get<'p>(scope: &Scope<'p>, name: &str) -> Option<Value<'p>> {
    let data = scope.borrow();
    match data.vars.get(name) {
        Some(v) => Some(v.clone()),
        None => data.parent.as_ref().and_then(|p| scope_get(p, name)),
    }
}

/// Affecte la variable là où elle existe déjà (fermetures), sinon la crée ici.
pub fn scope_set<'p>(scope: &Scope<'p>, name: &str, value: Value<'p>) {
    fn find<'p>(scope: &Scope<'p>, name: &str) -> Option<Scope<'p>> {
        let data = scope.borrow();
        if data.vars.contains_key(name) {
            return Some(scope.clone());
        }
        data.parent.as_ref().and_then(|p| find(p, name))
    }
    let target = find(scope, name).unwrap_or_else(|| scope.clone());
    target.borrow_mut().vars.insert(name.to_string(), value);
}

pub fn scope_define<'p>(scope: &Scope<'p>, name: &str, value: Value<'p>) {
    scope.borrow_mut().vars.insert(name.to_string(), value);
}
