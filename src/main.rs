fn main() {
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("barcode-read {}", env!("CARGO_PKG_VERSION"));
    }
}

#[cfg(test)]
mod tests {
    use zxingcpp::{BarcodeFormat, ImageFormat, ImageView, create, read, write};

    #[test]
    fn reads_back_a_written_qr_code() {
        let code = create(BarcodeFormat::QRCode)
            .from_str("barcode-read")
            .unwrap();
        let img = code.to_image_with(&write().scale(4)).unwrap();
        let (w, h, px) = (img.width() as u32, img.height() as u32, img.data());
        let found = read()
            .from(ImageView::from_slice(&px, w, h, ImageFormat::Lum).unwrap())
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text(), "barcode-read");
        assert_eq!(found[0].format(), BarcodeFormat::QRCode);
    }
}
