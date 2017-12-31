extern crate unicode_xid;

mod text;
mod tree;
mod lexer;

pub mod syntax_kinds;
pub use text::{TextUnit, TextRange};
pub use tree::{SyntaxKind, Token, FileBuilder, File, Node};
pub use lexer::{next_token, tokenize};
