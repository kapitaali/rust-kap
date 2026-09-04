use kap_core::Engine;
use kap_core::APLValue;

#[test]
fn inspect_decode_result() {
    let engine = Engine::new();
    let cases = vec![
        "(3⍴2) ⊤ 3",
        "(2⍴2) ⊤ 7",
        "2 3 6 ⊤ 15",
    ];
    for c in &cases {
        match engine.eval_string(c) {
            Ok(v) => {
                println!("\n=== {} ===", c);
                println!("format_value  = {}", v.format_value());
                println!("format_display= {}", v.format_display());
                println!("format_conform= {}", v.format_conform());
                println!("rank={} dims={:?}", v.rank(), v.dimensions());
                if let APLValue::Array(a) = v.as_ref() {
                    for (i, e) in a.elements().iter().enumerate() {
                        println!("  elem[{}]: format_display={}", i, e.format_display());
                        println!("           type={:?}", e);
                        if let APLValue::Array(inner) = e.as_ref() {
                            println!("           inner rank={} dims={:?}", inner.dimensions.len(), inner.dimensions);
                        }
                    }
                }
            }
            Err(e) => println!("{} => ERR {:?}", c, e),
        }
    }
}
