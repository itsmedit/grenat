# grenat_ext_macros

The procedural macros of [`grenat_ext`](https://crates.io/crates/grenat_ext), the SDK of
[Grenat](https://github.com/itsmedit/grenat)'s native facets. Depend on `grenat_ext`, which
re-exports them, not on this crate:

```rust
use grenat_ext::{GrenatType, export};
use serde::{Deserialize, Serialize};

/// A point of the plane.
#[derive(Serialize, Deserialize, GrenatType)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// The middle of two points.
#[export(pure)]
pub fn middle(a: Point, b: Point) -> Point {
    Point { x: (a.x + b.x) / 2.0, y: (a.y + b.y) / 2.0 }
}
```

- `#[export]` makes a function callable from Grenat: a C entry point of `grenat_ext`'s ABI
  (JSON arguments in, JSON result out, panics caught), listed in the library's manifest.
  Options: `effects = "fs.read, net(\"api.x.com\")"`, `pure`, `error = "SheetError"`.
- `#[derive(GrenatType)]` gives a struct its Grenat declaration (the struct also derives serde's
  `Serialize` and `Deserialize` too), so exported functions can take and return it.

A facet's crate then declares, in Grenat, `native def middle(a: Point, b: Point) -> Point pure`
(written by `setter install`). See <https://docs.rs/grenat_ext>.

## License

MIT or Apache-2.0, at your option.
