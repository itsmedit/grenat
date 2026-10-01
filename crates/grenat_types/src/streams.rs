//! Streaming, checked: the block of a prompt call, of an agent's `ask` or of
//! a conversation's `say` receives the answer's pieces — untrusted, as the
//! answer — and only a `String` answer streams; `stream do |out| … end`
//! builds a response whose `out` takes events (`out << data`,
//! `out.event(name, data)`): data may be untrusted (it is text, each line a
//! `data:` line), an event's name may not, and no secret is sent.

use grenat_ast::{Block, FnDef, Span, Type};

use crate::builtins::EVENT_STREAM;
use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// The block given to a call that answers with a model's text: each
    /// piece, an untrusted `String`.
    pub(crate) fn answer_block(&mut self, cx: &mut Ctx<'p>, span: Span, block: Option<&'p Block>) {
        self.walk_block(cx, block, &[V { ty: Ty::Str, taint: Some(span) }]);
    }

    /// A prompt called with a block: it must answer a `String`.
    pub(crate) fn prompt_stream(&mut self, cx: &mut Ctx<'p>, span: Span, def: &'p FnDef, block: Option<&'p Block>) {
        self.only_text(span, &def.name.name, def.ret.as_ref());
        self.answer_block(cx, span, block);
    }

    /// `agent.ask(Message(…)) { |chunk| … }`: the handler must answer a `String`.
    pub(crate) fn ask_stream(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        agent: &str,
        argv: &[ArgV],
        block: Option<&'p Block>,
    ) {
        let handler = match argv.first().map(|a| &a.v.ty) {
            Some(Ty::User(message)) => self.types.get(agent).and_then(|d| d.handlers.get(message.as_str()).copied()),
            _ => None,
        };
        if let Some(handler) = handler {
            self.only_text(span, &format!("{agent}#{}", handler.message.name), handler.ret.as_ref());
        }
        self.answer_block(cx, span, block);
    }

    /// Only a `String` answer streams: a structure is whole or nothing.
    fn only_text(&mut self, span: Span, owner: &str, ret: Option<&Type>) {
        let ty = ret.map_or(Ty::Str, |t| self.peek_ty(t));
        if !matches!(ty.base(), Ty::Str | Ty::Unknown) {
            self.error(E_TYPE, span, format!("`{owner}` answers `{ty}`: only an answer that is a `String` streams"));
        }
    }

    /// `stream do |out| … end`: a response; `out` takes events.
    pub(crate) fn stream_response(&mut self, cx: &mut Ctx<'p>, block: Option<&'p Block>) -> V {
        self.walk_block(cx, block, &[V::new(Ty::user(EVENT_STREAM))]);
        V::new(Ty::user(builtins::RESPONSE))
    }

    /// `out << data`: any text, but no secret.
    pub(crate) fn event_data(&mut self, arg: &V, span: Span) {
        if self.holds_secret(&arg.ty, span) {
            self.secret_to_client(span);
        }
    }

    /// `out.event(name, data)`: a name of the program's own.
    pub(crate) fn event_method(&mut self, span: Span, name: &str, argv: &[ArgV]) -> V {
        if name != "event" {
            self.error(E_TYPE, span, format!("unknown method `{name}` for `{EVENT_STREAM}`"));
            return V::unknown();
        }
        if argv.len() != 2 {
            self.error(E_TYPE, span, "`event` takes a name and its data: `out.event(\"done\", data)`");
        }
        if let Some(event) = argv.first()
            && let Some(origin) = event.v.taint
        {
            self.taint_violation(event.span, origin, "event", "web");
        }
        if let Some(data) = argv.get(1) {
            self.event_data(&data.v, data.span);
        }
        V::new(Ty::Nil)
    }
}
