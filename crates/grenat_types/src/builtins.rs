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
        (Ty::Array(_), "each_slice" | "each_cons") => vec![recv.base().clone()],
        (Ty::Array(t), "each_with_object") => vec![(**t).clone(), arg0.cloned().unwrap_or(Ty::Unknown)],
        (Ty::Array(t), "sort") => vec![(**t).clone(), (**t).clone()],
        (Ty::Array(t), _) => vec![(**t).clone()],
        (Ty::Range, "reduce" | "inject") => vec![arg0.cloned().unwrap_or(Ty::Int), Ty::Int],
        (Ty::Range, "each_slice" | "each_cons") => vec![Ty::array(Ty::Int)],
        (Ty::Range, "each_with_object") => vec![Ty::Int, arg0.cloned().unwrap_or(Ty::Unknown)],
        (Ty::Range, "sort") => vec![Ty::Int, Ty::Int],
        (Ty::Range, _) => vec![Ty::Int],
        (Ty::Hash(_, v), "transform_values") => vec![(**v).clone()],
        (Ty::Hash(k, _), "transform_keys" | "fetch") => vec![(**k).clone()],
        (Ty::Hash(k, v), "merge") => vec![(**k).clone(), (**v).clone(), (**v).clone()],
        // the `[key, value]` pair, then the object
        (Ty::Hash(..), "each_with_object") => vec![Ty::array(Ty::Unknown), arg0.cloned().unwrap_or(Ty::Unknown)],
        (Ty::Hash(k, v), _) => vec![(**k).clone(), (**v).clone()],
        (Ty::Result(_, e), "or_else") => vec![(**e).clone()],
        _ => Vec::new(),
    }
}

