use kap_core::{lexer::tokenise_with, parser::Parser, Engine, Environment};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = args.get(1).map(|s| s.as_str()).unwrap_or("(5 1)[1]");
    let engine = Engine::new();
    let env = Environment::new_root();
    let _ = engine.eval_string_in_env("use(\"standard-lib.kap\")", &env);
    let toks = tokenise_with(src, &engine.single_chars.borrow());
    let fn_names: Vec<String> = env.function_names();
    let macros = engine.macros.borrow().clone();
    let mut p = Parser {
        toks: &toks, pos: 0,
        known_functions: fn_names.clone(),
        known_ops: env.operator_names(), known_ops2: env.operator_names_2arg(),
        seed_functions: fn_names, tradfn_names: Vec::new(), macros,
        kotlin_close_stack: Vec::new(), list_stop: false, bool_stop: false,
        current_ns: "default".to_string(),
        ns_registry: std::rc::Rc::new(kap_core::NamespaceRegistry::default()),
    };
    let mut n = 0;
    loop {
        match p.parse_statements() {
            Ok(Some(instr)) => { n += 1; println!("--- stmt {} ---\n{:#?}", n, instr); }
            Ok(None) => break,
            Err(e) => { println!("PARSE ERROR at stmt {}: {:?}", n + 1, e); break; }
        }
    }
}
