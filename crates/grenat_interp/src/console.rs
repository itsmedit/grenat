//! `grenat console`: the program's declarations run (its database, its MCP
//! servers, its routes), then the console (`grenat_console`) is served —
//! no worker, no schedule: the console observes and operates, the
//! application runs elsewhere (`grenat serve`).

use grenat_ast::{Program, TypeKind};
use grenat_console::{Access, Agent, Application, Console, Exposure, Mcp, McpServer, Request};

use crate::eval::store::now;
use crate::value::Locked;
use crate::{Interp, Options, RuntimeError, on_interpreter_thread, spawner};

fn failure(ty: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError { ty: ty.into(), message: message.into(), span: None, trace: Vec::new() }
}

/// Serves the console of `program` (named `name`) on `address`, with a
/// token or not; `listening` is told where.
pub fn console(
    program: &Program,
    options: Options,
    name: &str,
    address: &str,
    token: Option<String>,
    listening: impl FnOnce(&str) + Send,
) -> Result<(), RuntimeError> {
    on_interpreter_thread(|green| {
        let mut interp = Interp::new(program, options, spawner(green))?;
        if let Err(ctrl) = interp.run_script() {
            return Err(interp.runtime_error(ctrl));
        }
        let Some(database) = interp.app_db.borrow().clone() else {
            return Err(failure("ArgumentError", "the console reads the application's database: declare one (`database Env.fetch(\"DATABASE_URL\")`)"));
        };
        let connection = interp.connection_of(&database).expect("a database");
        let server = grenat_serve::Server::bind(address).map_err(|e| failure("IoError", e))?;
        // the address bound: the port chosen when `address` asks for any
        let access = Access::for_address(&server.address(), token).map_err(|e| failure("ArgumentError", e))?;
        let journal_dir = interp.journal_dir.borrow().clone();
        let mut console = Console::new(application(&interp, name), journal_dir, access);
        listening(&server.address());
        loop {
            let Ok(incoming) = grenat_green::blocking(|| server.next()) else { continue };
            let request = Request {
                method: incoming.method.clone(),
                path: incoming.path.clone(),
                query: incoming.query.clone(),
                headers: incoming.headers.clone(),
                body: incoming.body.clone(),
            };
            let response = {
                let mut db = connection.lock();
                console.handle(&request, &mut **db, &mut Servers { interp: &mut interp }, now())
            };
            if interp.log {
                interp.write_err(&format!("[console] {} {} → {}\n", request.method, request.path, response.status));
            }
            incoming.respond(response.status, &response.content_type, &response.headers, response.body);
        }
    })
}

/// What the program declares, for the console.
fn application(interp: &Interp, name: &str) -> Application {
    let mut agents: Vec<Agent> = interp
        .types
        .values()
        .filter(|info| info.is(TypeKind::Agent))
        .map(|info| {
            let mut handlers: Vec<_> = info.handlers.values().collect();
            handlers.sort_by_key(|h| h.span.start);
            Agent { name: info.def.name.name.clone(), handlers: handlers.iter().map(|h| h.message.name.clone()).collect() }
        })
        .collect();
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    let functions = |kind| {
        let mut names: Vec<String> = interp.fns.values().filter(|d| d.kind == kind).map(|d| d.name.name.clone()).collect();
        names.sort();
        names
    };
    let mut routes: Vec<String> = interp.routes.borrow().iter().map(|r| format!("{} /{}", r.method, r.pattern.join("/"))).collect();
    routes.extend(interp.webhooks.borrow().iter().map(|w| format!("POST {} (webhook)", w.path)));
    let mut mcp_servers: Vec<McpServer> =
        interp.mcp_servers.borrow().iter().map(|(name, server)| McpServer { name: name.clone(), target: server.target() }).collect();
    mcp_servers.sort_by(|a, b| a.name.cmp(&b.name));
    let exposures = interp
        .exposures
        .borrow()
        .iter()
        .map(|e| Exposure { path: e.path.clone(), tools: e.entries.iter().map(|x| x.spec.name.clone()).collect(), public: e.token.is_none() })
        .collect();
    Application {
        name: name.to_string(),
        agents,
        workflows: functions(grenat_ast::FnKind::Workflow),
        tools: functions(grenat_ast::FnKind::Tool),
        routes,
        mcp_servers,
        exposures,
    }
}

/// The program's MCP servers, connected to when the console lists their tools.
struct Servers<'a, 'p> {
    interp: &'a mut Interp<'p>,
}

impl Mcp for Servers<'_, '_> {
    fn tools(&mut self, server: &str) -> Result<Vec<grenat_mcp::Tool>, String> {
        self.interp.mcp_tools(server).map_err(|ctrl| {
            let error = self.interp.runtime_error(ctrl);
            format!("{}: {}", error.ty, error.message)
        })
    }
}
