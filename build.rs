fn main() {
    // OCCT kernel (#86): only when built with `--features occt`. Cargo exposes an
    // enabled feature as CARGO_FEATURE_<NAME>.
    if std::env::var_os("CARGO_FEATURE_OCCT").is_some() {
        build_occt_shim();
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon_path = std::path::Path::new("target/generated/appicon.ico");
    if let Some(parent) = icon_path.parent() {
        std::fs::create_dir_all(parent).expect("create generated icon directory");
    }
    png_to_ico("src/assets/appicon.png", icon_path);

    let icon_path = icon_path
        .to_str()
        .expect("generated icon path should be valid UTF-8");
    let mut res = winres::WindowsResource::new();
    res.set_icon(icon_path);
    res.compile().expect("compile Windows icon resources");
}

/// Compile the C++ FFI shim (cpp/bearcad_kernel.cpp) and link it against a static
/// OpenCASCADE build (#86).
///
/// The OCCT install prefix is resolved from, in order:
///   1. the `OCCT_DIR` env var (point this at *your own* OCCT to rebuild against a
///      different version — see README.md), or
///   2. the default location produced by `scripts/build-occt.sh`
///      (`third_party/OCCT/occt-install`).
///
/// The prefix must contain `include/opencascade/*.hxx` and `lib/libTK*.a`.
fn build_occt_shim() {
    use std::path::PathBuf;

    println!("cargo:rerun-if-changed=cpp/bearcad_kernel.cpp");
    println!("cargo:rerun-if-changed=cpp/bearcad_kernel.hpp");
    println!("cargo:rerun-if-env-changed=OCCT_DIR");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let occt_dir = std::env::var_os("OCCT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("third_party/OCCT/occt-install"));

    let include = occt_dir.join("include/opencascade");
    let libdir = occt_dir.join("lib");
    if !include.is_dir() || !libdir.is_dir() {
        panic!(
            "OCCT not found under {}\n(expected {} and {}).\n\
             Build it first: `scripts/build-occt.sh`, or set OCCT_DIR to your own \
             OCCT install prefix. See README.md \"Building with the OCCT kernel\".",
            occt_dir.display(),
            include.display(),
            libdir.display(),
        );
    }

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("cpp/bearcad_kernel.cpp")
        .include("cpp")
        .include(&include)
        // Split every function/data item into its own section so the linker's
        // dead-code stripping (below) can drop the ones this binary never calls.
        .flag_if_supported("-ffunction-sections")
        .flag_if_supported("-fdata-sections")
        .compile("bearcad_kernel");

    println!("cargo:rustc-link-search=native={}", libdir.display());

    // OCCT toolkits, listed high-level -> low-level so a single-pass linker
    // resolves the (layered, acyclic) inter-toolkit dependencies. Only the
    // modeling toolkits are needed for the current kernel surface; visualization
    // / data-exchange are not linked.
    //
    // Static-archive linking already pulls in only the object files that are
    // referenced (unused .cxx compilation units never enter the binary). The
    // link-time dead-strip below goes finer-grained — dropping unreferenced
    // *functions/data* within the object files that do get pulled in — provided
    // OCCT itself was compiled with -ffunction-sections/-fdata-sections
    // (scripts/build-occt.sh sets that).
    for tk in OCCT_TOOLKITS {
        println!("cargo:rustc-link-lib=static={tk}");
    }

    // The C++ standard library the shim and OCCT need.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "macos" => {
            println!("cargo:rustc-link-lib=dylib=c++");
            // ld64: strip unreferenced code/data from the final binary.
            println!("cargo:rustc-link-arg=-Wl,-dead_strip");
        }
        "linux" => {
            println!("cargo:rustc-link-lib=dylib=stdc++");
            // GNU ld / lld: garbage-collect unreferenced sections.
            println!("cargo:rustc-link-arg=-Wl,--gc-sections");
        }
        _ => {}
    }
}

/// OCCT modeling toolkits, high-level first so a single-pass linker resolves the
/// layered (acyclic) inter-toolkit dependencies (see `build_occt_shim`). Covers
/// solids + booleans (TKBO/TKBool), shape healing they rely on (TKShHealing), and
/// triangulation (TKMesh).
const OCCT_TOOLKITS: &[&str] = &[
    "TKMesh",
    "TKOffset",
    "TKBool",
    "TKBO",
    "TKShHealing",
    "TKPrim",
    "TKTopAlgo",
    "TKGeomAlgo",
    "TKBRep",
    "TKGeomBase",
    "TKG3d",
    "TKG2d",
    "TKMath",
    "TKernel",
];

fn png_to_ico(png_path: &str, out_path: &std::path::Path) {
    use ico::{IconDir, IconImage};
    use image::imageops::FilterType;
    use std::fs::File;
    use std::io::BufWriter;

    let image = image::ImageReader::open(png_path)
        .expect("open app icon png")
        .decode()
        .expect("decode app icon png")
        .into_rgba8();

    let mut icon_dir = IconDir::new(ico::ResourceType::Icon);
    for size in [256u32, 48, 32, 16] {
        let resized = image::imageops::resize(&image, size, size, FilterType::Lanczos3);
        let (width, height) = resized.dimensions();
        let icon = IconImage::from_rgba_data(width, height, resized.into_raw());
        let entry = ico::IconDirEntry::encode(&icon).expect("encode icon size");
        icon_dir.add_entry(entry);
    }

    let file = File::create(out_path).expect("create ico file");
    icon_dir
        .write(BufWriter::new(file))
        .expect("write ico file");
}