//! Built-in library signatures — an exact mirror of what the interpreter
//! accepts (`grenat_interp/src/builtins/`).

use crate::ty::Ty;

/// Methods defined on every value.
pub fn universal(name: &str) -> Option<Ty> {
    Some(match name {
        "nil?" | "tainted?" | "is_a?" => Ty::Bool,
        "to_s" | "inspect" => Ty::Str,
        "class" => Ty::Unknown,
        _ => return None,
    })
}

/// Parameter types of the block passed to `recv.name { |…| }`.
pub fn block_params(recv: &Ty, name: &str, arg0: Option<&Ty>) -> Vec<Ty> {
    match (recv.base(), name) {
        (Ty::Int, "times" | "upto") => vec![Ty::Int],
        (Ty::Array(t), "each_with_index") => vec![(**t).clone(), Ty::Int],
        (Ty::Array(t), "reduce" | "inject") => vec![arg0.cloned().unwrap_or_else(|| (**t).clone()), (**t).clone()],
        (Ty::Array(t), _) => vec![(**t).clone()],
        (Ty::Range, "reduce" | "inject") => vec![arg0.cloned().unwrap_or(Ty::Int), Ty::Int],
        (Ty::Range, _) => vec![Ty::Int],
        (Ty::Hash(k, v), _) => vec![(**k).clone(), (**v).clone()],
        (Ty::Result(_, e), "or_else") => vec![(**e).clone()],
        _ => Vec::new(),
    }
}

/// Return type of `recv.name(…)`; `None` if the method does not exist.
pub fn method(recv: &Ty, name: &str, nargs: usize, block: Option<&Ty>) -> Option<Ty> {
    use Ty::*;
    let has_args = nargs > 0;
    Some(match recv.base() {
        Int => match name {
            "times" | "upto" | "to_i" | "round" | "floor" | "ceil" | "abs" | "succ" | "pred" => Int,
            "to_f" => Float,
            "zero?" | "even?" | "odd?" | "between?" => Bool,
            "s" | "sec" | "second" | "seconds" | "min" | "minute" | "minutes" | "h" | "hour" | "hours" | "day"
            | "days" => Duration,
            _ => return None,
        },
        Float => match name {
            "round" if has_args => Float,
            "round" | "floor" | "ceil" | "to_i" => Int,
            "to_f" | "abs" => Float,
            "zero?" | "between?" => Bool,
            _ => return None,
        },
        Money => match name {
            "to_f" => Float,
            _ => return None,
        },
        Duration => match name {
            "seconds" | "to_f" | "minutes" => Float,
            _ => return None,
        },
        Str => match name {
            "size" | "length" | "to_i" => Int,
            "upcase" | "downcase" | "capitalize" | "strip" | "lstrip" | "rstrip" | "reverse" | "sub" | "gsub"
            | "truncate" | "ljust" | "rjust" => Str,
            "chars" | "lines" | "split" => Ty::array(Str),
            "empty?" | "include?" | "start_with?" | "end_with?" => Bool,
            "index" => Ty::opt(Int),
            "to_f" => Float,
            "to_sym" => Sym,
            _ => return None,
        },
        Sym => match name {
            "to_sym" => Sym,
            "size" | "length" => Int,
            _ => return None,
        },
        Array(t) => array_method(t, name, has_args, block)?,
        Range => match name {
            "include?" | "cover?" => Bool,
            "first" | "last" if !has_args => Int,
            "size" | "count" if block.is_none() => Int,
            _ => array_method(&Int, name, has_args, block)?,
        },
        Hash(k, v) => match name {
            "size" | "length" | "count" => Int,
            "empty?" | "key?" | "has_key?" | "include?" | "any?" | "all?" => Bool,
            "keys" => Ty::array((**k).clone()),
            "values" => Ty::array((**v).clone()),
            "fetch" | "delete" => (**v).clone(),
            "merge" | "select" | "filter" | "reject" | "each" => recv.base().clone(),
            "map" => Ty::array(block.cloned().unwrap_or(Unknown)),
            "to_a" | "sort_by" => Ty::array(Unknown),
            "find" | "sum" | "min_by" | "max_by" => Unknown,
            _ => return None,
        },
        Result(t, e) => match name {
            "ok?" | "err?" => Bool,
            "value" | "unwrap" | "unwrap_or" => (**t).clone(),
            "error" => (**e).clone(),
            "or_else" => crate::ty::join(t, block.unwrap_or(&Unknown)),
            _ => return None,
        },
        Budget => match name {
            "spent" => Money,
            "tokens" => Int,
            "remaining" => Ty::opt(Money),
            _ => return None,
        },
        _ => return None,
    })
}

