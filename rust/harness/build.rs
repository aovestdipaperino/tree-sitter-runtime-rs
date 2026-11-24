//! Builds the reference: the unmodified C runtime (`lib.c`, one translation unit) with every
//! external symbol renamed to `ref_<name>`, so it can be linked next to the Rust runtime. Also
//! generates a copy of the bindgen bindings whose functions link to the renamed symbols.
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn configure(b: &mut cc::Build, src: &PathBuf, include: &PathBuf) {
    b.std("c11")
        .flag_if_supported("-fvisibility=hidden")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-incompatible-pointer-types")
        .include(src)
        .include(include)
        .define("_POSIX_C_SOURCE", "200112L")
        .define("_DEFAULT_SOURCE", None)
        .define("_BSD_SOURCE", None)
        .define("_DARWIN_C_SOURCE", None)
        .warnings(false)
        .file(src.join("lib.c"));
}

/// The reference C sources: upstream 0.26.13 plus the upstream fixes listed in `rust/patches/`
/// (ADR-013). Copied to `OUT_DIR` and patched there; the pristine `src/` is never modified.
fn patched_reference_src(root: &Path, out: &Path) -> PathBuf {
    let dst = out.join("ref-src");
    if dst.exists() {
        fs::remove_dir_all(&dst).unwrap();
    }
    copy_dir(&root.join("src"), &dst);
    let patches = root.join("rust/patches");
    println!("cargo::rerun-if-changed={}", patches.display());
    let mut files: Vec<_> = fs::read_dir(&patches)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "patch"))
        .collect();
    files.sort();
    for p in files {
        println!("cargo::rerun-if-changed={}", p.display());
        let status = Command::new("patch")
            .args(["-s", "-p2", "-d"])
            .arg(&dst)
            .arg("-i")
            .arg(&p)
            .status()
            .expect("`patch` is required");
        assert!(status.success(), "failed to apply {}", p.display());
    }
    dst
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            fs::copy(e.path(), target).unwrap();
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    let include = root.join("include");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let src = patched_reference_src(&root, &out);
    let pristine = root.join("src");
    println!("cargo::rerun-if-changed=build.rs");
    for e in fs::read_dir(&pristine).unwrap() {
        println!("cargo::rerun-if-changed={}", e.unwrap().path().display());
    }
    println!(
        "cargo::rerun-if-changed={}",
        root.join("binding_rust/bindings.rs").display()
    );

    // 1. Probe build: collect the global symbols the unity build defines.
    let probe_dir = out.join("probe");
    fs::create_dir_all(&probe_dir).unwrap();
    let mut probe = cc::Build::new();
    configure(&mut probe, &src, &include);
    probe
        .cargo_metadata(false)
        .out_dir(&probe_dir)
        .compile("tsprobe");
    let nm = Command::new("nm")
        .arg("-g")
        .arg("--defined-only")
        .arg(probe_dir.join("libtsprobe.a"))
        .output()
        .expect("nm");
    let apple = env::var("CARGO_CFG_TARGET_VENDOR")
        .map(|v| v == "apple")
        .unwrap_or(false);
    let mut symbols: Vec<String> = String::from_utf8_lossy(&nm.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (_addr, kind, name) = (it.next()?, it.next()?, it.next()?);
            if !matches!(kind, "T" | "D" | "B" | "S" | "R" | "C") {
                return None;
            }
            Some(if apple {
                name.strip_prefix('_').unwrap_or(name).to_string()
            } else {
                name.to_string()
            })
        })
        .collect();
    symbols.sort();
    symbols.dedup();
    assert!(
        symbols.iter().any(|s| s == "ts_parser_new"),
        "probe found no ts_parser_new: {symbols:?}"
    );

    // 2. rename.h and the real reference build.
    let rename: String = symbols
        .iter()
        .map(|s| format!("#define {s} ref_{s}\n"))
        .collect();
    let rename_h = out.join("rename.h");
    fs::write(&rename_h, rename).unwrap();
    let mut reference = cc::Build::new();
    configure(&mut reference, &src, &include);
    reference.flag("-include").flag(rename_h.to_str().unwrap());
    reference.compile("tsref");

    // 3. Bindings whose functions link to the renamed symbols.
    let bindings = fs::read_to_string(root.join("binding_rust/bindings.rs")).unwrap();
    let mut ref_bindings = String::new();
    for line in bindings.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("pub fn ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if symbols.contains(&name) {
                ref_bindings.push_str(&format!("    #[link_name = \"ref_{name}\"]\n"));
            }
        }
        ref_bindings.push_str(line);
        ref_bindings.push('\n');
    }
    fs::write(out.join("bindings_ref.rs"), ref_bindings).unwrap();
    println!(
        "cargo::rustc-env=BINDINGS_RS={}",
        root.join("binding_rust/bindings.rs").display()
    );
}
