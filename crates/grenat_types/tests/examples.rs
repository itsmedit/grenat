//! Les exemples du dépôt ne produisent aucun diagnostic.

mod common;

use common::*;

#[test]
fn examples_are_clean() {
    for name in ["bases.grn", "explorateur.grn", "support_desk.grn"] {
        let src = example(name);
        let d = diags(&src);
        assert!(d.is_empty(), "{name} :\n{}", render(&src, &d));
    }
}
