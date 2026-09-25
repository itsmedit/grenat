//! Ahead-of-time compilation: object files (linking is tested by `grenat build`).

use grenat_codegen::aot;

#[test]
fn an_object_file_holds_the_compiled_functions() {
    let src = "def double(n: Int) -> Int = n * 2\ndef greet(s: String) -> String = \"hi #{s}\"\ndef other(x) = x\n";
    let program = grenat_parser::parse(src).program;
    let object = aot::object(&program, src, "0 test.grn\n", grenat_codegen::Backend::Cranelift).expect("an object file");
    assert_eq!(object.report.compiled, ["double", "greet"]);
    // Mach-O (64-bit) or ELF
    let magic = &object.bytes[..4];
    assert!(magic == [0xcf, 0xfa, 0xed, 0xfe] || magic == [0x7f, b'E', b'L', b'F'], "{magic:?}");
    let exported = |name: &str| object.bytes.windows(name.len()).any(|w| w == name.as_bytes());
    assert!(exported(aot::IMAGE_SYMBOL));
    assert!(exported("grenat_str_push_str"), "runtime functions are imported by name");
    assert!(exported("hi "), "literals and source are embedded");
}
