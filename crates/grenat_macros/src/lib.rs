//! Macros: what Ruby does with `method_missing`, `define_method` or
//! `attr_accessor`, done at compile time.
//!
//! ```ruby
//! macro property(name, type)
//!   def {{name}} -> {{type}} = @{{name}}
//! end
//!
//! macro getters(*names)
//!   {% for n in names %}
//!   def {{n}} = @{{n}}
//!   {% end %}
//! end
//!
//! class Account
//!   @owner: String = ""
//!   property :owner, String
//! end
//! ```
//!
//! A macro is a template of declarations. It is invoked as a statement at
//! the top level, or in the body of a type, and replaced by its expansion
//! before the program is checked: `{{name}}` is an argument (a symbol gives
//! its name, anything else its source text), `{% for x in list %}` repeats
//! over a `*variadic` parameter. An expansion may invoke other macros.
//! What goes wrong in expanded code is reported at the invocation.

mod expand;
mod template;

pub use expand::expand;
pub use template::{Binding, render};

/// Expansions within expansions, at most (a recursive macro stops there).
pub const MAX_DEPTH: usize = 32;
