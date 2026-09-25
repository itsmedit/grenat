//! An HTTP server: requests in, one response each.


/// A request received, to answer with [`Incoming::respond`].
pub struct Incoming {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    request: tiny_http::Request,
}

impl Incoming {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn respond(self, status: u16, content_type: &str, body: String) {
        let header = tiny_http::Header::from_bytes("Content-Type", content_type).expect("a valid header");
        let response = tiny_http::Response::from_string(body).with_status_code(status).with_header(header);
        let _ = self.request.respond(response);
    }
}

pub struct Server {
    inner: tiny_http::Server,
}

impl Server {
    /// Listens on `address` (`127.0.0.1:0` for any free port).
    pub fn bind(address: &str) -> Result<Server, String> {
        tiny_http::Server::http(address).map(|inner| Server { inner }).map_err(|e| format!("cannot listen on {address}: {e}"))
    }

    /// Where it listens.
    pub fn address(&self) -> String {
        self.inner.server_addr().to_string()
    }

    /// The next request (blocks).
    pub fn next(&self) -> Result<Incoming, String> {
        let mut request = self.inner.recv().map_err(|e| e.to_string())?;
        let mut body = Vec::new();
        request.as_reader().read_to_end(&mut body).map_err(|e| e.to_string())?;
        let url = request.url().to_string();
        let (path, query) = url.split_once('?').map_or((url.clone(), String::new()), |(p, q)| (p.to_string(), q.to_string()));
        let headers = request.headers().iter().map(|h| (h.field.to_string(), h.value.to_string())).collect();
        Ok(Incoming { method: request.method().to_string(), path, query, headers, body, request })
    }
}
