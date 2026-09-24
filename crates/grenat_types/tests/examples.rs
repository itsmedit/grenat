//! The repository's examples produce no diagnostics.

mod common;

use common::*;

#[test]
fn examples_are_clean() {
    for name in ["basics.grn", "explorer.grn", "support_desk.grn"] {
        let src = example(name);
        let d = diags(&src);
        assert!(d.is_empty(), "{name} :\n{}", render(&src, &d));
    }
}
