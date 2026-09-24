//! Test provider: scripted replies, or replies computed from the request.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::Value as Json;

use crate::*;

pub(crate) type Responder = Box<dyn Fn(&Json) -> Response + Send + Sync>;

/// Test provider: replays replies in order (or computes them from the request,
/// for concurrent runs) and records every request.
pub struct Scripted {
    replies: Mutex<VecDeque<Response>>,
    responder: Option<Responder>,
    requests: Mutex<Vec<Json>>,
}

impl Scripted {
    pub fn new(replies: impl IntoIterator<Item = Response>) -> Self {
        Scripted { replies: Mutex::new(replies.into_iter().collect()), responder: None, requests: Mutex::default() }
    }

    /// Reply computed from the request body: deterministic whatever the call order.
    pub fn responder(f: impl Fn(&Json) -> Response + Send + Sync + 'static) -> Self {
        Scripted { replies: Mutex::default(), responder: Some(Box::new(f)), requests: Mutex::default() }
    }

    /// JSON body of every request received.
    pub fn requests(&self) -> Vec<Json> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Provider for Scripted {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        let body = request_body(request);
        self.requests.lock().unwrap_or_else(|e| e.into_inner()).push(body.clone());
        if let Some(responder) = &self.responder {
            return Ok(responder(&body));
        }
        self.replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .ok_or_else(|| LlmError::new("no scripted reply left"))
    }
}
