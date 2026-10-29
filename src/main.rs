fn main() {
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("barcode-read {}", env!("CARGO_PKG_VERSION"));
    }
}