fn array_method(t: &Ty, name: &str, has_args: bool, block: Option<&Ty>) -> Option<Ty> {
    use Ty::*;
    let elem = t.clone();
    let same = || Ty::array(elem.clone());
    let block_ty = || block.cloned().unwrap_or(Unknown);
    Some(match name {
        "size" | "length" | "count" => Int,
        "empty?" | "any?" | "all?" | "none?" | "include?" => Bool,
        "first" | "last" if has_args => same(),
        "first" | "last" | "find" | "detect" | "min" | "max" | "min_by" | "max_by" | "pop" | "shift" | "delete" => {
            elem.clone()
        }
        "index" | "find_index" => Ty::opt(Int),
        "each" | "each_with_index" | "select" | "filter" | "reject" | "sort" | "sort_by" | "reverse" | "push"
        | "append" | "unshift" | "uniq" | "compact" | "take" | "drop" | "to_a" | "dup" => same(),
        "map" | "collect" | "parallel_map" | "batch_map" => Ty::array(block_ty()),
        "flat_map" => match block_ty() {
            Array(inner) => Array(inner),
            other => Ty::array(other),
        },
        "partition" => Ty::array(same()),
        "sum" if block.is_some() => block_ty(),
        "sum" => elem.clone(),
        "join" => Str,
        "zip" => Ty::array(Ty::array(Unknown)),
        "reduce" | "inject" => block_ty(),
        "group_by" => Hash(Box::new(block_ty()), Box::new(same())),
        "tally" => Hash(Box::new(elem.clone()), Box::new(Int)),
        _ => return None,
    })
}

/// `Module.name(…)`: return type and effect.
pub fn static_method(module: &str, name: &str) -> Option<(Ty, Option<&'static str>)> {
    use Ty::*;
    Some(match (module, name) {
        ("File", "read") => (Str, Some("fs.read")),
        ("File", "lines") => (Ty::array(Str), Some("fs.read")),
        ("File", "exist?" | "directory?") => (Bool, Some("fs.read")),
        ("File", "write") => (Nil, Some("fs.write")),
        ("Dir", "list") => (Ty::array(Str), Some("fs.read")),
        ("Math", "pi" | "sqrt" | "log" | "sin" | "cos" | "exp") => (Float, None),
        ("Env", "get") => (Ty::opt(Str), Some("env")),
        ("Credentials", "fetch") => (Ty::Secret, Some("env")),
        ("Credentials", "dig") => (Ty::opt(Ty::Secret), Some("env")),
        ("Env", "fetch") => (Str, Some("env")),
        ("Json", "dump" | "generate") => (Str, None),
        ("Json", "parse") => (Unknown, None),
        ("Runtime", "on_approval") => (Nil, None),
        ("Cli", "confirm") => (Bool, Some("human")),
        ("Cli", "ask") => (Ty::opt(Str), Some("human")),
        ("Time", "now") => (Float, Some("time")),
        ("Time", "today") => (Str, Some("time")),
        ("Http", "get" | "post" | "put" | "patch" | "delete" | "head") => (User(HTTP_RESPONSE.into()), Some("net")),
        ("Db", "connect") => (User(DATABASE.into()), None),
        ("Shell", "run") => (User(SHELL_RESULT.into()), Some("shell")),
        ("Ssh", "connect") => (User(SSH_SESSION.into()), Some("ssh")),
        ("Pdf" | "Image", "read") => (User(ATTACHMENT.into()), Some("fs.read")),
        ("Pdf" | "Image", "url") => (User(ATTACHMENT.into()), None),
        ("Audio", "read") => (User(ATTACHMENT.into()), Some("fs.read")),
        // no provider fetches audio: Grenat downloads it
        ("Audio", "url") => (User(ATTACHMENT.into()), Some("net")),
        ("Mail", "connect") => (User(MAILER.into()), None),
        ("Html", "text" | "escape") => (Str, None),
        ("Conversation", "new") => (User(CONVERSATION.into()), None),
        ("Approvals", "pending") => (Ty::array(Ty::Hash(Box::new(Str), Box::new(Unknown))), Some("db.read")),
        ("Approvals", "approve" | "deny") => (Nil, Some("human")),
        ("Jobs", "enqueued") => (Ty::array(Ty::array(Unknown)), Some("db.read")),
        ("Jobs", "failed") => (Ty::array(Ty::array(Str)), Some("db.read")),
        ("Jobs", "perform") => (Int, Some("db.write")),
        ("Conversation", "load") => (User(CONVERSATION.into()), Some("fs.read")),
        ("Mail", "deliveries") => (Ty::array(Ty::Hash(Box::new(Str), Box::new(Unknown))), None),
        ("Mcp", "call") => (Str, Some("mcp")),
        ("Mcp", "tools") => (Ty::array(Str), Some("mcp")),
        _ => return None,
    })
}

