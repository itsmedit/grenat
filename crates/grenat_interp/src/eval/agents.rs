//! Agents-acteurs : démarrage, pools, messages (`ask`/`tell`), interblocages, supervision.

use crate::prelude::*;

impl<'p> Interp<'p> {
    // ── Agents ───────────────────────────────────────────────

    /// État neuf d'un agent : son `@…` initialisé.
    pub(crate) fn agent_state(&mut self, ty: &'p str) -> Result<Arc<Object<'p>>, Ctrl<'p>> {
        match self.new_object(ty, Args::default(), true)? {
            Value::Object(obj) => Ok(obj),
            _ => unreachable!("new_object renvoie un objet"),
        }
    }

    pub(crate) fn spawn(&mut self, target: &Value<'p>) -> R<'p> {
        self.spawn_agent(target, None).map(Value::Agent)
    }

    pub(crate) fn spawn_agent(
        &mut self,
        target: &Value<'p>,
        supervision: Option<Supervision>,
    ) -> Result<Arc<AgentRef<'p>>, Ctrl<'p>> {
        let Value::Type(name) = target else {
            return raise("TypeError", format!("`spawn` attend un type d'agent, reçu {}", target.inspect()));
        };
        let shared = self.shared.clone();
        let Some(info) = shared.types.get(&**name) else {
            return raise("NameError", format!("agent inconnu `{name}`"));
        };
        if !info.is(TypeKind::Agent) {
            return raise("TypeError", format!("`{name}` n'est pas un agent"));
        }
        let ty: &'p str = info.def.name.name.as_str();
        let budget = match info.directives.iter().find(|d| d.name.name == "budget") {
            Some(directive) => {
                let args = self.eval_args(&directive.args)?;
                Some(Arc::new(builtins::budget_from_args(&args)?))
            }
            None => None,
        };
        let state = self.agent_state(ty)?;
        Ok(Arc::new(AgentRef {
            id: self.next_id.fetch_add(1, AtomicOrdering::Relaxed),
            ty: ty.into(),
            state: Mutex::new(state),
            turn: Mutex::new(()),
            owner: Mutex::new(None),
            queued: AtomicUsize::new(0),
            budget,
            supervision,
            restarts: Mutex::new(Vec::new()),
            down: Mutex::new(None),
        }))
    }

    /// `spawn_pool(Writer, size: 4)`
    pub(crate) fn spawn_pool(&mut self, target: &Value<'p>, size: i64) -> R<'p> {
        if size < 1 {
            return raise("ArgumentError", "un pool contient au moins un agent");
        }
        let agents = (0..size).map(|_| self.spawn_agent(target, None)).collect::<Result<Vec<_>, _>>()?;
        Ok(Value::Pool(Arc::new(agents)))
    }

    pub(crate) fn supervision(&mut self, sup: &'p grenat_ast::TypeDef) -> Result<Supervision, Ctrl<'p>> {
        let mut supervision = Supervision {
            supervisor: sup.name.name.clone(),
            strategy: Strategy::OneForOne,
            max_restarts: 3,
            within: 60.0,
        };
        for option in &sup.options {
            let grenat_ast::Arg::Named { name, value: Some(expr) } = option else { continue };
            let value = self.eval(expr)?;
            match (name.name.as_str(), value) {
                ("strategy", Value::Symbol(s)) => {
                    supervision.strategy = match &*s {
                        "one_for_one" => Strategy::OneForOne,
                        "one_for_all" => Strategy::OneForAll,
                        "rest_for_one" => Strategy::RestForOne,
                        other => {
                            return raise("ArgumentError", format!("stratégie de supervision inconnue `:{other}`"));
                        }
                    }
                }
                ("max_restarts", Value::Int(n)) if n >= 0 => supervision.max_restarts = n as usize,
                ("within", Value::Duration(d)) => supervision.within = d,
                ("within", Value::Int(n)) => supervision.within = n as f64,
                ("within", Value::Float(f)) => supervision.within = f,
                (option, value) => {
                    return raise(
                        "ArgumentError",
                        format!("option de superviseur invalide `{option}: {}`", value.inspect()),
                    );
                }
            }
        }
        Ok(supervision)
    }

    /// Enfants déclarés d'un superviseur, dans l'ordre des `child`.
    pub(crate) fn declared_children(info: &TypeInfo<'p>) -> Vec<&'p str> {
        info.directives
            .iter()
            .filter(|d| d.name.name == "child")
            .filter_map(|d| match d.args.first() {
                Some(grenat_ast::Arg::Pos(Expr { kind: ExprKind::Const(p), .. })) => p.last().map(|i| i.name.as_str()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn supervisor_child(&mut self, sup: &str, agent: &str) -> R<'p> {
        let shared = self.shared.clone();
        let Some(info) = shared.types.get(sup).filter(|i| i.is(TypeKind::Supervisor)) else {
            return raise("TypeError", format!("`{sup}` n'est pas un superviseur"));
        };
        if !Self::declared_children(info).contains(&agent) {
            return raise("NameError", format!("`{agent}` n'est pas un enfant de `{sup}`"));
        }
        let key = (sup.to_string(), agent.to_string());
        if let Some(existing) = self.children.borrow().get(&key) {
            return Ok(Value::Agent(existing.clone()));
        }
        let def: &'p grenat_ast::TypeDef = info.def;
        let supervision = self.supervision(def)?;
        let child = self.spawn_agent(&Value::Type(agent.into()), Some(supervision))?;
        let child = self.children.borrow_mut().entry(key).or_insert(child).clone();
        Ok(Value::Agent(child))
    }

    /// Message à un agent ou à un pool : `ask` attend la réponse, `tell` non.
    pub(crate) fn send(&mut self, target: &Value<'p>, method: &str, args: Args<'p>) -> Option<R<'p>> {
        let agent = match target {
            Value::Agent(a) => a.clone(),
            // pool : l'agent le moins chargé, choisi et réservé d'un seul geste
            Value::Pool(pool) => {
                let _pick = self.pool_pick.borrow();
                pool.iter().min_by_key(|a| a.load()).expect("pool non vide").clone()
            }
            _ => return None,
        };
        agent.queued.fetch_add(1, AtomicOrdering::Relaxed);
        match method {
            "ask" => Some(self.agent_ask(agent, args)),
            "tell" => {
                let child = self.fork();
                self.spawn_task(child, move |task| {
                    if let Err(ctrl) = task.agent_ask(agent.clone(), args) {
                        let error = task.runtime_error(ctrl);
                        task.write_err(&format!(
                            "[tell] l'agent `{}` a échoué : {} : {}\n",
                            agent.ty, error.ty, error.message
                        ));
                    }
                });
                Some(Ok(Value::Nil))
            }
            _ => None,
        }
    }

    /// Enregistre que cette tâche attend `target` ; échoue si l'attente formerait un cycle.
    /// Enregistre que cette tâche attend `target` ; échoue si l'attente formerait un cycle.
    pub(crate) fn wait_for(&self, target: &Arc<AgentRef<'p>>) -> Result<(), Ctrl<'p>> {
        let mut waits = self.waits.borrow_mut();
        // agents dont cette tâche exécute actuellement un handler, du plus ancien au plus récent
        let mine: Vec<&str> = self.agents.iter().map(|f| &*f.agent.ty).collect();
        let me = mine.last().copied().unwrap_or("la tâche");
        let mut chain = vec![me.to_string(), target.ty.to_string()];
        let mut agent = target.clone();
        for _ in 0..1_000 {
            let Some(owner) = *agent.owner.borrow() else { break };
            if owner == self.task_id {
                // cycle refermé dans cette tâche : on repart de l'agent concerné
                let state = agent.state.borrow().clone();
                let from = self.agents.iter().rposition(|f| Arc::ptr_eq(&f.agent, &state)).unwrap_or(0);
                let mut cycle: Vec<String> = mine[from..].iter().map(|t| t.to_string()).collect();
                cycle.extend(chain.drain(1..));
                let message = if cycle.len() == 2 && cycle[0] == cycle[1] {
                    format!("`{}` s'envoie un message à lui-même et attendrait sa propre réponse", target.ty)
                } else {
                    format!("cycle d'attente entre agents : {}", cycle.join(" → "))
                };
                return raise("DeadlockError", message);
            }
            match waits.get(&owner) {
                Some(next) => {
                    chain.push(next.ty.to_string());
                    agent = next.clone();
                }
                None => break,
            }
        }
        waits.insert(self.task_id, target.clone());
        Ok(())
    }

    pub(crate) fn agent_ask(&mut self, agent: Arc<AgentRef<'p>>, args: Args<'p>) -> R<'p> {
        // la place réservée par `send` est rendue quand le tour commence, ou si l'envoi échoue avant
        let reservation = Reservation(&agent);
        let Some(message) = args.pos.into_iter().next() else {
            return raise("ArgumentError", "`ask` attend un message, par exemple `ask(Research(topic: t))`");
        };
        let Value::Record(record) = message.untainted().clone() else {
            return raise("TypeError", format!("message attendu, reçu {}", message.inspect()));
        };
        let shared = self.shared.clone();
        let info = &shared.types[&*agent.ty];
        let Some(handler) = info.handlers.get(&*record.ty).copied() else {
            let known: Vec<_> = info.handlers.keys().copied().collect();
            return raise(
                "NoMethodError",
                format!("l'agent `{}` ne gère pas `{}` (messages : {})", agent.ty, record.ty, known.join(", ")),
            );
        };
        let mut call_args = Args::default();
        for (name, value) in &record.fields {
            if name.starts_with('_') && name[1..].chars().all(|c| c.is_ascii_digit()) {
                call_args.pos.push(value.clone());
            } else {
                call_args.named.push((name.to_string(), value.clone()));
            }
        }
        if let Some(reason) = agent.down.borrow().clone() {
            return raise("AgentDown", format!("l'agent `{}` est arrêté : {reason}", agent.ty));
        }

        // un message à la fois : on attend le tour de l'agent
        self.wait_for(&agent)?;
        let turn = agent.turn.borrow();
        self.waits.borrow_mut().remove(&self.task_id);
        drop(reservation);
        *agent.owner.borrow_mut() = Some(self.task_id);
        let state = agent.state.borrow().clone();

        if let Some(b) = &agent.budget {
            self.budgets.push(b.clone());
        }
        self.agents.push(AgentFrame { agent: state.clone(), handler });
        let pushed = self.push_frame(Some(Value::Object(state)), new_scope(None));
        let result = pushed.and_then(|()| {
            let r = self
                .bind_params(&handler.params, call_args, &handler.message.name)
                .and_then(|()| self.eval_body(&handler.body));
            self.pop_frame();
            r
        });
        self.agents.pop();
        if agent.budget.is_some() {
            self.budgets.pop();
        }
        let result = match result {
            Ok(v) | Err(Ctrl::Return(v)) => Ok(v),
            Err(Ctrl::Raise(e)) => {
                e.trace.borrow_mut().push((format!("{}#{}", agent.ty, handler.message.name), handler.span));
                self.on_crash(&agent, &e)?;
                Err(Ctrl::Raise(e))
            }
            Err(other) => Err(other),
        };
        *agent.owner.borrow_mut() = None;
        drop(turn);
        result
    }

    /// Un handler a levé une erreur : le superviseur redémarre l'agent (état neuf),
    /// ou l'arrête s'il plante trop souvent.
    pub(crate) fn on_crash(&mut self, agent: &Arc<AgentRef<'p>>, error: &ErrorVal<'p>) -> Result<(), Ctrl<'p>> {
        let Some(sup) = &agent.supervision else { return Ok(()) };
        if &*error.ty == "Cancelled" {
            return Ok(());
        }
        let now = Instant::now();
        {
            let mut restarts = agent.restarts.borrow_mut();
            restarts.retain(|t| now.duration_since(*t).as_secs_f64() <= sup.within);
            if restarts.len() >= sup.max_restarts {
                let reason =
                    format!("{} plantages en moins de {}", restarts.len() + 1, crate::value::duration(sup.within));
                self.write_err(&format!("[superviseur {}] `{}` arrêté : {reason}\n", sup.supervisor, agent.ty));
                *agent.down.borrow_mut() = Some(reason);
                return Ok(());
            }
            restarts.push(now);
        }
        let targets: Vec<Arc<AgentRef<'p>>> = match sup.strategy {
            Strategy::OneForOne => vec![agent.clone()],
            strategy => {
                let shared = self.shared.clone();
                let order = Self::declared_children(&shared.types[sup.supervisor.as_str()]);
                let position = order.iter().position(|c| **c == *agent.ty).unwrap_or(0);
                let children = self.children.borrow();
                order
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| strategy == Strategy::OneForAll || *i >= position)
                    .filter_map(|(_, name)| children.get(&(sup.supervisor.clone(), name.to_string())).cloned())
                    .collect()
            }
        };
        for target in targets {
            let ty = self.types.get(&*target.ty).map(|t| t.def.name.name.as_str()).expect("agent déclaré");
            let fresh = self.agent_state(ty)?;
            *target.state.borrow_mut() = fresh;
            self.write_err(&format!(
                "[superviseur {}] `{}` redémarré après {} : {}\n",
                sup.supervisor, target.ty, error.ty, error.message
            ));
        }
        Ok(())
    }
}

/// Place réservée dans la file d'un agent (répartition des pools).
struct Reservation<'a, 'p>(&'a AgentRef<'p>);

impl Drop for Reservation<'_, '_> {
    fn drop(&mut self) {
        self.0.queued.fetch_sub(1, AtomicOrdering::Relaxed);
    }
}
