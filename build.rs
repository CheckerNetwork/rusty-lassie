use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=go.sum");
    println!("cargo:rerun-if-changed=go-lib/lassie.go");

    let v = get_lassie_version();
    // assert_eq!(
    //     v, "0.21.0",
    //     "New Lassie version detected. Update the build version in build.rs."
    // );
    // println!("cargo:rustc-env=LASSIE_VERSION=0.21.0-2cf1121");
    println!("cargo:rustc-env=LASSIE_VERSION={v}-rs");

    build_lassie();
}

#[cfg(not(all(target_os = "windows", target_env = "msvc")))]
fn build_lassie() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let out_file = &format!("{out_dir}/libgolassie.a");

    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let goarch = match arch.as_str() {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => panic!("Unsupported architecture: {arch}"),
    };

    eprintln!("Building {out_file} for {arch} (GOARCH={goarch})");

    let mut cmd = Command::new("go");
    cmd.current_dir("go-lib")
        .args([
            "build",
            "-tags",
            "netgo",
            "-o",
            out_file,
            "-buildmode=c-archive",
            "lassie-ffi.go",
        ])
        .env("GOARCH", goarch)
        // We must explicitly enable CGO when cross-compiling
        // See e.g. https://stackoverflow.com/q/74976549/69868
        .env("CGO_ENABLED", "1");

    if env::var("HOME") == Ok("/".to_string()) && env::var("CROSS_RUNNER").is_ok() {
        // When cross-compiling using `cross build`, HOME is set to `/` and go is trying to
        // create its cache dir in /.cache/go-build, which is not writable.
        // We fix the problem by explicitly setting the GOCACHE and GOMODCACHE env vars
        let target_dir = env::var("CARGO_TARGET_DIR")
            .expect("cannot obtain CARGO_TARGET_DIR from the environment");

        cmd.env("GOCACHE", format!("{target_dir}/go/cache"))
            .env("GOMODCACHE", format!("{target_dir}/go/pkg-mod-cache"));

        if arch == "aarch64" {
            // Overwrite Go CC config, unless it's already provided by the user
            // See https://github.com/golang/go/issues/28966
            if env::var("CC").is_err() {
                cmd.env("CC", "aarch64-linux-gnu-gcc");
            }
        }
    }

    let status = cmd.status().expect(
        "Cannot execute `go`, make sure it's installed.\nLearn more at https://go.dev/doc/install",
    );
    assert!(status.success(), "`go build` failed");

    println!("cargo:rustc-link-search=native={out_dir}");
}

#[cfg(all(target_os = "windows", target_env = "msvc"))]
fn build_lassie() {
    let out_dir = env::var("OUT_DIR").unwrap();

    //On windows platforms it's a `.dll` and there's no leading `lib`
    let out_file = format!("{out_dir}\\golassie.dll");
    eprintln!("Building {out_file}");

    let status = Command::new("go")
        .current_dir("go-lib")
        .args([
            "build",
            "-tags",
            "netgo",
            "-o",
            &out_file,
            "-buildmode=c-shared",
            "lassie-ffi.go",
        ])
        .status()
        .expect(
            "Cannot execute `go`, make sure it's installed.\n\
                 Learn more at https://go.dev/doc/install.\n\
                 On Windows, you need GCC installed too: https://jmeubank.github.io/tdm-gcc/",
        );
    assert!(status.success(), "`go build` failed");

    eprintln!("Building {out_file}.lib");

    let def_file = format!("{out_dir}\\golassie.def");
    std::fs::copy("go-lib\\golassie.def", &def_file)
        .unwrap_or_else(|_| panic!("cannot copy golassie.def to {def_file}"));
    println!("cargo:rerun-if-changed=go-lib/golassie.def");

    let mut lib_cmd = cc::windows_registry::find(&env::var("TARGET").unwrap(), "lib.exe")
        .expect("cannot find the path to MSVC link.exe");

    let status = lib_cmd
        .args([format!("/def:{def_file}"), format!("/out:{out_file}.lib")])
        .status()
        .expect("cannot execute 'link.exe'");
    assert!(status.success(), "`link.exe` failed");

    println!("cargo:rustc-link-search=native={out_dir}");

    // UGLY HACK:
    // - Rust/Cargo does not support resource files, we must copy the DLL manually
    // - Cargo does not tell us what is the target output directory. The dir can be `target\debug`,
    //   `target\x86_64-pc-windows-msvc\debug`, but also some custom dir configured via ENV vars
    // Related: https://github.com/rust-lang/cargo/issues/5305
    let dll_out = format!("{out_dir}\\..\\..\\..\\golassie.dll");
    std::fs::copy(&out_file, &dll_out)
        .unwrap_or_else(|_| panic!("cannot copy {out_file} to {dll_out}"));
}

const GO_SUM_LASSIE: &str = "github.com/filecoin-project/lassie v";

fn get_lassie_version() -> String {
    let text = std::fs::read_to_string("go.sum").expect("cannot open go.sum");

    for ln in text.lines() {
        if let Some(spec) = ln.strip_prefix(GO_SUM_LASSIE) {
            match spec.split_once(' ') {
                Some((v, _)) => return v.to_owned(),
                None => panic!("Malformed go.sum line: {ln}"),
            }
        }
    }

    panic!("lassie not found in go.sum file")
}
