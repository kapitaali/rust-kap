//! Token model for Kap (Phase 2).
//!
//! Mirrors `array/.../tokeniser.kt` token kinds. The Kotlin code uses singleton
//! `object` tokens (e.g. `OpenParen : Token()`); in Rust these become unit-variant
//! enum cases. Literal-bearing tokens (numbers, chars, strings, symbols) carry data.
//!
//! Reference: docs/reference.asciidoc §"Datatypes"/"Numbers"/"String syntax".

use crate::KapNumber;

/// A parsed literal value carried by a token.
#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    Number(KapNumber),
    Char(char),
    Str(String),
    /// Symbol name, with optional explicit namespace (`foo:bar` -> name=bar, ns=foo).
    Symbol { name: String, namespace: Option<String> },
    /// A quoted **symbol value** literal (`'foo`): evaluates to an `APLValue::Symbol`
    /// rather than being a variable reference. (Kotlin: `parser.kt` SymbolValue.)
    /// `namespace` mirrors `Symbol`: `None` for a bare `'foo`, `Some("kap")` for `'kap:array`.
    SymbolValue { name: String, namespace: Option<String> },
}

/// All Kap token kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // punctuation / structural
    Whitespace,
    EndOfFile,
    OpenParen,
    CloseParen,
    OpenBrace,    // {
    CloseBrace,   // }
    OpenFnDef,    // ∇
    OpenBracket,  // [
    CloseBracket, // ]
    StatementSeparator, // ⋄
    LeftArrow,    // ←
    FnDefSym,     // ∇
    FnDefArrow,   // ← (in fn header context; Kotlin has both LeftArrow & FnDefArrow)
    APLNullSym,   // ⍬
    QuotePrefix,  // ⍠
    LambdaToken,  // λ
    ApplyToken,   // ⍞
    ListSeparator,// ;
    Newline,
    NamespaceToken, // namespace(
    ImportToken,  // import(
    DeclareToken, // declare(
    IfToken,      // if(
    ElseToken,
    WhileToken,
    WhenToken,
    LeftForkToken,  // «
    RightForkToken, // »
    ComposeToken,   // ∘  (atop / compose: f ∘ g)
    ReverseComposeToken, // ⍛  (reverse compose: f ⍛ g)
    DynassignToken, // ⇐
    ColonSym,      // :  (guarded expression: cond : truthy ⋄ falsy)
    AndToken,    // and
    OrToken,     // or
    Comment,
    MemberDereferenceToken, // .
    MethodCallToken,
    FunctionCallOpenParen,
    FunctionCallCloseParen,
    DefsyntaxSubToken,
    DefsyntaxToken,
    IncludeToken,
    IncludeIfToken,
    NilToken,    // nil

    // literal-bearing
    Literal(LiteralValue),
    Error(String),
}

/// A token with its source position, for error reporting (strategy §4.7).
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    pub token: Token,
    pub line: usize,
    pub col: usize,
}
