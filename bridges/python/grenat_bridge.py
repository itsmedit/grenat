"""Python functions a Grenat program calls: the server of a bridge facet.

    from typing import List
    from grenat_bridge import export, struct, run

    struct("Link", fields={"href": str, "text": str}, doc="A link of a page.")

    @export(pure=True)
    def links(html: str) -> List["Link"]:
        \"\"\"The links of a page.\"\"\"
        return [{"href": h, "text": t} for h, t in re.findall(r'<a href="([^"]*)">([^<]*)<', html)]

    run()

Grenat starts the server (the `command` of the facet's `[bridge]`), and
speaks JSON-RPC 2.0 to it, a message per line: `describe` answers what is
exported (the manifest Grenat writes the facet's declarations from), `call`
runs a function with its arguments. While the server runs, what the
functions print goes to standard error, which Grenat logs: standard output
carries the protocol only.

Types are Python's (`str`, `int`, `float`, `bool`, `None`, `List[T]`,
`Dict[str, T]`, `Optional[T]`), or written as Grenat writes them: `"Link"`,
`"String?"`. They come from the function's annotations unless `params=` and
`returns=` say otherwise. Standard library only.
"""

import inspect
import json
import os
import re
import sys
import types
import typing

# The version of the protocol (the manifest's `abi`).
PROTOCOL = 1

_NAME = re.compile(r"\A[a-z_][a-z0-9_]*\Z")
# An error type Grenat code can rescue: `SheetError`.
_ERROR_NAME = re.compile(r"\A[A-Z][A-Za-z0-9]*Error\Z")
_SCALARS = {str: "String", int: "Int", float: "Float", bool: "Bool", type(None): "Nil"}

_functions = {}
_structs = []


class BridgeError(Exception):
    """An error a function raises to Grenat with a type of its choosing:
    `raise BridgeError("no such sheet", type="SheetError")` (a name ending
    in `Error`, else Grenat raises a `BridgeError`). Any other exception
    raises the function's `error=` type."""

    def __init__(self, message, type=None):
        super().__init__(message)
        self.type = type


class ProtocolError(Exception):
    """A request that is not JSON-RPC, or does not match what is exported."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def grenat_type(spec):
    """The Grenat type written `spec`."""
    if isinstance(spec, str):
        return spec
    if spec is None:
        return "Nil"
    if spec in _SCALARS:
        return _SCALARS[spec]
    origin, args = typing.get_origin(spec), typing.get_args(spec)
    if origin is list and len(args) == 1:
        return "Array(%s)" % grenat_type(args[0])
    if origin is dict and len(args) == 2:
        if args[0] is not str:
            raise TypeError("a dict type has string keys: Dict[str, T]")
        return "Hash(String, %s)" % grenat_type(args[1])
    if origin is typing.Union or origin is getattr(types, "UnionType", None):
        others = [a for a in args if a is not type(None)]
        if len(others) == 1 and len(args) == 2:
            return grenat_type(others[0]) + "?"
    if isinstance(spec, typing.ForwardRef):
        return spec.__forward_arg__
    raise TypeError("unknown type %r: use str, int, float, bool, None, List, Dict or Optional" % (spec,))


def export(function=None, *, name=None, params=None, returns=None, effects=(), pure=False, doc=None,
           error="BridgeError"):
    """Exports a function to Grenat: `@export` or `@export(pure=True, …)`.

    `params` ({"name": type}, in order) and `returns` default to the
    function's annotations; `effects` are Grenat's (`"net"`, `"fs.read"`); a
    pure function has none, and its result is trusted; `error` is the Grenat
    error its exceptions raise."""

    def register(function):
        exported = name or function.__name__
        if not _NAME.match(exported):
            raise ValueError("`%s` is not a Grenat function name" % exported)
        if pure and effects:
            raise ValueError("`%s` is pure, and has effects: a pure function has none" % exported)
        if not _ERROR_NAME.match(str(error)):
            raise ValueError("`%s` raises `%s`: an error type is a capitalized name ending in `Error`" % (exported, error))
        hints = _annotations(function)
        if params is None:
            names = list(inspect.signature(function).parameters)
            missing = [p for p in names if p not in hints]
            if missing:
                raise TypeError("`%s`: give the type of %s (an annotation, or params=)" % (exported, missing[0]))
            declared = [(p, hints[p]) for p in names]
        else:
            declared = list(params.items())
        result = returns if returns is not None else hints.get("return", None)
        _functions[exported] = {
            "function": function,
            "manifest": {
                "name": exported,
                "symbol": exported,
                "doc": doc if doc is not None else inspect.getdoc(function),
                "params": [{"name": p, "type": grenat_type(t), "doc": None} for p, t in declared],
                "returns": grenat_type(result),
                "effects": [str(e) for e in effects],
                "pure": bool(pure),
                "error": str(error),
            },
        }
        return function

    return register(function) if function is not None else register


def struct(name, fields, doc=None):
    """Declares a struct the functions take or return: a dict crosses as one."""
    _structs.append({
        "name": name,
        "doc": doc,
        "fields": [{"name": f, "type": grenat_type(t), "doc": None} for f, t in fields.items()],
    })
    return name


def manifest():
    """What is exported, as Grenat's manifest."""
    return {"abi": PROTOCOL, "functions": [f["manifest"] for f in _functions.values()], "structs": list(_structs)}


