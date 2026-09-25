fn main() {
    let path = std::env::args().nth(1).unwrap();
    let src = std::fs::read_to_string(path).unwrap();
    print!("{}", grenat_fmt::print_unchecked(&src).unwrap());
}
