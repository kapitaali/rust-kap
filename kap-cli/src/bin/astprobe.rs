fn main() {
    let src = std::env::args().nth(1).unwrap();
    let toks = kap_core::lexer::tokenise(&src);
    let (instrs, errs) = kap_core::parser::parse(&toks, &[], &[], &std::collections::HashMap::new());
    for i in &instrs { println!("{:#?}", i); }
    for e in &errs { println!("ERR: {}", e); }
}
