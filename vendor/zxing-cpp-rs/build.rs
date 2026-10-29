// Changed for barcode-read (see scripts/zxing-cpp.patch): builds ../zxing-cpp/core, matches the
// MSVC C runtime to Rust's crt-static setting, and keeps /O2 in MSVC release builds.
use std::env;

fn main() -> miette::Result<()> {
	if cfg!(feature = "bundled") {
		// Builds the project in the directory located in `core`, installing it into $OUT_DIR
		let mut dst = cmake::Config::new("../zxing-cpp/core")
			.define("BUILD_SHARED_LIBS", "OFF")
			.define("ZXING_READERS", "ON")
			.define("ZXING_WRITERS", "ON")
			.define("ZXING_EXPERIMENTAL_API", "OFF")
			.define("ZXING_C_API", "ON")
			.define("ZXING_USE_BUNDLED_ZINT", "ON")
			.define("CMAKE_INSTALL_LIBDIR", "lib")
			.define("CMAKE_MSVC_RUNTIME_LIBRARY", if env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default().contains("crt-static") { "MultiThreaded" } else { "MultiThreadedDLL" })
			.define("CMAKE_C_FLAGS_RELEASE", if env::var("TARGET").unwrap_or_default().contains("msvc") { "/O2 /Ob2 /DNDEBUG" } else { "-O3 -DNDEBUG" })
			.define("CMAKE_CXX_FLAGS_RELEASE", if env::var("TARGET").unwrap_or_default().contains("msvc") { "/O2 /Ob2 /DNDEBUG" } else { "-O3 -DNDEBUG" })
			.build();
		dst.push("lib");
		println!("cargo:rerun-if-changed=build.rs");
		println!("cargo:rerun-if-changed=../zxing-cpp/core");
		println!("cargo:rustc-link-search=native={}", dst.display());
		println!("cargo:rustc-link-lib=static=ZXing");

		if let Ok(target) = env::var("TARGET") {
			if target.contains("apple") {
				println!("cargo:rustc-link-lib=dylib=c++");
			} else if target.contains("linux") {
				println!("cargo:rustc-link-lib=dylib=stdc++");
			}
		}
	} else if let Ok(lib_dir) = env::var("ZXING_CPP_LIB_DIR") {
		println!("cargo:rustc-link-search=native={lib_dir}");
		println!("cargo:rustc-link-lib=dylib=ZXing");
	} else {
		// panic!("ZXing library not found. Use feature 'bundled' or set environment variabale ZXING_CPP_LIB_DIR.")
	}

	// manual bindings.rs generation:
	// bindgen core/src/ZXingC.h -o src/bindings.rs --no-prepend-enum-name --merge-extern-blocks --use-core --no-doc-comments --no-layout-tests --with-derive-partialeq --allowlist-item "ZXing.*" -- -Itarget/release/build/zxing-cpp-*/out/include

	// replace enum decimal constants with hexadecimal ones for better readability
	// perl -0777 -i -pe 's/(pub const (?:ZXing_(?:Barcode|Image)Format_\w+: ZXing_(?:Barcode|Image)Format) = )(\d+);/$1 . sprintf("0x%04X;", $2)/ge' src/bindings.rs
	Ok(())
}