def handle(line):
    """The answer to one line of the protocol (None for a notification)."""
    try:
        request = json.loads(line)
    except ValueError as e:
        return _failure(None, ProtocolError(-32700, "not JSON: %s" % e))
    request_id = request.get("id") if isinstance(request, dict) else None
    try:
        if not (isinstance(request, dict) and request.get("jsonrpc") == "2.0"
                and isinstance(request.get("method"), str)):
            raise ProtocolError(-32600, "not a JSON-RPC 2.0 request")
        result = _dispatch(request["method"], request.get("params"))
        return {"jsonrpc": "2.0", "id": request_id, "result": result} if "id" in request else None
    except (ProtocolError, BridgeError) as e:
        return _failure(request_id, e)


def run():
    """Serves requests until Grenat closes standard input."""
    requests = os.fdopen(os.dup(0), "r", encoding="utf-8")
    responses = os.fdopen(os.dup(1), "w", encoding="utf-8")
    # standard output is the protocol's: what the functions print goes to standard error
    null = os.open(os.devnull, os.O_RDONLY)
    os.dup2(null, 0)
    os.close(null)
    os.dup2(2, 1)
    sys.stdin = open(os.devnull)
    sys.stdout = sys.stderr
    for line in requests:
        if not line.strip():
            continue
        response = handle(line)
        if response is not None:
            responses.write(_encode(response) + "\n")
            responses.flush()


def _annotations(function):
    try:
        return typing.get_type_hints(function)
    except Exception:
        return dict(getattr(function, "__annotations__", {}))


def _dispatch(method, params):
    if method == "describe":
        return manifest()
    if method == "call":
        if not isinstance(params, dict):
            raise ProtocolError(-32602, "`call` takes {name, args}")
        return _call(params.get("name"), params.get("args"))
    raise ProtocolError(-32601, "no method `%s`: `describe` or `call`" % method)


def _call(name, args):
    exported = _functions.get(name)
    if exported is None:
        raise ProtocolError(-32602, "no function `%s` is exported" % name)
    arity = len(exported["manifest"]["params"])
    if not isinstance(args, list) or len(args) != arity:
        raise ProtocolError(-32602, "`%s` takes %d argument(s)" % (name, arity))
    try:
        return exported["function"](*args)
    except BridgeError as e:
        raise BridgeError(str(e), type=e.type or exported["manifest"]["error"])
    except Exception as e:
        raise BridgeError(str(e) or type(e).__name__, type=exported["manifest"]["error"])


def _failure(request_id, error):
    code = error.code if isinstance(error, ProtocolError) else -32000
    body = {"code": code, "message": str(error)}
    if isinstance(error, BridgeError):
        body["data"] = {"type": error.type}
    return {"jsonrpc": "2.0", "id": request_id, "error": body}


def _encode(response):
    try:
        text = json.dumps(response, allow_nan=False, ensure_ascii=False)
        # a lone surrogate (a file name that is not UTF-8, from os.listdir) is not JSON Grenat reads:
        # refused here (UnicodeEncodeError), rather than a response Grenat drops
        text.encode("utf-8")
        return text
    except (TypeError, ValueError) as e:
        failure = BridgeError("the result cannot be sent as JSON: %s" % e, type="BridgeError")
        return json.dumps(_failure(response.get("id"), failure))