/// Return type of `recv.name(args…)`; `None` if the method does not exist.
pub fn method(recv: &Ty, name: &str, args: &[Ty], block: Option<&Ty>) -> Option<Ty> {
    use Ty::*;
    let has_args = !args.is_empty();
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
            "to_i" => Int,
            _ => return None,
        },
        Str => match name {
            "size" | "length" | "to_i" => Int,
            "upcase" | "downcase" | "capitalize" | "swapcase" | "strip" | "lstrip" | "rstrip" | "reverse" | "sub"
            | "gsub" | "truncate" | "ljust" | "rjust" | "center" | "delete_prefix" | "delete_suffix" | "squeeze"
            | "tr" | "delete" => Str,
            "count" => Int,
            // `nil` out of range, as `s[start, length]`
            "slice" => Ty::opt(Str),
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
        Array(t) => array_method(t, name, args, block)?,
        Range => match name {
            "include?" | "cover?" => Bool,
            "first" | "last" if !has_args => Int,
            "size" | "count" if block.is_none() => Int,
            _ => array_method(&Int, name, args, block)?,
        },
        Hash(k, v) => match name {
            "size" | "length" | "count" => Int,
            "empty?" | "key?" | "has_key?" | "include?" | "any?" | "all?" | "none?" => Bool,
            "keys" => Ty::array((**k).clone()),
            "values" => Ty::array((**v).clone()),
            "fetch" | "delete" => (**v).clone(),
            "merge" | "select" | "filter" | "reject" | "each" | "each_pair" | "to_h" => recv.base().clone(),
            "transform_values" => Hash(k.clone(), Box::new(block.cloned().unwrap_or(Unknown))),
            "transform_keys" => Hash(Box::new(block.cloned().unwrap_or(Unknown)), v.clone()),
            "map" => Ty::array(block.cloned().unwrap_or(Unknown)),
            "flat_map" | "filter_map" => array_method(&Unknown, name, args, block)?,
            "each_with_object" => args.first().cloned().unwrap_or(Unknown),
            "group_by" => Hash(Box::new(block.cloned().unwrap_or(Unknown)), Box::new(Ty::array(Ty::array(Unknown)))),
            "partition" => Ty::array(Ty::array(Ty::array(Unknown))),
            "to_a" | "sort_by" => Ty::array(Unknown),
            "find" | "sum" | "min_by" | "max_by" | "dig" => Unknown,
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

fn array_method(t: &Ty, name: &str, args: &[Ty], block: Option<&Ty>) -> Option<Ty> {
    use Ty::*;
    let has_args = !args.is_empty();
    let elem = t.clone();
    let same = || Ty::array(elem.clone());
    let block_ty = || block.cloned().unwrap_or(Unknown);
    Some(match name {
        "size" | "length" | "count" => Int,
        "empty?" | "any?" | "all?" | "none?" | "one?" | "include?" => Bool,
        "first" | "last" | "min" | "max" if has_args => same(),
        // `xs.slice(i)` is an item; `xs.slice(start, length)`, `xs.slice(a..b)` an array, or `nil`
        "slice" if matches!(args, [Int]) => elem.clone(),
        "slice" => Ty::opt(same()),
        "first" | "last" | "find" | "detect" | "min" | "max" | "min_by" | "max_by" | "pop" | "shift" | "delete" => {
            elem.clone()
        }
        "index" | "find_index" => Ty::opt(Int),
        "each" | "each_with_index" | "select" | "filter" | "reject" | "sort" | "sort_by" | "reverse" | "push"
        | "append" | "unshift" | "uniq" | "compact" | "take" | "drop" | "to_a" | "dup" | "take_while"
        | "drop_while" | "rotate" => same(),
        "each_slice" | "each_cons" if block.is_some() => same(),
        "each_slice" | "each_cons" => Ty::array(same()),
        "flatten" => flattened(&elem, !has_args),
        "filter_map" => Ty::array(block_ty().base().clone()),
        "minmax" => same(),
        "product" if args.iter().all(|a| *a == same()) => Ty::array(same()),
        "product" => Ty::array(Ty::array(Unknown)),
        "each_with_object" => args.first().cloned().unwrap_or(Unknown),
        "to_h" => Hash(Box::new(Unknown), Box::new(Unknown)),
        "dig" => Unknown,
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
        // `reduce(:+)`, `inject(0, :+)`: the operator gives the receiver's kind of value
        "reduce" | "inject" if block.is_none() => match args {
            [init, Sym] => crate::ty::join(init, &elem),
            _ => elem.clone(),
        },
        "reduce" | "inject" => block_ty(),
        "group_by" => Hash(Box::new(block_ty()), Box::new(same())),
        "tally" => Hash(Box::new(elem.clone()), Box::new(Int)),
        _ => return None,
    })
}

/// What `flatten` makes of an array of `elem`: one level less, or all
/// levels when `all` (`flatten(depth)` does not say how many: unknown below).
fn flattened(elem: &Ty, all: bool) -> Ty {
    match elem.base() {
        Ty::Array(inner) if all => flattened(inner, true),
        Ty::Array(inner) if matches!(inner.base(), Ty::Array(_)) => Ty::array(Ty::Unknown),
        Ty::Array(inner) => Ty::array((**inner).clone()),
        other => Ty::array(other.clone()),
    }
}

/// The methods whose result is made of their arguments too: what they
/// make of an untrusted argument is untrusted.
pub fn embeds_args(name: &str) -> bool {
    matches!(
        name,
        "sub"
            | "gsub"
            | "ljust"
            | "rjust"
            | "center"
            | "tr"
            | "zip"
            | "product"
            | "merge"
            | "each_with_object"
            | "reduce"
            | "inject"
            | "fetch"
    )
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
        // pure: computed in UTC (see `clock`)
        ("Time", "parse" | "at") => (Float, None),
        ("Time", "iso" | "date") => (Str, None),
        ("Time", "weekday") => (Int, None),
        ("Http", "get" | "post" | "put" | "patch" | "delete" | "head") => (User(HTTP_RESPONSE.into()), Some("net")),
        ("Db", "connect") => (User(DATABASE.into()), None),
        ("Shell", "run") => (User(SHELL_RESULT.into()), Some("shell")),
        ("Ssh", "connect") => (User(SSH_SESSION.into()), Some("ssh")),
        ("Pdf" | "Image", "read") => (User(ATTACHMENT.into()), Some("fs.read")),
        ("Pdf" | "Image", "url") => (User(ATTACHMENT.into()), None),
        ("Audio", "read") => (User(ATTACHMENT.into()), Some("fs.read")),
        // no provider fetches audio: Grenat downloads it
        ("Audio", "url") => (User(ATTACHMENT.into()), Some("net")),
        // the SMTP server's host: `net("smtp.acme.io")` for a literal URL
        ("Mail", "connect") => (User(MAILER.into()), Some("net")),
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

/// Built-in functions that need an effect granted but do nothing that
/// differs from one run to the next (a mailer opens no connection until it
/// sends): in a workflow, they need no `step`.
pub fn opens_nothing(module: &str, name: &str) -> bool {
    matches!((module, name), ("Mail", "connect"))
}

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

/// The records Grenat builds itself, which a program cannot declare: a
/// `struct Attachment` of its own would pass for audio read from a file.
/// Mirrors `grenat_interp`'s `builtins::RECORD_NAMES`.
pub const RECORD_NAMES: &[&str] = &[
    ATTACHMENT,
    TRANSCRIPT_SEGMENT,
    CONVERSATION,
    DATABASE,
    HTTP_RESPONSE,
    REQUEST,
    RESPONSE,
    MAILER,
    SHELL_RESULT,
    SSH_SESSION,
    SSH_RESULT,
    SFTP,
    SFTP_ENTRY,
    EVENT_STREAM,
];

pub const TYPE_NAMES: &[&str] = &[
    "Int", "Float", "String", "Bool", "Array", "Hash", "Symbol", "Nil", "Range", "Money", "Duration", "Secret",
    "Vector",
];

pub const GLOBALS: &[&str] = &[
    "puts",
    "print",
    "p",
    "format",
    "warn",
    "raise",
    "spawn",
    "spawn_pool",
    "budget",
    "within",
    "step",
    "approve!",
    "with_human",
    "freeze_time",
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
