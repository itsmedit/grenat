"""The Python side of the facet `numbers`: typed functions, errors, a
process that dies, and one function given the network."""

import os
import socket
from typing import Dict, List, Optional

from grenat_bridge import BridgeError, export, run, struct

struct("Point", fields={"x": float, "y": float}, doc="A point of the plane.")


@export(pure=True)
def mean(values: List[float]) -> Optional[float]:
    """The mean of the values, if there are some."""
    return sum(values) / len(values) if values else None


@export(params={"a": int, "b": int}, returns=int, pure=True)
def add(a, b):
    return a + b


@export(pure=True)
def histogram(values: List[int]) -> Dict[str, int]:
    counts = {}
    for value in values:
        counts[str(value)] = counts.get(str(value), 0) + 1
    return counts


@export(pure=True)
def middle(a: "Point", b: "Point") -> "Point":
    return {"x": (a["x"] + b["x"]) / 2, "y": (a["y"] + b["y"]) / 2}


@export(error="MathError")
def ratio(a: float, b: float) -> float:
    return a / b


@export
def odd(n: int) -> int:
    if n % 2 == 0:
        raise BridgeError("%d is even" % n, type="NotOdd")
    return n


@export(effects=["net"])
def fetch(port: int) -> str:
    with socket.create_connection(("127.0.0.1", port), timeout=3) as connection:
        return connection.recv(100).decode()


@export
def crash(status: int) -> None:
    print("crashing")
    os._exit(status)


@export
def pid() -> int:
    return os.getpid()


run()
