use kap_core::Engine;
use kap_core::APLValue;

fn show(v: &APLValue) -> String {
    match v {
        APLValue::Array(a) => {
            let elems: Vec<String> = a.elements().iter().map(|e| show(e)).collect();
            if a.dimensions.len() > 1 {
                format!("[{:?} {}]", a.dimensions, elems.join(" "))
            } else {
                format!("({})", elems.join(" "))
            }
        }
        APLValue::Number(n) => format!("{}", n),
        APLValue::Null => "⍬".to_string(),
        _ => format!("{:?}", v),
    }
}

#[test]
fn show_decode_outputs() {
    let engine = Engine::new();
    let cases = vec![
        "(3⍴2) ⊤ 3",       // expected: [0 1 1]
        "(2⍴2) ⊤ 7",       // expected: [1 1]
        "2 3 6 ⊤ 15",      // expected: [0 2 3]
        "2 ⊤ 100",         // expected: [1 1 0 0 1 0 0]
        "10 ⊤ 1234",       // expected: [1 2 3 4]
    ];
    for c in &cases {
        match engine.eval_string(c) {
            Ok(v) => println!("{} => {} [rank {} dims {:?}]", c, show(&v), v.rank(), v.dimensions()),
            Err(e) => println!("{} => ERR {:?}", c, e),
        }
    }
}
