use kap_core::Engine;
use kap_core::APLValue;

fn show(v: &APLValue) -> String {
    match v {
        APLValue::Null => "⍬".to_string(),
        APLValue::Array(a) => {
            let elems: Vec<String> = a.elements().iter().map(|e| show(e)).collect();
            if a.dimensions.len() > 1 {
                format!("[{}]", elems.join(" "))
            } else {
                format!("({})", elems.join(" "))
            }
        }
        APLValue::Number(n) => n.format(false),
        _ => format!("{:?}", v),
    }
}

#[test]
fn print_null_results() {
    let e = Engine::new();
    let cases = ["⍬", "+⍬", "⍬+1", "⍬=⍬", "+/⍬", "⍬⍴5", "⊂⍬", "⍴⍬", "⍬,1", "⍬,⍬"];
    for c in &cases {
        match e.eval_string(c) {
            Ok(v) => println!("{} = {}", c, show(&v)),
            Err(e) => println!("{} => ERR {:?}", c, e),
        }
    }
}
