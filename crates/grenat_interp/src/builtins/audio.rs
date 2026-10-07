//! `Audio`: recordings given to a model — to `transcribe`, or in a prompt
//! where the model's provider takes audio.
//!
//! ```ruby
//! notes = transcribe(:whisper, Audio.read("meeting.mp3"))
//! Audio.url("https://files.acme.io/standup.m4a")   # downloaded when called
//! ```
//!
//! Reading a file is an `fs.read` effect. No provider fetches audio by URL:
//! `Audio.url` downloads it, a `net` effect whose host is checked as
//! `Http`'s (an untrusted URL is a `TaintError`). Either way, audio is
//! 25 MB at most — the largest any provider takes — said before anything
//! is read whole or uploaded.

use std::io::Read;
use std::time::Duration;

use grenat_llm::audio::{extensions, media_type_of_header, media_type_of_path, megabytes};
use grenat_llm::catalog::MB;

use crate::http::{Download, HttpRequest, ProxyChoice, host};
use crate::prelude::*;

use super::*;

/// The most audio Grenat reads or downloads: OpenAI's transcription limit.
pub(crate) const MAX_AUDIO_BYTES: usize = 25 * MB;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) fn call_audio<'p>(interp: &mut Interp<'p>, name: &str, args: &Args<'p>) -> R<'p> {
    match name {
        "read" => read(interp, args),
        "url" => download(interp, args),
        _ => raise("NoMethodError", format!("unknown method `Audio.{name}`")),
    }
}

/// `Audio.read(path)`: the file, if it is audio Grenat knows and not too large.
fn read<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    let path = str_arg(args, 0, "read")?.to_string();
    interp.check_fs("fs.read", &path)?;
    let Some(media_type) = media_type_of_path(&path) else {
        return raise("ArgumentError", format!("`{path}`: audio is {}", extensions()));
    };
    let size = std::fs::metadata(&path).or_else(|e| io_error(&path, e))?.len();
    if size > MAX_AUDIO_BYTES as u64 {
        return raise("ArgumentError", too_large(&path, Some(size as usize)));
    }
    // a pipe or a device says no size: what is read stops at the limit
    let mut bytes = Vec::new();
    let file = std::fs::File::open(&path).or_else(|e| io_error(&path, e))?;
    file.take(MAX_AUDIO_BYTES as u64 + 1).read_to_end(&mut bytes).or_else(|e| io_error(&path, e))?;
    if bytes.len() > MAX_AUDIO_BYTES {
        return raise("ArgumentError", too_large(&path, None));
    }
    Ok(attachment("audio", media_type, "base64", base64(&bytes), Some(file_name(&path))))
}

fn io_error<'p, T>(path: &str, e: std::io::Error) -> Result<T, Ctrl<'p>> {
    raise("IoError", format!("reading `{path}`: {e}"))
}

/// `Audio.url(url)`: downloaded now, its format read from its extension or
/// its `Content-Type`.
fn download<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    if args.pos.iter().any(Value::contains_taint) {
        return raise("TaintError", "an untrusted value reaches `Audio.url` (effect `net`) without validation");
    }
    let url = text_arg(args, 0, "url")?;
    let Some(host) = host(&url).map(str::to_string) else {
        return raise("ArgumentError", format!("`Audio.url` expects an http(s) URL, got {url:?}"));
    };
    interp.check_net(&host, &url)?;
    // a URL holding a secret is not named
    let shown = if args.pos[0].contains_secret() { format!("{host} [secret]") } else { url.clone() };
    let request = HttpRequest {
        method: "GET",
        url: url.clone(),
        headers: Vec::new(),
        body: None,
        timeout: DOWNLOAD_TIMEOUT,
        proxy: ProxyChoice::Environment,
    };
    interp.record_request("GET", &url, args);
    let got = match interp.http_stub(&request) {
        Some(reply) => {
            let body = reply.body.into_bytes();
            let too_large = body.len() > MAX_AUDIO_BYTES;
            Download { status: reply.status, headers: reply.headers, body, too_large }
        }
        None if interp.offline => {
            return raise("HttpError", format!("no network in tests: `GET {shown}` is not stubbed with `mock_http`"));
        }
        None => grenat_green::blocking(|| crate::http::download(&request, MAX_AUDIO_BYTES))
            .or_else(|e| raise("HttpError", format!("GET {shown}: {}", e.replace(&url, &shown))))?,
    };
    if interp.log {
        interp.write_err(&format!("[http] GET {shown} → {} ({} bytes)\n", got.status, got.body.len()));
    }
    if got.status != 200 {
        return raise("HttpError", format!("GET {shown}: HTTP {}", got.status));
    }
    if got.too_large {
        return raise("ArgumentError", too_large(&shown, None));
    }
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let header = got.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.as_str());
    let Some(media_type) = media_type_of_path(path).or_else(|| header.and_then(media_type_of_header)) else {
        return raise(
            "ArgumentError",
            format!(
                "`{shown}`: neither its extension nor its Content-Type ({}) is audio Grenat knows ({})",
                header.unwrap_or("none"),
                extensions()
            ),
        );
    };
    Ok(attachment("audio", media_type, "base64", base64(&got.body), None))
}

/// What to do with audio over the limit; its size, when known.
fn too_large(what: &str, size: Option<usize>) -> String {
    let size = size.map_or_else(|| "over 25 MB".to_string(), |n| format!("{n} bytes ({})", megabytes(n)));
    format!(
        "`{what}` is {size}: audio is given to models up to {MAX_AUDIO_BYTES} bytes (25 MB) — compress it (mp3, m4a) or split it into parts"
    )
}