pub const MODULES: &[&str] = &[
    "File",
    "Dir",
    "Math",
    "Env",
    "Credentials",
    "Json",
    "Runtime",
    "Cli",
    "Time",
    "Http",
    "Db",
    "Shell",
    "Ssh",
    "Mcp",
    "Pdf",
    "Image",
    "Audio",
    "Mail",
    "Html",
    "Conversation",
    "Jobs",
    "Approvals",
];

/// What `Conversation.new` returns.
pub const CONVERSATION: &str = "Conversation";

/// Methods of built-in records whose result comes from a model or outside.
pub fn untrusted_method(record: &str, name: &str) -> bool {
    matches!((record, name), (CONVERSATION, "say") | (SFTP, "read" | "list"))
}

/// What `Mail.connect` returns.
pub const MAILER: &str = "Mailer";

/// What a route or webhook handler receives.
pub const REQUEST: &str = "Request";

/// What `html`, `json`, `status`, `redirect` and `stream` build.
pub const RESPONSE: &str = "Response";

/// The `out` of `stream do |out| … end`: it takes events.
pub const EVENT_STREAM: &str = "EventStream";

/// Built-in functions that make an untrusted value safe (escaping).
pub fn sanitizes(module: &str, name: &str) -> bool {
    matches!((module, name), ("Html", "escape"))
}

/// What `Pdf.read`, `Image.read`, `Audio.read`… return: a document, an
/// image or audio for a model.
pub const ATTACHMENT: &str = "Attachment";

/// What `transcribe(…, segments: true)` lists: a stretch of speech.
pub const TRANSCRIPT_SEGMENT: &str = "TranscriptSegment";

/// Built-in functions whose result comes from outside, hence untrusted.
pub fn untrusted_result(module: &str, name: &str) -> bool {
    matches!((module, name), ("Mcp", "call"))
}

/// What `Shell.run` returns.
pub const SHELL_RESULT: &str = "ShellResult";

/// What `Db.connect` returns.
pub const DATABASE: &str = "Database";

/// What `Ssh.connect` returns, what its `run` returns, what its `sftp`
/// returns, and an entry of `sftp.list`.
pub const SSH_SESSION: &str = "SshSession";
pub const SSH_RESULT: &str = "SshResult";
pub const SFTP: &str = "Sftp";
pub const SFTP_ENTRY: &str = "SftpEntry";

/// The records whose methods reach an SSH server.
pub fn is_ssh(record: &str) -> bool {
    matches!(record, SSH_SESSION | SFTP)
}

/// The local file of a transfer: which argument, and the effect on it.
pub fn local_side(record: &str, name: &str) -> Option<(usize, &'static str)> {
    match (record, name) {
        (SSH_SESSION | SFTP, "upload") => Some((0, "fs.read")),
        (SSH_SESSION | SFTP, "download") => Some((1, "fs.write")),
        _ => None,
    }
}

/// A method with arguments of a built-in record: its return type and effect.
pub fn record_method(record: &str, name: &str) -> Option<(Ty, &'static str)> {
    Some(match (record, name) {
        (DATABASE, "query") => (Ty::array(Ty::Unknown), "db.read"),
        (DATABASE, "first") => (Ty::Unknown, "db.read"),
        (DATABASE, "execute") => (Ty::Int, "db.write"),
        (DATABASE, "migrate") => (Ty::Nil, "db.write"),
        (DATABASE, "vector") => (Ty::Str, "db.write"),
        (DATABASE, "transaction") => (Ty::Unknown, "db.write"),
        (MAILER, "send") => (Ty::Nil, "net"),
        (CONVERSATION, "say") => (Ty::Str, "llm"),
        (CONVERSATION, "save") => (Ty::Nil, "fs.write"),
        (SSH_SESSION, "run") => (Ty::User(SSH_RESULT.into()), "ssh"),
        (SSH_SESSION, "sftp") => (Ty::User(SFTP.into()), "ssh"),
        (SSH_SESSION | SFTP, "upload" | "download") => (Ty::Nil, "ssh"),
        (SFTP, "list") => (Ty::array(Ty::User(SFTP_ENTRY.into())), "ssh"),
        (SFTP, "read") => (Ty::Str, "ssh"),
        (SFTP, "write" | "remove" | "mkdir" | "rename") => (Ty::Nil, "ssh"),
        (SFTP, "exists?") => (Ty::Bool, "ssh"),
        _ => return None,
    })
}

