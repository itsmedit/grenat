//! Streamed model calls: the provider reads the answer on a thread of its
//! own and sends each piece over a channel; this task — the one running the
//! program — receives them and hands them to the program's block as they
//! come, then accounts for the call as for any other.
//!
//! When the block fails (an error, a client gone), the channel is dropped:
//! the provider stops at its next piece, and the call ends with the block's
//! error. A batched call (`batch_map`) is answered whole, as one piece.

use std::time::Instant;

use crate::prelude::*;
use grenat_llm::{Delta, Request, Response};

/// What the provider's thread sends: a piece, or the end.
enum Piece {
    Text(String),
    ToolInput { id: String, name: String, json: String },
    Done(Result<Response, String>),
}

impl Piece {
    fn of(delta: Delta) -> Piece {
        match delta {
            Delta::Text(text) => Piece::Text(text.into()),
            Delta::ToolInput { id, name, json } => {
                Piece::ToolInput { id: id.into(), name: name.into(), json: json.into() }
            }
        }
    }

    fn delta(&self) -> Delta<'_> {
        match self {
            Piece::Text(text) => Delta::Text(text),
            Piece::ToolInput { id, name, json } => Delta::ToolInput { id, name, json },
            Piece::Done(_) => unreachable!("the end is not a piece"),
        }
    }
}

/// Receives the pieces of a streamed answer, on the program's task.
pub(crate) type OnPiece<'a, 'p> = dyn FnMut(&mut Interp<'p>, Delta) -> Result<(), Ctrl<'p>> + 'a;

impl<'p> Interp<'p> {
    /// A model call whose answer is given to `on_piece` as it arrives; the
    /// whole response once done, accounted as [`Interp::llm_call`] does.
    pub(crate) fn llm_stream(
        &mut self,
        request: &Request,
        on_piece: &mut OnPiece<'_, 'p>,
    ) -> Result<Response, Ctrl<'p>> {
        if self.batch.is_some() {
            let response = self.llm_call(request)?;
            return self.give_whole(&response, on_piece).map(|()| response);
        }
        self.before_call(request)?;
        let started = Instant::now();
        let provider = self.provider(request.model)?;
        let (sender, pieces) = grenat_green::channel();
        let model = request.model.clone();
        let owned =
            (request.system.clone(), request.messages.clone(), request.tools.clone(), request.output_schema.clone());
        std::thread::Builder::new()
            .name("grenat-stream".into())
            .spawn(move || {
                let (system, messages, tools, output_schema) = owned;
                let request = Request { model: &model, system, messages, tools, output_schema };
                let answer = provider.stream(&request, &mut |delta| {
                    // the program no longer listens: stop reading
                    if sender.send(Piece::of(delta)).is_ok() {
                        std::ops::ControlFlow::Continue(())
                    } else {
                        std::ops::ControlFlow::Break(())
                    }
                });
                let _ = sender.send(Piece::Done(answer.map_err(|e| e.message)));
            })
            .map_err(|e| Ctrl::Raise(Arc::new(ErrorVal::new("LlmError", format!("cannot stream: {e}")))))?;
        let answer = loop {
            match pieces.recv() {
                Some(Piece::Done(answer)) => break answer,
                Some(piece) => {
                    // dropping `pieces` on an error stops the provider
                    on_piece(self, piece.delta())?;
                    self.check_cancel()?;
                }
                None => break Err("the stream ended without an answer".into()),
            }
        };
        match answer {
            Ok(response) => self.after_call(request, response, started, false),
            Err(message) => raise("LlmError", message),
        }
    }

    /// An answer that came whole, given as streamed ones are.
    fn give_whole(&mut self, response: &Response, on_piece: &mut OnPiece<'_, 'p>) -> Result<(), Ctrl<'p>> {
        let text = response.text();
        if !text.is_empty() {
            on_piece(self, Delta::Text(&text))?;
        }
        for call in response.tool_uses() {
            on_piece(self, Delta::ToolInput { id: &call.id, name: &call.name, json: &call.input.to_string() })?;
        }
        Ok(())
    }
}