/// What `Http` requests return.
pub const HTTP_RESPONSE: &str = "HttpResponse";

/// A field (or method without arguments) of a built-in record: its type,
/// and whether it is untrusted (tainted).
pub fn record_field(record: &str, name: &str) -> Option<(Ty, bool)> {
    Some(match (record, name) {
        (DATABASE, "primary_key") => (Ty::Str, false),
        (DATABASE, "dialect") => (Ty::Sym, false),
        (HTTP_RESPONSE, "status") => (Ty::Int, false),
        (HTTP_RESPONSE, "ok?") => (Ty::Bool, false),
        (HTTP_RESPONSE, "body") => (Ty::Str, true),
        (HTTP_RESPONSE, "headers") => (Ty::Hash(Box::new(Ty::Str), Box::new(Ty::Str)), true),
        (HTTP_RESPONSE, "json") => (Ty::Unknown, true),
        (REQUEST, "method" | "path") => (Ty::Str, false),
        (REQUEST, "query") => (Ty::Str, true),
        (REQUEST, "params") => (Ty::Hash(Box::new(Ty::Str), Box::new(Ty::Str)), true),
        (REQUEST, "body") => (Ty::Str, true),
        (REQUEST, "headers") => (Ty::Hash(Box::new(Ty::Str), Box::new(Ty::Str)), true),
        (REQUEST, "json") => (Ty::Unknown, true),
        (CONVERSATION, "history") => (Ty::array(Ty::Hash(Box::new(Ty::Str), Box::new(Ty::Str))), false),
        (CONVERSATION, "summary") => (Ty::Str, true),
        (SHELL_RESULT, "status") => (Ty::Int, false),
        (SHELL_RESULT, "ok?") => (Ty::Bool, false),
        (SHELL_RESULT, "stdout" | "stderr") => (Ty::Str, true),
        (SSH_SESSION | SFTP, "id") => (Ty::Int, false),
        (SSH_SESSION | SFTP, "host") => (Ty::Str, false),
        (SSH_SESSION, "user") => (Ty::Str, false),
        (SSH_SESSION, "port") => (Ty::Int, false),
        (SSH_SESSION, "close") => (Ty::Nil, false),
        (SSH_RESULT, "status") => (Ty::Int, false),
        (SSH_RESULT, "ok?") => (Ty::Bool, false),
        (SSH_RESULT, "signal") => (Ty::opt(Ty::Str), true),
        (SSH_RESULT, "stdout" | "stderr") => (Ty::Str, true),
        (SFTP_ENTRY, "name") => (Ty::Str, false),
        (SFTP_ENTRY, "size") => (Ty::Int, false),
        (SFTP_ENTRY, "dir?") => (Ty::Bool, false),
        (SFTP_ENTRY, "modified") => (Ty::opt(Ty::Float), false),
        (TRANSCRIPT_SEGMENT, "start" | "end") => (Ty::Float, false),
        (TRANSCRIPT_SEGMENT, "text") => (Ty::Str, true),
        (TRANSCRIPT_SEGMENT, "speaker") => (Ty::opt(Ty::Str), true),
        _ => return None,
    })
}

pub const TYPE_NAMES: &[&str] = &[
    "Int", "Float", "String", "Bool", "Array", "Hash", "Symbol", "Nil", "Range", "Money", "Duration", "Secret",
    "Vector",
];

pub const GLOBALS: &[&str] = &[
    "puts",
    "print",
    "p",
    "warn",
    "raise",
    "spawn",
    "spawn_pool",
    "budget",
    "within",
    "step",
    "approve!",
    "with_human",
    "deny_all",
    "approve_all",
    "test",
    "assert",
    "assert_equal",
    "assert_raises",
    "mock",
    "mock_http",
    "mock_shell",
    "mcp",
    "mock_mcp",
    "mock_ssh",
    "mock_credentials",
    "mock_embed",
    "mock_transcribe",
    "database",
    "enqueue",
    "expose",
    "migration",
    "every",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "html",
    "json",
    "status",
    "redirect",
    "stream",
    "request",
    "on_webhook",
    "deliver_webhook",
    "cassette",
    "fixture",
    "call",
    "eval",
    "judge",
    "embed",
    "transcribe",
    "loop",
    "sleep",
    "exit",
    "race",
];

/// Known fields of built-in errors.
pub fn error_field(error: &str, name: &str) -> Option<Ty> {
    Some(match (error, name) {
        (_, "message" | "type" | "full_message") => Ty::Str,
        ("BudgetExceeded", "spent") => Ty::Money,
        ("BudgetExceeded", "tokens") => Ty::Int,
        _ => return None,
    })
}
